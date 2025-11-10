use criterion::{Criterion, criterion_group, criterion_main};
use rand::Rng;
use std::arch::x86_64::{
    _mm256_loadu_ps, _mm256_max_ps, _mm256_min_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_storeu_ps,
    _mm512_loadu_ps, _mm512_max_ps, _mm512_min_ps, _mm512_mul_ps, _mm512_set1_ps, _mm512_storeu_ps,
};
use std::hint::black_box;

pub fn bench_mul(c: &mut Criterion) {
    let mut frame = dummy_float_frame();

    c.bench_function("wide mul", |b| {
        b.iter(|| wide_mul(black_box(&mut frame), black_box(2_f32)))
    });

    c.bench_function("scalar mul", |b| {
        b.iter(|| scalar_mul(black_box(&mut frame), black_box(2_f32)))
    });

    unsafe {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            c.bench_function("avx2 mul", |b| {
                b.iter(|| mul_simd_avx2(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }

    unsafe {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx512f") {
            c.bench_function("avx512 mul", |b| {
                b.iter(|| mul_simd_avx512(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }
}

// pub fn bench_rms(c: &mut Criterion) {
//     let frame = dummy_float_frame();
//
//     c.bench_function("rms_new", |b| b.iter(|| calculate_rms(black_box(&frame))));
//     c.bench_function("rms_old", |b| b.iter(|| rms_old(black_box(&frame))));
//
//     #[cfg(target_arch = "x86_64")]
//     if is_x86_feature_detected!("avx2") {
//         unsafe {
//             c.bench_function("rms_gpt", |b| b.iter(|| rms_avx2_fma(black_box(&frame))));
//         }
//     }
// }

pub fn bench_int_conversions(c: &mut Criterion) {
    let mut pre_buf = [0_f32; 4096];
    let frame = dummy_int_frame();
    let scale = 1_f32 / i16::MAX as f32;

    c.bench_function("wide_i16_to_f32", |b| {
        b.iter(|| wide_i16_to_f32(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    c.bench_function("i16_to_f32_scalar", |b| {
        b.iter(|| i16_to_f32_scalar(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    unsafe {
        c.bench_function("int conversion avx2", |b| {
            b.iter(|| i16_to_f32_avx2(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
        });
    }

    c.bench_function("wide_float_scaler", |b| {
        b.iter(|| {
            wide_float_scaler(
                black_box(&mut pre_buf),
                black_box(i16::MAX as f32),
                black_box(i16::MIN as f32),
                black_box(i16::MAX as f32),
            )
        })
    });

    c.bench_function("float_to_int_scalar", |b| {
        b.iter(|| {
            float_to_int_scalar(
                black_box(&mut pre_buf),
                black_box(i16::MAX as f32),
                black_box(i16::MIN as f32),
                black_box(i16::MAX as f32),
            )
        })
    });

    unsafe {
        c.bench_function("float_conversion_avx", |b| {
            b.iter(|| {
                float_to_int_avx(
                    black_box(&mut pre_buf),
                    black_box(i16::MAX as f32),
                    black_box(i16::MIN as f32),
                    black_box(i16::MAX as f32),
                )
            })
        });
    }

    c.bench_function("old_f32_to_i16", |b| {
        let pre_buf = pre_buf[..480].try_into().unwrap();
        let mut out = [0_i16; 480];
        b.iter(|| old_f32_to_i16(black_box(pre_buf), black_box(&mut out)))
    });
}

/// mul with internal selection of optimal implementation
pub(crate) fn wide_mul(frame: &mut [f32], factor: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") && frame.len().is_multiple_of(16) {
            unsafe { mul_simd_avx512(frame, factor) }
            return;
        }
        if is_x86_feature_detected!("avx2") && frame.len().is_multiple_of(8) {
            unsafe { mul_simd_avx2(frame, factor) }
            return;
        }
    }

    scalar_mul(frame, factor);
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
unsafe fn mul_simd_avx2(frame: &mut [f32], factor: f32) {
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
unsafe fn mul_simd_avx512(frame: &mut [f32], factor: f32) {
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

/// Safety: input length must be a multiple of 16
pub(crate) fn wide_i16_to_f32(ints: &[i16], out: &mut [f32; 4096], scale: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            unsafe { i16_to_f32_avx2(ints, out, scale) }
            return;
        }
    }

    i16_to_f32_scalar(ints, out, scale);
}

/// scalar implementation of i16 to f32 conversion with scaling
fn i16_to_f32_scalar(ints: &[i16], out: &mut [f32; 4096], scale: f32) {
    for (out, &x) in out.iter_mut().zip(ints.iter()) {
        *out = (x as f32 * scale).clamp(-1_f32, 1_f32);
    }
}

/// avx2 implementation of i16 to f32 conversion with scaling
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn i16_to_f32_avx2(ints: &[i16], out: &mut [f32], scale: f32) {
    use core::arch::x86_64::*;
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

// TODO implement avx512
/// Safety: input length must be a multiple of 8
pub(crate) fn wide_float_scaler(floats: &mut [f32], scale: f32, min: f32, max: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx") {
            unsafe { float_to_int_avx(floats, scale, min, max) }
            return;
        }
    }

    float_to_int_scalar(floats, scale, min, max);
}

fn float_to_int_scalar(floats: &mut [f32], scale: f32, min: f32, max: f32) {
    for x in floats.iter_mut() {
        *x *= scale;
        *x = x.trunc().clamp(min, max);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn float_to_int_avx(floats: &mut [f32], scale: f32, min: f32, max: f32) {
    use core::arch::x86_64::*;
    let n = floats.len();
    let mut i = 0;

    let scale_v = _mm256_set1_ps(scale);
    let min_v = _mm256_set1_ps(min);
    let max_v = _mm256_set1_ps(max);

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

fn old_f32_to_i16(floats: [f32; 480], out: &mut [i16; 480]) {
    *out = floats.map(|x| x as i16);
}

fn dummy_float_frame() -> [f32; 4096] {
    let mut frame = [0_f32; 4096];
    let mut rng = rand::thread_rng();
    rng.fill(&mut frame[..]);

    for x in &mut frame {
        *x = x.clamp(i16::MIN as f32, i16::MAX as f32);
        *x /= i16::MAX as f32;
    }

    frame
}

fn dummy_int_frame() -> [i16; 4096] {
    let mut frame = [0_i16; 4096];
    let mut rng = rand::thread_rng();
    rng.fill(&mut frame[..]);
    frame
}

criterion_group!(benches, bench_mul, bench_int_conversions);
criterion_main!(benches);
