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
    let name = format_device_name(
        description.name(),
        description.manufacturer(),
        description.extended(),
    );
    let id = device.id().ok()?.to_string();
    Some(AudioDeviceInfo { name, id })
}

fn format_device_name(name: &str, manufacturer: Option<&str>, extended: &[String]) -> String {
    let name = name.trim();
    let normalized_name = normalize_device_label(name);

    let detail = extended
        .iter()
        .map(String::as_str)
        .chain(manufacturer)
        .map(str::trim)
        .find(|detail| is_useful_device_detail(&normalized_name, detail));

    match detail {
        Some(detail) if is_preformatted_device_label(&normalized_name, detail) => {
            detail.to_string()
        }
        Some(detail) => format!("{name} ({detail})"),
        None => name.to_string(),
    }
}

fn is_preformatted_device_label(normalized_name: &str, detail: &str) -> bool {
    let normalized_detail = normalize_device_label(detail);
    normalized_detail.starts_with(&format!("{normalized_name} ("))
        && normalized_detail.ends_with(')')
}

fn is_useful_device_detail(normalized_name: &str, detail: &str) -> bool {
    if detail.is_empty() {
        return false;
    }

    let normalized_detail = normalize_device_label(detail);
    normalized_detail != normalized_name && !normalized_name.contains(&normalized_detail)
}

fn normalize_device_label(label: &str) -> String {
    label.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_input_name_includes_model_detail() {
        let extended = vec!["USB Audio Interface".to_string()];

        let name = format_device_name("Microphone", None, &extended);

        assert_eq!(name, "Microphone (USB Audio Interface)");
    }

    #[test]
    fn generic_output_name_includes_model_detail() {
        let extended = vec!["Studio Monitor DAC".to_string()];

        let name = format_device_name("Speakers", None, &extended);

        assert_eq!(name, "Speakers (Studio Monitor DAC)");
    }

    #[test]
    fn already_descriptive_name_is_preserved() {
        let extended = vec!["USB Audio Interface".to_string()];

        let name = format_device_name("Microphone (USB Audio Interface)", None, &extended);

        assert_eq!(name, "Microphone (USB Audio Interface)");
    }

    #[test]
    fn preformatted_extended_input_detail_is_not_nested() {
        let extended = vec!["Microphone (Yeti Stereo Microphone)".to_string()];

        let name = format_device_name("Microphone", None, &extended);

        assert_eq!(name, "Microphone (Yeti Stereo Microphone)");
    }

    #[test]
    fn preformatted_extended_output_detail_is_not_nested() {
        let extended = vec!["Line 1 (Virtual Audio Cable)".to_string()];

        let name = format_device_name("Line 1", None, &extended);

        assert_eq!(name, "Line 1 (Virtual Audio Cable)");
    }

    #[test]
    fn duplicate_and_blank_details_are_ignored() {
        let extended = vec![" ".to_string(), "Microphone".to_string()];

        let name = format_device_name("Microphone", None, &extended);

        assert_eq!(name, "Microphone");
    }

    #[test]
    fn manufacturer_is_used_when_extended_details_are_missing() {
        let name = format_device_name("Speakers", Some("Focusrite"), &[]);

        assert_eq!(name, "Speakers (Focusrite)");
    }

    #[test]
    fn manufacturer_is_used_when_extended_details_are_not_useful() {
        let extended = vec!["Speakers".to_string()];

        let name = format_device_name("Speakers", Some("Focusrite"), &extended);

        assert_eq!(name, "Speakers (Focusrite)");
    }
}
