//! Throughput benchmarks for telepathy_audio processing stacks.
//!
//! Measures throughput for both input and output processing stacks
//! using simulated audio through mock implementations. Uses cosine waves
//! instead of pink noise for simpler, more deterministic, and easier to
//! verify benchmark inputs.
//!
//! Run with: `cargo bench --bench throughput_bench`
//!
//! Note: These benchmarks are native-only and do not run on WASM targets.

#![cfg(not(target_family = "wasm"))]

mod mock_input;
mod mock_output;

use bytes::{Bytes, BytesMut};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kanal;
use mock_input::MockAudioInput;
use mock_output::NullOutput;
use nnnoiseless::DenoiseState;
use std::f32::consts::PI;
use std::hint::black_box;
use std::thread;
use std::time::Duration;
use telepathy_audio::FRAME_SIZE;
use telepathy_audio::internal::NETWORK_FRAME;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::sea::codec::common::SeaError;
use telepathy_audio::sea::codec::file::SeaFileHeader;
use telepathy_audio::sea::decoder::SeaDecoder;
use telepathy_audio::sea::encoder::{EncoderSettings, SeaEncoder};

/// Number of frames to process in each benchmark iteration.
const BENCHMARK_FRAMES: usize = 1000;

#[derive(Clone)]
struct InputBenchConfig {
    name: &'static str,
    codec: bool,
    denoise: bool,
    resample: bool,
}

#[derive(Clone)]
struct OutputBenchConfig {
    name: &'static str,
    codec: bool,
    resample: bool,
}

/// Benchmarks input stack throughput
fn bench_input_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("input_throughput");
    group.sample_size(40);
    group.measurement_time(Duration::from_secs(60));

    let configs = vec![
        InputBenchConfig {
            name: "pure",
            codec: false,
            denoise: false,
            resample: false,
        },
        InputBenchConfig {
            name: "encode",
            codec: true,
            denoise: false,
            resample: false,
        },
        InputBenchConfig {
            name: "resample",
            codec: false,
            denoise: false,
            resample: true,
        },
        InputBenchConfig {
            name: "denoise",
            codec: false,
            denoise: true,
            resample: false,
        },
        InputBenchConfig {
            name: "denoise+encode",
            codec: true,
            denoise: true,
            resample: false,
        },
        InputBenchConfig {
            name: "resample+denoise",
            codec: false,
            denoise: true,
            resample: true,
        },
        InputBenchConfig {
            name: "resample+denoise+encode",
            codec: true,
            denoise: true,
            resample: true,
        },
    ];

    for config in configs {
        group.bench_with_input(
            BenchmarkId::new("input_throughput", config.name),
            &config,
            |b, config| {
                let denoiser = DenoiseState::new();
                let config = config.clone();

                b.iter(|| {
                    let (mock_input, input_rate) = if config.resample {
                        (
                            MockAudioInput::new_44100hz(Some(benchmark_samples())),
                            44_100,
                        )
                    } else {
                        (MockAudioInput::new_48khz(Some(benchmark_samples())), 48_000)
                    };
                    let (processor_tx, processor_rx) = kanal::unbounded();
                    let denoiser = config.denoise.then_some(denoiser.clone());
                    let a = Default::default();
                    let b = Default::default();
                    let c = Default::default();
                    let d = Default::default();
                    let state = InputProcessorState::new(&a, &b, &c, d, 1024);

                    let sample_rate = if config.denoise || config.resample {
                        48_000
                    } else {
                        input_rate
                    };
                    let ratio = sample_rate as f64 / input_rate as f64;

                    let encoder = if config.codec {
                        SeaEncoder::new(1, sample_rate, EncoderSettings::default()).ok()
                    } else {
                        None
                    };

                    let processor_handle = thread::spawn(move || {
                        input_processor(mock_input, processor_tx, ratio, denoiser, state, encoder)
                    });

                    let mut count = 0;
                    while count < BENCHMARK_FRAMES {
                        if processor_rx.recv_timeout(Duration::from_secs(1)).is_ok() {
                            count += 1;
                        } else {
                            break;
                        }
                    }

                    drop(processor_rx);
                    let _ = processor_handle.join();
                    black_box(count)
                });
            },
        );
    }

    group.finish();
}

/// Benchmarks output stack throughput
fn bench_output_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("output_throughput");
    group.sample_size(40);
    group.measurement_time(Duration::from_secs(30));

    let configs = vec![
        OutputBenchConfig {
            name: "pure",
            codec: false,
            resample: false,
        },
        OutputBenchConfig {
            name: "resample",
            codec: false,
            resample: true,
        },
        OutputBenchConfig {
            name: "codec",
            codec: true,
            resample: false,
        },
        OutputBenchConfig {
            name: "codec_resample",
            codec: true,
            resample: true,
        },
    ];

    for config in configs {
        group.bench_with_input(
            BenchmarkId::new("output_throughput", config.name),
            &config,
            |b, config| {
                let config = config.clone();

                // Pre-generate raw audio frames based on noise type
                let raw_frames = generate_cos_frames(BENCHMARK_FRAMES);

                // Pre-encode frames based on codec setting
                let pre_encoded_frames: Vec<Bytes> = if config.codec {
                    encode_frames_sea(&raw_frames).expect("Failed to encode frames")
                } else {
                    frames_to_bytes(&raw_frames)
                };

                b.iter(|| {
                    let (input_tx, input_rx) = kanal::unbounded();
                    let mock_output = NullOutput::new();
                    let state = OutputProcessorState::default();

                    let decoder = if config.codec {
                        SeaDecoder::new(SeaFileHeader {
                            version: 1,
                            channels: 1,
                            chunk_size: 960,
                            frames_per_chunk: 480,
                            sample_rate: 48_000,
                        })
                        .ok()
                    } else {
                        None
                    };

                    let ratio = if config.resample { 0.9 } else { 1.0 };

                    let handle = thread::spawn(move || {
                        output_processor(input_rx, mock_output, ratio, state, decoder)
                    });

                    // Send pre-generated frames to the output processor
                    for frame in &pre_encoded_frames {
                        if input_tx.send(frame.clone()).is_err() {
                            break;
                        }
                    }

                    drop(input_tx);
                    let _ = handle.join();
                    black_box(BENCHMARK_FRAMES)
                });
            },
        );
    }

    group.finish();
}

/// Generates cosine wave frames at 440 Hz with 48kHz sample rate.
///
/// Pre-generates frames to avoid measuring generation overhead in benchmarks.
fn generate_cos_frames(num_frames: usize) -> Vec<[i16; FRAME_SIZE]> {
    let sample_rate = 48_000.0_f64;
    let frequency = 440.0_f64;
    let amplitude = 0.5_f32;
    let phase_increment = 2.0 * PI as f64 * frequency / sample_rate;

    let mut phase = 0.0_f64;
    let mut frames = Vec::with_capacity(num_frames);

    for _ in 0..num_frames {
        let mut frame = [0i16; FRAME_SIZE];
        for sample in frame.iter_mut() {
            let f32_sample = (phase.sin() as f32) * amplitude;
            // Convert f32 [-1.0, 1.0] to i16 range
            *sample = (f32_sample * i16::MAX as f32) as i16;
            phase += phase_increment;
            if phase >= 2.0 * PI as f64 {
                phase -= 2.0 * PI as f64;
            }
        }
        frames.push(frame);
    }

    frames
}

/// Converts i16 frames to raw Bytes for non-codec benchmarks.
///
/// Each frame is converted to NETWORK_FRAME (960) bytes.
fn frames_to_bytes(frames: &[[i16; FRAME_SIZE]]) -> Vec<Bytes> {
    frames
        .iter()
        .map(|frame| {
            // Safety: i16 slice can be reinterpreted as u8 slice
            let bytes =
                unsafe { std::slice::from_raw_parts(frame.as_ptr() as *const u8, NETWORK_FRAME) };
            Bytes::copy_from_slice(bytes)
        })
        .collect()
}

/// Encodes i16 frames using SEA codec for codec benchmarks.
///
/// Returns encoded Bytes ready to be sent to output processor.
fn encode_frames_sea(frames: &[[i16; FRAME_SIZE]]) -> Result<Vec<Bytes>, SeaError> {
    let mut encoder = SeaEncoder::new(1, 48_000, EncoderSettings::default())?;
    let mut encoded_frames = Vec::with_capacity(frames.len());

    for frame in frames {
        let mut buffer = BytesMut::with_capacity(NETWORK_FRAME);
        buffer.resize(NETWORK_FRAME, 0);
        encoder.encode_frame(*frame, &mut buffer)?;
        encoded_frames.push(buffer.freeze());
    }

    Ok(encoded_frames)
}

/// Calculate samples needed for benchmark.
fn benchmark_samples() -> usize {
    // Input processor reads in_len samples per frame, where in_len = FRAME_SIZE / ratio
    // For 48kHz (ratio=1), in_len = FRAME_SIZE = 480
    // We need enough samples for BENCHMARK_FRAMES frames
    FRAME_SIZE * (BENCHMARK_FRAMES + 10) // Extra buffer for resampling
}

criterion_group!(benches, bench_input_throughput, bench_output_throughput,);

criterion_main!(benches);
