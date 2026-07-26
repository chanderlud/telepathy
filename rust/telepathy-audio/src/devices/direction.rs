//! Identifies whether a device query was for an input or output.

use std::fmt;

/// Identifies a CPAL device direction for the sake of error reporting.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DeviceDirection {
    /// Input (microphone capture) device.
    Input,
    /// Output (speaker) device.
    Output,
}

impl fmt::Display for DeviceDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceDirection::Input => f.write_str("input"),
            DeviceDirection::Output => f.write_str("output"),
        }
    }
}
