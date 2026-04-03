use super::{chunk::ChunkInfo, common::clamp_i16, dqt::SeaDequantTab, lms::SeaLMS};

pub struct Decoder {
    channels: usize,
    scale_factor_bits: usize,

    dequant_tab: SeaDequantTab,
    scratch_lms: Vec<SeaLMS>,
}

impl Decoder {
    pub fn init(channels: usize, scale_factor_bits: usize) -> Self {
        Self {
            channels,
            scale_factor_bits,

            dequant_tab: SeaDequantTab::init(scale_factor_bits),
            scratch_lms: Vec::new(),
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
        assert_eq!(
            chunk_info.scale_factor_bits as usize,
            self.scale_factor_bits
        );

        let expected_len = chunk_info.frames_per_chunk * self.channels;
        assert!(output.len() >= expected_len);

        // Clone LMS into scratch buffer
        self.scratch_lms.clear();
        self.scratch_lms.extend(lms.iter().cloned());

        let dqts: &Vec<Vec<i32>> = self.dequant_tab.get_dqt(chunk_info.residual_size as usize);

        let mut output_index = 0;
        for (scale_factor_index, subchunk_residuals) in residuals
            .chunks(self.channels * chunk_info.scale_factor_frames as usize)
            .enumerate()
        {
            let scale_factors_slice = &scale_factors[scale_factor_index * self.channels..];

            for channel_residuals in subchunk_residuals.chunks(self.channels) {
                for (channel_index, residual) in channel_residuals.iter().enumerate() {
                    let scale_factor = scale_factors_slice[channel_index];
                    let predicted = self.scratch_lms[channel_index].predict();
                    let quantized: usize = *residual as usize;
                    let dequantized = dqts[scale_factor as usize][quantized];
                    let reconstructed = clamp_i16(predicted + dequantized);
                    output[output_index] = reconstructed;
                    output_index += 1;
                    self.scratch_lms[channel_index].update(reconstructed, dequantized);
                }
            }
        }

        // Update caller's LMS
        for (i, lms_item) in self.scratch_lms.iter().enumerate() {
            lms[i] = lms_item.clone();
        }
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
        assert_eq!(
            chunk_info.scale_factor_bits as usize,
            self.scale_factor_bits
        );

        let expected_len = chunk_info.frames_per_chunk * self.channels;
        assert!(output.len() >= expected_len);

        // Clone LMS into scratch buffer
        self.scratch_lms.clear();
        self.scratch_lms.extend(lms.iter().cloned());

        let mut output_index = 0;
        for (scale_factor_index, subchunk_residuals) in residuals
            .chunks(self.channels * chunk_info.scale_factor_frames as usize)
            .enumerate()
        {
            let scale_factors_slice = &scale_factors[scale_factor_index * self.channels..];
            let vbr_residuals = &vbr_residual_sizes[scale_factor_index * self.channels..];

            for channel_residuals in subchunk_residuals.chunks(self.channels) {
                for (channel_index, residual) in channel_residuals.iter().enumerate() {
                    let residual_size: usize = vbr_residuals[channel_index] as usize;
                    let scale_factor = scale_factors_slice[channel_index];
                    let predicted = self.scratch_lms[channel_index].predict();
                    let quantized: usize = *residual as usize;
                    // Get DQT inline to avoid cloning
                    let dqts: &Vec<Vec<i32>> = self.dequant_tab.get_dqt(residual_size);
                    let dequantized = dqts[scale_factor as usize][quantized];
                    let reconstructed = clamp_i16(predicted + dequantized);
                    output[output_index] = reconstructed;
                    output_index += 1;
                    self.scratch_lms[channel_index].update(reconstructed, dequantized);
                }
            }
        }

        // Update caller's LMS
        for (i, lms_item) in self.scratch_lms.iter().enumerate() {
            lms[i] = lms_item.clone();
        }
    }
}
