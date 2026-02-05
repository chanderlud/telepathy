//! Audio device enumeration and selection module.
//!
//! This module provides types and functions for enumerating and selecting
//! audio input/output devices across platforms.

use cpal::traits::{DeviceTrait, HostTrait};
use std::fmt;
use std::sync::Arc;

/// Error type for device operations.
#[derive(Debug, Clone)]
pub enum DeviceError {
    /// Device with the specified ID was not found
    DeviceNotFound(String),
    /// No default device is available
    NoDefaultDevice,
    /// Failed to enumerate devices
    EnumerationFailed(String),
    /// Device ID parsing failed
    InvalidDeviceId(String),
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
        }
    }
}

impl std::error::Error for DeviceError {}

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

/// Handle to an audio device for use by audio processing code.
///
/// This struct wraps a `cpal::Device` and provides access to the underlying
/// device while storing the device ID for comparison purposes.
pub struct DeviceHandle {
    device: cpal::Device,
    device_id: String,
}

impl DeviceHandle {
    /// Creates a new device handle from a cpal device.
    fn new(device: cpal::Device, device_id: String) -> Self {
        Self { device, device_id }
    }

    /// Returns a reference to the underlying cpal device.
    ///
    /// This method provides access to the underlying cpal device for
    /// consumers that need to query device configuration.
    pub fn device(&self) -> &cpal::Device {
        &self.device
    }

    /// Returns the device ID string.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Returns the device name.
    pub fn name(&self) -> Result<String, DeviceError> {
        self.device
            .description()
            .map(|desc| desc.name().to_string())
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))
    }
}

impl fmt::Debug for DeviceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceHandle")
            .field("device_id", &self.device_id)
            .finish()
    }
}

/// Central audio host for device management.
///
/// The `AudioHost` wraps the platform-specific audio backend and provides
/// thread-safe access to device enumeration and selection functionality.
///
/// A single `AudioHost` instance should be shared across all components
/// that need audio device access.
///
/// ## Platform Behavior
///
/// - On **WASM**: Attempts to use `cpal::HostId::AudioWorklet` for better performance,
///   falls back to cpal's default host if AudioWorklet is unavailable. The fallback
///   is determined by cpal and may vary by browser.
/// - On **Windows**: Uses WASAPI (Windows Audio Session API)
/// - On **macOS**: Uses CoreAudio
/// - On **Linux**: Uses ALSA
///
/// ## Thread Safety
///
/// `AudioHost` uses `Arc` internally and is safe to clone and share across threads.
/// The underlying cpal host is wrapped in `Arc<cpal::Host>` for efficient sharing.
#[derive(Clone)]
pub struct AudioHost {
    host: Arc<cpal::Host>,
}

impl AudioHost {
    /// Creates a new audio host with platform-appropriate initialization.
    ///
    /// On wasm, this attempts to use the AudioWorklet host for better
    /// performance, falling back to the default WebAudio host if unavailable.
    ///
    /// On native platforms, this uses the platform's default audio host.
    #[cfg(target_family = "wasm")]
    pub fn new() -> Self {
        let host = cpal::host_from_id(cpal::HostId::AudioWorklet).unwrap_or_else(|_| {
            log::warn!("AudioWorklet host unavailable, falling back to default host");
            cpal::default_host()
        });
        Self {
            host: Arc::new(host),
        }
    }

    /// Creates a new audio host with platform-appropriate initialization.
    ///
    /// On native platforms, this uses the platform's default audio host
    /// (WASAPI on Windows, ALSA on Linux, CoreAudio on macOS).
    #[cfg(not(target_family = "wasm"))]
    pub fn new() -> Self {
        let host = cpal::default_host();
        Self {
            host: Arc::new(host),
        }
    }

    /// Returns a reference to the underlying cpal host.
    ///
    /// This method is intended for internal use.
    pub(crate) fn inner(&self) -> &cpal::Host {
        &self.host
    }
}

impl Default for AudioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Arc<cpal::Host>> for AudioHost {
    fn from(host: Arc<cpal::Host>) -> Self {
        Self { host }
    }
}

impl fmt::Debug for AudioHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioHost").finish()
    }
}

/// Lists all available input (recording) devices.
///
/// Returns a vector of `AudioDeviceInfo` for each available input device.
/// Devices that fail to provide name or ID information are filtered out.
///
/// # Errors
///
/// Returns `DeviceError::EnumerationFailed` if the device list cannot be retrieved.
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, list_input_devices};
///
/// let host = AudioHost::new();
/// let devices = list_input_devices(&host).unwrap();
/// for device in devices {
///     println!("Input: {} (ID: {})", device.name, device.id);
/// }
/// ```
pub fn list_input_devices(host: &AudioHost) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
    let devices = host
        .inner()
        .input_devices()
        .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

    Ok(devices.filter_map(|d| device_to_info(&d)).collect())
}

/// Lists all available output (playback) devices.
///
/// Returns a vector of `AudioDeviceInfo` for each available output device.
/// Devices that fail to provide name or ID information are filtered out.
///
/// # Errors
///
/// Returns `DeviceError::EnumerationFailed` if the device list cannot be retrieved.
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, list_output_devices};
///
/// let host = AudioHost::new();
/// let devices = list_output_devices(&host).unwrap();
/// for device in devices {
///     println!("Output: {} (ID: {})", device.name, device.id);
/// }
/// ```
pub fn list_output_devices(host: &AudioHost) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
    let devices = host
        .inner()
        .output_devices()
        .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

    Ok(devices.filter_map(|d| device_to_info(&d)).collect())
}

/// Lists all available input and output devices.
///
/// This is a convenience function that combines `list_input_devices` and
/// `list_output_devices` into a single call.
///
/// # Errors
///
/// Returns `DeviceError::EnumerationFailed` if either device list cannot be retrieved.
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, list_all_devices};
///
/// let host = AudioHost::new();
/// let devices = list_all_devices(&host).unwrap();
/// println!("Found {} input devices", devices.input_devices.len());
/// println!("Found {} output devices", devices.output_devices.len());
/// ```
pub fn list_all_devices(host: &AudioHost) -> Result<AudioDeviceList, DeviceError> {
    let input_devices = list_input_devices(host)?;
    let output_devices = list_output_devices(host)?;

    Ok(AudioDeviceList {
        input_devices,
        output_devices,
    })
}

/// Gets an input device by ID, falling back to the default if not found.
///
/// If `device_id` is `Some`, attempts to find the device with that ID.
/// If the device is not found or `device_id` is `None`, returns the default
/// input device.
///
/// # Errors
///
/// - `DeviceError::NoDefaultDevice` if no input device is available
/// - `DeviceError::EnumerationFailed` if devices cannot be enumerated
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, get_input_device};
///
/// let host = AudioHost::new();
///
/// // Get specific device
/// let device = get_input_device(&host, Some("My Microphone"));
///
/// // Get default device
/// let default = get_input_device(&host, None);
/// ```
pub fn get_input_device(
    host: &AudioHost,
    device_id: Option<&str>,
) -> Result<DeviceHandle, DeviceError> {
    if let Some(id) = device_id {
        // Try to find the device by ID
        let devices = host
            .inner()
            .input_devices()
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

        if let Some(device) = find_device_by_id(devices, id) {
            return Ok(DeviceHandle::new(device, id.to_string()));
        }

        // Fall back to default device
        log::warn!("Input device '{}' not found, falling back to default", id);
    }

    // Get default device
    let device = host
        .inner()
        .default_input_device()
        .ok_or(DeviceError::NoDefaultDevice)?;

    let device_id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(DeviceHandle::new(device, device_id))
}

/// Gets an output device by ID, falling back to the default if not found.
///
/// If `device_id` is `Some`, attempts to find the device with that ID.
/// If the device is not found or `device_id` is `None`, returns the default
/// output device.
///
/// # Errors
///
/// - `DeviceError::NoDefaultDevice` if no output device is available
/// - `DeviceError::EnumerationFailed` if devices cannot be enumerated
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, get_output_device};
///
/// let host = AudioHost::new();
///
/// // Get specific device
/// let device = get_output_device(&host, Some("My Speakers"));
///
/// // Get default device
/// let default = get_output_device(&host, None);
/// ```
pub fn get_output_device(
    host: &AudioHost,
    device_id: Option<&str>,
) -> Result<DeviceHandle, DeviceError> {
    if let Some(id) = device_id {
        // Try to find the device by ID
        let devices = host
            .inner()
            .output_devices()
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

        if let Some(device) = find_device_by_id(devices, id) {
            return Ok(DeviceHandle::new(device, id.to_string()));
        }

        // Fall back to default device
        log::warn!("Output device '{}' not found, falling back to default", id);
    }

    // Get default device
    let device = host
        .inner()
        .default_output_device()
        .ok_or(DeviceError::NoDefaultDevice)?;

    let device_id = device
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(DeviceHandle::new(device, device_id))
}

/// Gets the default input (recording) device.
///
/// This is a convenience wrapper around `get_input_device(host, None)`.
///
/// # Errors
///
/// Returns `DeviceError::NoDefaultDevice` if no input device is available.
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, get_default_input_device};
///
/// let host = AudioHost::new();
/// let device = get_default_input_device(&host).unwrap();
/// println!("Default input: {}", device.name().unwrap());
/// ```
pub fn get_default_input_device(host: &AudioHost) -> Result<DeviceHandle, DeviceError> {
    get_input_device(host, None)
}

/// Gets the default output (playback) device.
///
/// This is a convenience wrapper around `get_output_device(host, None)`.
///
/// # Errors
///
/// Returns `DeviceError::NoDefaultDevice` if no output device is available.
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::{AudioHost, get_default_output_device};
///
/// let host = AudioHost::new();
/// let device = get_default_output_device(&host).unwrap();
/// println!("Default output: {}", device.name().unwrap());
/// ```
pub fn get_default_output_device(host: &AudioHost) -> Result<DeviceHandle, DeviceError> {
    get_output_device(host, None)
}

/// Converts a cpal device to AudioDeviceInfo.
///
/// Returns `None` if the device name or ID cannot be extracted.
fn device_to_info(device: &cpal::Device) -> Option<AudioDeviceInfo> {
    let description = device.description().ok()?;
    let name = description.name().to_string();
    let id = device.id().ok()?.to_string();
    Some(AudioDeviceInfo { name, id })
}

/// Finds a device by its ID string from an iterator of devices.
fn find_device_by_id<I>(devices: I, device_id: &str) -> Option<cpal::Device>
where
    I: Iterator<Item = cpal::Device>,
{
    for device in devices {
        if let Ok(id) = device.id()
            && id.to_string() == device_id
        {
            return Some(device);
        }
    }
    None
}

/// Unit tests for device enumeration.
///
/// Note: Many of these tests depend on the system's audio configuration.
/// Tests that interact with real hardware may fail on headless CI systems
/// or systems without audio devices. They are designed to not panic even
/// when no devices are available.
#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
mod tests {
    use super::*;

    /// Verifies AudioHost can be created without panicking.
    #[test]
    fn test_host_creation() {
        let host = AudioHost::new();
        // Just verify it doesn't panic
        let _ = format!("{:?}", host);
    }

    /// Verifies AudioHost::default() works correctly.
    #[test]
    fn test_host_default() {
        let host = AudioHost::default();
        let _ = format!("{:?}", host);
    }

    /// Verifies AudioHost cloning works and both instances are independent.
    #[test]
    fn test_host_clone() {
        let host1 = AudioHost::new();
        let host2 = host1.clone();
        // Both should work independently
        let _ = format!("{:?}", host1);
        let _ = format!("{:?}", host2);
    }

    /// Tests device enumeration functions.
    /// May return empty lists on systems without audio hardware.
    #[test]
    fn test_device_enumeration() {
        let host = AudioHost::new();

        // These may succeed or fail depending on the system's audio configuration
        // We just verify they don't panic
        let _ = list_input_devices(&host);
        let _ = list_output_devices(&host);
        let _ = list_all_devices(&host);
    }

    /// Tests default device selection.
    /// May return NoDefaultDevice on systems without audio hardware.
    #[test]
    fn test_default_device_selection() {
        let host = AudioHost::new();

        // These may succeed or fail depending on the system's audio configuration
        // We just verify they don't panic
        let _ = get_default_input_device(&host);
        let _ = get_default_output_device(&host);
    }

    /// Verifies fallback behavior when requesting a non-existent device.
    /// Should fall back to default device or return NoDefaultDevice error.
    #[test]
    fn test_device_selection_with_invalid_id() {
        let host = AudioHost::new();

        // Should fall back to default (or return NoDefaultDevice error)
        let _ = get_input_device(&host, Some("nonexistent-device-id-12345"));
        let _ = get_output_device(&host, Some("nonexistent-device-id-12345"));
    }

    /// Verifies AudioDeviceInfo equality comparison.
    #[test]
    fn test_device_info_equality() {
        let info1 = AudioDeviceInfo {
            name: "Test Device".to_string(),
            id: "test-id".to_string(),
        };
        let info2 = AudioDeviceInfo {
            name: "Test Device".to_string(),
            id: "test-id".to_string(),
        };
        let info3 = AudioDeviceInfo {
            name: "Other Device".to_string(),
            id: "other-id".to_string(),
        };

        assert_eq!(info1, info2);
        assert_ne!(info1, info3);
    }

    /// Verifies AudioDeviceInfo cloning preserves all fields.
    #[test]
    fn test_device_info_clone() {
        let info = AudioDeviceInfo {
            name: "Test Device".to_string(),
            id: "test-id".to_string(),
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    /// Verifies all DeviceError variants display correctly.
    #[test]
    fn test_error_display() {
        let err1 = DeviceError::DeviceNotFound("test-id".to_string());
        assert!(err1.to_string().contains("test-id"));

        let err2 = DeviceError::NoDefaultDevice;
        assert!(err2.to_string().contains("No default device"));

        let err3 = DeviceError::EnumerationFailed("some error".to_string());
        assert!(err3.to_string().contains("some error"));

        let err4 = DeviceError::InvalidDeviceId("bad-id".to_string());
        assert!(err4.to_string().contains("bad-id"));
    }

    /// Verifies AudioDeviceList cloning preserves device counts.
    #[test]
    fn test_device_list_clone() {
        let list = AudioDeviceList {
            input_devices: vec![AudioDeviceInfo {
                name: "Input".to_string(),
                id: "input-id".to_string(),
            }],
            output_devices: vec![AudioDeviceInfo {
                name: "Output".to_string(),
                id: "output-id".to_string(),
            }],
        };
        let cloned = list.clone();
        assert_eq!(list.input_devices.len(), cloned.input_devices.len());
        assert_eq!(list.output_devices.len(), cloned.output_devices.len());
    }
}
