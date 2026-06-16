use cpal::{BuildStreamError, DefaultStreamConfigError, PlayStreamError};
use std::fmt;

/// Errors raised by device enumeration, selection, and stream construction.
#[derive(Debug, Clone)]
pub enum DeviceError {
    DeviceNotFound(String),
    NoDefaultDevice,
    EnumerationFailed(String),
    InvalidDeviceId(String),
    DefaultConfigMissing(String),
    BuildStream(String),
    UnsupportedConfig(String),
    PlayStream(String),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::DeviceNotFound(id) => write!(f, "Device not found: {}", id),
            DeviceError::NoDefaultDevice => write!(f, "No default device available"),
            DeviceError::EnumerationFailed(msg) => {
                write!(f, "Failed to enumerate devices: {}", msg)
            }
            DeviceError::InvalidDeviceId(id) => write!(f, "Invalid device ID: {}", id),
            DeviceError::DefaultConfigMissing(error) => {
                write!(f, "Default config missing: {}", error)
            }
            DeviceError::BuildStream(error) => write!(f, "Failed to build stream: {}", error),
            DeviceError::UnsupportedConfig(error) => write!(f, "Unsupported config: {}", error),
            DeviceError::PlayStream(error) => write!(f, "Failed to play stream: {}", error),
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<DefaultStreamConfigError> for DeviceError {
    fn from(err: DefaultStreamConfigError) -> Self {
        Self::DefaultConfigMissing(err.to_string())
    }
}

impl From<BuildStreamError> for DeviceError {
    fn from(err: BuildStreamError) -> Self {
        Self::BuildStream(err.to_string())
    }
}

impl From<PlayStreamError> for DeviceError {
    fn from(err: PlayStreamError) -> Self {
        Self::PlayStream(err.to_string())
    }
}
