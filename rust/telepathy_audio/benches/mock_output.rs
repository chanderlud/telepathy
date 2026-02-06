//! Mock audio output implementation for benchmarks.
//!
//! Provides a `MockAudioOutput` that accepts and stores audio samples
//! in an internal buffer, tracking timestamps for latency measurement.

use std::time::Instant;
use telepathy_audio::error::AudioError;
use telepathy_audio::internal::traits::AudioOutput;

/// Mock audio output that stores received samples.
///
/// This implementation accepts audio samples and stores them in an internal
/// buffer, useful for testing the audio processing pipeline without hardware.
pub struct MockAudioOutput {
    /// Internal buffer for received samples
    buffer: Vec<f32>,
    /// Maximum buffer size before reporting full
    max_buffer_size: usize,
    /// Timestamps when frames are received
    pub frame_timestamps: Vec<Instant>,
    /// Number of samples received
    samples_received: usize,
    /// Number of samples dropped due to buffer full
    samples_dropped: usize,
    /// Whether to simulate backpressure (return is_full = true)
    simulate_backpressure: bool,
    /// Frames after which to simulate backpressure
    backpressure_after_frames: Option<usize>,
}

impl MockAudioOutput {
    /// Creates a new mock audio output.
    ///
    /// # Arguments
    ///
    /// * `max_buffer_size` - Maximum number of samples to buffer before reporting full
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(max_buffer_size),
            max_buffer_size,
            frame_timestamps: Vec::new(),
            samples_received: 0,
            samples_dropped: 0,
            simulate_backpressure: false,
            backpressure_after_frames: None,
        }
    }

    /// Creates a mock output with a large buffer (suitable for benchmarks).
    pub fn new_large() -> Self {
        // Buffer for ~10 seconds of audio at 48kHz
        Self::new(480_000)
    }

    /// Creates a mock output with a small buffer (for testing backpressure).
    pub fn new_small() -> Self {
        // Buffer for ~50ms of audio at 48kHz
        Self::new(2_400)
    }

    /// Configure to simulate backpressure after a certain number of frames.
    pub fn with_backpressure_after(mut self, frames: usize) -> Self {
        self.backpressure_after_frames = Some(frames);
        self
    }

    /// Returns the number of samples received.
    pub fn samples_received(&self) -> usize {
        self.samples_received
    }

    /// Returns the number of samples dropped.
    pub fn samples_dropped(&self) -> usize {
        self.samples_dropped
    }

    /// Returns a reference to the received samples.
    pub fn samples(&self) -> &[f32] {
        &self.buffer
    }

    /// Drains and returns all received samples.
    pub fn drain(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.buffer)
    }

    /// Resets the output state for a new benchmark run.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.frame_timestamps.clear();
        self.samples_received = 0;
        self.samples_dropped = 0;
        self.simulate_backpressure = false;
    }

    /// Returns the number of frames received.
    pub fn frames_received(&self) -> usize {
        self.frame_timestamps.len()
    }
}

impl AudioOutput for MockAudioOutput {
    fn is_full(&self) -> bool {
        // Check if we should simulate backpressure
        if let Some(threshold) = self.backpressure_after_frames {
            if self.frame_timestamps.len() >= threshold {
                return true;
            }
        }

        self.simulate_backpressure || self.buffer.len() >= self.max_buffer_size
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
        // Record timestamp
        self.frame_timestamps.push(Instant::now());

        // Calculate how many samples we can accept
        let space_available = self.max_buffer_size.saturating_sub(self.buffer.len());
        let samples_to_write = samples.len().min(space_available);
        let samples_to_drop = samples.len() - samples_to_write;

        // Write samples to buffer
        self.buffer.extend_from_slice(&samples[..samples_to_write]);
        self.samples_received += samples_to_write;
        self.samples_dropped += samples_to_drop;

        // Return number of dropped samples
        Ok(samples_to_drop)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_output_receives_samples() {
        let mut output = MockAudioOutput::new_large();
        let samples = [0.5f32; 480];

        let dropped = output.write_samples(&samples).unwrap();
        assert_eq!(dropped, 0);
        assert_eq!(output.samples_received(), 480);
        assert_eq!(output.samples().len(), 480);
    }

    #[test]
    fn test_mock_output_tracks_frames() {
        let mut output = MockAudioOutput::new_large();
        let samples = [0.5f32; 480];

        output.write_samples(&samples).unwrap();
        output.write_samples(&samples).unwrap();

        assert_eq!(output.frames_received(), 2);
        assert_eq!(output.frame_timestamps.len(), 2);
    }

    #[test]
    fn test_mock_output_reports_full() {
        let mut output = MockAudioOutput::new(100);
        let samples = [0.5f32; 80];

        output.write_samples(&samples).unwrap();
        assert!(!output.is_full());

        output.write_samples(&samples).unwrap();
        assert!(output.is_full());
    }

    #[test]
    fn test_mock_output_drops_when_full() {
        let mut output = MockAudioOutput::new(100);
        let samples = [0.5f32; 80];

        output.write_samples(&samples).unwrap();
        let dropped = output.write_samples(&samples).unwrap();

        assert_eq!(dropped, 60); // 80 - 20 available = 60 dropped
        assert_eq!(output.samples_dropped(), 60);
    }

    #[test]
    fn test_null_output() {
        let mut output = NullOutput::new();
        let samples = [0.5f32; 480];

        let dropped = output.write_samples(&samples).unwrap();
        assert_eq!(dropped, 0);
        assert_eq!(output.samples_received(), 480);
        assert!(!output.is_full());
    }

    #[test]
    fn test_backpressure_simulation() {
        let mut output = MockAudioOutput::new_large().with_backpressure_after(2);
        let samples = [0.5f32; 480];

        output.write_samples(&samples).unwrap();
        assert!(!output.is_full());

        output.write_samples(&samples).unwrap();
        assert!(output.is_full()); // Should be full after 2 frames
    }
}
