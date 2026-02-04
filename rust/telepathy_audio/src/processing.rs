//! SIMD-optimized audio processing functions.
//!
//! This module provides optimized implementations for common audio processing
//! operations using SIMD instructions where available, with scalar fallbacks.
//!
//! ## Performance Characteristics
//!
//! All public functions in this module automatically select the optimal
//! implementation based on runtime CPU feature detection:
//!
//! | Function | AVX-512 | AVX2/AVX | Scalar |
//! |----------|---------|----------|--------|
//! | [`wide_mul`] | 16 floats/iter | 8 floats/iter | 1 float/iter |
//! | [`wide_i16_to_f32`] | N/A | 16 samples/iter | 1 sample/iter |
//! | [`wide_float_scaler`] | 16 floats/iter | 8 floats/iter | 1 float/iter |
//!
//! ## Alignment Requirements
//!
//! SIMD paths are only used when frame length meets alignment requirements:
//! - AVX-512: frame length must be a multiple of 16
//! - AVX2/AVX: frame length must be a multiple of 8
//! - Scalar fallback handles any length
//!
//! The standard `FRAME_SIZE` (480) is a multiple of 16, so SIMD paths are
//! typically used for normal audio processing.
//!
//! ## Safety
//!
//! All public functions are safe. SIMD intrinsics are used internally with
//! proper `#[target_feature]` annotations and runtime feature detection.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

const MAX_I16_F32: f32 = i16::MAX as f32;
const MIN_I16_F32: f32 = i16::MIN as f32;

/// Multiplies frame samples by a factor with automatic SIMD optimization.
///
/// Selects the optimal implementation based on available CPU features:
/// - AVX-512 for 16-element aligned frames (processes 16 floats per iteration)
/// - AVX2 for 8-element aligned frames (processes 8 floats per iteration)
/// - Scalar fallback for other cases (processes 1 float per iteration)
///
/// Results are clamped to [-1.0, 1.0].
///
/// # Performance Notes
///
/// For optimal performance, ensure `frame.len()` is a multiple of 16.
/// The standard `FRAME_SIZE` (480) satisfies this requirement.
///
/// # Example
///
/// ```rust
/// use telepathy_audio::wide_mul;
///
/// let mut samples = [0.5f32; 480];
/// wide_mul(&mut samples, 0.8); // Apply 80% volume
/// assert!(samples.iter().all(|&s| s == 0.4));
/// ```
pub fn wide_mul(frame: &mut [f32], factor: f32) {
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

/// Converts i16 samples to f32 with scaling and automatic SIMD optimization.
///
/// Converts 16-bit integer samples to 32-bit float samples, applying a scale
/// factor. Results are clamped to [-1.0, 1.0].
///
/// # Performance Notes
///
/// - AVX2: processes 16 samples per iteration
/// - Scalar: processes 1 sample per iteration
///
/// For optimal performance, ensure `ints.len()` is a multiple of 16.
/// The standard `FRAME_SIZE` (480) satisfies this requirement.
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

/// Scales float samples with truncation and automatic SIMD optimization.
///
/// Multiplies float samples by a scale factor and truncates toward zero.
/// Results are clamped to i16 range [-32768.0, 32767.0].
///
/// This is used to convert normalized audio [-1.0, 1.0] to i16 range
/// before noise suppression or codec encoding.
///
/// # Performance Notes
///
/// - AVX-512: processes 16 floats per iteration
/// - AVX: processes 8 floats per iteration
/// - Scalar: processes 1 float per iteration
///
/// For optimal performance, ensure `floats.len()` is a multiple of 16.
/// The standard `FRAME_SIZE` (480) satisfies this requirement.
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

/// Calculates the RMS (Root Mean Square) of the frame.
///
/// Uses loop unrolling for optimization.
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

/// Scalar multiplication for any length input.
fn scalar_mul(frame: &mut [f32], factor: f32) {
    for p in frame.iter_mut() {
        *p *= factor;
        *p = p.clamp(-1_f32, 1_f32);
    }
}

/// Optimized multiplication for AVX2 with 8|frame.len==true.
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

/// Optimized multiplication for AVX-512 with 16|frame.len==true.
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

/// Scalar implementation of i16 to f32 conversion with scaling.
fn i16_to_f32_scalar(ints: &[i16], out: &mut [f32], scale: f32) {
    for (out, &x) in out.iter_mut().zip(ints.iter()) {
        *out = (x as f32 * scale).clamp(-1_f32, 1_f32);
    }
}

/// AVX2 implementation of i16 to f32 conversion with scaling.
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

/// Scalar implementation of float scaling with truncation.
fn scalar_float_scaler(floats: &mut [f32], scale: f32) {
    for x in floats.iter_mut() {
        *x *= scale;
        *x = x.trunc().clamp(MIN_I16_F32, MAX_I16_F32);
    }
}

/// AVX implementation of float scaling with truncation.
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

/// AVX-512 implementation of float scaling with truncation.
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

/// Unit tests for SIMD processing functions.
///
/// These tests verify that all SIMD implementations produce identical results
/// to the scalar fallback implementations. Tests run on x86_64 architecture
/// and conditionally test AVX2 and AVX-512 paths based on CPU support.
#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    #![allow(unused_imports)]

    use nnnoiseless::FRAME_SIZE;

    /// Creates a test frame with values ranging from -0.5 to 0.5.
    fn dummy_frame() -> [f32; FRAME_SIZE] {
        let mut frame = [0_f32; FRAME_SIZE];
        for (i, sample) in frame.iter_mut().enumerate() {
            *sample = ((i as f32 / FRAME_SIZE as f32) * 2.0 - 1.0) * 0.5;
        }
        frame
    }

    /// Creates a test frame of i16 samples.
    fn dummy_int_frame() -> [i16; FRAME_SIZE] {
        let mut frame = [0_i16; FRAME_SIZE];
        for (i, sample) in frame.iter_mut().enumerate() {
            *sample = ((i as f32 / FRAME_SIZE as f32) * 2.0 - 1.0) as i16 * 16000;
        }
        frame
    }

    /// Verifies all multiplication variants produce identical output.
    ///
    /// Tests scalar, AVX2, AVX-512, and wide_mul against each other.
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn MulVariants_DummyFrame_EqualOutputs() {
        let frame = dummy_frame();
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

    /// Verifies i16 to f32 conversion variants produce identical output.
    ///
    /// Tests scalar and AVX2 implementations against each other.
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn IntConversion_DummyFrame_EqualOutputs() {
        let frame = dummy_int_frame();
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

    /// Verifies float scaling variants produce identical output.
    ///
    /// Tests scalar, AVX, and AVX-512 implementations against each other.
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn FloatConversion_DummyFrame_EqualOutputs() {
        let frame = dummy_frame();
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
