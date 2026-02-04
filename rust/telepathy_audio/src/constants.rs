//! Audio processing constants and configuration parameters.
//!
//! This module contains constants used throughout the audio processing pipeline.
//! These values have been chosen to balance audio quality with real-time performance
//! requirements.

pub use nnnoiseless::FRAME_SIZE;
use rubato::{SincInterpolationParameters, SincInterpolationType, WindowFunction};

/// Parameters used for resampling throughout the application.
///
/// These parameters provide high-quality resampling suitable for real-time
/// audio processing. See [rubato documentation](https://docs.rs/rubato) for
/// detailed parameter descriptions.
///
/// ## Parameter Rationale
///
/// - **`sinc_len: 256`** - Sinc filter length. Higher values provide better
///   frequency response but increase latency and CPU usage. 256 offers an
///   excellent quality/performance balance for voice audio.
///
/// - **`f_cutoff: 0.95`** - Anti-aliasing filter cutoff as fraction of Nyquist.
///   0.95 preserves frequencies up to 95% of Nyquist while attenuating aliases.
///   Suitable for voice which doesn't require the full spectrum.
///
/// - **`interpolation: Linear`** - Interpolation type between sinc table entries.
///   Linear interpolation is fast and sufficient for the oversampling factor used.
///
/// - **`oversampling_factor: 256`** - Resolution of the sinc table. Higher values
///   improve accuracy at the cost of memory. 256 provides excellent precision.
///
/// - **`window: BlackmanHarris2`** - Window function applied to sinc filter.
///   Blackman-Harris provides excellent stopband attenuation (~92dB) which
///   effectively eliminates aliasing artifacts.
pub(crate) const RESAMPLER_PARAMETERS: SincInterpolationParameters = SincInterpolationParameters {
    sinc_len: 256,
    f_cutoff: 0.95,
    interpolation: SincInterpolationType::Linear,
    oversampling_factor: 256,
    window: WindowFunction::BlackmanHarris2,
};

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
pub(crate) const MINIMUM_SILENCE_LENGTH: u8 = 40;

/// Number of samples used for silence transitions (fade in/out).
///
/// ## Rationale
///
/// This controls the length of the linear ramp when transitioning to or
/// from silence. At 48kHz, 96 samples represents 2ms.
///
/// This duration is chosen because:
/// - 2ms is imperceptible as a distinct fade effect
/// - Long enough to eliminate audible clicks/pops
/// - Short enough to preserve speech transients (plosives, etc.)
///
/// ## Audio Engineering Note
///
/// The transition prevents discontinuities in the audio waveform that would
/// create high-frequency artifacts perceived as clicks. The linear ramp
/// spreads the energy change across multiple samples.
pub(crate) const TRANSITION_LENGTH: usize = 96;
