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

/// Creates a transition buffer for ramping up from silence to audio.
///
/// This function generates a smooth linear ramp from 0 to the target sample
/// value, preventing audible clicks when transitioning from silence to audio.
/// The ramp is placed at the end of a 480-sample ([`FRAME_SIZE`]) frame, with
/// zeros filling the beginning.
///
/// ## Audio Engineering Rationale
///
/// Abrupt transitions between silence and audio create high-frequency
/// transients that are perceived as clicks or pops. Linear ramps spread the
/// energy change over multiple samples, making the transition imperceptible.
///
/// ## Ramp Pattern
///
/// ```text
/// Sample value
///     ^
///     |        ╱ target
///     |      ╱
///     |    ╱
///     |__╱____________
///     0   start   480
///         ↑
///     (FRAME_SIZE - length)
/// ```
///
/// # Arguments
///
/// * `length` - The length of the transition in samples (must be <= 480)
/// * `sample` - The target sample value to ramp up to
///
/// # Panics
///
/// Panics if `length` > `FRAME_SIZE` (480).
pub(crate) fn make_transition_up(length: usize, sample: i16) -> [i16; FRAME_SIZE] {
    assert!(length <= FRAME_SIZE, "length must be <= {}", FRAME_SIZE);

    let mut buf = [0; FRAME_SIZE];

    let start = FRAME_SIZE - length;
    let f = sample as i32 / length as i32;

    for i in 0..length {
        // i goes from 0 to length-1
        // Last value (i = length-1) will be: s * length / length = s
        let value = f * (i as i32 + 1);
        buf[start + i] = value as i16;
    }

    buf
}

/// Creates a transition buffer for ramping down from audio to silence.
///
/// This function generates a smooth linear ramp from the starting sample
/// value down to 0, preventing audible clicks when transitioning from audio
/// to silence. The ramp is placed at the beginning of a 480-sample
/// ([`FRAME_SIZE`]) frame, with zeros filling the remainder.
///
/// ## Audio Engineering Rationale
///
/// See [`make_transition_up`] for the rationale. This function handles the
/// opposite case: fading out to silence.
///
/// ## Ramp Pattern
///
/// ```text
/// Sample value
///     ^
/// start ╲
///     |  ╲
///     |   ╲
///     |____╲__________
///     0   length   480
/// ```
///
/// # Arguments
///
/// * `length` - The length of the transition in samples (must be <= 480)
/// * `sample` - The starting sample value to ramp down from
///
/// # Panics
///
/// Panics if `length` > `FRAME_SIZE` (480).
pub(crate) fn make_transition_down(length: usize, sample: i16) -> [i16; FRAME_SIZE] {
    assert!(length <= FRAME_SIZE, "length must be <= {}", FRAME_SIZE);

    let mut buf = [0; FRAME_SIZE];

    let l = length as i32;
    let f = sample as i32 / l;

    // First length items: linear ramp from `sample` down toward 0
    // Remaining (FRAME_SIZE - length) items are left as 0.
    for (i, item) in buf.iter_mut().enumerate().take(length) {
        // i = 0       → value ≈ sample
        // i = length - 1   → value ≈ sample * 1/m
        let value = f * (l - i as i32);
        *item = value as i16;
    }

    buf
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    #[test]
    fn transition_up_creates_smooth_ramp() {
        let transition = make_transition_up(10, 1000);

        // First 470 samples should be zero
        assert!(transition[..470].iter().all(|&x| x == 0));

        // Last 10 samples should ramp from ~100 to 1000
        assert!(transition[470] > 0 && transition[470] < 200);
        assert!(transition[479] > 900 && transition[479] <= 1000);
    }

    #[test]
    fn transition_down_creates_smooth_ramp() {
        let transition = make_transition_down(10, 1000);

        // First sample should be close to 1000
        assert!(transition[0] > 900 && transition[0] <= 1000);

        // 10th sample should be close to 0
        assert!(transition[9] >= 0 && transition[9] < 200);

        // Remaining samples should be zero
        assert!(transition[10..].iter().all(|&x| x == 0));
    }
}
