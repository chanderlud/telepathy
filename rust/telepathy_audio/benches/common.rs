#![allow(dead_code)]

use std::f32::consts::PI;
use std::time::Instant;
use telepathy_audio::Error;
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};

/// Mock audio input that generates simulated audio samples.
///
/// This implementation generates a sine wave at a configurable frequency,
/// useful for testing the audio processing pipeline without hardware.
pub struct MockAudioInput {
    /// Sample rate in Hz
    sample_rate: f64,
    /// Frequency of the generated sine wave in Hz
    frequency: f64,
    /// Current phase of the sine wave (0.0 to 2π)
    phase: f64,
    /// Number of samples generated
    samples_generated: usize,
    /// Maximum samples to generate (None = infinite)
    max_samples: Option<usize>,
    /// Timestamp when each frame is generated
    pub frame_timestamps: Vec<Instant>,
    /// Amplitude of the generated audio (0.0 to 1.0)
    amplitude: f32,
}

impl MockAudioInput {
    /// Creates a new mock audio input.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Sample rate in Hz (e.g., 44100, 48000)
    /// * `frequency` - Frequency of the sine wave to generate (e.g., 440.0 for A4)
    /// * `max_samples` - Maximum number of samples to generate, or None for infinite
    pub fn new(sample_rate: f64, frequency: f64, max_samples: Option<usize>) -> Self {
        Self {
            sample_rate,
            frequency,
            phase: 0.0,
            samples_generated: 0,
            max_samples,
            frame_timestamps: Vec::new(),
            amplitude: 0.5,
        }
    }

    /// Creates a mock input configured for 48kHz (standard for voice processing).
    pub fn new_48khz(max_samples: Option<usize>) -> Self {
        Self::new(48_000.0, 440.0, max_samples)
    }

    /// Creates a mock input configured for 44.1kHz (CD quality, tests resampling).
    pub fn new_44100hz(max_samples: Option<usize>) -> Self {
        Self::new(44_100.0, 440.0, max_samples)
    }
}

impl AudioInput for MockAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
        // Check if we've reached the sample limit
        if let Some(max) = self.max_samples {
            if self.samples_generated >= max {
                return Ok(0); // End of stream
            }
        }

        // Record timestamp for this frame
        self.frame_timestamps.push(Instant::now());

        // Calculate phase increment per sample
        let phase_increment = 2.0 * PI as f64 * self.frequency / self.sample_rate;

        let samples_to_generate = if let Some(max) = self.max_samples {
            dst.len().min(max - self.samples_generated)
        } else {
            dst.len()
        };

        // Generate sine wave samples
        for sample in dst.iter_mut().take(samples_to_generate) {
            *sample = (self.phase.sin() as f32) * self.amplitude;
            self.phase += phase_increment;

            // Keep phase in [0, 2π) to prevent precision loss
            if self.phase >= 2.0 * PI as f64 {
                self.phase -= 2.0 * PI as f64;
            }
        }

        self.samples_generated += samples_to_generate;
        Ok(samples_to_generate)
    }
}

/// A mock output that discards all samples but tracks timing.
///
/// Useful for benchmarking throughput without memory overhead.
pub struct NullOutput {
    /// Timestamps when frames are received
    pub frame_timestamps: Vec<Instant>,
    /// Number of samples received
    samples_received: usize,
}

impl NullOutput {
    /// Creates a new null output.
    pub fn new() -> Self {
        Self {
            frame_timestamps: Vec::with_capacity(10_000),
            samples_received: 0,
        }
    }
}

impl Default for NullOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutput for NullOutput {
    fn is_full(&self) -> bool {
        false // Never full
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        self.frame_timestamps.push(Instant::now());
        self.samples_received += samples.len();
        Ok(0) // No samples dropped
    }
}

pub fn dummy_float_frame() -> [f32; 4096] {
    let mut frame = [0_f32; 4096];
    for (i, x) in frame.iter_mut().enumerate() {
        // Deterministic pattern – no RNG so the benchmark is reproducible on WASM
        *x = ((i as f32 / 4096.0) * 2.0 - 1.0) * 0.9;
    }
    frame
}

pub fn dummy_int_frame() -> [i16; 4096] {
    let mut frame = [0_i16; 4096];
    for (i, x) in frame.iter_mut().enumerate() {
        *x = ((i as f32 / 4096.0) * 2.0 - 1.0) as i16 * 16000;
    }
    frame
}
