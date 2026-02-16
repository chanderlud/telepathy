#![cfg(target_family = "wasm")]

//! WASM-specific benchmarks for core audio operations.
//!
//! These benchmarks measure the per-frame cost of the four key operations used
//! in the production audio pipeline:
//! - SEA codec encode
//! - SEA codec decode
//! - nnnoiseless denoise
//! - rubato resample
//!
//! Run with:
//!
//! ```sh
//! export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
//! export RUSTUP_TOOLCHAIN=nightly
//! export RUSTFLAGS="-Ctarget-feature=+atomics -Ctarget-feature=+simd128 -Clink-args=--shared-memory -Clink-args=--max-memory=1073741824 -Clink-args=--import-memory -Clink-args=--export=__wasm_init_tls -Clink-args=--export=__tls_size -Clink-args=--export=__tls_align -Clink-args=--export=__tls_base"
//! cargo bench --target wasm32-unknown-unknown --bench wasm_operations -Z build-std=std,panic_abort
//! ```

use std::hint::black_box;

use bytes::BytesMut;
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use rubato::{Resampler, SincFixedIn};
use wasm_bindgen_test::{Criterion, wasm_bindgen_bench};

use telepathy_audio::internal::utils::resampler_factory;
use telepathy_audio::sea::codec::file::SeaFileHeader;
use telepathy_audio::sea::decoder::SeaDecoder;
use telepathy_audio::sea::encoder::{EncoderSettings, SeaEncoder};

use crate::common::{dummy_float_frame, dummy_int_frame};

mod common;

/// Benchmarks SEA encoding of a single 48kHz mono frame (480 i16 samples).
#[wasm_bindgen_bench]
pub fn bench_encode(c: &mut Criterion) {
    let src = dummy_int_frame();
    let frame: [i16; FRAME_SIZE] = src[..FRAME_SIZE].try_into().expect("FRAME_SIZE slice");

    let mut encoder =
        SeaEncoder::new(1, 48_000, EncoderSettings::default()).expect("SeaEncoder::new");

    // Warm up once to ensure the encoder determines chunk_size.
    let mut warm_buf = BytesMut::new();
    encoder
        .encode_frame(frame, &mut warm_buf)
        .expect("encode warmup");

    let mut buffer = BytesMut::with_capacity(encoder.chunk_size() as usize);

    c.bench_function("sea_encode_frame", |b| {
        b.iter(|| {
            encoder
                .encode_frame(black_box(frame), black_box(&mut buffer))
                .expect("encode_frame");
            black_box(&buffer);
        })
    });
}

/// Benchmarks SEA decoding of a single frame back to 480 i16 samples.
#[wasm_bindgen_bench]
pub fn bench_decode(c: &mut Criterion) {
    let header = SeaFileHeader {
        version: 1,
        channels: 1,
        chunk_size: 960,
        frames_per_chunk: 480,
        sample_rate: 48_000,
    };

    let mut decoder = SeaDecoder::new(header).expect("SeaDecoder::new");

    // Pre-encode a realistic frame to obtain representative encoded bytes.
    let src = dummy_int_frame();
    let frame: [i16; FRAME_SIZE] = src[..FRAME_SIZE].try_into().expect("FRAME_SIZE slice");
    let mut encoder =
        SeaEncoder::new(1, 48_000, EncoderSettings::default()).expect("SeaEncoder::new");
    let mut encoded_buf = BytesMut::new();
    encoder
        .encode_frame(frame, &mut encoded_buf)
        .expect("encode for decode bench");
    let encoded: Vec<u8> = encoded_buf.to_vec();

    let mut output = [0_i16; FRAME_SIZE];

    c.bench_function("sea_decode_frame", |b| {
        b.iter(|| {
            decoder
                .decode_frame(black_box(&encoded), black_box(&mut output))
                .expect("decode_frame");
            black_box(&output);
        })
    });
}

/// Benchmarks nnnoiseless denoising of a single 48kHz frame (480 f32 samples).
#[wasm_bindgen_bench]
pub fn bench_denoise(c: &mut Criterion) {
    let mut src = dummy_float_frame();
    for x in &mut src {
        *x = (*x * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32);
    }
    let input: [f32; FRAME_SIZE] = src[..FRAME_SIZE].try_into().expect("FRAME_SIZE slice");

    let mut denoiser = DenoiseState::new();
    let mut output = [0_f32; FRAME_SIZE];

    c.bench_function("nnnoiseless_process_frame", |b| {
        b.iter(|| {
            denoiser.process_frame(black_box(&mut output), black_box(&input[..]));
            black_box(&output);
        })
    });
}

/// Benchmarks rubato resampling for a realistic 44.1kHz↔48kHz ratio.
#[wasm_bindgen_bench]
pub fn bench_resample(c: &mut Criterion) {
    let ratio = 44_100.0_f64 / 48_000.0_f64;
    let in_len = (FRAME_SIZE as f64 / ratio).ceil() as usize;

    let src = dummy_float_frame();
    let pre_vec: Vec<f32> = src[..in_len].to_vec();
    let pre_buf = [pre_vec];

    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 + 10_f64) as usize;
    let mut post_vec = Vec::with_capacity(post_len);
    post_vec.resize(post_len, 0_f32);
    let mut post_buf = [post_vec];

    let mut resampler: Option<SincFixedIn<f32>> =
        resampler_factory(ratio, 1, in_len).expect("resampler_factory");

    c.bench_function("rubato_process_into_buffer", |b| {
        b.iter(|| {
            if let Some(ref mut resampler) = resampler {
                let processed = resampler
                    .process_into_buffer(black_box(&pre_buf), black_box(&mut post_buf), None)
                    .expect("process_into_buffer");
                black_box(processed.1);
            } else {
                black_box(&pre_buf);
            }
        })
    });
}

fn main() {}
