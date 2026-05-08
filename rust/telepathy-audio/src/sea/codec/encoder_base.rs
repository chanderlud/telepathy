use super::{
    common::{SeaResidualSize, clamp_i16},
    dqt::SeaDequantTab,
    lms::SeaLMS,
    qt::SeaQuantTab,
};

pub struct EncoderBase {
    channels: usize,
    scale_factor_bits: usize,

    current_residuals: Vec<u8>,
    prev_scalefactor: Vec<i32>,
    best_residual_bits: Vec<u8>,
    dequant_tab: SeaDequantTab,
    quant_tab: SeaQuantTab,
    pub lms: Vec<SeaLMS>,
}

#[inline(always)]
pub fn sea_div(v: i32, scalefactor_reciprocal: i64) -> i32 {
    let n = (v as i64 * scalefactor_reciprocal + (1 << 15)) >> 16;
    (n + (v.signum() as i64 - n.signum())) as i32
}

#[allow(clippy::too_many_arguments)]
fn calculate_residuals(
    channels: usize,
    dequant_tab: &[i32],
    dqt_stride: usize,
    samples: &[i16],
    scalefactor: i32,
    lms: &mut SeaLMS,
    best_rank: u64,
    residual_size: SeaResidualSize,
    scalefactor_reciprocals: &[i32],
    skip_penalty: bool,
    quant_tab: &SeaQuantTab,
    current_residuals: &mut [u8],
) -> u64 {
    let mut current_rank: u64 = 0;

    let clamp_limit = residual_size.to_binary_combinations() as i32;
    let quant_tab_offset = clamp_limit + quant_tab.offsets[residual_size as usize] as i32;

    for (index, sample_i16) in samples.iter().step_by(channels).enumerate() {
        let sample = *sample_i16 as i32;
        let predicted = lms.predict();
        let residual = sample - predicted;
        let scaled = sea_div(
            residual,
            scalefactor_reciprocals[scalefactor as usize] as i64,
        );
        let clamped = scaled.clamp(-clamp_limit, clamp_limit);
        let quantized = quant_tab.quant_tab[(quant_tab_offset + clamped) as usize];

        let dequantized = dequant_tab[scalefactor as usize * dqt_stride + quantized as usize];
        let reconstructed = clamp_i16(predicted + dequantized);

        let error: i64 = sample as i64 - reconstructed as i64;
        let error_sq = error.pow(2) as u64;

        current_rank += if skip_penalty {
            error_sq
        } else {
            error_sq + lms.get_weights_penalty()
        };
        if current_rank > best_rank {
            break;
        }

        lms.update(reconstructed, dequantized);
        current_residuals[index] = quantized;
    }

    current_rank
}

#[allow(clippy::too_many_arguments)]
fn get_residuals_with_scalefactor_limit(
    scale_factor_bits: usize,
    channels: usize,
    dequant_tab: &[i32],
    dqt_stride: usize,
    scalefactor_reciprocals: &[i32],
    samples: &[i16],
    prev_scalefactor: i32,
    ref_lms: &SeaLMS,
    residual_size: SeaResidualSize,
    max_candidates: usize,
    skip_penalty: bool,
    quant_tab: &SeaQuantTab,
    best_residual_bits: &mut [u8],
    current_residuals: &mut [u8],
) -> (u64, SeaLMS, i32) {
    let mut best_rank: u64 = u64::MAX;

    let mut best_lms = SeaLMS::new();
    let mut best_scalefactor: i32 = 0;

    let mut current_lms: SeaLMS = ref_lms.clone();

    let scalefactor_end = 1_i32 << scale_factor_bits;
    let candidate_count = max_candidates.min(scalefactor_end as usize);

    for sfi in 0..candidate_count {
        let scalefactor = ((sfi as i32) + prev_scalefactor) % scalefactor_end;
        if sfi != 0 {
            current_lms.clone_from(ref_lms);
        }

        let current_rank = calculate_residuals(
            channels,
            dequant_tab,
            dqt_stride,
            samples,
            scalefactor,
            &mut current_lms,
            best_rank,
            residual_size,
            scalefactor_reciprocals,
            skip_penalty,
            quant_tab,
            current_residuals,
        );

        if current_rank < best_rank {
            best_rank = current_rank;
            best_residual_bits[..current_residuals.len()].clone_from_slice(current_residuals);
            best_lms.clone_from(&current_lms);
            best_scalefactor = scalefactor;
        }
    }

    (best_rank, best_lms, best_scalefactor)
}

#[allow(clippy::too_many_arguments)]
fn get_residuals_with_best_scalefactor(
    scale_factor_bits: usize,
    channels: usize,
    dequant_tab: &[i32],
    dqt_stride: usize,
    scalefactor_reciprocals: &[i32],
    samples: &[i16],
    prev_scalefactor: i32,
    ref_lms: &SeaLMS,
    residual_size: SeaResidualSize,
    quant_tab: &SeaQuantTab,
    best_residual_bits: &mut [u8],
    current_residuals: &mut [u8],
) -> (u64, SeaLMS, i32) {
    get_residuals_with_scalefactor_limit(
        scale_factor_bits,
        channels,
        dequant_tab,
        dqt_stride,
        scalefactor_reciprocals,
        samples,
        prev_scalefactor,
        ref_lms,
        residual_size,
        1usize << scale_factor_bits,
        false,
        quant_tab,
        best_residual_bits,
        current_residuals,
    )
}

#[inline(always)]
fn interleave_channel(channels: usize, channel_offset: usize, src: &[u8], dst: &mut [u8]) {
    if channels == 1 {
        dst[..src.len()].copy_from_slice(src);
        return;
    }

    for i in 0..src.len() {
        dst[i * channels + channel_offset] = src[i];
    }
}

impl EncoderBase {
    pub fn new(channels: usize, scale_factor_bits: usize) -> Self {
        Self {
            channels,
            scale_factor_bits,

            current_residuals: Vec::new(),
            prev_scalefactor: vec![0; channels],
            best_residual_bits: Vec::new(),
            dequant_tab: SeaDequantTab::init(scale_factor_bits),
            quant_tab: SeaQuantTab::init(),
            lms: SeaLMS::init_vec(channels as u32),
        }
    }

    pub fn get_residuals_for_chunk(
        &mut self,
        samples: &[i16],
        residual_size: &[SeaResidualSize],
        scale_factors: &mut [u8],
        residuals: &mut [u8],
        ranks: &mut [u64],
    ) {
        self.best_residual_bits
            .resize(samples.len() / self.channels, 0);
        self.current_residuals
            .resize(self.best_residual_bits.len(), 0);

        for channel_offset in 0..self.channels {
            let (dqt, dqt_stride) = self
                .dequant_tab
                .get_dqt(residual_size[channel_offset] as usize);

            let scalefactor_reciprocals = self
                .dequant_tab
                .get_scalefactor_reciprocals(residual_size[channel_offset] as usize);

            let (best_rank, best_lms, best_scalefactor) = get_residuals_with_best_scalefactor(
                self.scale_factor_bits,
                self.channels,
                dqt,
                dqt_stride,
                scalefactor_reciprocals,
                &samples[channel_offset..],
                self.prev_scalefactor[channel_offset],
                &self.lms[channel_offset],
                residual_size[channel_offset],
                &self.quant_tab,
                &mut self.best_residual_bits,
                &mut self.current_residuals,
            );

            self.prev_scalefactor[channel_offset] = best_scalefactor;
            self.lms[channel_offset] = best_lms;

            scale_factors[channel_offset] = best_scalefactor as u8;
            ranks[channel_offset] = best_rank;

            interleave_channel(
                self.channels,
                channel_offset,
                &self.best_residual_bits,
                residuals,
            );
        }
    }

    pub fn get_residuals_for_chunk_fast(
        &mut self,
        samples: &[i16],
        residual_size: &[SeaResidualSize],
        scale_factors: &mut [u8],
        residuals: &mut [u8],
        ranks: &mut [u64],
    ) {
        self.best_residual_bits
            .resize(samples.len() / self.channels, 0);
        self.current_residuals
            .resize(self.best_residual_bits.len(), 0);

        for channel_offset in 0..self.channels {
            let (dqt, dqt_stride) = self
                .dequant_tab
                .get_dqt(residual_size[channel_offset] as usize);

            let scalefactor_reciprocals = self
                .dequant_tab
                .get_scalefactor_reciprocals(residual_size[channel_offset] as usize);

            let (best_rank, best_lms, best_scalefactor) = get_residuals_with_scalefactor_limit(
                self.scale_factor_bits,
                self.channels,
                dqt,
                dqt_stride,
                scalefactor_reciprocals,
                &samples[channel_offset..],
                self.prev_scalefactor[channel_offset],
                &self.lms[channel_offset],
                residual_size[channel_offset],
                2,
                true,
                &self.quant_tab,
                &mut self.best_residual_bits,
                &mut self.current_residuals,
            );

            self.prev_scalefactor[channel_offset] = best_scalefactor;
            self.lms[channel_offset] = best_lms;

            scale_factors[channel_offset] = best_scalefactor as u8;
            ranks[channel_offset] = best_rank;

            interleave_channel(
                self.channels,
                channel_offset,
                &self.best_residual_bits,
                residuals,
            );
        }
    }
}
