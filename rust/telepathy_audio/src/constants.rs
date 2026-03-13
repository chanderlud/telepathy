//! Audio processing constants and configuration parameters.
//!
//! This module contains constants used throughout the audio processing pipeline.
//! These values have been chosen to balance audio quality with real-time performance
//! requirements.

pub use nnnoiseless::FRAME_SIZE;

/// Minimum number of consecutive silent frames before silence is detected.
///
/// ## Rationale
///
/// Silences shorter than this threshold aren't considered true silence,
/// preventing choppy audio from brief pauses in speech. At 48kHz with
/// 480-sample frames (10ms each), 40 frames represents 400ms of silence.
///
/// This duration is chosen because:
/// - Natural speech pauses are typically 200-600ms
/// - Shorter pauses (breathing, consonants) should not trigger silence
/// - Longer pauses indicate intentional silence or speaker change
///
/// ## Performance Impact
///
/// Higher values reduce false positives but delay silence detection.
/// Lower values are more responsive but may create choppy audio.
pub(crate) const MINIMUM_SILENCE_LENGTH: u16 = 40;

/// Number of samples used for silence transitions (fade in/out).
///
/// ## Rationale
///
/// This controls the length of the linear ramp when transitioning to or
/// from silence. At 48kHz, 48 samples represents 1ms.
///
/// This duration is chosen because:
/// - 1ms is imperceptible as a distinct fade effect
/// - Long enough to eliminate audible clicks/pops
/// - Short enough to preserve speech transients (plosives, etc.)
///
/// ## Audio Engineering Note
///
/// The transition prevents discontinuities in the audio waveform that would
/// create high-frequency artifacts perceived as clicks. The linear ramp
/// spreads the energy change across multiple samples.
pub(crate) const TRANSITION_LENGTH: usize = 48;
