#![cfg(target_family = "wasm")]
//! WASM-specific benchmarks for audio processing SIMD operations.
//!
//! These benchmarks use wasm-bindgen-test for WASM-specific benchmarking.
//! Run with:
//!
//! ```sh
//! export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
//! export RUSTUP_TOOLCHAIN=nightly
//! export RUSTFLAGS="-Ctarget-feature=+atomics -Ctarget-feature=+simd128 -Clink-args=--shared-memory -Clink-args=--max-memory=1073741824 -Clink-args=--import-memory  -Clink-args=--export=__wasm_init_tls -Clink-args=--export=__tls_size -Clink-args=--export=__tls_align -Clink-args=--export=__tls_base"
//! cargo bench --target wasm32-unknown-unknown --bench processing_bench_wasm -Z build-std=std,panic_abort
//! ```

use std::hint::black_box;
use wasm_bindgen_test::{Criterion, wasm_bindgen_bench};

use telepathy_audio::internal::processing::*;

// ---------------------------------------------------------------------------
// Helper data generators (matching processing_bench.rs)
// ---------------------------------------------------------------------------

fn dummy_float_frame() -> [f32; 4096] {
    let mut frame = [0_f32; 4096];
    for (i, x) in frame.iter_mut().enumerate() {
        // Deterministic pattern – no RNG so the benchmark is reproducible on WASM
        *x = ((i as f32 / 4096.0) * 2.0 - 1.0) * 0.9;
    }
    frame
}

fn dummy_int_frame() -> [i16; 4096] {
    let mut frame = [0_i16; 4096];
    for (i, x) in frame.iter_mut().enumerate() {
        *x = ((i as f32 / 4096.0) * 2.0 - 1.0) as i16 * 16000;
    }
    frame
}

/// Benchmarks scalar vs wide (auto-selecting) multiplication.
#[wasm_bindgen_bench]
pub fn bench_multiplication(c: &mut Criterion) {
    let mut frame = dummy_float_frame();

    c.bench_function("scalar_mul", |b| {
        b.iter(|| scalar_mul(black_box(&mut frame), black_box(2_f32)))
    });

    c.bench_function("wide_mul", |b| {
        b.iter(|| wide_mul(black_box(&mut frame), black_box(2_f32)))
    });
}

/// Benchmarks scalar vs wide i16→f32 conversion.
#[wasm_bindgen_bench]
pub fn bench_i16_to_f32_conversion(c: &mut Criterion) {
    let mut pre_buf = [0_f32; 4096];
    let frame = dummy_int_frame();
    let scale = 1_f32 / i16::MAX as f32;

    c.bench_function("i16_to_f32_scalar", |b| {
        b.iter(|| i16_to_f32_scalar(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });

    c.bench_function("wide_i16_to_f32", |b| {
        b.iter(|| wide_i16_to_f32(black_box(&frame), black_box(&mut pre_buf), black_box(scale)))
    });
}

/// Benchmarks scalar vs wide float scaling with truncation.
#[wasm_bindgen_bench]
pub fn bench_float_scaling(c: &mut Criterion) {
    let mut pre_buf = dummy_float_frame();

    c.bench_function("scalar_float_scaler", |b| {
        b.iter(|| scalar_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
    });

    c.bench_function("wide_float_scaler", |b| {
        b.iter(|| wide_float_scaler(black_box(&mut pre_buf), black_box(i16::MAX as f32)))
    });
}

/// Benchmarks scalar vs wide f32→i16 conversion.
#[wasm_bindgen_bench]
pub fn bench_f32_to_i16_conversion(c: &mut Criterion) {
    let float_frame: Vec<f32> = dummy_float_frame()
        .iter()
        .map(|x| x * i16::MAX as f32)
        .collect();
    let mut output = [0_i16; 4096];

    c.bench_function("f32_to_i16_scalar", |b| {
        b.iter(|| {
            for (out, &f) in black_box(&mut output)
                .iter_mut()
                .zip(black_box(&float_frame).iter())
            {
                *out = f as i16;
            }
        })
    });

    c.bench_function("wide_f32_to_i16", |b| {
        b.iter(|| wide_f32_to_i16(black_box(&float_frame), black_box(&mut output)))
    });
}

fn main() {}
