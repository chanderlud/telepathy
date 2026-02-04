//! # Telepathy Audio Library
//!
//! A standalone audio device management library that encapsulates `cpal` and provides
//! a clean device enumeration and selection API.
//!
//! ## Features
//!
//! - Device enumeration for input and output audio devices
//! - Device selection by ID with automatic fallback to default devices
//! - Platform-agnostic initialization with wasm AudioWorklet support
//! - Thread-safe host sharing across components
//!
//! ## Platform Support
//!
//! - **Windows**: WASAPI backend
//! - **Linux**: ALSA backend
//! - **macOS**: CoreAudio backend
//! - **Web**: AudioWorklet/WebAudio backend (preferred) with fallback
//!
//! ## Basic Usage
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, list_all_devices, get_default_input_device};
//!
//! // Create an audio host
//! let host = AudioHost::new();
//!
//! // List all available devices
//! let devices = list_all_devices(&host).unwrap();
//! println!("Input devices: {:?}", devices.input_devices);
//! println!("Output devices: {:?}", devices.output_devices);
//!
//! // Get the default input device
//! let input_device = get_default_input_device(&host).unwrap();
//! ```
//!
//! ## Device Selection
//!
//! Devices can be selected by ID or by using the default device:
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, get_input_device, get_output_device};
//!
//! let host = AudioHost::new();
//!
//! // Select specific device by ID
//! let input = get_input_device(&host, Some("device-id-string"));
//!
//! // Use default device
//! let output = get_output_device(&host, None);
//! ```

pub mod devices;

pub use devices::{
    AudioDeviceInfo, AudioDeviceList, AudioHost, DeviceError, DeviceHandle,
    get_default_input_device, get_default_output_device, get_input_device, get_output_device,
    list_all_devices, list_input_devices, list_output_devices,
};
