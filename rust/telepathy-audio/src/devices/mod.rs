//! Audio device enumeration and selection module.
//!
//! This module provides types and functions for enumerating and selecting
//! audio input/output devices across platforms.

mod cpal_host;
mod error;
mod mock_host;

#[cfg(not(target_family = "wasm"))]
use crate::internal::traits::AudioInput;
use crate::internal::traits::AudioOutput;
use crate::io::StreamErrorCallback;
use cpal::Device;
use cpal::traits::DeviceTrait;
pub use cpal_host::CpalAudioHost;
pub use error::DeviceError;
pub use mock_host::{MockAudioHost, MockAudioInput, MockAudioOutput};

/// Host abstraction for device enumeration, selection, and stream lifecycle.
///
/// Implementations wrap a platform audio backend (or a test double) and own the
/// platform stream handle. The returned `*Stream` associated types must be kept
/// alive for as long as the caller wants the stream to keep producing or
/// consuming audio: dropping them stops the underlying stream.
///
/// Implementations must be `Send + Sync`; the input and output processor halves
/// are typically shipped to independent tasks. Error semantics are documented
/// per-method and surface exclusively through `DeviceError`.
pub trait AudioHost {
    type InputStream: Send + Sync + 'static;
    type OutputStream: Send + Sync + 'static;

    /// Lists available input (recording) devices.
    fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError>;

    /// Lists available output (playback) devices.
    fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError>;

    /// Lists both input and output devices in one call.
    fn list_all_devices(&self) -> Result<AudioDeviceList, DeviceError>;

    /// Returns the default sample rate for the given input device, or the system
    /// default when `device_id` is `None`.
    fn input_sample_rate(&self, device_id: Option<&str>) -> Result<u32, DeviceError>;

    /// Returns the default sample rate for the given output device, or the system
    /// default when `device_id` is `None`.
    fn output_sample_rate(&self, device_id: Option<&str>) -> Result<u32, DeviceError>;

    /// Opens an input stream and returns the processor input, the device's
    /// native sample rate, and the platform stream handle. The platform handle
    /// must outlive all use of the processor input.
    ///
    /// `error_callback`, when `Some`, replaces the default error log path.
    #[cfg(not(target_family = "wasm"))]
    fn open_input(
        &self,
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioInput + Send + 'static, u32, Self::InputStream), DeviceError>;

    /// Opens an output stream and returns the processor output, the device's
    /// native sample rate, and the platform stream handle. The platform handle
    /// must outlive all use of the processor output.
    ///
    /// `error_callback`, when `Some`, replaces the default error log path.
    fn open_output(
        &self,
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioOutput + Send + 'static, u32, Self::OutputStream), DeviceError>;
}

/// Information about an audio device.
///
/// This struct contains the human-readable name and unique identifier for
/// an audio device. The ID can be used to select the device later.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioDeviceInfo {
    /// Human-readable device name
    pub name: String,
    /// Unique device identifier (can be used for device selection)
    pub id: String,
}

/// Collection of available input and output audio devices.
#[derive(Debug, Clone)]
pub struct AudioDeviceList {
    /// List of available input (recording) devices
    pub input_devices: Vec<AudioDeviceInfo>,
    /// List of available output (playback) devices
    pub output_devices: Vec<AudioDeviceInfo>,
}

/// Converts a cpal device to AudioDeviceInfo.
///
/// Returns `None` if the device name or ID cannot be extracted.
fn device_to_info(device: &Device) -> Option<AudioDeviceInfo> {
    let description = device.description().ok()?;
    let name = description.name().to_string();
    let id = device.id().ok()?.to_string();
    Some(AudioDeviceInfo { name, id })
}
