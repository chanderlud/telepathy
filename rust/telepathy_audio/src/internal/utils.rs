//! Utility functions for audio processing.
//!
//! This module provides helper functions for resampling, silence transitions,
//! and audio stream management.
//!
//! ## Resampling
//!
//! [`resampler_factory`] creates a high-quality sinc resampler when sample rate
//! conversion is needed. Returns `None` when ratio is 1.0 to avoid unnecessary
//! processing overhead.
//!
//! ## Silence Transitions
//!
//! [`make_transition_up`] and [`make_transition_down`] create smooth ramps
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
//! [`SendStream`] wraps a cpal stream to allow sending across thread boundaries.
//! This is necessary because cpal streams are not inherently `Send`.

use crate::constants::RESAMPLER_PARAMETERS;
use crate::error::Error;
use nnnoiseless::FRAME_SIZE;
use rubato::SincFixedIn;

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

/// Creates a resampler if needed based on the sample rate ratio.
///
/// Returns `None` if no resampling is needed (ratio == 1.0), which allows
/// callers to skip resampling entirely and pass through samples unchanged.
/// This optimization avoids the computational overhead of resampling when
/// source and target sample rates match.
///
/// The resampler uses high-quality sinc interpolation with parameters from
/// [`RESAMPLER_PARAMETERS`](crate::constants::RESAMPLER_PARAMETERS).
///
/// # Arguments
///
/// * `ratio` - The resampling ratio (target_rate / source_rate)
///   - `ratio > 1.0`: upsampling (e.g., 44100 → 48000)
///   - `ratio < 1.0`: downsampling (e.g., 48000 → 44100)
///   - `ratio == 1.0`: no resampling needed
/// * `channels` - Number of audio channels (typically 1 for mono)
/// * `size` - Input chunk size (number of samples per channel)
///
/// # Returns
///
/// * `Ok(Some(resampler))` - A configured sinc resampler when ratio != 1.0
/// * `Ok(None)` - When no resampling is needed (pass through samples)
/// * `Err(_)` - When resampler creation fails (invalid parameters)
pub fn resampler_factory(
    ratio: f64,
    channels: usize,
    size: usize,
) -> Result<Option<SincFixedIn<f32>>, Error> {
    if ratio == 1_f64 {
        Ok(None)
    } else {
        // create the resampler if needed
        Ok(Some(SincFixedIn::<f32>::new(
            ratio,
            2_f64,
            RESAMPLER_PARAMETERS,
            size,
            channels,
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
