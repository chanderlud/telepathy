//! Error types for audio operations.
//!
//! This module provides a comprehensive error type covering all audio
//! processing operations including device errors, stream errors, and
//! processing errors.
//!
//! ## Error Categories
//!
//! - [`Error::Device`] - Device enumeration and selection failures
//! - [`Error::Stream`] - Audio stream creation and playback failures
//! - [`Error::Processing`] - Resampling, codec, and conversion failures
//! - [`Error::Channel`] - Inter-thread communication failures
//! - [`Error::Config`] - Invalid configuration (e.g., missing callback)
//!
//! ## Error Conversions
//!
//! This module provides `From` implementations for common error types from
//! dependencies (cpal, rubato, sea_codec), allowing seamless error
//! propagation with the `?` operator.

use crate::DeviceError;
use std::fmt;
use tokio::task::JoinError;

/// Comprehensive error type for audio operations.
///
/// This enum covers all error conditions that can occur during audio
/// device management, stream operations, and audio processing.
#[derive(Debug)]
pub enum Error {
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
    JoinError(JoinError),
    /// WASM threading errors (WASM only).
    ///
    /// Occurs when WASM threading operations fail, such as when
    /// `SharedArrayBuffer` is not available (missing COOP/COEP headers)
    /// or when blocking operations are attempted on the browser's main thread.
    #[cfg(target_family = "wasm")]
    WasmThreading(String),
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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Device(msg) => write!(f, "Device error: {}", msg),
            Error::Stream(msg) => write!(f, "Stream error: {}", msg),
            Error::Processing(msg) => write!(f, "Processing error: {}", msg),
            Error::Channel(msg) => write!(f, "Channel error: {}", msg),
            Error::Config(msg) => write!(f, "Configuration error: {}", msg),
            Error::JoinError(msg) => write!(f, "Join error: {}", msg),
            #[cfg(target_family = "wasm")]
            Error::WasmThreading(msg) => write!(f, "WASM threading error: {}", msg),
            Error::InvalidWav => write!(f, "Invalid WAV file"),
            Error::UnknownSampleFormat => write!(f, "Unknown sample format"),
        }
    }
}

impl std::error::Error for Error {}

impl From<DeviceError> for Error {
    fn from(e: DeviceError) -> Self {
        Error::Device(e.to_string())
    }
}

/// Converts cpal device enumeration errors.
///
/// Occurs when [`list_input_devices`](crate::list_input_devices) or
/// [`list_output_devices`](crate::list_output_devices) fails to enumerate devices.
impl From<cpal::DevicesError> for Error {
    fn from(err: cpal::DevicesError) -> Self {
        Error::Device(err.to_string())
    }
}

/// Converts cpal stream configuration errors.
///
/// Occurs when querying device's default stream configuration during
/// [`AudioInputBuilder::build`](crate::AudioInputBuilder::build) or
/// [`AudioOutputBuilder::build`](crate::AudioOutputBuilder::build).
impl From<cpal::DefaultStreamConfigError> for Error {
    fn from(err: cpal::DefaultStreamConfigError) -> Self {
        Error::Stream(err.to_string())
    }
}

/// Converts cpal stream creation errors.
///
/// Occurs when `build_input_stream` or `build_output_stream` fails during
/// builder `build()` methods.
impl From<cpal::BuildStreamError> for Error {
    fn from(err: cpal::BuildStreamError) -> Self {
        Error::Stream(err.to_string())
    }
}

/// Converts cpal stream play errors.
///
/// Occurs when `stream.play()` fails after stream creation.
impl From<cpal::PlayStreamError> for Error {
    fn from(err: cpal::PlayStreamError) -> Self {
        Error::Stream(err.to_string())
    }
}

/// Converts cpal stream pause errors.
///
/// Occurs when attempting to pause a stream (currently unused).
impl From<cpal::PauseStreamError> for Error {
    fn from(err: cpal::PauseStreamError) -> Self {
        Error::Stream(err.to_string())
    }
}

/// Converts rubato resampler construction errors.
///
/// Occurs when [`resampler_factory`](crate::resampler_factory) fails to
/// create a resampler with invalid parameters.
impl From<rubato::ResamplerConstructionError> for Error {
    fn from(err: rubato::ResamplerConstructionError) -> Self {
        Error::Processing(format!("Resampler construction error: {}", err))
    }
}

/// Converts rubato resampling errors.
///
/// Occurs during audio processing when the resampler encounters an error
/// processing audio frames.
impl From<rubato::ResampleError> for Error {
    fn from(err: rubato::ResampleError) -> Self {
        Error::Processing(format!("Resample error: {}", err))
    }
}

impl From<rtrb::chunks::ChunkError> for Error {
    fn from(err: rtrb::chunks::ChunkError) -> Self {
        Error::Channel(format!("Chunk error: {}", err))
    }
}

/// Converts SEA codec errors.
///
/// Occurs during encoding in [`encoder`](crate::encoder) or decoding in
/// [`decoder`](crate::decoder) when the codec encounters invalid data.
impl From<crate::sea::codec::common::SeaError> for Error {
    fn from(err: crate::sea::codec::common::SeaError) -> Self {
        Error::Processing(format!("Codec error: {:?}", err))
    }
}

/// Converts slice conversion errors.
///
/// Occurs when audio frame data cannot be converted to the expected array size.
impl From<std::array::TryFromSliceError> for Error {
    fn from(err: std::array::TryFromSliceError) -> Self {
        Error::Processing(format!("Slice conversion error: {}", err))
    }
}

/// Converts IO errors.
///
/// Occurs during file operations or other IO-related processing.
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Processing(format!("IO error: {}", err))
    }
}

impl From<JoinError> for Error {
    fn from(err: JoinError) -> Self {
        Error::JoinError(err)
    }
}

/// Converts tokio oneshot receive errors (WASM only).
///
/// Occurs when the sender is dropped without sending, e.g. if the spawned
/// thread panics before sending the result.
#[cfg(target_family = "wasm")]
impl From<tokio::sync::oneshot::error::RecvError> for Error {
    fn from(_: tokio::sync::oneshot::error::RecvError) -> Self {
        Error::Processing("blocking task channel closed".to_string())
    }
}

/// Converts JavaScript errors (WASM only).
///
/// Occurs during Web Audio API operations when JavaScript throws an error,
/// such as permission denied for microphone access.
#[cfg(target_family = "wasm")]
impl From<wasm_bindgen::JsValue> for Error {
    fn from(err: wasm_bindgen::JsValue) -> Self {
        Error::Processing(format!("JavaScript error: {:?}", err))
    }
}

impl From<ten_vad_rs::TenVadError> for Error {
    fn from(err: ten_vad_rs::TenVadError) -> Self {
        Error::Processing(format!("ten_vad_rs::TenVad error: {:?}", err))
    }
}
