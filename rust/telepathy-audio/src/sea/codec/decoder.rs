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
                        Some(vbr_residuals) => {
                            self.dequant_tab.get_dqt(vbr_residuals[channel_index] as usize)
                        }
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
