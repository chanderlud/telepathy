use criterion::{Criterion, criterion_group, criterion_main};
use rand::Rng;
use std::arch::x86_64::{
    _mm256_loadu_ps, _mm256_max_ps, _mm256_min_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_storeu_ps,
    _mm512_loadu_ps, _mm512_max_ps, _mm512_min_ps, _mm512_mul_ps, _mm512_set1_ps, _mm512_storeu_ps,
};
use std::hint::black_box;

// #[path = "../src/api/utils.rs"]
// mod utils;

pub fn bench_mul(c: &mut Criterion) {
    let mut frame = dummy_float_frame();

    c.bench_function("scalar mul", |b| {
        b.iter(|| scalar_mul(black_box(&mut frame), black_box(2_f32)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        c.bench_function("wide mul", |b| {
            b.iter(|| wide_mul(black_box(&mut frame), black_box(2_f32)))
        });
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

    c.bench_function("int conversion before", |b| {
        b.iter(|| int_conversion_before(black_box(&frame), black_box(&mut pre_buf)))
    });

    c.bench_function("int conversion after", |b| {
        b.iter(|| int_conversion_after(black_box(&frame), black_box(&mut pre_buf)))
    });

    unsafe {
        c.bench_function("int conversion avx2", |b| {
            b.iter(|| i16_to_f32_avx2(black_box(&frame), black_box(&mut pre_buf)))
        });
    }
}

fn int_conversion_before(ints: &[i16], pre_buf: &mut [f32; 4096]) {
    let max_i16_f32 = i16::MAX as f32;

    ints.iter()
        .enumerate()
        .for_each(|(i, &x)| pre_buf[i] = x as f32 / max_i16_f32)
}

fn int_conversion_after(ints: &[i16], pre_buf: &mut [f32; 4096]) {
    let scale = 1_f32 / i16::MAX as f32;

    for (out, &x) in pre_buf.iter_mut().zip(ints.iter()) {
        *out = x as f32 * scale;
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
unsafe fn i16_to_f32_avx2(ints: &[i16], out: &mut [f32]) {
    use core::arch::x86_64::*;
    let n = ints.len();
    let mut i = 0usize;

    let scale_ps = _mm256_set1_ps(1.0f32 / i16::MAX as f32);

    // process 16 i16 -> 16 f32 per loop
    while i + 16 <= n {
        // load 16 i16
        let v16 = _mm256_loadu_si256(ints.as_ptr().add(i) as *const __m256i);

        // lower 8 i16 -> 8 i32 -> 8 f32
        let lo16 = _mm256_castsi256_si128(v16);
        let lo32 = _mm256_cvtepi16_epi32(lo16);
        let lo_ps = _mm256_mul_ps(_mm256_cvtepi32_ps(lo32), scale_ps);

        // upper 8 i16 -> 8 i32 -> 8 f32
        let hi16 = _mm256_extracti128_si256::<1>(v16);
        let hi32 = _mm256_cvtepi16_epi32(hi16);
        let hi_ps = _mm256_mul_ps(_mm256_cvtepi32_ps(hi32), scale_ps);

        // store 16 f32
        _mm256_storeu_ps(out.as_mut_ptr().add(i), lo_ps);
        _mm256_storeu_ps(out.as_mut_ptr().add(i + 8), hi_ps);

        i += 16;
    }
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

/// mul for inputs with 8|len==true
pub(crate) fn wide_mul(frame: &mut [f32], factor: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") {
            unsafe { mul_simd_avx512(frame, factor) }
            return;
        }
        if is_x86_feature_detected!("avx2") {
            unsafe { mul_simd_avx2(frame, factor) }
            return;
        }
    }

    scalar_mul(frame, factor);
}

/// mul for any length input
pub(crate) fn scalar_mul(frame: &mut [f32], factor: f32) {
    for p in frame.iter_mut() {
        *p *= factor;
        *p = p.clamp(-1_f32, 1_f32);
    }
}

/// optimized mul for avx2
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

/// optimized mul for avx512f
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

criterion_group!(benches, bench_mul, bench_int_conversions);
criterion_main!(benches);
