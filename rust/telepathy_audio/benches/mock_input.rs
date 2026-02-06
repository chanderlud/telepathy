//! Mock audio input implementation for benchmarks.
//!
//! Provides a `MockAudioInput` that generates simulated audio samples
//! (sine wave or pink noise) at a configurable sample rate.

use std::f32::consts::PI;
use std::time::Instant;
use telepathy_audio::error::AudioError;
use telepathy_audio::internal::traits::AudioInput;

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

    /// Sets the amplitude of the generated audio.
    pub fn with_amplitude(mut self, amplitude: f32) -> Self {
        self.amplitude = amplitude.clamp(0.0, 1.0);
        self
    }

    /// Returns the number of samples generated so far.
    pub fn samples_generated(&self) -> usize {
        self.samples_generated
    }

    /// Returns the sample rate.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Resets the input state for a new benchmark run.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.samples_generated = 0;
        self.frame_timestamps.clear();
    }
}

impl AudioInput for MockAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError> {
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

/// Mock audio input that generates pink noise.
///
/// Pink noise has equal energy per octave, making it useful for
/// testing frequency-dependent processing like noise reduction.
pub struct PinkNoiseInput {
    /// Sample rate in Hz
    sample_rate: f64,
    /// Number of samples generated
    samples_generated: usize,
    /// Maximum samples to generate
    max_samples: Option<usize>,
    /// Pink noise filter state (Voss-McCartney algorithm)
    rows: [f32; 16],
    /// Current index for the Voss algorithm
    index: u32,
    /// Random number generator state
    rng_state: u64,
    /// Amplitude multiplier
    amplitude: f32,
    /// Frame timestamps
    pub frame_timestamps: Vec<Instant>,
}

impl PinkNoiseInput {
    /// Creates a new pink noise input.
    pub fn new(sample_rate: f64, max_samples: Option<usize>) -> Self {
        Self {
            sample_rate,
            samples_generated: 0,
            max_samples,
            rows: [0.0; 16],
            index: 0,
            rng_state: 0x12345678,
            amplitude: 0.3,
            frame_timestamps: Vec::new(),
        }
    }

    /// Creates a pink noise input at 48kHz.
    pub fn new_48khz(max_samples: Option<usize>) -> Self {
        Self::new(48_000.0, max_samples)
    }

    /// Sets the amplitude.
    pub fn with_amplitude(mut self, amplitude: f32) -> Self {
        self.amplitude = amplitude.clamp(0.0, 1.0);
        self
    }

    /// Returns the sample rate.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Simple LCG random number generator.
    fn next_random(&mut self) -> f32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        // Convert to float in range [-1, 1]
        ((self.rng_state >> 33) as i32 as f32) / (i32::MAX as f32)
    }

    /// Generates the next pink noise sample using Voss-McCartney algorithm.
    fn next_pink_sample(&mut self) -> f32 {
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

        sum * self.amplitude / 4.0 // Normalize
    }

    /// Resets the input state.
    pub fn reset(&mut self) {
        self.samples_generated = 0;
        self.rows = [0.0; 16];
        self.index = 0;
        self.frame_timestamps.clear();
    }
}

impl AudioInput for PinkNoiseInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError> {
        if let Some(max) = self.max_samples {
            if self.samples_generated >= max {
                return Ok(0);
            }
        }

        self.frame_timestamps.push(Instant::now());

        let samples_to_generate = if let Some(max) = self.max_samples {
            dst.len().min(max - self.samples_generated)
        } else {
            dst.len()
        };

        for sample in dst.iter_mut().take(samples_to_generate) {
            *sample = self.next_pink_sample();
        }

        self.samples_generated += samples_to_generate;
        Ok(samples_to_generate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_input_generates_samples() {
        let mut input = MockAudioInput::new_48khz(Some(1000));
        let mut buffer = [0.0f32; 480];

        let read = input.read_into(&mut buffer).unwrap();
        assert_eq!(read, 480);
        assert_eq!(input.samples_generated(), 480);

        // Verify samples are in valid range
        for sample in &buffer {
            assert!(*sample >= -1.0 && *sample <= 1.0);
        }
    }

    #[test]
    fn test_mock_input_ends_at_max_samples() {
        let mut input = MockAudioInput::new_48khz(Some(500));
        let mut buffer = [0.0f32; 480];

        let read1 = input.read_into(&mut buffer).unwrap();
        assert_eq!(read1, 480);

        let read2 = input.read_into(&mut buffer).unwrap();
        assert_eq!(read2, 20); // Only 20 samples remaining

        let read3 = input.read_into(&mut buffer).unwrap();
        assert_eq!(read3, 0); // End of stream
    }

    #[test]
    fn test_pink_noise_generates_samples() {
        let mut input = PinkNoiseInput::new_48khz(Some(1000));
        let mut buffer = [0.0f32; 480];

        let read = input.read_into(&mut buffer).unwrap();
        assert_eq!(read, 480);

        // Verify samples are in valid range
        for sample in &buffer {
            assert!(*sample >= -1.0 && *sample <= 1.0);
        }
    }
}
