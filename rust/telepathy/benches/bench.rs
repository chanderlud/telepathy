use criterion::{Criterion, criterion_group, criterion_main};
use rand::Rng;
use std::arch::x86_64::{
    _mm256_loadu_ps, _mm256_max_ps, _mm256_min_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_storeu_ps,
    _mm512_loadu_ps, _mm512_max_ps, _mm512_min_ps, _mm512_mul_ps, _mm512_set1_ps, _mm512_storeu_ps,
};
use std::hint::black_box;

include!("../src/api/audio/processing.rs");

pub fn bench_mul(c: &mut Criterion) {
    let mut frame = dummy_float_frame();

    c.bench_function("wide_mul", |b| {
        b.iter(|| wide_mul(black_box(&mut frame), black_box(2_f32)))
    });

    c.bench_function("scalar_mul", |b| {
        b.iter(|| scalar_mul(black_box(&mut frame), black_box(2_f32)))
    });

    unsafe {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx2") {
            c.bench_function("avx2_mul", |b| {
                b.iter(|| avx2_mul(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }

    unsafe {
        #[cfg(target_arch = "x86_64")]
        if is_x86_feature_detected!("avx512f") {
            c.bench_function("avx512_mul", |b| {
                b.iter(|| avx512_mul(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }
}

pub fn bench_rms(c: &mut Criterion) {
    let frame = dummy_float_frame();
    c.bench_function("rms", |b| b.iter(|| calculate_rms(black_box(&frame))));
}

pub fn bench_frame_conversions(c: &mut Criterion) {
    let mut pre_buf = [0_f32; 4096];
    let frame = dummy_int_frame();
    let scale = 1_f32 / i16::MAX as f32;

    c.bench_function("wide_i16_to_f32", |b| {
        b.iter(|| wide_i16_to_f32(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    c.bench_function("i16_to_f32_scalar", |b| {
        b.iter(|| i16_to_f32_scalar(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            c.bench_function("i16_to_f32_avx2", |b| {
                b.iter(|| {
                    i16_to_f32_avx2(black_box(&frame), black_box(&mut pre_buf), black_box(scale))
                })
            });
        }
    }

    c.bench_function("wide_float_scaler", |b| {
        b.iter(|| wide_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
    });

    c.bench_function("scalar_float_scaler", |b| {
        b.iter(|| scalar_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx") {
        unsafe {
            c.bench_function("avx_float_scaler", |b| {
                b.iter(|| avx_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
            });
        }
    }

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx512f") {
        unsafe {
            c.bench_function("avx512_float_scaler", |b| {
                b.iter(|| avx512_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
            });
        }
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

criterion_group!(benches, bench_mul, bench_frame_conversions, bench_rms);
criterion_main!(benches);
