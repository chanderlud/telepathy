use crate::sea::{
    codec::{common::SeaResidualSize, lms::LMS_LEN},
    encoder::EncoderSettings,
};

use super::{common::SeaEncoderTrait, encoder_base::EncoderBase, file::SeaFileHeader, lms::SeaLMS};

pub struct VbrEncoder {
    channels: usize,
    scale_factor_frames: u8,
    vbr_target_bitrate: f32,
    base_encoder: EncoderBase,
    scratch_ranks: Vec<u64>,
    scratch_residual_sizes: Vec<SeaResidualSize>,
    scratch_errors: Vec<u64>,
    scratch_indices: Vec<u16>,
    scratch_lms_backup: Vec<SeaLMS>,
    scratch_analyze_scale_factors: Vec<u8>,
    scratch_analyze_residuals: Vec<u8>,
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
            scratch_ranks: Vec::new(),
            scratch_residual_sizes: Vec::new(),
            scratch_errors: Vec::new(),
            scratch_indices: Vec::new(),
            scratch_lms_backup: Vec::new(),
            scratch_analyze_scale_factors: Vec::new(),
            scratch_analyze_residuals: Vec::new(),
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
        let mut assigned = 0usize;
        for i in 0..4 {
            res[i] = libm::floorf(items as f32 * percentages[i]) as usize;
            assigned += res[i];
        }
        res[1] += items - assigned;

        res
    }

    fn choose_residual_len_from_errors(
        input_len: usize,
        scale_factor_frames: u8,
        vbr_target_bitrate: f32,
        errors: &[u64],
        scratch_indices: &mut Vec<u16>,
        residual_sizes: &mut Vec<u8>,
    ) {
        // we need to ensure that last partial frames are not touched (it would debalance the frame size)
        let sortable_items = input_len / scale_factor_frames as usize;

        scratch_indices.clear();
        scratch_indices.extend(0..sortable_items as u16);
        scratch_indices.sort_unstable_by(|&a, &b| errors[a as usize].cmp(&errors[b as usize]));

        let [minus_one_items, _, plus_one_items, plus_two_items] =
            Self::interpolate_distribution(sortable_items, vbr_target_bitrate);

        let base_residual_bits = vbr_target_bitrate as u8;

        residual_sizes.resize(errors.len(), 0);
        residual_sizes.fill(base_residual_bits);

        for index in scratch_indices.iter().take(minus_one_items) {
            residual_sizes[*index as usize] = base_residual_bits - 1;
        }

        for index in scratch_indices[(sortable_items - plus_two_items - plus_one_items)..]
            .iter()
            .take(plus_one_items)
        {
            residual_sizes[*index as usize] = base_residual_bits + 1;
        }

        for index in scratch_indices[sortable_items - plus_two_items..]
            .iter()
            .take(plus_two_items)
        {
            residual_sizes[*index as usize] = base_residual_bits + 2;
        }
    }

    fn analyze(&mut self, input_slice: &[i16], residual_bits: &mut Vec<u8>) {
        let analyze_residual_size = SeaResidualSize::from(self.vbr_target_bitrate as u8);

        let slice_size = self.scale_factor_frames as usize * self.channels;

        // Backup LMS
        self.scratch_lms_backup.clone_from(&self.base_encoder.lms);

        self.scratch_residual_sizes
            .resize(self.channels, analyze_residual_size);
        self.scratch_residual_sizes.fill(analyze_residual_size);

        self.scratch_analyze_scale_factors.resize(slice_size, 0);
        self.scratch_analyze_residuals.resize(slice_size, 0);

        let errors_len = (input_slice.len() / self.channels)
            .div_ceil(self.scale_factor_frames as usize)
            * self.channels;
        self.scratch_errors.resize(errors_len, 0);

        for (slice_index, input_slice_chunk) in input_slice.chunks(slice_size).enumerate() {
            self.base_encoder.get_residuals_for_chunk_fast(
                input_slice_chunk,
                &self.scratch_residual_sizes,
                &mut self.scratch_analyze_scale_factors,
                &mut self.scratch_analyze_residuals,
                &mut self.scratch_errors[slice_index * self.channels..],
            );
        }

        // Restore LMS
        self.base_encoder.lms.clone_from(&self.scratch_lms_backup);

        // Call choose_residual_len_from_errors with scratch_errors directly
        // Restructured to avoid clone by passing needed fields as parameters
        Self::choose_residual_len_from_errors(
            input_slice.len(),
            self.scale_factor_frames,
            self.vbr_target_bitrate,
            &self.scratch_errors,
            &mut self.scratch_indices,
            residual_bits,
        );
    }
}

impl SeaEncoderTrait for VbrEncoder {
    fn encode_into(
        &mut self,
        samples: &[i16],
        scale_factors: &mut Vec<u8>,
        residuals: &mut Vec<u8>,
        residual_bits: &mut Vec<u8>,
    ) {
        let scale_factors_len = (samples.len() / self.channels)
            .div_ceil(self.scale_factor_frames as usize)
            * self.channels;
        scale_factors.resize(scale_factors_len, 0);
        residuals.resize(samples.len(), 0);

        self.analyze(samples, residual_bits);

        let slice_size = self.scale_factor_frames as usize * self.channels;

        self.scratch_residual_sizes
            .resize(self.channels, SeaResidualSize::from(2));
        self.scratch_ranks.resize(self.channels, 0);

        for (slice_index, input_slice) in samples.chunks(slice_size).enumerate() {
            for channel_offset in 0..self.channels {
                self.scratch_residual_sizes[channel_offset] = SeaResidualSize::from(
                    residual_bits[slice_index * self.channels + channel_offset],
                );
            }

            self.base_encoder.get_residuals_for_chunk(
                input_slice,
                &self.scratch_residual_sizes,
                &mut scale_factors[slice_index * self.channels..],
                &mut residuals[slice_index * slice_size..],
                &mut self.scratch_ranks,
            );
        }
    }
}
