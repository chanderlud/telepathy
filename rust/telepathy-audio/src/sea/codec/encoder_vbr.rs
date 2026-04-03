use crate::sea::{
    codec::{common::SeaResidualSize, lms::LMS_LEN},
    encoder::EncoderSettings,
};

use super::{
    common::{EncodedSamples, SeaEncoderTrait},
    encoder_base::EncoderBase,
    file::SeaFileHeader,
    lms::SeaLMS,
};

pub struct VbrEncoder {
    channels: usize,
    scale_factor_frames: u8,
    vbr_target_bitrate: f32,
    base_encoder: EncoderBase,
}

// const TARGET_RESIDUAL_DISTRIBUTION: [f32; 6] = [0.00, 0.09, 0.82, 0.07, 0.02, 0.00]; // ([0, target-1, target, target+1, target+2, 0])
const TARGET_RESIDUAL_DISTRIBUTION: [f32; 6] = [0.00, 0.00, 0.95, 0.05, 0.00, 0.00]; // TODO: it needs tuning

impl VbrEncoder {
    pub fn new(file_header: &SeaFileHeader, encoder_settings: &EncoderSettings) -> Self {
        VbrEncoder {
            channels: file_header.channels as usize,
            scale_factor_frames: encoder_settings.scale_factor_frames,
            base_encoder: EncoderBase::new(
                file_header.channels as usize,
                encoder_settings.scale_factor_bits as usize,
            ),
            vbr_target_bitrate: Self::get_normalized_vbr_bitrate(encoder_settings),
        }
    }

    pub fn get_lms(&self) -> &Vec<SeaLMS> {
        &self.base_encoder.lms
    }

    fn get_normalized_vbr_bitrate(encoder_settings: &EncoderSettings) -> f32 {
        let mut vbr_bitrate = encoder_settings.residual_bits;

        // compensate lms
        vbr_bitrate -= (LMS_LEN as f32 * 16.0 * 2.0) / encoder_settings.frames_per_chunk as f32;

        // compensate scale factor data
        vbr_bitrate -=
            encoder_settings.scale_factor_bits as f32 / encoder_settings.scale_factor_frames as f32;

        // compensate vbr data
        vbr_bitrate -= 2.0 / encoder_settings.scale_factor_frames as f32;

        // compensate with target distribution
        let base_residuals = libm::floorf(encoder_settings.residual_bits);
        let new_bitrate = TARGET_RESIDUAL_DISTRIBUTION[1] * (base_residuals - 1.0)
            + TARGET_RESIDUAL_DISTRIBUTION[2] * base_residuals
            + TARGET_RESIDUAL_DISTRIBUTION[3] * (base_residuals + 1.0)
            + TARGET_RESIDUAL_DISTRIBUTION[4] * (base_residuals + 2.0);
        let diff = new_bitrate - base_residuals;
        vbr_bitrate -= diff;

        vbr_bitrate
    }

    // returns items count [target-1, target, target+1, target+2]
    fn interpolate_distribution(items: usize, target_rate: f32) -> [usize; 4] {
        let (frac, _) = libm::modff(target_rate);
        let om_frac = 1.0 - frac;

        let mut percentages = [0f32; 4];
        for i in 0..4 {
            percentages[i] = TARGET_RESIDUAL_DISTRIBUTION[i] * frac
                + TARGET_RESIDUAL_DISTRIBUTION[i + 1] * om_frac;
        }

        let mut res = [0usize; 4];
        let mut sum = 0usize;

        // distribute remaining using TARGET_RESIDUAL_DISTRIBUTION
        while sum < items {
            let remaining = items - sum;
            for i in 0..4 {
                let value = (remaining as f32 * percentages[i]) as usize;
                sum += value;
                res[i] += value;
            }

            // if remaining is not enough to distribute based on TARGET_RESIDUAL_DISTRIBUTION
            if items - sum == remaining {
                sum += remaining;
                res[1] += remaining
            }
        }

        res
    }

    fn choose_residual_len_from_errors(&self, input_len: usize, errors: &[u64]) -> Vec<u8> {
        // we need to ensure that last partial frames are not touched (it would debalance the frame size)
        let sortable_items = input_len / self.scale_factor_frames as usize;

        let mut indices: Vec<u16> = (0..sortable_items as u16).collect();
        indices.sort_unstable_by(|&a, &b| errors[a as usize].cmp(&errors[b as usize]));

        let [minus_one_items, _, plus_one_items, plus_two_items] =
            Self::interpolate_distribution(sortable_items, self.vbr_target_bitrate);

        let base_residual_bits = self.vbr_target_bitrate as u8;

        let mut residual_sizes = vec![base_residual_bits; errors.len()];

        for index in indices.iter().take(minus_one_items) {
            residual_sizes[*index as usize] = base_residual_bits - 1;
        }

        for index in indices[(sortable_items - plus_two_items - plus_one_items)..]
            .iter()
            .take(plus_one_items)
        {
            residual_sizes[*index as usize] = base_residual_bits + 1;
        }

        for index in indices[sortable_items - plus_two_items..]
            .iter()
            .take(plus_two_items)
        {
            residual_sizes[*index as usize] = base_residual_bits + 2;
        }

        // count how many times each residual size appears
        let mut residual_size_counts = [0; 9];
        for i in 0..errors.len() {
            residual_size_counts[residual_sizes[i] as usize] += 1;
        }

        residual_sizes
    }

    fn analyze(&mut self, input_slice: &[i16]) -> Vec<u8> {
        let analyze_residual_size = SeaResidualSize::from(self.vbr_target_bitrate as u8 + 1);

        let slice_size = self.scale_factor_frames as usize * self.channels;

        let original_lms = self.base_encoder.lms.clone();

        let residual_sizes = vec![analyze_residual_size; self.channels];

        let mut scale_factors = vec![0u8; slice_size];
        let mut residuals: Vec<u8> = vec![0u8; slice_size];

        let mut errors = vec![
            0u64;
            (input_slice.len() / self.channels)
                .div_ceil(self.scale_factor_frames as usize)
                * self.channels
        ];

        for (slice_index, input_slice) in input_slice.chunks(slice_size).enumerate() {
            self.base_encoder.get_residuals_for_chunk(
                input_slice,
                &residual_sizes,
                &mut scale_factors,
                &mut residuals,
                &mut errors[slice_index * self.channels..],
            );
        }

        self.base_encoder.lms = original_lms;

        self.choose_residual_len_from_errors(input_slice.len(), &errors)
    }
}

impl SeaEncoderTrait for VbrEncoder {
    fn encode(&mut self, samples: &[i16]) -> EncodedSamples {
        let mut scale_factors = vec![
            0u8;
            (samples.len() / self.channels)
                .div_ceil(self.scale_factor_frames as usize)
                * self.channels
        ];

        let mut residuals: Vec<u8> = vec![0u8; samples.len()];

        let residual_bits: Vec<u8> = self.analyze(samples);

        let slice_size = self.scale_factor_frames as usize * self.channels;

        let mut residual_sizes = vec![SeaResidualSize::from(2); self.channels];

        let mut ranks = vec![0u64; self.channels];

        for (slice_index, input_slice) in samples.chunks(slice_size).enumerate() {
            for channel_offset in 0..self.channels {
                residual_sizes[channel_offset] = SeaResidualSize::from(
                    residual_bits[slice_index * self.channels + channel_offset],
                );
            }

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
            residual_bits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VbrEncoder;
    use crate::sea::codec::{
        common::SeaEncoderTrait,
        file::{SeaFile, SeaFileHeader},
    };
    use crate::sea::encoder::EncoderSettings;

    fn synthetic_frame() -> [i16; 480] {
        let mut frame = [0i16; 480];
        for (i, sample) in frame.iter_mut().enumerate() {
            let a = ((i as i32 * 73) % 2200) - 1100;
            let b = ((i as i32 * 19) % 500) - 250;
            *sample = (a * 10 + b * 8) as i16;
        }
        frame
    }

    fn vbr_settings() -> EncoderSettings {
        EncoderSettings {
            vbr: true,
            ..EncoderSettings::default()
        }
    }

    #[test]
    fn vbr_encoder_emits_residual_size_stream() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let settings = vbr_settings();
        let mut encoder = VbrEncoder::new(&header, &settings);
        let input = synthetic_frame();

        let encoded = encoder.encode(&input);
        assert_eq!(encoded.residuals.len(), input.len());
        assert_eq!(
            encoded.scale_factors.len(),
            (input.len() / header.channels as usize)
                .div_ceil(settings.scale_factor_frames as usize)
                * header.channels as usize
        );
        assert_eq!(encoded.residual_bits.len(), encoded.scale_factors.len());
    }

    #[test]
    fn vbr_end_to_end_frame_round_trip() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let settings = vbr_settings();
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
