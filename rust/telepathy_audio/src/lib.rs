//! # Telepathy Audio Library
//!
//! A standalone audio processing library that provides device enumeration,
//! audio capture, playback, and processing capabilities with codec support.
//!
//! ## Features
//!
//! - **Device Management**: Enumerate and select audio input/output devices
//! - **Audio Capture**: High-quality audio input with optional noise suppression
//! - **Audio Playback**: Low-latency audio output with automatic resampling
//! - **Codec Support**: SEA codec encoding/decoding for efficient transmission
//! - **SIMD Optimization**: Hardware-accelerated audio processing with automatic detection
//!   - AVX-512 for 16-element aligned frames (on supported CPUs)
//!   - AVX2 for 8-element aligned frames (on supported CPUs)
//!   - Scalar fallback when alignment requirements aren't met
//! - **Cross-Platform**: Native support for Windows, macOS, Linux, and WebAssembly
//!
//! ## Module Organization
//!
//! The library is organized into the following modules:
//!
//! - **Public Modules**:
//!   - [`devices`] - Device enumeration and selection
//!   - [`io`] - Audio input/output builders and handles
//!   - [`player`] - Audio file playback (WAV and SEA codec)
//!   - [`error`] - Error types
//!   - [`constants`] - Public constants (FRAME_SIZE, etc.)
//!
//! - **Internal Modules** (not part of public API):
//!   - `internal` - Processing pipeline implementation
//!   - `platform` - Platform-specific code (WASM)
//!
//! ## Platform Support
//!
//! | Platform | Backend |
//! |----------|---------|
//! | Windows  | WASAPI  |
//! | macOS    | CoreAudio |
//! | Linux    | ALSA    |
//! | Web      | AudioWorklet/WebAudio |
//!
//! ## Basic Usage
//!
//! ### Device Enumeration
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
//! ### Audio Input
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioInputBuilder};
//!
//! let host = AudioHost::new();
//!
//! // Create an audio input with callback
//! let input = AudioInputBuilder::new()
//!     .volume(1.0)
//!     .denoise(true, None)  // Enable noise suppression with default model
//!     .rms_threshold(0.01)  // Silence detection
//!     .callback(|data| {
//!         // Process or transmit the audio data
//!         println!("Received {} bytes", data.len());
//!     })
//!     .build(&host)
//!     .unwrap();
//!
//! // Control the input
//! input.mute();
//! input.set_volume(0.8);
//! input.unmute();
//! ```
//!
//! ### Audio Output
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//!
//! let host = AudioHost::new();
//!
//! // Create an audio output
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .volume(1.0)
//!     .build(&host)
//!     .unwrap();
//!
//! // Get sender for feeding audio data
//! let sender = output.sender();
//!
//! // Control the output
//! output.set_volume(0.8);
//! output.deafen();
//! output.undeafen();
//! ```
//!
//! ### Multiple Outputs
//!
//! The library supports creating multiple independent output streams:
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//!
//! let host = AudioHost::new();
//!
//! // Create multiple outputs for different audio sources
//! let output1 = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .build(&host)
//!     .unwrap();
//!
//! let output2 = AudioOutputBuilder::new()
//!     .sample_rate(44100)  // Different sample rate
//!     .build(&host)
//!     .unwrap();
//!
//! // Each output has its own sender
//! let sender1 = output1.sender();
//! let sender2 = output2.sender();
//! ```
//!
//! ## With Codec Support
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};
//!
//! let host = AudioHost::new();
//!
//! // Input with codec encoding
//! let input = AudioInputBuilder::new()
//!     .codec(true, false, 5.0)  // enabled, VBR disabled, 5 residual bits
//!     .callback(|encoded_data| {
//!         // Send encoded data over network
//!     })
//!     .build(&host)
//!     .unwrap();
//!
//! // Output with codec decoding
//! let output = AudioOutputBuilder::new()
//!     .codec(true, None)  // enabled, no pre-defined header
//!     .build(&host)
//!     .unwrap();
//! ```
//!
//! ## With Shared Atomic State
//!
//! For real-time state synchronization between core components and audio processing,
//! use the shared atomic builder methods:
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
//! use atomic_float::AtomicF32;
//!
//! // Core state that can be shared across components
//! let input_volume = Arc::new(AtomicF32::new(1.0));
//! let output_volume = Arc::new(AtomicF32::new(1.0));
//! let muted = Arc::new(AtomicBool::new(false));
//! let deafened = Arc::new(AtomicBool::new(false));
//!
//! let host = AudioHost::new();
//!
//! // Input using shared atomics - changes to core state affect processing immediately
//! let input = AudioInputBuilder::new()
//!     .input_volume_shared(&input_volume)
//!     .muted_shared(&muted)
//!     .callback(|data| { /* process audio */ })
//!     .build(&host)
//!     .unwrap();
//!
//! // Output using shared atomics
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .output_volume_shared(&output_volume)
//!     .deafened_shared(&deafened)
//!     .build(&host)
//!     .unwrap();
//!
//! // Changes to shared state immediately affect audio processing
//! input_volume.store(0.5, Relaxed);  // Reduce input volume
//! muted.store(true, Relaxed);        // Mute input
//! output_volume.store(0.8, Relaxed); // Reduce output volume
//! deafened.store(true, Relaxed);     // Deafen output
//! ```

// =============================================================================
// Public modules
// =============================================================================

/// Device enumeration and selection.
///
/// This module provides functionality for listing available audio devices
/// and selecting specific devices for input or output.
pub mod devices;

/// Audio I/O module.
///
/// This module provides high-level builders and handles for audio capture
/// and playback. Use [`io::input::AudioInputBuilder`] for audio input and
/// [`io::output::AudioOutputBuilder`] for audio output.
pub mod io;

/// Audio file playback.
///
/// This module provides a framework-agnostic audio player supporting
/// WAV and SEA codec files with automatic resampling and volume control.
pub mod player;

/// Public constants.
///
/// Contains audio processing constants like frame size.
pub mod constants;

/// Error types.
///
/// Contains error types used throughout the library.
pub mod error;

// =============================================================================
// Internal modules (private)
// =============================================================================

/// Internal implementation details (not part of public API).
mod internal;

/// Platform-specific implementations (not part of public API).
mod platform;

// =============================================================================
// Re-exports for backward compatibility
// =============================================================================

// Re-export device enumeration API
pub use devices::{
    AudioDeviceInfo, AudioDeviceList, AudioHost, DeviceError, DeviceHandle,
    get_default_input_device, get_default_output_device, get_input_device, get_output_device,
    list_all_devices, list_input_devices, list_output_devices,
};

// Re-export audio input/output API (from new io module)
pub use io::input::{AudioInputBuilder, AudioInputConfig, AudioInputHandle};
pub use io::output::{AudioOutputBuilder, AudioOutputConfig, AudioOutputHandle};

// Re-export error types
pub use error::AudioError;

// Re-export constants
pub use constants::FRAME_SIZE;

/// SEA codec file header structure.
///
/// Re-exported from `sea_codec` for consumers that need to construct
/// custom codec configurations for room calls or file encoding.
pub use sea_codec::codec::file::SeaFileHeader;

// Re-export processing utilities

/// Converts decibel values to linear multipliers.
///
/// Uses the standard audio engineering formula: `10^(dB / 20)`.
/// Useful for volume control where 0 dB = unity gain.
pub use internal::utils::db_to_multiplier;

/// SIMD-optimized audio multiplication with automatic CPU feature detection.
///
/// Multiplies audio samples by a factor, using AVX-512, AVX2, or scalar
/// implementations based on runtime CPU capabilities. Results are clamped
/// to [-1.0, 1.0]. See [`internal::processing`] module for details.
pub use internal::processing::wide_mul;

// Re-export player API
/// Framework-agnostic audio player for WAV and SEA codec files.
///
/// Supports automatic format detection, resampling, volume control,
/// and graceful cancellation with fade-out.
pub use player::{AudioPlayer, SoundHandle, wav_to_sea};

// Re-export web audio types for WASM consumers
#[cfg(target_family = "wasm")]
pub use platform::web_audio::{WebAudioInput, WebAudioWrapper};
