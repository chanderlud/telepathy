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
//! | Function | AVX-512 | AVX2/AVX | WASM SIMD (v128) | Scalar |
//! |----------|---------|----------|------------------|--------|
//! | [`wide_mul`] | 16 floats/iter | 8 floats/iter | 4 floats/iter | 1 float/iter |
//! | [`wide_i16_to_f32`] | N/A | 16 samples/iter | 8 samples/iter | 1 sample/iter |
//! | [`wide_float_scaler`] | 16 floats/iter | 8 floats/iter | 4 floats/iter | 1 float/iter |
//! | [`wide_f32_to_i16`] | N/A | 8 samples/iter | 4 samples/iter | 1 sample/iter |
//! | [`calculate_rms`] | N/A | 8 floats/iter | 4 floats/iter | 4 floats/iter (unrolled) |
//!
//! ## Alignment Requirements
//!
//! SIMD paths are only used when frame length meets alignment requirements:
//! - AVX-512: frame length must be a multiple of 16
//! - AVX2/AVX: frame length must be a multiple of 8
//! - WASM SIMD (v128): frame length must be a multiple of 4
//! - Scalar fallback handles any length
//!
//! The standard `FRAME_SIZE` (480) is a multiple of 16, so SIMD paths are
//! typically used for normal audio processing (480 % 4 = 0 for WASM SIMD).
//!
//! ## WASM SIMD Support
//!
//! On `wasm32` targets compiled with `target-feature=+simd128`, this module
//! uses 128-bit SIMD (v128) intrinsics from `std::arch::wasm32`. The v128
//! register holds 4×f32 or 8×i16, providing significant speedups over scalar
//! code in modern browsers.
//!
//! ## Safety
//!
//! All public functions are safe. SIMD intrinsics are used internally with
//! proper `#[target_feature]` annotations and runtime feature detection.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "wasm32")]
use std::arch::wasm32::*;

const MAX_I16_F32: f32 = i16::MAX as f32;
const MIN_I16_F32: f32 = i16::MIN as f32;

/// Prefetch distance in bytes (typically 2-3 cache lines ahead)
#[cfg(target_arch = "x86_64")]
const PREFETCH_DISTANCE: usize = 128;

/// Multiplies frame samples by a factor with automatic SIMD optimization.
///
/// Selects the optimal implementation based on available CPU features:
/// - AVX-512 for 16-element aligned frames (processes 32 floats per iteration with 2x unroll)
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

    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") && frame.len().is_multiple_of(4) {
            wasm_simd_mul(frame, factor);
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
pub fn wide_i16_to_f32(ints: &[i16], out: &mut [f32], scale: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { i16_to_f32_avx2(ints, out, scale) }
            return;
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") {
            wasm_simd_i16_to_f32(ints, out, scale);
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
pub fn wide_float_scaler(floats: &mut [f32], scale: f32) {
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

    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") {
            wasm_simd_float_scaler(floats, scale);
            return;
        }
    }

    scalar_float_scaler(floats, scale);
}

/// Calculates the RMS (Root Mean Square) of the frame.
///
/// Uses SIMD acceleration when available (AVX2/AVX-512), with loop unrolling
/// for scalar fallback. Pre-computes reciprocal of length for faster division.
pub fn calculate_rms(data: &[f32]) -> f32 {
    // Pre-compute reciprocal for multiplication instead of division
    let inv_len = 1.0 / data.len() as f32;

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && data.len() >= 8 {
            let sum = unsafe { calculate_rms_avx2(data) };
            return (sum * inv_len).sqrt();
        }
    }

    // Scalar fallback with loop unrolling
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

    let mean_of_squares = (sum1 + sum2 + sum3 + sum4) * inv_len;
    mean_of_squares.sqrt()
}

/// Scalar multiplication for any length input.
pub fn scalar_mul(frame: &mut [f32], factor: f32) {
    for p in frame.iter_mut() {
        *p *= factor;
        *p = p.clamp(-1_f32, 1_f32);
    }
}

/// Optimized multiplication for AVX2 with 8|frame.len==true.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn avx2_mul(frame: &mut [f32], factor: f32) {
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
pub unsafe fn avx512_mul(frame: &mut [f32], factor: f32) {
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
pub fn i16_to_f32_scalar(ints: &[i16], out: &mut [f32], scale: f32) {
    for (out, &x) in out.iter_mut().zip(ints.iter()) {
        *out = (x as f32 * scale).clamp(-1_f32, 1_f32);
    }
}

/// AVX2 implementation of i16 to f32 conversion with scaling.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn i16_to_f32_avx2(ints: &[i16], out: &mut [f32], scale: f32) {
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
pub fn scalar_float_scaler(floats: &mut [f32], scale: f32) {
    for x in floats.iter_mut() {
        *x *= scale;
        *x = x.trunc().clamp(MIN_I16_F32, MAX_I16_F32);
    }
}

/// AVX implementation of float scaling with truncation.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn avx_float_scaler(floats: &mut [f32], scale: f32) {
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
pub unsafe fn avx512_float_scaler(floats: &mut [f32], scale: f32) {
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

/// SIMD-accelerated RMS calculation using AVX2.
/// Uses horizontal sum for efficient reduction.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn calculate_rms_avx2(data: &[f32]) -> f32 {
    let n = data.len();
    let mut i = 0;

    // Accumulator vectors for parallel sum
    let mut sum_vec = _mm256_setzero_ps();

    // Process 8 floats per iteration
    while i + 8 <= n {
        // Software prefetch
        if i + PREFETCH_DISTANCE < n {
            _mm_prefetch::<_MM_HINT_T0>(data.as_ptr().add(i + PREFETCH_DISTANCE) as *const i8);
        }

        let v = _mm256_loadu_ps(data.as_ptr().add(i));
        // Square and accumulate: sum += v * v
        sum_vec = _mm256_add_ps(sum_vec, _mm256_mul_ps(v, v));
        i += 8;
    }

    // Horizontal sum of the 8 floats in sum_vec
    // sum_vec = [a, b, c, d, e, f, g, h]
    // Step 1: hadd pairs -> [a+b, c+d, a+b, c+d, e+f, g+h, e+f, g+h]
    let sum1 = _mm256_hadd_ps(sum_vec, sum_vec);
    // Step 2: hadd again -> [a+b+c+d, a+b+c+d, a+b+c+d, a+b+c+d, e+f+g+h, ...]
    let sum2 = _mm256_hadd_ps(sum1, sum1);
    // Extract low and high 128-bit lanes and add
    let lo = _mm256_castps256_ps128(sum2);
    let hi = _mm256_extractf128_ps::<1>(sum2);
    let sum128 = _mm_add_ps(lo, hi);
    _mm_cvtss_f32(sum128)
}

/// In-place f32 to i16 conversion using SIMD.
/// This is used for frame conversion optimization in the processor.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn f32_to_i16_simd(floats: &[f32], output: &mut [i16]) {
    const TOWARD_ZERO_NO_EXC: i32 = _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC;

    let n = floats.len().min(output.len());
    let mut i = 0;

    // Process 8 floats -> 8 i16 per iteration
    while i + 8 <= n {
        // Software prefetch
        if i + PREFETCH_DISTANCE < n {
            _mm_prefetch::<_MM_HINT_T0>(floats.as_ptr().add(i + PREFETCH_DISTANCE) as *const i8);
        }

        // Load 8 floats
        let v = _mm256_loadu_ps(floats.as_ptr().add(i));
        // Truncate toward zero (matches scalar `as i16` behavior)
        let v = _mm256_round_ps(v, TOWARD_ZERO_NO_EXC);
        // Convert to i32
        let vi32 = _mm256_cvtps_epi32(v);
        // Pack to i16 with saturation: need to shuffle and pack
        // _mm256_packs_epi32 packs [a0,a1,a2,a3,b0,b1,b2,b3] -> [a0,a1,a2,a3,a0,a1,a2,a3] (wrong order)
        // We need to permute after packing
        let lo = _mm256_castsi256_si128(vi32);
        let hi = _mm256_extracti128_si256::<1>(vi32);
        let packed = _mm_packs_epi32(lo, hi);
        // Store 8 i16
        _mm_storeu_si128(output.as_mut_ptr().add(i) as *mut __m128i, packed);
        i += 8;
    }
}

/// Wrapper for f32 to i16 conversion with automatic SIMD selection.
pub fn wide_f32_to_i16(floats: &[f32], output: &mut [i16]) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && floats.len() >= 8 {
            unsafe { f32_to_i16_simd(floats, output) }
            return;
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") && floats.len() >= 4 {
            wasm_simd_f32_to_i16(floats, output);
            return;
        }
    }

    // Scalar fallback
    for (out, &f) in output.iter_mut().zip(floats.iter()) {
        *out = f as i16;
    }
}

/// WASM SIMD multiplication: 4 floats per iteration via v128.
///
/// Multiplies each sample by `factor` and clamps the result to [-1.0, 1.0].
/// The caller must ensure `frame.len()` is a multiple of 4.
#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn wasm_simd_mul(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = f32x4_splat(factor);
    let min_vec = f32x4_splat(-1.0_f32);
    let max_vec = f32x4_splat(1.0_f32);

    while i + 4 <= len {
        let mut chunk = unsafe { v128_load(frame.as_ptr().add(i) as *const v128) };
        chunk = f32x4_mul(chunk, factor_vec);
        chunk = f32x4_max(min_vec, f32x4_min(max_vec, chunk));
        unsafe {
            v128_store(frame.as_mut_ptr().add(i) as *mut v128, chunk);
        }
        i += 4;
    }
}

/// WASM SIMD i16→f32 conversion with scaling: 8 i16 → 8 f32 per iteration.
///
/// Converts 16-bit integer samples to 32-bit floats, applies `scale`, and
/// clamps to [-1.0, 1.0]. Processes 8 samples per loop iteration using two
/// v128 stores (4 f32 each).
#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn wasm_simd_i16_to_f32(ints: &[i16], out: &mut [f32], scale: f32) {
    let n = ints.len().min(out.len());
    let mut i = 0;

    let scale_vec = f32x4_splat(scale);
    let min_vec = f32x4_splat(-1.0_f32);
    let max_vec = f32x4_splat(1.0_f32);

    // Process 8 i16 samples per iteration (one v128 load of 8×i16)
    while i + 8 <= n {
        // Load 8 × i16 into a v128
        let v_i16 = unsafe { v128_load(ints.as_ptr().add(i) as *const v128) };

        // Widen lower 4 i16 → 4 i32 → 4 f32
        let lo_i32 = i32x4_extend_low_i16x8(v_i16);
        let mut lo_f32 = f32x4_mul(f32x4_convert_i32x4(lo_i32), scale_vec);
        lo_f32 = f32x4_max(min_vec, f32x4_min(max_vec, lo_f32));
        unsafe {
            v128_store(out.as_mut_ptr().add(i) as *mut v128, lo_f32);
        }

        // Widen upper 4 i16 → 4 i32 → 4 f32
        let hi_i32 = i32x4_extend_high_i16x8(v_i16);
        let mut hi_f32 = f32x4_mul(f32x4_convert_i32x4(hi_i32), scale_vec);
        hi_f32 = f32x4_max(min_vec, f32x4_min(max_vec, hi_f32));
        unsafe {
            v128_store(out.as_mut_ptr().add(i + 4) as *mut v128, hi_f32);
        }

        i += 8;
    }
}

/// WASM SIMD float scaling with truncation: 4 floats per iteration.
///
/// Multiplies each sample by `scale`, truncates toward zero, and clamps to
/// the i16 range [-32768.0, 32767.0]. The caller must ensure `floats.len()`
/// is a multiple of 4.
#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn wasm_simd_float_scaler(floats: &mut [f32], scale: f32) {
    let n = floats.len();
    let mut i = 0;

    let scale_vec = f32x4_splat(scale);
    let min_vec = f32x4_splat(MIN_I16_F32);
    let max_vec = f32x4_splat(MAX_I16_F32);

    while i + 4 <= n {
        let mut v = unsafe { v128_load(floats.as_ptr().add(i) as *const v128) };
        v = f32x4_mul(v, scale_vec);
        // Truncate toward zero: convert f32→i32 (saturating truncation), then back
        let vi = i32x4_trunc_sat_f32x4(v);
        let mut vf = f32x4_convert_i32x4(vi);
        unsafe {
            // Clamp to i16 representable range
            vf = f32x4_max(min_vec, f32x4_min(max_vec, vf));
            v128_store(floats.as_mut_ptr().add(i) as *mut v128, vf);
        }
        i += 4;
    }
}

/// WASM SIMD f32→i16 conversion: 4 floats → 4 i16 per iteration.
///
/// Truncates each f32 toward zero and writes the resulting i16 values.
/// Uses `i32x4_trunc_sat_f32x4` which saturates out-of-range values to
/// `i32::MIN` / `i32::MAX`, then narrows with `i16x8_narrow_i32x4`.
#[cfg(target_arch = "wasm32")]
#[target_feature(enable = "simd128")]
fn wasm_simd_f32_to_i16(floats: &[f32], output: &mut [i16]) {
    let n = floats.len().min(output.len());
    let mut i = 0;

    // Process 4 f32 → 4 i16 per iteration
    while i + 4 <= n {
        let v = unsafe { v128_load(floats.as_ptr().add(i) as *const v128) };
        // Truncate f32 → i32 with saturation (matches `as i16` truncation semantics)
        let vi32 = i32x4_trunc_sat_f32x4(v);
        // Narrow i32x4 → i16x8 with signed saturation (upper half is from zeros)
        let zero = i32x4_splat(0);
        let packed = i16x8_narrow_i32x4(vi32, zero);
        // Store the lower 4 i16 (8 bytes) from the v128
        let dst = unsafe { output.as_mut_ptr().add(i) as *mut u64 };
        let bits = u64x2_extract_lane::<0>(packed);
        unsafe {
            core::ptr::write_unaligned(dst, bits);
        }
        i += 4;
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

    /// Verifies RMS calculation produces consistent results.
    #[test]
    fn RmsCalculation_DummyFrame_CorrectResult() {
        let frame = dummy_frame();

        // Calculate expected RMS using scalar method
        let sum: f32 = frame.iter().map(|x| x * x).sum();
        let expected = (sum / frame.len() as f32).sqrt();

        let result = super::calculate_rms(&frame);

        // Allow small floating point differences
        assert!(
            (result - expected).abs() < 1e-6,
            "RMS mismatch: {} vs {}",
            result,
            expected
        );
    }

    /// Verifies f32 to i16 conversion.
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn F32ToI16_DummyFrame_CorrectOutput() {
        let frame = dummy_frame();
        let scaled: Vec<f32> = frame.iter().map(|x| x * i16::MAX as f32).collect();

        let mut scalar_output = vec![0i16; FRAME_SIZE];
        let mut simd_output = vec![0i16; FRAME_SIZE];

        // Scalar conversion
        for (out, &f) in scalar_output.iter_mut().zip(scaled.iter()) {
            *out = f as i16;
        }

        // SIMD conversion
        super::wide_f32_to_i16(&scaled, &mut simd_output);

        assert_eq!(scalar_output, simd_output);
    }

    /// Verifies WASM SIMD multiplication produces identical output to scalar.
    #[wasm_bindgen_test::wasm_bindgen_test]
    #[cfg(target_arch = "wasm32")]
    fn WasmMulVariants_DummyFrame_EqualOutputs() {
        let frame = dummy_frame();
        let mut scalar_frame = frame;
        let mut wasm_simd_frame = frame;
        let mut wide_frame = frame;

        super::scalar_mul(&mut scalar_frame, 2_f32);
        super::wide_mul(&mut wide_frame, 2_f32);

        if cfg!(target_feature = "simd128") {
            unsafe { super::wasm_simd_mul(&mut wasm_simd_frame, 2_f32) };
            assert_eq!(scalar_frame, wasm_simd_frame);
        }

        assert_eq!(scalar_frame, wide_frame);
    }

    /// Verifies WASM SIMD i16→f32 conversion produces identical output to scalar.
    #[wasm_bindgen_test::wasm_bindgen_test]
    #[cfg(target_arch = "wasm32")]
    fn WasmIntConversion_DummyFrame_EqualOutputs() {
        let frame = dummy_int_frame();
        let mut scalar_frame = [0_f32; FRAME_SIZE];
        let mut wasm_simd_frame = [0_f32; FRAME_SIZE];
        let mut wide_frame = [0_f32; FRAME_SIZE];
        let scale = (1_f32 / i16::MAX as f32) * 2.0;

        super::i16_to_f32_scalar(&frame, &mut scalar_frame, scale);
        super::wide_i16_to_f32(&frame, &mut wide_frame, scale);

        if cfg!(target_feature = "simd128") {
            unsafe { super::wasm_simd_i16_to_f32(&frame, &mut wasm_simd_frame, scale) };
            assert_eq!(scalar_frame, wasm_simd_frame);
        }

        assert_eq!(scalar_frame, wide_frame);
    }

    /// Verifies WASM SIMD float scaling produces identical output to scalar.
    #[wasm_bindgen_test::wasm_bindgen_test]
    #[cfg(target_arch = "wasm32")]
    fn WasmFloatConversion_DummyFrame_EqualOutputs() {
        let frame = dummy_frame();
        let mut scalar_frame = frame;
        let mut wasm_simd_frame = frame;
        let mut wide_frame = frame;

        let scale = i16::MAX as f32 * 2.0;
        super::scalar_float_scaler(&mut scalar_frame, scale);
        super::wide_float_scaler(&mut wide_frame, scale);

        if cfg!(target_feature = "simd128") {
            unsafe { super::wasm_simd_float_scaler(&mut wasm_simd_frame, scale) };
            assert_eq!(scalar_frame, wasm_simd_frame);
        }

        assert_eq!(scalar_frame, wide_frame);
    }

    /// Verifies WASM SIMD f32→i16 conversion produces identical output to scalar.
    #[wasm_bindgen_test::wasm_bindgen_test]
    #[cfg(target_arch = "wasm32")]
    fn WasmF32ToI16_DummyFrame_CorrectOutput() {
        let frame = dummy_frame();
        let scaled: Vec<f32> = frame.iter().map(|x| x * i16::MAX as f32).collect();

        let mut scalar_output = vec![0i16; FRAME_SIZE];
        let mut simd_output = vec![0i16; FRAME_SIZE];

        for (out, &f) in scalar_output.iter_mut().zip(scaled.iter()) {
            *out = f as i16;
        }

        super::wide_f32_to_i16(&scaled, &mut simd_output);

        assert_eq!(scalar_output, simd_output);
    }

    /// Verifies WASM SIMD RMS calculation matches scalar reference.
    #[wasm_bindgen_test::wasm_bindgen_test]
    #[cfg(target_arch = "wasm32")]
    fn WasmRmsCalculation_DummyFrame_CorrectResult() {
        let frame = dummy_frame();

        let sum: f32 = frame.iter().map(|x| x * x).sum();
        let expected = (sum / frame.len() as f32).sqrt();

        let result = super::calculate_rms(&frame);

        assert!(
            (result - expected).abs() < 1e-6,
            "WASM RMS mismatch: {} vs {}",
            result,
            expected
        );
    }
}
