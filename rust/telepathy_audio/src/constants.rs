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

/// The number of samples per VAD frame
pub(crate) const VAD_HOP: usize = 256;
/// TenVAD speech probability threshold.
pub(crate) const VAD_THRESHOLD: f32 = 0.65_f32;
/// Minimum speech duration before opening gate (milliseconds).
pub(crate) const VAD_MIN_SPEECH_MS: usize = 5;
/// Minimum silence duration before closing gate (milliseconds).
pub(crate) const VAD_MIN_SILENCE_MS: usize = 300;
/// Extra padding around speech boundaries (milliseconds).
pub(crate) const VAD_SPEECH_PAD_MS: usize = 10;
/// Maximum continuous speech duration before forced split (seconds).
pub(crate) const VAD_MAX_SPEECH_S: f32 = 30.0_f32;
/// RMS threshold ceiling while VAD gate is closed.
pub(crate) const VAD_SILENCE_CEILING: f32 = 1000_f32;
/// Exponential close rate for smoothing threshold transitions.
pub(crate) const VAD_CLOSE_RATE: f32 = 0.01_f32;
/// Pre-speech probability that triggers early threshold decay.
pub(crate) const VAD_PRE_SPEECH_THRESHOLD: f32 = 0.5_f32;
/// Multiplicative decay rate while pre-speech is detected.
pub(crate) const VAD_OPEN_RATE: f32 = 0.5_f32;
