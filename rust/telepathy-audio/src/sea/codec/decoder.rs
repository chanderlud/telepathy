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
            bits::BitUnpacker,
            chunk::SeaChunk,
            common::{SEAC_MAGIC, SeaError, SeaResidualSize},
            file::{SeaFile, SeaFileHeader},
            lms::SeaLMS,
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
        let mut encoded = Vec::new();
        file.make_chunk(&input, &mut encoded).unwrap();
        let (chunk_info, mut lms, scale_factors, residuals, _) =
            parse_chunk(&encoded, &file.header).unwrap();

        let mut decoder = Decoder::init(1, chunk_info.scale_factor_bits as usize);
        let mut decoded = [0i16; 480];
        decoder.decode_cbr_into(
            &chunk_info,
            &mut lms,
            &scale_factors,
            &residuals,
            &mut decoded,
        );
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
        let mut encoded = Vec::new();
        file.make_chunk(&input, &mut encoded).unwrap();
        let (chunk_info, mut lms, scale_factors, residuals, vbr_residual_sizes) =
            parse_chunk(&encoded, &file.header).unwrap();

        let mut decoder = Decoder::init(1, chunk_info.scale_factor_bits as usize);
        let mut decoded = [0i16; 480];
        decoder.decode_vbr_into(
            &chunk_info,
            &mut lms,
            &scale_factors,
            &vbr_residual_sizes,
            &residuals,
            &mut decoded,
        );
        assert!(mean_abs_error(&input, &decoded) <= 2500);
    }

    #[test]
    fn sea_error_variants_and_residual_size_boundaries() {
        assert!(matches!(
            parse_chunk(&[1, 0, 0], &header()),
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

    type ParsedChunk = (super::ChunkInfo, Vec<SeaLMS>, Vec<u8>, Vec<u8>, Vec<u8>);

    fn parse_chunk(encoded: &[u8], header: &SeaFileHeader) -> Result<ParsedChunk, SeaError> {
        let mut lms = Vec::<SeaLMS>::new();
        let mut scale_factors = Vec::new();
        let mut vbr_residual_sizes = Vec::new();
        let mut residuals = Vec::new();
        let mut vbr_bitlengths = Vec::new();
        let mut unpacker = BitUnpacker::new_const_bits(1);

        let chunk_info = SeaChunk::parse_into(
            encoded,
            header,
            &mut lms,
            &mut scale_factors,
            &mut vbr_residual_sizes,
            &mut residuals,
            &mut vbr_bitlengths,
            &mut unpacker,
        )?;

        Ok((
            chunk_info,
            lms,
            scale_factors,
            residuals,
            vbr_residual_sizes,
        ))
    }
}
