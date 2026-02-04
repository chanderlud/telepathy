//! Error types for audio operations.
//!
//! This module provides a comprehensive error type covering all audio
//! processing operations including device errors, stream errors, and
//! processing errors.
//!
//! ## Error Categories
//!
//! - [`AudioError::Device`] - Device enumeration and selection failures
//! - [`AudioError::Stream`] - Audio stream creation and playback failures
//! - [`AudioError::Processing`] - Resampling, codec, and conversion failures
//! - [`AudioError::Channel`] - Inter-thread communication failures
//! - [`AudioError::Config`] - Invalid configuration (e.g., missing callback)
//! - [`AudioError::Platform`] - Platform-specific issues (e.g., WASM restrictions)
//!
//! ## Error Conversions
//!
//! This module provides `From` implementations for common error types from
//! dependencies (cpal, rubato, kanal, sea_codec), allowing seamless error
//! propagation with the `?` operator.

use std::fmt;

/// Comprehensive error type for audio operations.
///
/// This enum covers all error conditions that can occur during audio
/// device management, stream operations, and audio processing.
#[derive(Debug)]
pub enum AudioError {
    /// Device-related errors (e.g., device not found, enumeration failed).
    ///
    /// Occurs during device enumeration via [`list_input_devices`](crate::list_input_devices),
    /// [`list_output_devices`](crate::list_output_devices), or device selection.
    Device(String),
    /// Audio stream errors (e.g., stream creation failed, stream error during playback).
    ///
    /// Occurs during [`AudioInputBuilder::build`](crate::AudioInputBuilder::build) or
    /// [`AudioOutputBuilder::build`](crate::AudioOutputBuilder::build), or when
    /// the underlying audio stream encounters an error.
    Stream(String),
    /// Audio processing errors (e.g., resampling failed, codec error).
    ///
    /// Occurs during audio processing, including resampler construction,
    /// sample rate conversion, codec encoding/decoding, or data conversion.
    Processing(String),
    /// Channel communication errors.
    ///
    /// Occurs when inter-thread communication fails, typically when a
    /// channel is closed unexpectedly.
    Channel(String),
    /// Configuration errors.
    ///
    /// Occurs when builder configuration is invalid, such as missing
    /// required callback in [`AudioInputBuilder`](crate::AudioInputBuilder).
    Config(String),
    /// Platform-specific errors.
    ///
    /// Occurs for platform restrictions, such as calling synchronous `build`
    /// on WASM where async initialization is required.
    Platform(String),
    /// Invalid WAV file error.
    ///
    /// Occurs when a WAV file has an invalid or corrupted header,
    /// or when the file is too short to contain a valid header.
    InvalidWav,
    /// Unknown sample format error.
    ///
    /// Occurs when the audio file uses a sample format that is not
    /// supported (e.g., unsupported bit depth or encoding).
    UnknownSampleFormat,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Device(msg) => write!(f, "Device error: {}", msg),
            AudioError::Stream(msg) => write!(f, "Stream error: {}", msg),
            AudioError::Processing(msg) => write!(f, "Processing error: {}", msg),
            AudioError::Channel(msg) => write!(f, "Channel error: {}", msg),
            AudioError::Config(msg) => write!(f, "Configuration error: {}", msg),
            AudioError::Platform(msg) => write!(f, "Platform error: {}", msg),
            AudioError::InvalidWav => write!(f, "Invalid WAV file"),
            AudioError::UnknownSampleFormat => write!(f, "Unknown sample format"),
        }
    }
}

impl std::error::Error for AudioError {}

// ============================================================================
// Conversions from cpal errors
// ============================================================================

/// Converts cpal device enumeration errors.
///
/// Occurs when [`list_input_devices`](crate::list_input_devices) or
/// [`list_output_devices`](crate::list_output_devices) fails to enumerate devices.
impl From<cpal::DevicesError> for AudioError {
    fn from(err: cpal::DevicesError) -> Self {
        AudioError::Device(err.to_string())
    }
}

/// Converts cpal stream configuration errors.
///
/// Occurs when querying device's default stream configuration during
/// [`AudioInputBuilder::build`](crate::AudioInputBuilder::build) or
/// [`AudioOutputBuilder::build`](crate::AudioOutputBuilder::build).
impl From<cpal::DefaultStreamConfigError> for AudioError {
    fn from(err: cpal::DefaultStreamConfigError) -> Self {
        AudioError::Stream(err.to_string())
    }
}

/// Converts cpal stream creation errors.
///
/// Occurs when `build_input_stream` or `build_output_stream` fails during
/// builder `build()` methods.
impl From<cpal::BuildStreamError> for AudioError {
    fn from(err: cpal::BuildStreamError) -> Self {
        AudioError::Stream(err.to_string())
    }
}

/// Converts cpal stream play errors.
///
/// Occurs when `stream.play()` fails after stream creation.
impl From<cpal::PlayStreamError> for AudioError {
    fn from(err: cpal::PlayStreamError) -> Self {
        AudioError::Stream(err.to_string())
    }
}

/// Converts cpal stream pause errors.
///
/// Occurs when attempting to pause a stream (currently unused).
impl From<cpal::PauseStreamError> for AudioError {
    fn from(err: cpal::PauseStreamError) -> Self {
        AudioError::Stream(err.to_string())
    }
}

// ============================================================================
// Conversions from rubato errors
// ============================================================================

/// Converts rubato resampler construction errors.
///
/// Occurs when [`resampler_factory`](crate::resampler_factory) fails to
/// create a resampler with invalid parameters.
impl From<rubato::ResamplerConstructionError> for AudioError {
    fn from(err: rubato::ResamplerConstructionError) -> Self {
        AudioError::Processing(format!("Resampler construction error: {}", err))
    }
}

/// Converts rubato resampling errors.
///
/// Occurs during audio processing when the resampler encounters an error
/// processing audio frames.
impl From<rubato::ResampleError> for AudioError {
    fn from(err: rubato::ResampleError) -> Self {
        AudioError::Processing(format!("Resample error: {}", err))
    }
}

// ============================================================================
// Conversions from kanal errors
// ============================================================================

/// Converts kanal send errors.
///
/// Occurs when sending audio data between threads fails, typically because
/// the receiving end was dropped.
impl From<kanal::SendError> for AudioError {
    fn from(err: kanal::SendError) -> Self {
        AudioError::Channel(format!("Send error: {}", err))
    }
}

/// Converts kanal receive errors.
///
/// Occurs when receiving audio data between threads fails, typically because
/// the sending end was dropped.
impl From<kanal::ReceiveError> for AudioError {
    fn from(err: kanal::ReceiveError) -> Self {
        AudioError::Channel(format!("Receive error: {}", err))
    }
}

/// Converts kanal close errors.
///
/// Occurs when closing a channel fails, typically when attempting to
/// close an already-closed channel.
impl From<kanal::CloseError> for AudioError {
    fn from(err: kanal::CloseError) -> Self {
        AudioError::Channel(format!("Close error: {}", err))
    }
}

// ============================================================================
// Conversions from sea_codec errors
// ============================================================================

/// Converts SEA codec errors.
///
/// Occurs during encoding in [`encoder`](crate::encoder) or decoding in
/// [`decoder`](crate::decoder) when the codec encounters invalid data.
impl From<sea_codec::codec::common::SeaError> for AudioError {
    fn from(err: sea_codec::codec::common::SeaError) -> Self {
        AudioError::Processing(format!("Codec error: {:?}", err))
    }
}

// ============================================================================
// Conversions from std errors
// ============================================================================

/// Converts slice conversion errors.
///
/// Occurs when audio frame data cannot be converted to the expected array size.
impl From<std::array::TryFromSliceError> for AudioError {
    fn from(err: std::array::TryFromSliceError) -> Self {
        AudioError::Processing(format!("Slice conversion error: {}", err))
    }
}

/// Converts IO errors.
///
/// Occurs during file operations or other IO-related processing.
impl From<std::io::Error> for AudioError {
    fn from(err: std::io::Error) -> Self {
        AudioError::Processing(format!("IO error: {}", err))
    }
}

// ============================================================================
// WASM-specific conversions
// ============================================================================

/// Converts JavaScript errors (WASM only).
///
/// Occurs during Web Audio API operations when JavaScript throws an error,
/// such as permission denied for microphone access.
#[cfg(target_family = "wasm")]
impl From<wasm_bindgen::JsValue> for AudioError {
    fn from(err: wasm_bindgen::JsValue) -> Self {
        AudioError::Platform(format!("JavaScript error: {:?}", err))
    }
}
