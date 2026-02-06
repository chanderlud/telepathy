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
use telepathy_audio::FRAME_SIZE;
use telepathy_audio::internal::codec::encoder;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};

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
                    let (processor_to_encoder_tx, processor_to_encoder_rx) = kanal::unbounded();
                    let (encoder_to_output_tx, encoder_to_output_rx) = kanal::unbounded();
                    let denoiser = config.denoise.then_some(denoiser.clone());
                    let state = InputProcessorState::default();

                    let (processor_output, encoder_handle) = if config.codec {
                        let encoder_rate = if config.denoise { 48_000 } else { input_rate };

                        let encoder_handle = thread::spawn(move || {
                            encoder(
                                processor_to_encoder_rx,
                                encoder_to_output_tx,
                                encoder_rate,
                                true,
                                5.0,
                            )
                        });

                        (processor_to_encoder_tx, Some(encoder_handle))
                    } else {
                        (encoder_to_output_tx, None)
                    };

                    let processor_handle = thread::spawn(move || {
                        input_processor(
                            mock_input,
                            processor_output,
                            input_rate as f64,
                            denoiser,
                            state,
                        )
                    });

                    let mut count = 0;
                    while count < BENCHMARK_FRAMES {
                        if encoder_to_output_rx
                            .recv_timeout(Duration::from_secs(1))
                            .is_ok()
                        {
                            count += 1;
                        } else {
                            break;
                        }
                    }

                    drop(encoder_to_output_rx);
                    if let Some(handle) = encoder_handle {
                        _ = handle.join();
                    }
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

    for (ratio, label) in [(0.9, "48kHz downsample"), (1.0, "48kHz_to_48kHz")] {
        group.bench_function(label, |b| {
            b.iter(|| {
                let (input_tx, input_rx) = kanal::unbounded::<BytesMut>();
                let mock_output = NullOutput::new();
                let state = OutputProcessorState::default();

                let handle =
                    thread::spawn(move || output_processor(input_rx, mock_output, ratio, state));

                for _ in 0..BENCHMARK_FRAMES {
                    let mut frame = BytesMut::with_capacity(FRAME_SIZE * 2);
                    frame.resize(FRAME_SIZE * 2, 0);
                    if input_tx.send(frame).is_err() {
                        break;
                    }
                }

                drop(input_tx);
                let _ = handle.join();
                black_box(BENCHMARK_FRAMES)
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_input_throughput, bench_output_throughput,);

criterion_main!(benches);
