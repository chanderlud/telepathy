//! Mock audio output implementation for benchmarks.
//!
//! Provides a `MockAudioOutput` that accepts and stores audio samples
//! in an internal buffer, tracking timestamps for latency measurement.

use std::time::Instant;
use telepathy_audio::error::AudioError;
use telepathy_audio::internal::traits::AudioOutput;

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

    /// Returns the number of samples received.
    pub fn samples_received(&self) -> usize {
        self.samples_received
    }

    /// Returns the number of frames received.
    pub fn frames_received(&self) -> usize {
        self.frame_timestamps.len()
    }

    /// Resets the output state.
    pub fn reset(&mut self) {
        self.frame_timestamps.clear();
        self.samples_received = 0;
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

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
        self.frame_timestamps.push(Instant::now());
        self.samples_received += samples.len();
        Ok(0) // No samples dropped
    }
}
