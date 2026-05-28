use super::{chunk::ChunkInfo, common::clamp_i16, dqt::SeaDequantTab, lms::SeaLMS};

pub struct Decoder {
    channels: usize,
    scale_factor_bits: usize,

    dequant_tab: SeaDequantTab,
}

impl Decoder {
    pub fn init(channels: usize, scale_factor_bits: usize) -> Self {
        Self {
            channels,
            scale_factor_bits,

            dequant_tab: SeaDequantTab::init(scale_factor_bits),
        }
    }

    fn decode_inner(
        &mut self,
        chunk_info: &ChunkInfo,
        lms: &mut [SeaLMS],
        scale_factors: &[u8],
        residuals: &[u8],
        vbr_residual_sizes: Option<&[u8]>,
        output: &mut [i16],
    ) {
        assert_eq!(
            chunk_info.scale_factor_bits as usize,
            self.scale_factor_bits
        );

        let expected_len = chunk_info.frames_per_chunk * self.channels;
        assert!(output.len() >= expected_len);

        let fixed_dqt = vbr_residual_sizes
            .is_none()
            .then(|| self.dequant_tab.get_dqt(chunk_info.residual_size as usize));

        let mut output_index = 0;
        for (scale_factor_index, subchunk_residuals) in residuals
            .chunks(self.channels * chunk_info.scale_factor_frames as usize)
            .enumerate()
        {
            let scale_factors_slice = &scale_factors[scale_factor_index * self.channels..];
            let vbr_residuals =
                vbr_residual_sizes.map(|sizes| &sizes[scale_factor_index * self.channels..]);

            for channel_residuals in subchunk_residuals.chunks(self.channels) {
                for (channel_index, residual) in channel_residuals.iter().enumerate() {
                    let scale_factor = scale_factors_slice[channel_index];
                    let predicted = lms[channel_index].predict();
                    let quantized = *residual as usize;
                    let (flat_dqt, stride) = match vbr_residuals {
                        Some(vbr_residuals) => self
                            .dequant_tab
                            .get_dqt(vbr_residuals[channel_index] as usize),
                        None => fixed_dqt.unwrap(),
                    };
                    let dequantized = flat_dqt[scale_factor as usize * stride + quantized];
                    let reconstructed = clamp_i16(predicted + dequantized);
                    output[output_index] = reconstructed;
                    output_index += 1;
                    lms[channel_index].update(reconstructed, dequantized);
                }
            }
        }
    }

    pub fn decode_cbr_into(
        &mut self,
        chunk_info: &ChunkInfo,
        lms: &mut [SeaLMS],
        scale_factors: &[u8],
        residuals: &[u8],
        output: &mut [i16],
    ) {
        self.decode_inner(chunk_info, lms, scale_factors, residuals, None, output);
    }

    pub fn decode_vbr_into(
        &mut self,
        chunk_info: &ChunkInfo,
        lms: &mut [SeaLMS],
        scale_factors: &[u8],
        vbr_residual_sizes: &[u8],
        residuals: &[u8],
        output: &mut [i16],
    ) {
        self.decode_inner(
            chunk_info,
            lms,
            scale_factors,
            residuals,
            Some(vbr_residual_sizes),
            output,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::Decoder;
    use crate::sea::{
        codec::{
            chunk::SeaChunk,
            common::{SEAC_MAGIC, SeaError, SeaResidualSize},
            file::{SeaFile, SeaFileHeader},
        },
        encoder::EncoderSettings,
    };

    fn synthetic_frame() -> [i16; 480] {
        let mut frame = [0i16; 480];
        for (i, sample) in frame.iter_mut().enumerate() {
            let a = ((i as i32 * 89) % 2100) - 1050;
            let b = ((i as i32 * 23) % 460) - 230;
            *sample = (a * 11 + b * 7) as i16;
        }
        frame
    }

    fn header() -> SeaFileHeader {
        SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        }
    }

    fn mean_abs_error(a: &[i16], b: &[i16]) -> u64 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
            .sum::<u64>()
            / a.len() as u64
    }

    #[test]
    fn decode_cbr_round_trip_is_within_error_bounds() {
        let input = synthetic_frame();
        let mut file = SeaFile::new(header(), &EncoderSettings::default()).unwrap();
        let encoded = file.make_chunk(&input).unwrap();
        let chunk = SeaChunk::from_slice(&encoded, &file.header).unwrap();

        let decoder = Decoder::init(1, chunk.scale_factor_bits as usize);
        let decoded = decoder.decode_cbr(&chunk);
        assert!(mean_abs_error(&input, &decoded) <= 2500);
    }

    #[test]
    fn decode_vbr_round_trip_is_within_error_bounds() {
        let input = synthetic_frame();
        let settings = EncoderSettings {
            vbr: true,
            ..EncoderSettings::default()
        };
        let mut file = SeaFile::new(header(), &settings).unwrap();
        let encoded = file.make_chunk(&input).unwrap();
        let chunk = SeaChunk::from_slice(&encoded, &file.header).unwrap();

        let decoder = Decoder::init(1, chunk.scale_factor_bits as usize);
        let decoded = decoder.decode_vbr(&chunk);
        assert!(mean_abs_error(&input, &decoded) <= 2500);
    }

    #[test]
    fn sea_error_variants_and_residual_size_boundaries() {
        assert!(matches!(
            SeaChunk::from_slice(&[1, 0, 0], &header()),
            Err(SeaError::InvalidFrame)
        ));

        assert!(matches!(
            SeaFileHeader::from_frame(&[]),
            Err(SeaError::InvalidFile)
        ));

        let mut unsupported = Vec::new();
        unsupported.extend_from_slice(&SEAC_MAGIC.to_be_bytes());
        unsupported.push(2); // unsupported version
        unsupported.push(1); // channels
        unsupported.extend_from_slice(&64u16.to_le_bytes());
        unsupported.extend_from_slice(&480u16.to_le_bytes());
        unsupported.extend_from_slice(&16_000u32.to_le_bytes());
        assert!(matches!(
            SeaFileHeader::from_frame(&unsupported),
            Err(SeaError::UnsupportedVersion)
        ));

        assert_eq!(SeaResidualSize::try_from_u8(0), None);
        assert_eq!(SeaResidualSize::try_from_u8(1), Some(SeaResidualSize::One));
        assert_eq!(
            SeaResidualSize::try_from_u8(8),
            Some(SeaResidualSize::Eight)
        );
        assert_eq!(SeaResidualSize::try_from_u8(9), None);
    }
}
