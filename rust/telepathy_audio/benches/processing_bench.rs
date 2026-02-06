use criterion::{Criterion, criterion_group, criterion_main};
use rand::Rng;
use std::hint::black_box;

use telepathy_audio::internal::processing::*;

/// Benchmarks for multiplication operations.
/// Compares scalar, wide (auto-selecting), AVX2, AVX2+FMA, and AVX-512 implementations.
pub fn bench_multiplication(c: &mut Criterion) {
    let mut group = c.benchmark_group("multiplication");

    let mut frame = dummy_float_frame();

    group.bench_function("scalar_mul", |b| {
        b.iter(|| scalar_mul(black_box(&mut frame), black_box(2_f32)))
    });

    group.bench_function("wide_mul", |b| {
        b.iter(|| wide_mul(black_box(&mut frame), black_box(2_f32)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            group.bench_function("avx2_mul", |b| {
                b.iter(|| avx2_mul(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx512f") {
        unsafe {
            group.bench_function("avx512_mul", |b| {
                b.iter(|| avx512_mul(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }

    group.finish();
}

/// Benchmarks for i16 to f32 conversion operations.
/// Compares scalar, wide (auto-selecting), and AVX2 implementations.
pub fn bench_i16_to_f32_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("i16_to_f32_conversion");

    let mut pre_buf = [0_f32; 4096];
    let frame = dummy_int_frame();
    let scale = 1_f32 / i16::MAX as f32;

    group.bench_function("i16_to_f32_scalar", |b| {
        b.iter(|| i16_to_f32_scalar(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    group.bench_function("wide_i16_to_f32", |b| {
        b.iter(|| wide_i16_to_f32(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            group.bench_function("i16_to_f32_avx2", |b| {
                b.iter(|| {
                    i16_to_f32_avx2(black_box(&frame), black_box(&mut pre_buf), black_box(scale))
                })
            });
        }
    }

    group.finish();
}

/// Benchmarks for float scaling operations.
/// Compares scalar, wide (auto-selecting), AVX, and AVX-512 implementations.
pub fn bench_float_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("float_scaling");

    let mut pre_buf = [0_f32; 4096];
    // Initialize buffer with valid float data
    let mut rng = rand::thread_rng();
    for x in &mut pre_buf {
        *x = rng.gen_range(-1.0..1.0);
    }

    group.bench_function("scalar_float_scaler", |b| {
        b.iter(|| scalar_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
    });

    group.bench_function("wide_float_scaler", |b| {
        b.iter(|| wide_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx") {
        unsafe {
            group.bench_function("avx_float_scaler", |b| {
                b.iter(|| avx_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
            });
        }
    }

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx512f") {
        unsafe {
            group.bench_function("avx512_float_scaler", |b| {
                b.iter(|| avx512_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
            });
        }
    }

    group.finish();
}

/// Benchmarks for f32 to i16 conversion operations.
/// Compares scalar, wide (auto-selecting), and SIMD implementations.
pub fn bench_f32_to_i16_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("f32_to_i16_conversion");

    // Create f32 frame scaled to i16 range
    let mut float_frame = [0_f32; 4096];
    let mut rng = rand::thread_rng();
    for x in &mut float_frame {
        *x = rng.gen_range(i16::MIN as f32..i16::MAX as f32);
    }

    let mut output = [0_i16; 4096];

    // Scalar fallback benchmark
    group.bench_function("f32_to_i16_scalar", |b| {
        b.iter(|| {
            for (out, &f) in black_box(&mut output)
                .iter_mut()
                .zip(black_box(&float_frame).iter())
            {
                *out = f as i16;
            }
        })
    });

    group.bench_function("f32_to_i16_old", |b| {
        b.iter(|| {
            output = float_frame.map(|x| x as i16);
        })
    });

    group.bench_function("wide_f32_to_i16", |b| {
        b.iter(|| wide_f32_to_i16(black_box(&float_frame), black_box(&mut output)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            group.bench_function("f32_to_i16_simd", |b| {
                b.iter(|| f32_to_i16_simd(black_box(&float_frame), black_box(&mut output)))
            });
        }
    }

    group.finish();
}

/// Benchmarks for RMS (Root Mean Square) calculation.
/// Compares scalar with unrolling, wide (auto-selecting), and AVX2 implementations.
pub fn bench_rms_calculation(c: &mut Criterion) {
    let mut group = c.benchmark_group("rms_calculation");

    let frame = dummy_float_frame();

    // Scalar with manual unrolling (for comparison)
    group.bench_function("rms_scalar_unrolled", |b| {
        b.iter(|| {
            let data = black_box(&frame);
            let inv_len = 1.0 / data.len() as f32;

            let mut sum1 = 0.0_f32;
            let mut sum2 = 0.0_f32;
            let mut sum3 = 0.0_f32;
            let mut sum4 = 0.0_f32;

            let mut i = 0;
            while i + 3 < data.len() {
                sum1 += data[i] * data[i];
                sum2 += data[i + 1] * data[i + 1];
                sum3 += data[i + 2] * data[i + 2];
                sum4 += data[i + 3] * data[i + 3];
                i += 4;
            }

            while i < data.len() {
                sum1 += data[i] * data[i];
                i += 1;
            }

            ((sum1 + sum2 + sum3 + sum4) * inv_len).sqrt()
        })
    });

    group.bench_function("calculate_rms_wide", |b| {
        b.iter(|| calculate_rms(black_box(&frame)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            group.bench_function("calculate_rms_avx2", |b| {
                b.iter(|| {
                    let data = black_box(&frame);
                    let inv_len = 1.0 / data.len() as f32;
                    let sum = calculate_rms_avx2(data);
                    (sum * inv_len).sqrt()
                })
            });
        }
    }

    group.finish();
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

criterion_group!(
    benches,
    bench_multiplication,
    bench_i16_to_f32_conversion,
    bench_float_scaling,
    bench_f32_to_i16_conversion,
    bench_rms_calculation
);
criterion_main!(benches);
