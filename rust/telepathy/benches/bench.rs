use criterion::{Criterion, criterion_group, criterion_main};
use rand::Rng;
use std::arch::x86_64::{_mm256_loadu_ps, _mm256_max_ps, _mm256_min_ps, _mm256_mul_ps, _mm256_set1_ps, _mm256_storeu_ps, _mm512_loadu_ps, _mm512_max_ps, _mm512_min_ps, _mm512_mul_ps, _mm512_set1_ps, _mm512_storeu_ps};
use std::hint::black_box;

// #[path = "../src/api/utils.rs"]
// mod utils;

pub fn bench_mul(c: &mut Criterion) {
    let mut frame = dummy_float_frame();

    c.bench_function("mul", |b| {
        b.iter(|| mul(black_box(&mut frame), black_box(2_f32)))
    });

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            c.bench_function("mul_avx2", |b| {
                b.iter(|| mul_simd_avx2(black_box(&mut frame), black_box(2_f32)))
            });
        }
    }
}

pub fn bench_rms(c: &mut Criterion) {
    let frame = dummy_float_frame();

    c.bench_function("rms_new", |b| b.iter(|| calculate_rms(black_box(&frame))));
    c.bench_function("rms_old", |b| b.iter(|| rms_old(black_box(&frame))));

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") {
        unsafe {
            c.bench_function("rms_gpt", |b| b.iter(|| rms_avx2_fma(black_box(&frame))));
        }
    }
}

pub fn bench_int_conversions(c: &mut Criterion) {
    let mut pre_buf = [&mut [0_f32; 4096]];
    let frame = dummy_int_frame();

    c.bench_function("int conversion before", |b| {
        b.iter(|| int_conversion_before(black_box(&frame), black_box(&mut pre_buf)))
    });

    c.bench_function("int conversion after", |b| {
        b.iter(|| int_conversion_after(black_box(&frame), black_box(&mut pre_buf)))
    });
}

fn int_conversion_before(ints: &[i16], pre_buf: &mut [&mut [f32; 4096]; 1]) {
    let max_i16_f32 = i16::MAX as f32;

    ints.iter()
        .enumerate()
        .for_each(|(i, &x)| pre_buf[0][i] = x as f32 / max_i16_f32)
}

fn int_conversion_after(ints: &[i16], pre_buf: &mut [&mut [f32; 4096]; 1]) {
    let scale = 1_f32 / i16::MAX as f32;

    for (out, &x) in pre_buf[0].iter_mut().zip(ints.iter()) {
        *out = x as f32 * scale;
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

fn rms_old(data: &[f32]) -> f32 {
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

fn mul(frame: &mut [f32], factor: f32) {
    for p in frame.iter_mut() {
        *p *= factor;
        *p = p.clamp(-1_f32, 1_f32);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn mul_simd_avx2(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = _mm256_set1_ps(factor);
    let min_vec = _mm256_set1_ps(-1_f32);
    let max_vec = _mm256_set1_ps(1_f32);

    unsafe {
        while i + 8 <= len {
            let mut chunk = _mm256_loadu_ps(frame.as_ptr().add(i)); // load
            chunk = _mm256_mul_ps(chunk, factor_vec); // multiply
            chunk = _mm256_max_ps(min_vec, _mm256_min_ps(max_vec, chunk)); // clamp
            _mm256_storeu_ps(frame.as_mut_ptr().add(i), chunk); // write
            i += 8;
        }
    }

    // masked tail for 1..7 elements
    let rem = len - i;
    if rem > 0 {
        // temporary buffer of 8 elements, initialized to 0
        let mut buf = [0f32; 8];
        buf[..rem].copy_from_slice(&frame[i..]);

        unsafe {
            let mut v = _mm256_loadu_ps(buf.as_ptr());
            v = _mm256_mul_ps(v, factor_vec);
            v = _mm256_max_ps(min_vec, _mm256_min_ps(max_vec, v));
            _mm256_storeu_ps(buf.as_mut_ptr(), v);
        }

        // copy only the valid part back
        frame[i..].copy_from_slice(&buf[..rem]);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
fn mul_simd_avx512(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = _mm512_set1_ps(factor);
    let min_vec = _mm512_set1_ps(-1_f32);
    let max_vec = _mm512_set1_ps(1_f32);

    unsafe {
        // process 16 floats per iteration
        while i + 16 <= len {
            let mut chunk = _mm512_loadu_ps(frame.as_ptr().add(i));
            chunk = _mm512_mul_ps(chunk, factor_vec);
            chunk = _mm512_min_ps(max_vec, _mm512_max_ps(min_vec, chunk));
            _mm512_storeu_ps(frame.as_mut_ptr().add(i), chunk);
            i += 16;
        }
    }

    for j in i..len {
        frame[j] *= factor;
        frame[j] = frame[j].clamp(-1_f32, 1_f32);
    }
}

/// calculates the RMS of the frame (loop is unrolled for optimization)
fn calculate_rms(data: &[f32]) -> f32 {
    let chunk_length = data.len() / 4;
    (data
        .chunks(chunk_length)
        .map(|c| c.iter().map(|x| x * x).sum::<f32>())
        .max_by(|a, b| a.total_cmp(b))
        .unwrap_or(0_f32)
        / chunk_length as f32)
        .sqrt()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn rms_avx2_fma(xs: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let n = xs.len();
    let mut i = 0usize;
    let mut acc = _mm256_setzero_ps();

    while i + 8 <= n {
        let v = _mm256_loadu_ps(xs.as_ptr().add(i));
        acc = _mm256_fmadd_ps(v, v, acc); // v*v + acc
        i += 8;
    }

    let mut tmp = [0.0f32; 8];
    _mm256_storeu_ps(tmp.as_mut_ptr(), acc);
    let mut sum = tmp.iter().copied().sum::<f32>();

    while i < n {
        let x = *xs.get_unchecked(i);
        sum = x.mul_add(x, sum);
        i += 1;
    }

    (sum / n as f32).sqrt()
}

criterion_group!(benches, bench_mul, bench_rms, bench_int_conversions);
criterion_main!(benches);
