//! Utility functions for audio processing.
//!
//! This module provides helper functions for resampling, silence transitions,
//! and audio stream management.
//!
//! ## Resampling
//!
//! [`resampler_factory`] creates a `rubato::Fft` resampler when the input and
//! output sample rates differ. It returns `None` when the rates already match so
//! callers can pass samples through unchanged.
//!
//! ## Silence Transitions
//!
//! `hann_fade_in` and `hann_fade_out` create smooth ramps
//! between silence and audio to prevent audible clicks and pops. These are
//! used by the input processor when silence detection is enabled.
//!
//! ## Volume Conversion
//!
//! [`db_to_multiplier`] converts decibel values to linear multipliers for
//! volume control. This is the standard formula used in audio engineering.
//!
//! ## Stream Wrapper
//!
//! `SendStream` wraps a cpal stream to allow sending across thread boundaries.
//! This is necessary because cpal streams are not inherently `Send`.

use crate::error::Error;
use rubato::{Fft, FixedSync};

/// Converts a decibel value to a linear multiplier.
///
/// This uses the standard audio engineering formula:
/// `multiplier = 10^(dB / 20)`
///
/// # Arguments
///
/// * `db` - The decibel value to convert. Can be negative (attenuation)
///   or positive (amplification).
///
/// # Returns
///
/// The linear multiplier corresponding to the decibel value.
///
/// # Examples
///
/// ```rust
/// use telepathy_audio::db_to_multiplier;
///
/// // 0 dB = unity gain (multiplier of 1.0)
/// assert!((db_to_multiplier(0.0) - 1.0).abs() < 0.001);
///
/// // -6 dB ≈ half amplitude
/// assert!((db_to_multiplier(-6.0) - 0.5).abs() < 0.02);
///
/// // +6 dB ≈ double amplitude
/// assert!((db_to_multiplier(6.0) - 2.0).abs() < 0.05);
/// ```
pub fn db_to_multiplier(db: f32) -> f32 {
    10_f32.powf(db / 20_f32)
}

/// Creates a resampler when the input and output sample rates differ.
///
/// Returns `None` when the rates are equal, which lets callers skip
/// resampling entirely and pass samples through unchanged.
///
/// When resampling is required this builds a `rubato::Fft<f32>` configured
/// directly from the input rate, output rate, per-channel frame size, channel
/// count, and [`FixedSync`] mode selected by the caller.
///
/// # Arguments
///
/// * `input_rate` - Source sample rate in Hz
/// * `output_rate` - Destination sample rate in Hz
/// * `channels` - Number of audio channels (typically 1 for mono)
/// * `size` - Input chunk size in frames per channel
/// * `mode` - Synchronization mode used by the FFT resampler
///
/// # Returns
///
/// * `Ok(Some(resampler))` - A configured FFT resampler when the rates differ
/// * `Ok(None)` - When no resampling is needed (pass through samples)
/// * `Err(_)` - When resampler creation fails (invalid parameters)
pub fn resampler_factory(
    input_rate: usize,
    output_rate: usize,
    channels: usize,
    size: usize,
    mode: FixedSync,
) -> Result<Option<Fft<f32>>, Error> {
    if input_rate == output_rate {
        Ok(None)
    } else if channels == 0 {
        Err(Error::Processing("Resampler requires > 0 channels".to_string()))
    } else {
        // create the resampler if needed
        Ok(Some(Fft::<f32>::new(
            input_rate,
            output_rate,
            size,
            1,
            channels,
            mode,
        )?))
    }
}

#[inline]
pub(crate) fn hann_fade_in(i: usize, len: usize) -> f32 {
    // t in (0, 1]
    let t = (i + 1) as f32 / len as f32;
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

#[inline]
pub(crate) fn hann_fade_out(i: usize, len: usize) -> f32 {
    // t in (0, 1]
    let t = (i + 1) as f32 / len as f32;
    0.5 + 0.5 * (std::f32::consts::PI * t).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, tol: f32) {
        assert!(
            (a - b).abs() <= tol,
            "expected {a} to be within {tol} of {b}"
        );
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn resampler_factory_same_rate_returns_none() {
        let result = resampler_factory(48_000, 48_000, 1, 480, FixedSync::Input).unwrap();
        assert!(result.is_none());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn resampler_factory_different_rates_returns_some() {
        let result = resampler_factory(48_000, 44_100, 1, 480, FixedSync::Input).unwrap();
        assert!(result.is_some());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn resampler_factory_zero_channel_errors() {
        let result = resampler_factory(48_000, 44_100, 0, 480, FixedSync::Input);
        assert!(result.is_err());
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn resampler_factory_zero_size_errors() {
        let result = resampler_factory(48_000, 44_100, 1, 0, FixedSync::Input);
        assert!(result.is_err());
    }

    #[test]
    fn hann_fade_in_starts_near_zero() {
        let len = 64;
        let expected = 0.5 - 0.5 * (std::f32::consts::PI / len as f32).cos();
        approx_eq(hann_fade_in(0, len), expected, f32::EPSILON * 4.0);
    }

    #[test]
    fn hann_fade_in_ends_at_one() {
        let len = 64;
        approx_eq(hann_fade_in(len - 1, len), 1.0, f32::EPSILON * 4.0);
    }

    #[test]
    fn hann_fade_out_starts_near_one() {
        let len = 64;
        approx_eq(hann_fade_out(0, len), 1.0, 0.001);
    }

    #[test]
    fn hann_fade_out_ends_near_zero() {
        let len = 64;
        approx_eq(hann_fade_out(len - 1, len), 0.0, f32::EPSILON * 4.0);
    }

    #[test]
    fn hann_fade_in_is_monotonically_increasing() {
        let len = 64;
        for i in 0..(len - 1) {
            assert!(hann_fade_in(i + 1, len) >= hann_fade_in(i, len));
        }
    }

    #[test]
    fn hann_fade_out_is_monotonically_decreasing() {
        let len = 64;
        for i in 0..(len - 1) {
            assert!(hann_fade_out(i + 1, len) <= hann_fade_out(i, len));
        }
    }

    #[test]
    fn hann_fade_in_plus_fade_out_equals_one() {
        let len = 64;
        for i in 0..len {
            approx_eq(
                hann_fade_in(i, len) + hann_fade_out(i, len),
                1.0,
                f32::EPSILON * 4.0,
            );
        }
    }

    #[test]
    fn db_to_multiplier_zero_db() {
        approx_eq(db_to_multiplier(0.0), 1.0, f32::EPSILON * 4.0);
    }

    #[test]
    fn db_to_multiplier_negative_6db() {
        approx_eq(db_to_multiplier(-6.0), 0.5, 0.02);
    }

    #[test]
    fn db_to_multiplier_positive_6db() {
        approx_eq(db_to_multiplier(6.0), 2.0, 0.05);
    }
}
