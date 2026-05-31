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

/// Host abstraction for device enumeration and selection.
///
/// This trait defines the public audio host operations used by the crate:
/// listing devices and resolving input/output handles from optional IDs.
/// Implementations may wrap platform APIs directly or provide test doubles.
pub trait AudioHost {
    type InputStream: Send + Sync + 'static;
    type OutputStream: Send + Sync + 'static;

    /// Lists all available input (recording) devices.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::EnumerationFailed` when the backend fails to enumerate input devices
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let devices = host.list_input_devices().unwrap();
    /// for device in devices {
    ///     println!("Input: {} (ID: {})", device.name, device.id);
    /// }
    /// ```
    fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError>;

    /// Lists all available output (playback) devices.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::EnumerationFailed` when the backend fails to enumerate output devices
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let devices = host.list_output_devices().unwrap();
    /// for device in devices {
    ///     println!("Output: {} (ID: {})", device.name, device.id);
    /// }
    /// ```
    fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError>;

    /// Lists both input and output devices in one call.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::EnumerationFailed` when input or output enumeration fails
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let devices = host.list_all_devices().unwrap();
    /// println!("Found {} input devices", devices.input_devices.len());
    /// println!("Found {} output devices", devices.output_devices.len());
    /// ```
    fn list_all_devices(&self) -> Result<AudioDeviceList, DeviceError>;

    /// Returns the default sample rate for the given input device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - Optional device ID string. When `None`, queries the system default input device.
    ///
    /// # Errors
    ///
    /// Returns `DeviceError::NoDefaultDevice` if no default device is available,
    /// or `DeviceError::DefaultConfigMissing` if the device config cannot be read.
    fn input_sample_rate(&self, device_id: Option<&str>) -> Result<u32, DeviceError>;

    /// Returns the default sample rate for the given output device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - Optional device ID string. When `None`, queries the system default output device.
    ///
    /// # Errors
    ///
    /// Returns `DeviceError::NoDefaultDevice` if no default device is available,
    /// or `DeviceError::DefaultConfigMissing` if the device config cannot be read.
    fn output_sample_rate(&self, device_id: Option<&str>) -> Result<u32, DeviceError>;

    /// Opens an input stream on the specified device and returns the processor input,
    /// the device's native sample rate, and the platform stream handle.
    ///
    /// The returned `InputStream` must be kept alive for the duration of recording;
    /// dropping it stops the stream.
    ///
    /// # Arguments
    ///
    /// * `device_id` - Optional device ID string. When `None`, uses the system default input device.
    ///   Falls back to the default device if the specified ID is not found.
    /// * `error_callback` - Optional callback invoked on stream errors. When `None`, errors are logged.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::NoDefaultDevice` if no default device is available
    /// - `DeviceError::InvalidDeviceId` if the device ID string cannot be parsed
    /// - `DeviceError::DefaultConfigMissing` if the device's default config cannot be read
    /// - `DeviceError::UnsupportedConfig` if the device uses an unsupported sample format
    /// - `DeviceError::BuildStream` if the underlying stream cannot be created
    /// - `DeviceError::PlayStream` if the stream cannot be started
    #[cfg(not(target_family = "wasm"))]
    fn open_input(
        &self,
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioInput + Send + 'static, u32, Self::InputStream), DeviceError>;

    /// Opens an output stream on the specified device and returns the processor output,
    /// the device's native sample rate, and the platform stream handle.
    ///
    /// The returned `OutputStream` must be kept alive for the duration of playback;
    /// dropping it stops the stream.
    ///
    /// # Arguments
    ///
    /// * `device_id` - Optional device ID string. When `None`, uses the system default output device.
    ///   Falls back to the default device if the specified ID is not found.
    /// * `error_callback` - Optional callback invoked on stream errors. When `None`, errors are logged.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::NoDefaultDevice` if no default device is available
    /// - `DeviceError::InvalidDeviceId` if the device ID string cannot be parsed
    /// - `DeviceError::DefaultConfigMissing` if the device's default config cannot be read
    /// - `DeviceError::UnsupportedConfig` if the device uses an unsupported sample format
    /// - `DeviceError::BuildStream` if the underlying stream cannot be created
    /// - `DeviceError::PlayStream` if the stream cannot be started
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
