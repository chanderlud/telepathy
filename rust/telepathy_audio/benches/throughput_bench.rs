//! Latency benchmarks for telepathy_audio processing stacks.
//!
//! Measures end-to-end latency for both input and output processing stacks
//! using simulated audio through mock implementations.
//!
//! Run with: `cargo bench --bench latency_bench`

mod mock_input;
mod mock_output;

use bytes::BytesMut;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kanal;
use mock_input::MockAudioInput;
use mock_output::NullOutput;
use nnnoiseless::DenoiseState;
use std::hint::black_box;
use std::thread;
use std::time::Duration;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::sea::decoder::SeaDecoder;
use telepathy_audio::sea::encoder::{EncoderSettings, SeaEncoder};
use telepathy_audio::{FRAME_SIZE, SeaFileHeader};

/// Number of frames to process in each benchmark iteration.
const BENCHMARK_FRAMES: usize = 1000;

/// Calculate samples needed for benchmark.
fn benchmark_samples() -> usize {
    // Input processor reads in_len samples per frame, where in_len = FRAME_SIZE / ratio
    // For 48kHz (ratio=1), in_len = FRAME_SIZE = 480
    // We need enough samples for BENCHMARK_FRAMES frames
    FRAME_SIZE * (BENCHMARK_FRAMES + 10) // Extra buffer for resampling
}

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
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(20));

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

                    let encoder = if config.codec {
                        SeaEncoder::new(
                            1,
                            if config.denoise { 48_000 } else { input_rate },
                            EncoderSettings::default(),
                        )
                        .ok()
                    } else {
                        None
                    };

                    let processor_handle = thread::spawn(move || {
                        input_processor(
                            mock_input,
                            processor_tx,
                            input_rate as f64,
                            denoiser,
                            state,
                            encoder,
                        )
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
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(10));

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

                    // TODO generate real frames and encoded frames
                    for _ in 0..BENCHMARK_FRAMES {
                        let mut frame = BytesMut::with_capacity(FRAME_SIZE * 2);
                        frame.resize(FRAME_SIZE * 2, 0);
                        // if input_tx.send(frame).is_err() {
                        //     break;
                        // }
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

criterion_group!(benches, bench_input_throughput, bench_output_throughput,);

criterion_main!(benches);
