use crate::utils::*;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use rand::Rng;

#[path = "../src/api/utils.rs"]
mod utils;

pub fn bench_mul(c: &mut Criterion) {
    let mut frame = dummy_float_frame();

    c.bench_function("mul", |b| {
        b.iter(|| mul(black_box(&mut frame), black_box(2_f32)))
    });
}

pub fn bench_rms(c: &mut Criterion) {
    let frame = dummy_float_frame();

    c.bench_function("rms", |b| b.iter(|| calculate_rms(black_box(&frame))));
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
        *x /= i16::MAX as f32;
        *x = x.trunc().clamp(i16::MIN as f32, i16::MAX as f32);
    }

    frame
}

fn dummy_int_frame() -> [i16; 4096] {
    let mut frame = [0_i16; 4096];
    let mut rng = rand::thread_rng();
    rng.fill(&mut frame[..]);
    frame
}

criterion_group!(benches, bench_mul, bench_rms, bench_int_conversions);
criterion_main!(benches);
