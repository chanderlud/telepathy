use crate::sea::encoder::EncoderSettings;

use super::{
    common::{EncodedSamples, SeaEncoderTrait, SeaResidualSize},
    encoder_base::EncoderBase,
    file::SeaFileHeader,
    lms::SeaLMS,
};

pub struct CbrEncoder {
    channels: usize,
    residual_size: SeaResidualSize,
    scale_factor_frames: usize,
    base_encoder: EncoderBase,
}

impl CbrEncoder {
    pub fn new(file_header: &SeaFileHeader, encoder_settings: &EncoderSettings) -> Self {
        CbrEncoder {
            channels: file_header.channels as usize,
            residual_size: SeaResidualSize::from(libm::floorf(encoder_settings.residual_bits) as u8),
            scale_factor_frames: encoder_settings.scale_factor_frames as usize,
            base_encoder: EncoderBase::new(
                file_header.channels as usize,
                encoder_settings.scale_factor_bits as usize,
            ),
        }
    }

    pub fn get_lms(&self) -> &Vec<SeaLMS> {
        &self.base_encoder.lms
    }
}

impl SeaEncoderTrait for CbrEncoder {
    fn encode(&mut self, samples: &[i16]) -> EncodedSamples {
        let mut scale_factors =
            vec![
                0u8;
                (samples.len() / self.channels).div_ceil(self.scale_factor_frames) * self.channels
            ];

        let mut residuals: Vec<u8> = vec![0u8; samples.len()];

        let mut ranks = vec![0u64; self.channels];

        let slice_size = self.scale_factor_frames * self.channels;

        let residual_sizes = vec![self.residual_size; self.channels];

        for (slice_index, input_slice) in samples.chunks(slice_size).enumerate() {
            self.base_encoder.get_residuals_for_chunk(
                input_slice,
                &residual_sizes,
                &mut scale_factors[slice_index * self.channels..],
                &mut residuals[slice_index * slice_size..],
                &mut ranks,
            );
        }

        EncodedSamples {
            scale_factors,
            residuals,
            residual_bits: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CbrEncoder;
    use crate::sea::codec::{
        common::SeaEncoderTrait,
        file::{SeaFile, SeaFileHeader},
    };
    use crate::sea::encoder::EncoderSettings;

    fn synthetic_frame() -> [i16; 480] {
        let mut frame = [0i16; 480];
        for (i, sample) in frame.iter_mut().enumerate() {
            let a = ((i as i32 * 61) % 1800) - 900;
            let b = ((i as i32 * 17) % 300) - 150;
            *sample = (a * 14 + b * 7) as i16;
        }
        frame
    }

    #[test]
    fn cbr_encoder_emits_expected_shapes() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let settings = EncoderSettings::default();
        let mut encoder = CbrEncoder::new(&header, &settings);
        let input = synthetic_frame();

        let encoded = encoder.encode(&input);
        assert_eq!(encoded.residuals.len(), input.len());
        assert_eq!(encoded.residual_bits.len(), 0);
        assert_eq!(
            encoded.scale_factors.len(),
            (input.len() / header.channels as usize)
                .div_ceil(settings.scale_factor_frames as usize)
                * header.channels as usize
        );
    }

    #[test]
    fn cbr_end_to_end_frame_round_trip() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let settings = EncoderSettings::default();
        let input = synthetic_frame();

        let mut encoder_file = SeaFile::new(header, &settings).unwrap();
        let encoded_frame = encoder_file.make_chunk(&input).unwrap();

        let mut decoder_file = SeaFile {
            header: encoder_file.header.clone(),
            decoder: None,
            encoder: None,
            encoder_settings: None,
        };
        let mut decoded = [0i16; 480];
        decoder_file
            .samples_from_frame(&encoded_frame, &mut decoded)
            .unwrap();

        let mae = input
            .iter()
            .zip(decoded.iter())
            .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64)
            .sum::<u64>()
            / input.len() as u64;
        assert!(mae <= 2500, "mae too high: {mae}");
    }
}
