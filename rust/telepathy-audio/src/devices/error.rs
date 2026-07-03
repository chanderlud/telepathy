use std::fmt;

/// Errors raised by device enumeration, selection, and stream construction.
#[derive(Debug, Clone)]
pub enum DeviceError {
    DeviceNotFound(String),
    NoDefaultDevice,
    EnumerationFailed(String),
    InvalidDeviceId(String),
    UnsupportedConfig(String),
    Cpal(cpal::Error),
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
            DeviceError::UnsupportedConfig(error) => write!(f, "Unsupported config: {}", error),
            DeviceError::Cpal(error) => write!(f, "Cpal error: {}", error),
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<cpal::Error> for DeviceError {
    fn from(error: cpal::Error) -> Self {
        DeviceError::Cpal(error)
    }
}