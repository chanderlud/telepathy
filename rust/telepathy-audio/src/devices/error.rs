//! Device-related errors.
//!
//! Errors raised by device enumeration, selection, and stream construction.
//! Variants preserve the underlying source error (cpal or others) so callers
//! can inspect or unwrap the original cause without re-parsing display text.

use crate::devices::DeviceDirection;
use cpal::{SampleFormat, StreamConfig};
use std::fmt;

/// Errors raised by device enumeration, selection, and stream construction.
#[derive(Debug)]
pub enum DeviceError {
    /// The requested device ID could not be found.
    DeviceNotFound {
        /// Whether this lookup was for an input or output device.
        direction: DeviceDirection,
        /// The device identifier that was requested.
        id: String,
    },
    /// No default device is available for the requested direction.
    NoDefaultDevice {
        /// Whether the lookup was for an input or output device.
        direction: DeviceDirection,
    },
    /// CPAL enumeration failed for an input device query.
    EnumerateInput(cpal::Error),
    /// CPAL enumeration failed for an output device query.
    EnumerateOutput(cpal::Error),
    /// The provided device ID could not be parsed by CPAL.
    InvalidDeviceId {
        /// Whether this lookup was for an input or output device.
        direction: DeviceDirection,
        /// The device identifier that failed to parse.
        id: String,
        /// The CPAL parse error returned by `DeviceId::from_str`.
        parse: cpal::Error,
    },
    /// The device's default stream config uses a sample format this crate cannot handle.
    UnsupportedSampleFormat {
        /// Whether this lookup was for an input or output device.
        direction: DeviceDirection,
        /// The actual sample format exposed by the device.
        sample_format: SampleFormat,
    },
    /// The device's default stream config could not be obtained.
    DefaultConfig {
        /// Whether this lookup was for an input or output device.
        direction: DeviceDirection,
        /// The CPAL error returned by `default_*_config`.
        source: cpal::Error,
    },
    /// CPAL could not build the requested input stream.
    BuildInputStream {
        /// The actual config query that failed, when available.
        config: Option<StreamConfig>,
        /// The CPAL error returned by `build_input_stream`.
        source: cpal::Error,
    },
    /// CPAL could not build the requested output stream.
    BuildOutputStream {
        /// The actual config query that failed, when available.
        config: Option<StreamConfig>,
        /// The CPAL error returned by `build_output_stream`.
        source: cpal::Error,
    },
    /// CPAL could not start an input or output stream playback.
    StreamPlay {
        /// Whether this lookup was for an input or output device.
        direction: DeviceDirection,
        /// The CPAL error returned by `Stream::play`.
        source: cpal::Error,
    },
    /// Catch-all for CPAL failures that don't fit the contextual variants above.
    Other {
        /// A short, human-readable operation name for log/diagnostic use.
        operation: &'static str,
        /// The CPAL error that was raised.
        source: cpal::Error,
    },
    /// No output device was available for an audio playback operation.
    NoOutputDevice,
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::DeviceNotFound { direction, id } => {
                write!(f, "{} device not found: {}", direction, id)
            }
            DeviceError::NoDefaultDevice { direction } => {
                write!(f, "no default {} device available", direction)
            }
            DeviceError::EnumerateInput(source) => {
                write!(f, "failed to enumerate input devices: {}", source)
            }
            DeviceError::EnumerateOutput(source) => {
                write!(f, "failed to enumerate output devices: {}", source)
            }
            DeviceError::InvalidDeviceId { direction, id, .. } => {
                write!(
                    f,
                    "invalid {} device ID {:?} (could not be parsed by CPAL)",
                    direction, id
                )
            }
            DeviceError::UnsupportedSampleFormat {
                direction,
                sample_format,
            } => {
                write!(
                    f,
                    "unsupported sample format {:?} for {} device",
                    sample_format, direction
                )
            }
            DeviceError::DefaultConfig {
                direction, source, ..
            } => {
                write!(
                    f,
                    "failed to obtain default {} stream config: {}",
                    direction, source
                )
            }
            DeviceError::BuildInputStream { source, .. } => {
                write!(f, "failed to build input stream: {}", source)
            }
            DeviceError::BuildOutputStream { source, .. } => {
                write!(f, "failed to build output stream: {}", source)
            }
            DeviceError::StreamPlay { direction, source } => {
                write!(
                    f,
                    "failed to start {} stream playback: {}",
                    direction, source
                )
            }
            DeviceError::Other { operation, source } => {
                write!(f, "cpal error during {}: {}", operation, source)
            }
            DeviceError::NoOutputDevice => {
                write!(f, "no output device available")
            }
        }
    }
}

impl std::error::Error for DeviceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeviceError::EnumerateInput(source)
            | DeviceError::EnumerateOutput(source)
            | DeviceError::DefaultConfig { source, .. }
            | DeviceError::BuildInputStream { source, .. }
            | DeviceError::BuildOutputStream { source, .. }
            | DeviceError::StreamPlay { source, .. }
            | DeviceError::Other { source, .. } => Some(source),
            DeviceError::InvalidDeviceId { parse, .. } => Some(parse),
            DeviceError::DeviceNotFound { .. }
            | DeviceError::NoDefaultDevice { .. }
            | DeviceError::UnsupportedSampleFormat { .. }
            | DeviceError::NoOutputDevice => None,
        }
    }
}
