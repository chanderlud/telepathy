#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

const MAX_I16_F32: f32 = i16::MAX as f32;
const MIN_I16_F32: f32 = i16::MIN as f32;

/// mul with internal selection of optimal implementation
pub(crate) fn wide_mul(frame: &mut [f32], factor: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") && frame.len().is_multiple_of(16) {
            unsafe { avx512_mul(frame, factor) }
            return;
        }
        if is_x86_feature_detected!("avx2") && frame.len().is_multiple_of(8) {
            unsafe { avx2_mul(frame, factor) }
            return;
        }
    }

    scalar_mul(frame, factor);
}

/// Safety: input length must be a multiple of 16
pub(crate) fn wide_i16_to_f32(ints: &[i16], out: &mut [f32], scale: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { i16_to_f32_avx2(ints, out, scale) }
            return;
        }
    }

    i16_to_f32_scalar(ints, out, scale);
}

/// Safety: input length must be a multiple of 16
pub(crate) fn wide_float_scaler(floats: &mut [f32], scale: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            unsafe { avx512_float_scaler(floats, scale) }
            return;
        }
        if is_x86_feature_detected!("avx") {
            unsafe { avx_float_scaler(floats, scale) }
            return;
        }
    }

    scalar_float_scaler(floats, scale);
}

/// calculates the RMS of the frame (loop is unrolled for optimization)
pub(crate) fn calculate_rms(data: &[f32]) -> f32 {
    let mut sum1 = 0.0;
    let mut sum2 = 0.0;
    let mut sum3 = 0.0;
    let mut sum4 = 0.0;

    let mut i = 0;
    while i + 3 < data.len() {
        sum1 += data[i] * data[i];
        sum2 += data[i + 1] * data[i + 1];
        sum3 += data[i + 2] * data[i + 2];
        sum4 += data[i + 3] * data[i + 3];
        i += 4;
    }

    let mean_of_squares = (sum1 + sum2 + sum3 + sum4) / data.len() as f32;
    mean_of_squares.sqrt()
}

/// mul for any length input
fn scalar_mul(frame: &mut [f32], factor: f32) {
    for p in frame.iter_mut() {
        *p *= factor;
        *p = p.clamp(-1_f32, 1_f32);
    }
}

/// optimized mul for avx2 with 8|frame.len==true
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn avx2_mul(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = _mm256_set1_ps(factor);
    let min_vec = _mm256_set1_ps(-1_f32);
    let max_vec = _mm256_set1_ps(1_f32);

    while i + 8 <= len {
        let mut chunk = _mm256_loadu_ps(frame.as_ptr().add(i)); // load
        chunk = _mm256_mul_ps(chunk, factor_vec); // multiply
        chunk = _mm256_max_ps(min_vec, _mm256_min_ps(max_vec, chunk)); // clamp
        _mm256_storeu_ps(frame.as_mut_ptr().add(i), chunk); // write
        i += 8;
    }
}

/// optimized mul for avx512f with 16|frame.len==true
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn avx512_mul(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = _mm512_set1_ps(factor);
    let min_vec = _mm512_set1_ps(-1_f32);
    let max_vec = _mm512_set1_ps(1_f32);

    // process 16 floats per iteration
    while i + 16 <= len {
        let mut chunk = _mm512_loadu_ps(frame.as_ptr().add(i)); // load
        chunk = _mm512_mul_ps(chunk, factor_vec); // multiply
        chunk = _mm512_min_ps(max_vec, _mm512_max_ps(min_vec, chunk)); // clamp
        _mm512_storeu_ps(frame.as_mut_ptr().add(i), chunk); // write
        i += 16;
    }
}

/// scalar implementation of i16 to f32 conversion with scaling
fn i16_to_f32_scalar(ints: &[i16], out: &mut [f32], scale: f32) {
    for (out, &x) in out.iter_mut().zip(ints.iter()) {
        *out = (x as f32 * scale).clamp(-1_f32, 1_f32);
    }
}

/// avx2 implementation of i16 to f32 conversion with scaling
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn i16_to_f32_avx2(ints: &[i16], out: &mut [f32], scale: f32) {
    let n = ints.len();
    let mut i = 0;

    let scale_ps = _mm256_set1_ps(scale);
    let min_ps = _mm256_set1_ps(-1_f32);
    let max_ps = _mm256_set1_ps(1_f32);

    // process 16 i16 -> 16 f32 per loop
    while i + 16 <= n {
        // load 16 i16
        let v16 = _mm256_loadu_si256(ints.as_ptr().add(i) as *const __m256i);

        // lower 8 i16 -> 8 i32 -> 8 f32
        let lo16 = _mm256_castsi256_si128(v16);
        let lo32 = _mm256_cvtepi16_epi32(lo16);
        let mut lo_ps = _mm256_mul_ps(_mm256_cvtepi32_ps(lo32), scale_ps);
        lo_ps = _mm256_max_ps(_mm256_min_ps(lo_ps, max_ps), min_ps);

        // upper 8 i16 -> 8 i32 -> 8 f32
        let hi16 = _mm256_extracti128_si256::<1>(v16);
        let hi32 = _mm256_cvtepi16_epi32(hi16);
        let mut hi_ps = _mm256_mul_ps(_mm256_cvtepi32_ps(hi32), scale_ps);
        hi_ps = _mm256_max_ps(_mm256_min_ps(hi_ps, max_ps), min_ps);

        // store 16 f32
        _mm256_storeu_ps(out.as_mut_ptr().add(i), lo_ps);
        _mm256_storeu_ps(out.as_mut_ptr().add(i + 8), hi_ps);

        i += 16;
    }
}

fn scalar_float_scaler(floats: &mut [f32], scale: f32) {
    for x in floats.iter_mut() {
        *x *= scale;
        *x = x.trunc().clamp(MIN_I16_F32, MAX_I16_F32);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn avx_float_scaler(floats: &mut [f32], scale: f32) {
    let n = floats.len();
    let mut i = 0;

    let scale_v = _mm256_set1_ps(scale);
    let min_v = _mm256_set1_ps(MIN_I16_F32);
    let max_v = _mm256_set1_ps(MAX_I16_F32);

    // flags: truncate toward zero, avoid changing MXCSR exceptions
    const TOWARD_ZERO_NO_EXC: i32 = _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC;

    while i + 8 <= n {
        let mut v = _mm256_loadu_ps(floats.as_ptr().add(i));
        v = _mm256_mul_ps(v, scale_v);
        v = _mm256_round_ps(v, TOWARD_ZERO_NO_EXC);
        v = _mm256_max_ps(v, min_v);
        v = _mm256_min_ps(v, max_v);
        _mm256_storeu_ps(floats.as_mut_ptr().add(i), v);
        i += 8;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn avx512_float_scaler(floats: &mut [f32], scale: f32) {
    let n = floats.len();
    let mut i = 0;

    let scale_v = _mm512_set1_ps(scale);
    let min_v = _mm512_set1_ps(MIN_I16_F32);
    let max_v = _mm512_set1_ps(MAX_I16_F32);

    // flags: truncate toward zero, avoid changing MXCSR exceptions
    const TOWARD_ZERO_NO_EXC: i32 = _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC;

    // Process 16 floats per iteration
    while i + 16 <= n {
        // load
        let v = _mm512_loadu_ps(floats.as_ptr().add(i));
        // scale
        let v = _mm512_mul_ps(v, scale_v);
        // trunc toward zero by converting to i32 with rounding mode, then back to f32
        let vi = _mm512_cvt_roundps_epi32(v, TOWARD_ZERO_NO_EXC);
        let mut vf = _mm512_cvtepi32_ps(vi);
        // clamp
        vf = _mm512_max_ps(vf, min_v);
        vf = _mm512_min_ps(vf, max_v);
        // store
        _mm512_storeu_ps(floats.as_mut_ptr().add(i), vf);

        i += 16;
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(unused_imports)]

    use nnnoiseless::FRAME_SIZE;

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn MulVariants_DummyFrame_EqualOutputs() {
        let frame = crate::telepathy::tests::dummy_frame();
        let mut scalar_frame = frame.clone();
        let mut wide_frame = frame.clone();

        super::scalar_mul(&mut scalar_frame, 2_f32);
        super::wide_mul(&mut wide_frame, 2_f32);

        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            let mut simd_avx2_frame = frame.clone();
            unsafe {
                super::avx2_mul(&mut simd_avx2_frame, 2_f32);
            }

            assert_eq!(scalar_frame, simd_avx2_frame);
        }

        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx512f") {
            let mut simd_avx512_frame = frame.clone();
            unsafe {
                super::avx512_mul(&mut simd_avx512_frame, 2_f32);
            }

            assert_eq!(scalar_frame, simd_avx512_frame);
        }

        assert_eq!(scalar_frame, wide_frame);
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn IntConversion_DummyFrame_EqualOutputs() {
        let frame = crate::telepathy::tests::dummy_int_frame();
        let mut scalar_frame = [0_f32; FRAME_SIZE];
        let mut wide_frame = [0_f32; FRAME_SIZE];
        let scale = (1_f32 / i16::MAX as f32) * 2.0;

        super::wide_i16_to_f32(&frame, &mut wide_frame, scale);
        super::i16_to_f32_scalar(&frame, &mut scalar_frame, scale);

        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            let mut simd_avx2_frame = [0_f32; FRAME_SIZE];
            unsafe { super::i16_to_f32_avx2(&frame, &mut simd_avx2_frame, scale) };
            assert_eq!(scalar_frame, simd_avx2_frame);
        }

        assert_eq!(scalar_frame, wide_frame);
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn FloatConversion_DummyFrame_EqualOutputs() {
        let frame = crate::telepathy::tests::dummy_frame();
        let mut scalar_frame = frame.clone();
        let mut wide_frame = frame.clone();

        let scale = i16::MAX as f32 * 2.0;
        super::scalar_float_scaler(&mut scalar_frame, scale);
        super::wide_float_scaler(&mut wide_frame, scale);

        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx") {
            let mut avx_frame = frame.clone();
            unsafe { super::avx_float_scaler(&mut avx_frame, scale) };
            assert_eq!(scalar_frame, avx_frame);
        }

        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx512f") {
            let mut avx_frame = frame.clone();
            unsafe { super::avx512_float_scaler(&mut avx_frame, scale) };
            assert_eq!(scalar_frame, avx_frame);
        }

        assert_eq!(scalar_frame, wide_frame);
    }
}
