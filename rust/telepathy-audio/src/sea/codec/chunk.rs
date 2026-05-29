use crate::sea::{
    codec::{bits::BitUnpacker, lms::LMS_LEN},
    encoder::EncoderSettings,
};

use super::{
    bits::BitPacker,
    common::{SeaError, SeaResidualSize},
    file::SeaFileHeader,
    lms::SeaLMS,
};

#[derive(Debug, Clone, Copy)]
pub enum SeaChunkType {
    Cbr = 0x01,
    Vbr = 0x02,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkInfo {
    pub frames_per_chunk: usize,
    pub chunk_type: SeaChunkType,
    pub scale_factor_bits: u8,
    pub scale_factor_frames: u8,
    pub residual_size: SeaResidualSize,
}

#[derive(Debug)]
pub struct SeaChunk;

impl SeaChunk {
    #[allow(clippy::too_many_arguments)]
    pub fn serialize_into(
        file_header: &SeaFileHeader,
        lms: &[SeaLMS],
        encoder_settings: &EncoderSettings,
        scale_factors: &[u8],
        vbr_residual_sizes: &[u8],
        residuals: &[u8],
        output: &mut Vec<u8>,
        packer: &mut BitPacker,
    ) -> Result<(), SeaError> {
        encoder_settings.validate()?;

        let is_vbr = !vbr_residual_sizes.is_empty();
        let chunk_type = if is_vbr {
            SeaChunkType::Vbr
        } else {
            SeaChunkType::Cbr
        };
        let residual_size =
            SeaResidualSize::from(libm::floorf(encoder_settings.residual_bits) as u8);
        let scale_factor_bits = encoder_settings.scale_factor_bits;
        let scale_factor_frames = encoder_settings.scale_factor_frames;
        if file_header.frames_per_chunk != encoder_settings.frames_per_chunk {
            return Err(SeaError::InvalidParameters);
        }

        let channels = file_header.channels as usize;
        let frames_per_chunk = file_header.frames_per_chunk as usize;
        let scale_factor_items = frames_per_chunk.div_ceil(scale_factor_frames as usize) * channels;
        let packed_scale_factor_bytes =
            (scale_factor_items * scale_factor_bits as usize).div_ceil(8);
        let packed_vbr_residual_sizes_bytes = if is_vbr {
            (scale_factor_items * 2).div_ceil(8)
        } else {
            0
        };
        let packed_residual_bytes = if is_vbr {
            let mut residual_bits = 0usize;
            let mut vbr_residual_index = 0;
            let mut frames_written_since_update = 0;
            for _ in residuals.chunks_exact(channels) {
                for channel_index in 0..channels {
                    residual_bits +=
                        vbr_residual_sizes[vbr_residual_index + channel_index] as usize;
                }
                frames_written_since_update += 1;
                if frames_written_since_update == scale_factor_frames as usize {
                    vbr_residual_index += channels;
                    frames_written_since_update = 0;
                }
            }
            residual_bits.div_ceil(8)
        } else {
            (residuals.len() * residual_size as usize).div_ceil(8)
        };
        let expected_chunk_bytes = 4
            + (channels * LMS_LEN * 4)
            + packed_scale_factor_bytes
            + packed_vbr_residual_sizes_bytes
            + packed_residual_bytes;

        output.clear();
        output.reserve(expected_chunk_bytes);
        output.extend_from_slice(&[
            chunk_type as u8,
            (scale_factor_bits << 4) | residual_size as u8,
            scale_factor_frames,
            0x5A,
        ]);

        // Serialize LMS directly into output buffer
        for lms_item in lms {
            output.extend_from_slice(&lms_item.serialize());
        }

        // Serialize scale factors using reusable packer
        packer.reset_writer();
        for scale_factor in scale_factors.iter() {
            packer.push_into(*scale_factor as u32, scale_factor_bits, output);
        }
        packer.finish_into(output);

        // Serialize VBR residual sizes if VBR
        if matches!(chunk_type, SeaChunkType::Vbr) {
            packer.reset_writer();
            for vbr_residual_size in vbr_residual_sizes.iter() {
                let relative_size = *vbr_residual_size as i32 - residual_size as i32 + 1;
                packer.push_into(relative_size as u32, 2, output);
            }
            packer.finish_into(output);
        }

        // Serialize residuals using reusable packer
        packer.reset_writer();
        if matches!(chunk_type, SeaChunkType::Vbr) {
            let mut vbr_residual_index = 0;
            let mut frames_written_since_update = 0;
            for residual in residuals.chunks_exact(channels) {
                for (channel_index, item) in residual.iter().enumerate().take(channels) {
                    packer.push_into(
                        *item as u32,
                        vbr_residual_sizes[vbr_residual_index + channel_index],
                        output,
                    );
                }
                frames_written_since_update += 1;
                if frames_written_since_update == scale_factor_frames as usize {
                    vbr_residual_index += channels;
                    frames_written_since_update = 0;
                }
            }
        } else {
            for residual in residuals.iter() {
                packer.push_into(*residual as u32, residual_size as u8, output);
            }
        }
        packer.finish_into(output);

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn parse_into(
        encoded: &[u8],
        file_header: &SeaFileHeader,
        lms: &mut Vec<SeaLMS>,
        scale_factors: &mut Vec<u8>,
        vbr_residual_sizes: &mut Vec<u8>,
        residuals: &mut Vec<u8>,
        vbr_bitlengths: &mut Vec<u8>,
        unpacker: &mut BitUnpacker,
    ) -> Result<ChunkInfo, SeaError> {
        if encoded.len() > file_header.chunk_size as usize {
            return Err(SeaError::InvalidFrame);
        }
        if encoded.len() < 4 {
            return Err(SeaError::InvalidFrame);
        }

        let chunk_type: SeaChunkType = match encoded[0] {
            0x01 => SeaChunkType::Cbr,
            0x02 => SeaChunkType::Vbr,
            _ => return Err(SeaError::InvalidFrame),
        };

        let scale_factor_bits = encoded[1] >> 4;
        if scale_factor_bits == 0 || scale_factor_bits > 8 {
            return Err(SeaError::InvalidFrame);
        }

        let residual_size =
            SeaResidualSize::try_from_u8(encoded[1] & 0b1111).ok_or(SeaError::InvalidFrame)?;
        let scale_factor_frames = encoded[2];
        if scale_factor_frames == 0 {
            return Err(SeaError::InvalidFrame);
        }
        let _reserved = encoded[3];

        let mut encoded_index = 4;

        // Parse LMS into scratch buffer
        lms.clear();
        for _ in 0..file_header.channels as usize {
            let needed = LMS_LEN * 4;
            if encoded_index + needed > encoded.len() {
                return Err(SeaError::InvalidFrame);
            }
            let mut lms_bytes = [0u8; LMS_LEN * 4];
            lms_bytes.copy_from_slice(&encoded[encoded_index..encoded_index + needed]);
            lms.push(SeaLMS::from_bytes(&lms_bytes));
            encoded_index += LMS_LEN * 4;
        }

        let frames_in_this_chunk = file_header.frames_per_chunk as usize;
        if !frames_in_this_chunk.is_multiple_of(scale_factor_frames as usize) {
            return Err(SeaError::InvalidFrame);
        }

        let scale_factor_items = frames_in_this_chunk.div_ceil(scale_factor_frames as usize)
            * file_header.channels as usize;

        // Parse scale factors into scratch buffer
        {
            let packed_scale_factor_bytes =
                (scale_factor_items * scale_factor_bits as usize).div_ceil(8);

            if encoded_index + packed_scale_factor_bytes > encoded.len() {
                return Err(SeaError::InvalidFrame);
            }
            let packed_scale_factors =
                &encoded[encoded_index..encoded_index + packed_scale_factor_bytes];
            encoded_index += packed_scale_factor_bytes;

            unpacker.reset_const(scale_factor_bits);
            unpacker.process_bytes(packed_scale_factors);
            std::mem::swap(scale_factors, unpacker.take_output());
            scale_factors.resize(scale_factor_items, 0);
        }

        // Parse VBR residual sizes if VBR
        if matches!(chunk_type, SeaChunkType::Vbr) {
            let packed_vbr_residual_sizes_bytes = (scale_factor_items * 2).div_ceil(8);
            if encoded_index + packed_vbr_residual_sizes_bytes > encoded.len() {
                return Err(SeaError::InvalidFrame);
            }
            let packed_vbr_residual_sizes =
                &encoded[encoded_index..encoded_index + packed_vbr_residual_sizes_bytes];
            encoded_index += packed_vbr_residual_sizes_bytes;

            unpacker.reset_const(2);
            unpacker.process_bytes(packed_vbr_residual_sizes);
            std::mem::swap(vbr_residual_sizes, unpacker.take_output());
            vbr_residual_sizes.resize(scale_factor_items, 0);
            for item in vbr_residual_sizes.iter_mut() {
                *item += residual_size as u8 - 1;
            }
        } else {
            vbr_residual_sizes.clear();
        }

        // Parse residuals into scratch buffer
        {
            let packed_residuals_bytes = if matches!(chunk_type, SeaChunkType::Vbr) {
                let mut residual_bits: u32 = vbr_residual_sizes
                    [..vbr_residual_sizes.len() - file_header.channels as usize]
                    .iter()
                    .map(|x| *x as u32)
                    .sum();

                residual_bits *= scale_factor_frames as u32;

                let last_frame_samples = frames_in_this_chunk as u32 % scale_factor_frames as u32;
                let multiplier = if last_frame_samples == 0 {
                    scale_factor_frames as u32
                } else {
                    last_frame_samples
                };

                for size in vbr_residual_sizes
                    [(vbr_residual_sizes.len() - file_header.channels as usize)..]
                    .iter()
                {
                    residual_bits += *size as u32 * multiplier;
                }

                let residual_bytes = residual_bits.div_ceil(8);
                residual_bytes as usize
            } else {
                (frames_in_this_chunk * residual_size as usize * file_header.channels as usize)
                    .div_ceil(8)
            };

            if encoded_index + packed_residuals_bytes > encoded.len() {
                return Err(SeaError::InvalidFrame);
            }
            let packed_residuals = &encoded[encoded_index..encoded_index + packed_residuals_bytes];

            if matches!(chunk_type, SeaChunkType::Vbr) {
                vbr_bitlengths.clear();
                for vbr_chunk in vbr_residual_sizes.chunks_exact(file_header.channels as usize) {
                    for _ in 0..scale_factor_frames {
                        for item in vbr_chunk.iter().take(file_header.channels as usize) {
                            vbr_bitlengths.push(*item);
                        }
                    }
                }
                unpacker.reset_var(vbr_bitlengths);
            } else {
                unpacker.reset_const(residual_size as u8);
            }

            unpacker.process_bytes(packed_residuals);
            std::mem::swap(residuals, unpacker.take_output());
            residuals.resize(frames_in_this_chunk * file_header.channels as usize, 0);
        }

        Ok(ChunkInfo {
            frames_per_chunk: file_header.frames_per_chunk as usize,
            chunk_type,
            scale_factor_bits,
            scale_factor_frames,
            residual_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SeaChunk;
    use crate::sea::{
        codec::{
            bits::BitUnpacker,
            common::{SeaError, SeaResidualSize},
            file::{SeaFile, SeaFileHeader},
            lms::SeaLMS,
        },
        encoder::EncoderSettings,
    };

    fn synthetic_frame() -> [i16; 480] {
        let mut frame = [0i16; 480];
        for (i, sample) in frame.iter_mut().enumerate() {
            let a = ((i as i32 * 97) % 2000) - 1000;
            let b = ((i as i32 * 31) % 400) - 200;
            *sample = (a * 12 + b * 6) as i16;
        }
        frame
    }

    fn mean_abs_error(a: &[i16], b: &[i16]) -> u64 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
            .sum::<u64>()
            / a.len() as u64
    }

    #[test]
    fn cbr_chunk_encode_decode_within_error_bounds() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let settings = EncoderSettings::default();
        let input = synthetic_frame();

        let mut file = SeaFile::new(header, &settings).unwrap();
        let mut encoded = Vec::new();
        file.make_chunk(&input, &mut encoded).unwrap();

        let mut decoder_file = SeaFile::new_for_decoding(file.header.clone()).unwrap();
        let mut decoded = [0i16; 480];
        decoder_file
            .samples_from_frame(&encoded, &mut decoded)
            .unwrap();

        let mae = mean_abs_error(&input, &decoded);
        assert!(mae <= 2500, "mae too high: {mae}");
    }

    #[test]
    fn from_slice_rejects_malformed_chunk_headers() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 128,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };

        let invalid_type = [0xFF, 0x31, 20, 0x5A];
        assert!(matches!(
            parse_chunk(&invalid_type, &header),
            Err(SeaError::InvalidFrame)
        ));

        let invalid_scalefactor_bits = [0x01, 0x01, 20, 0x5A];
        assert!(matches!(
            parse_chunk(&invalid_scalefactor_bits, &header),
            Err(SeaError::InvalidFrame)
        ));
    }

    fn parse_chunk(encoded: &[u8], header: &SeaFileHeader) -> Result<(), SeaError> {
        let mut lms = Vec::<SeaLMS>::new();
        let mut scale_factors = Vec::new();
        let mut vbr_residual_sizes = Vec::new();
        let mut residuals = Vec::new();
        let mut vbr_bitlengths = Vec::new();
        let mut unpacker = BitUnpacker::new_const_bits(1);

        SeaChunk::parse_into(
            encoded,
            header,
            &mut lms,
            &mut scale_factors,
            &mut vbr_residual_sizes,
            &mut residuals,
            &mut vbr_bitlengths,
            &mut unpacker,
        )
        .map(|_| ())
    }

    #[test]
    fn residual_size_try_from_u8_boundaries() {
        assert_eq!(SeaResidualSize::try_from_u8(0), None);
        assert_eq!(SeaResidualSize::try_from_u8(1), Some(SeaResidualSize::One));
        assert_eq!(
            SeaResidualSize::try_from_u8(8),
            Some(SeaResidualSize::Eight)
        );
        assert_eq!(SeaResidualSize::try_from_u8(9), None);
    }
}
