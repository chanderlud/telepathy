use crate::sea::encoder::EncoderSettings;

use super::{
    common::{SeaEncoderTrait, SeaResidualSize},
    encoder_base::EncoderBase,
    file::SeaFileHeader,
    lms::SeaLMS,
};

pub struct CbrEncoder {
    channels: usize,
    residual_size: SeaResidualSize,
    scale_factor_frames: usize,
    base_encoder: EncoderBase,
    scratch_ranks: Vec<u64>,
    scratch_residual_sizes: Vec<SeaResidualSize>,
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
            scratch_ranks: Vec::new(),
            scratch_residual_sizes: Vec::new(),
        }
    }

    pub fn get_lms(&self) -> &Vec<SeaLMS> {
        &self.base_encoder.lms
    }
}

impl SeaEncoderTrait for CbrEncoder {
    fn encode_into(
        &mut self,
        samples: &[i16],
        scale_factors: &mut Vec<u8>,
        residuals: &mut Vec<u8>,
        residual_bits: &mut Vec<u8>,
    ) {
        let scale_factors_len =
            (samples.len() / self.channels).div_ceil(self.scale_factor_frames) * self.channels;
        scale_factors.resize(scale_factors_len, 0);
        residuals.resize(samples.len(), 0);
        self.scratch_ranks.resize(self.channels, 0);
        self.scratch_residual_sizes
            .resize(self.channels, self.residual_size);

        let slice_size = self.scale_factor_frames * self.channels;

        for (slice_index, input_slice) in samples.chunks(slice_size).enumerate() {
            self.base_encoder.get_residuals_for_chunk(
                input_slice,
                &self.scratch_residual_sizes,
                &mut scale_factors[slice_index * self.channels..],
                &mut residuals[slice_index * slice_size..],
                &mut self.scratch_ranks,
            );
        }

        residual_bits.clear();
    }
}
