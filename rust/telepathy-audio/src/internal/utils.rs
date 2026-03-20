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
