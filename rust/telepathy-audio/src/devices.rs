//! Audio device enumeration and selection module.
//!
//! This module provides types and functions for enumerating and selecting
//! audio input/output devices across platforms.

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{DefaultStreamConfigError, DeviceId, DeviceIdError};
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
    DefaultConfigMissing(String),
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
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<DefaultStreamConfigError> for DeviceError {
    fn from(err: DefaultStreamConfigError) -> Self {
        Self::DefaultConfigMissing(err.to_string())
    }
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

/// Handle to an audio device for use by audio processing code.
///
/// This struct wraps either a real `cpal::Device` or a mock variant and
/// stores the device ID for comparison purposes.
pub struct DeviceHandle {
    inner: DeviceInner,
    device_id: String,
    device_type: DeviceType,
}

enum DeviceInner {
    Real(cpal::Device),
    #[cfg(feature = "mock-audio")]
    Mock,
}

impl DeviceHandle {
    /// Creates a new device handle from a cpal device.
    fn new(device: cpal::Device, device_id: String, device_type: DeviceType) -> Self {
        Self {
            inner: DeviceInner::Real(device),
            device_id,
            device_type,
        }
    }

    #[cfg(feature = "mock-audio")]
    pub(crate) fn mock(device_id: String, device_type: DeviceType) -> Self {
        Self {
            inner: DeviceInner::Mock,
            device_id,
            device_type,
        }
    }

    /// Returns a reference to the underlying cpal device.
    ///
    /// This method provides access to the underlying cpal device for
    /// consumers that need to query device configuration.
    pub fn device(&self) -> &cpal::Device {
        match &self.inner {
            DeviceInner::Real(device) => device,
            #[cfg(feature = "mock-audio")]
            DeviceInner::Mock => {
                panic!("DeviceHandle::device() is unavailable for mock devices")
            }
        }
    }

    /// Returns the device ID string.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn sample_rate(&self) -> Result<u32, DeviceError> {
        Ok(match &self.inner {
            DeviceInner::Real(device) => match self.device_type {
                DeviceType::Input => device.default_input_config()?.sample_rate(),
                DeviceType::Output => device.default_output_config()?.sample_rate(),
            },
            #[cfg(feature = "mock-audio")]
            DeviceInner::Mock => 48_000,
        })
    }

    /// Returns the device name.
    pub fn name(&self) -> Result<String, DeviceError> {
        match &self.inner {
            DeviceInner::Real(device) => device
                .description()
                .map(|desc| desc.name().to_string())
                .map_err(|e| DeviceError::EnumerationFailed(e.to_string())),
            #[cfg(feature = "mock-audio")]
            DeviceInner::Mock => Ok("mock".to_string()),
        }
    }
}

impl fmt::Debug for DeviceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match &self.inner {
            DeviceInner::Real(device) => format!("{:?}", device.description()),
            #[cfg(feature = "mock-audio")]
            DeviceInner::Mock => "MockDevice".to_string(),
        };
        f.debug_struct("DeviceHandle")
            .field("device_id", &self.device_id)
            .field("description", &description)
            .finish()
    }
}

#[derive(Debug)]
pub enum DeviceType {
    Input,
    Output,
}

/// Host abstraction for device enumeration and selection.
///
/// This trait defines the public audio host operations used by the crate:
/// listing devices and resolving input/output handles from optional IDs.
/// Implementations may wrap platform APIs directly or provide test doubles.
pub trait AudioHost {
    /// Creates a new host with platform-appropriate initialization.
    ///
    /// # Errors
    ///
    /// This constructor does not return a `Result`; backend initialization
    /// fallback behavior is implementation-defined.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let _ = host.list_all_devices();
    /// ```
    fn new() -> Self
    where
        Self: Sized;

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

    /// Resolves an input device by ID, with default fallback semantics.
    ///
    /// When `device_id` is `Some`, implementations should attempt to resolve that
    /// ID first and fall back to the default input device when missing.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::InvalidDeviceId` when the provided ID cannot be parsed
    /// - `DeviceError::NoDefaultDevice` when no input device is available
    /// - `DeviceError::EnumerationFailed` when backend calls fail
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let _specific = host.get_input_device(Some("my-microphone-id"));
    /// let _default = host.get_input_device(None);
    /// ```
    fn get_input_device(&self, device_id: Option<&str>) -> Result<DeviceHandle, DeviceError>;

    /// Resolves an output device by ID, with default fallback semantics.
    ///
    /// When `device_id` is `Some`, implementations should attempt to resolve that
    /// ID first and fall back to the default output device when missing.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::InvalidDeviceId` when the provided ID cannot be parsed
    /// - `DeviceError::NoDefaultDevice` when no output device is available
    /// - `DeviceError::EnumerationFailed` when backend calls fail
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let _specific = host.get_output_device(Some("my-speaker-id"));
    /// let _default = host.get_output_device(None);
    /// ```
    fn get_output_device(&self, device_id: Option<&str>) -> Result<DeviceHandle, DeviceError>;

    /// Resolves the default input device.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::NoDefaultDevice` when no input device is available
    /// - `DeviceError::EnumerationFailed` when backend calls fail
    /// - `DeviceError::InvalidDeviceId` for implementation-specific parsing failures
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let device = host.get_default_input_device().unwrap();
    /// println!("Default input: {}", device.name().unwrap());
    /// ```
    fn get_default_input_device(&self) -> Result<DeviceHandle, DeviceError>;

    /// Resolves the default output device.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - `DeviceError::NoDefaultDevice` when no output device is available
    /// - `DeviceError::EnumerationFailed` when backend calls fail
    /// - `DeviceError::InvalidDeviceId` for implementation-specific parsing failures
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::devices::{AudioHost, CpalAudioHost};
    ///
    /// let host = CpalAudioHost::new();
    /// let device = host.get_default_output_device().unwrap();
    /// println!("Default output: {}", device.name().unwrap());
    /// ```
    fn get_default_output_device(&self) -> Result<DeviceHandle, DeviceError>;
}

/// CPAL-backed audio host for device management.
///
/// The `CpalAudioHost` wraps the platform-specific audio backend and provides
/// thread-safe access to device enumeration and selection functionality.
///
/// A single `CpalAudioHost` instance should be shared across all components
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
/// `CpalAudioHost` uses `Arc` internally and is safe to clone and share across threads.
/// The underlying cpal host is wrapped in `Arc<cpal::Host>` for efficient sharing.
#[derive(Clone)]
pub struct CpalAudioHost {
    host: Arc<cpal::Host>,
}

impl CpalAudioHost {
    /// Creates a new audio host with platform-appropriate initialization.
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

    /// Returns an atomic reference to the underlying cpal host.
    ///
    /// This method is intended for internal use.
    pub(crate) fn clone_inner(&self) -> Arc<cpal::Host> {
        Arc::clone(&self.host)
    }
}

impl Default for CpalAudioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Arc<cpal::Host>> for CpalAudioHost {
    fn from(host: Arc<cpal::Host>) -> Self {
        Self { host }
    }
}

impl fmt::Debug for CpalAudioHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpalAudioHost").finish()
    }
}

impl AudioHost for CpalAudioHost {
    fn new() -> Self
    where
        Self: Sized,
    {
        CpalAudioHost::new()
    }

    fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        let devices = self
            .inner()
            .input_devices()
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

        Ok(devices.filter_map(|d| device_to_info(&d)).collect())
    }

    fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        let devices = self
            .inner()
            .output_devices()
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

        Ok(devices.filter_map(|d| device_to_info(&d)).collect())
    }

    fn list_all_devices(&self) -> Result<AudioDeviceList, DeviceError> {
        let input_devices = self.list_input_devices()?;
        let output_devices = self.list_output_devices()?;

        Ok(AudioDeviceList {
            input_devices,
            output_devices,
        })
    }

    fn get_input_device(&self, device_id: Option<&str>) -> Result<DeviceHandle, DeviceError> {
        if let Some(id) = device_id {
            let parsed: DeviceId = id
                .parse()
                .map_err(|e: DeviceIdError| DeviceError::InvalidDeviceId(e.to_string()))?;

            // Try to find the device by ID
            if let Some(device) = self.inner().device_by_id(&parsed) {
                return Ok(DeviceHandle::new(device, id.to_string(), DeviceType::Input));
            }

            // Fall back to default device
            tracing::warn!(device.id = id, "input_device_not_found_fallback_to_default");
        }

        // Get default device
        let device = self
            .inner()
            .default_input_device()
            .ok_or(DeviceError::NoDefaultDevice)?;

        let device_id = device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        Ok(DeviceHandle::new(device, device_id, DeviceType::Input))
    }

    fn get_output_device(&self, device_id: Option<&str>) -> Result<DeviceHandle, DeviceError> {
        if let Some(id) = device_id {
            let parsed: DeviceId = id
                .parse()
                .map_err(|e: DeviceIdError| DeviceError::InvalidDeviceId(e.to_string()))?;

            // Try to find the device by ID
            if let Some(device) = self.inner().device_by_id(&parsed) {
                return Ok(DeviceHandle::new(
                    device,
                    id.to_string(),
                    DeviceType::Output,
                ));
            }

            // Fall back to default device
            tracing::warn!(
                device.id = id,
                "output_device_not_found_fallback_to_default"
            );
        }

        // Get default device
        let device = self
            .inner()
            .default_output_device()
            .ok_or(DeviceError::NoDefaultDevice)?;

        let device_id = device
            .id()
            .map(|id| id.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        Ok(DeviceHandle::new(device, device_id, DeviceType::Output))
    }

    fn get_default_input_device(&self) -> Result<DeviceHandle, DeviceError> {
        self.get_input_device(None)
    }

    fn get_default_output_device(&self) -> Result<DeviceHandle, DeviceError> {
        self.get_output_device(None)
    }
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
