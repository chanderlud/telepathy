//! Utility functions for audio processing.
//!
//! This module provides helper functions for resampling and silence transitions.
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

use crate::constants::RESAMPLER_PARAMETERS;
use crate::error::AudioError;
use nnnoiseless::FRAME_SIZE;
use rubato::SincFixedIn;

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
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::resampler_factory;
///
/// // Upsampling from 44100 to 48000 Hz
/// let ratio = 48000.0 / 44100.0;
/// let resampler = resampler_factory(ratio, 1, 480)?;
///
/// match resampler {
///     Some(mut r) => { /* use r.process_into_buffer(...) */ }
///     None => { /* pass through unchanged */ }
/// }
/// # Ok::<(), telepathy_audio::AudioError>(())
/// ```
pub fn resampler_factory(
    ratio: f64,
    channels: usize,
    size: usize,
) -> Result<Option<SincFixedIn<f32>>, AudioError> {
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
