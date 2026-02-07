//! Latency benchmarks for telepathy_audio processing stacks.
//!
//! Measures end-to-end latency for both input and output processing stacks
//! using simulated audio through mock implementations.
//!
//! Run with: `cargo bench --bench latency_bench`

mod mock_input;
mod mock_output;

use bytes::{Bytes, BytesMut};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kanal;
use mock_output::NullOutput;
use nnnoiseless::DenoiseState;
use std::f32::consts::PI;
use std::hint::black_box;
use std::thread;
use std::time::Duration;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::sea::codec::common::SeaError;
use telepathy_audio::sea::decoder::SeaDecoder;
use telepathy_audio::sea::encoder::{EncoderSettings, SeaEncoder};
use telepathy_audio::{FRAME_SIZE, SeaFileHeader};
use telepathy_audio::internal::NETWORK_FRAME;
use crate::mock_input::PinkNoiseInput;

/// Number of frames to process in each benchmark iteration.
const BENCHMARK_FRAMES: usize = 1000;

/// Type of noise to generate for benchmarks
#[derive(Clone, Copy)]
enum NoiseType {
    /// Cosine wave at 440 Hz
    Cosine,
    /// Pink noise (Voss-McCartney algorithm)
    PinkNoise,
}

/// Pink noise generator state for Voss-McCartney algorithm.
struct PinkNoiseGenerator {
    rows: [f32; 16],
    index: u32,
    rng_state: u64,
    amplitude: f32,
}

impl PinkNoiseGenerator {
    fn new() -> Self {
        Self {
            rows: [0.0; 16],
            index: 0,
            rng_state: 0x12345678,
            amplitude: 0.3,
        }
    }

    /// Simple LCG random number generator.
    fn next_random(&mut self) -> f32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        ((self.rng_state >> 33) as i32 as f32) / (i32::MAX as f32)
    }

    /// Generates the next pink noise sample using Voss-McCartney algorithm.
    fn next_sample(&mut self) -> f32 {
        let last_index = self.index;
        self.index = self.index.wrapping_add(1);
        let diff = last_index ^ self.index;

        let mut sum = 0.0;
        for i in 0..16 {
            if diff & (1 << i) != 0 {
                self.rows[i] = self.next_random();
            }
            sum += self.rows[i];
        }

        sum * self.amplitude / 4.0
    }
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
    noise_type: NoiseType,
}

/// Benchmarks input stack throughput
fn bench_input_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("input_throughput");
    group.sample_size(40);
    group.measurement_time(Duration::from_secs(50));

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
                            PinkNoiseInput::new(44_100_f64, Some(benchmark_samples())),
                            44_100,
                        )
                    } else {
                        (PinkNoiseInput::new_48khz(Some(benchmark_samples())), 48_000)
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
    group.sample_size(40);
    group.measurement_time(Duration::from_secs(30));

    let configs = vec![
        OutputBenchConfig {
            name: "pure",
            codec: false,
            resample: false,
            noise_type: NoiseType::PinkNoise,
        },
        OutputBenchConfig {
            name: "resample",
            codec: false,
            resample: true,
            noise_type: NoiseType::PinkNoise,
        },
        OutputBenchConfig {
            name: "codec",
            codec: true,
            resample: false,
            noise_type: NoiseType::PinkNoise,
        },
        OutputBenchConfig {
            name: "codec_resample",
            codec: true,
            resample: true,
            noise_type: NoiseType::PinkNoise,
        },
    ];

    for config in configs {
        group.bench_with_input(
            BenchmarkId::new("output_throughput", config.name),
            &config,
            |b, config| {
                let config = config.clone();

                // Pre-generate raw audio frames based on noise type
                let raw_frames = match config.noise_type {
                    NoiseType::Cosine => generate_cos_frames(BENCHMARK_FRAMES),
                    NoiseType::PinkNoise => generate_pink_noise_frames(BENCHMARK_FRAMES),
                };

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

/// Generates pink noise frames using Voss-McCartney algorithm.
///
/// Pre-generates frames to avoid measuring generation overhead in benchmarks.
fn generate_pink_noise_frames(num_frames: usize) -> Vec<[i16; FRAME_SIZE]> {
    let mut generator = PinkNoiseGenerator::new();
    let mut frames = Vec::with_capacity(num_frames);

    for _ in 0..num_frames {
        let mut frame = [0i16; FRAME_SIZE];
        for sample in frame.iter_mut() {
            let f32_sample = generator.next_sample();
            // Convert f32 [-1.0, 1.0] to i16 range, clamping to prevent overflow
            *sample = (f32_sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
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
            let bytes = unsafe {
                std::slice::from_raw_parts(frame.as_ptr() as *const u8, NETWORK_FRAME)
            };
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

criterion_group!(benches,
    bench_input_throughput,
    bench_output_throughput,);

criterion_main!(benches);
