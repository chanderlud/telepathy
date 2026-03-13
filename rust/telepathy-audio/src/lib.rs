//! # Telepathy Real Time Audio
//!
//! A standalone audio processing library that provides device enumeration,
//! audio capture, playback, and processing capabilities in real time.
//!
//! ## Features
//!
//! - **Device Management**: Enumerate and select audio input/output devices
//! - **Audio Capture**: High-quality audio input with optional noise suppression
//! - **Audio Playback**: Low-latency audio output with automatic resampling
//! - **Codec Support**: Encoding/decoding for efficient transmission using a modified verison of <https://github.com/Daninet/sea-codec>
//! - **SIMD Optimization**: Hardware-accelerated audio processing with automatic detection
//!   - AVX-512 for 16-element aligned frames (on supported CPUs)
//!   - AVX2 for 8-element aligned frames (on supported CPUs)
//!   - Scalar fallback when alignment requirements aren't met
//! - **Cross-Platform**: Native support for Windows, macOS, Linux, Android, iOS, and WebAssembly
//!
//!
//! ## Platform Support
//!
//! | Platform | Backend | Threading | Denoising | SEA Codec |
//! |----------|---------|-----------|------|-----|
//! | Windows  | WASAPI  | OS threads | ✅ | ✅ |
//! | macOS    | CoreAudio | OS threads |✅|✅|
//! | Linux    | ALSA    | OS threads |✅|✅|
//! | Android  | AAudio  |  OS threads |✅|✅|
//! | iOS      | CoreAudio  |  OS threads |✅|✅|
//! | Web      | AudioWorklet/WebAudio | Web Workers |✅|✅|
//!
//! ## Module Organization
//!
//! The library is organized into the following modules:
//!
//! - **Public Modules**:
//!   - [`devices`] - Device enumeration and selection
//!   - [`adapters`] - Ready-to-use channel adapters (std::sync::mpsc)
//!   - [`io`] - Audio input/output builders and handles
//!   - [`player`] - Audio file playback (WAV and SEA codec)
//!   - [`error`] - Error types
//!
//! ### WASM Threading
//!
//! On WASM targets, the library uses the [`wasm_thread`](https://crates.io/crates/wasm_thread)
//! crate to spawn processor and callback threads as Web Workers. This requires:
//!
//! - **`SharedArrayBuffer` support**: The web server must send COOP/COEP headers:
//!   - `Cross-Origin-Opener-Policy: same-origin`
//!   - `Cross-Origin-Embedder-Policy: require-corp`
//! - **Nightly Rust** (optional): For atomics support via `build-std`
//!
//! See the README for detailed WASM setup instructions and browser compatibility.
//!
//! ## Channel adapters
//!
//! The crate is trait-based (`AudioDataSink` / `AudioDataSource`) and does not require any
//! specific channel library. For convenience, `telepathy_audio::adapters` provides ready-to-use
//! `std::sync::mpsc` implementations: [`MpscSink`](crate::adapters::MpscSink) and [`MpscSource`](crate::adapters::MpscSource).
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
//!         println!("Received {} bytes", data.as_ref().len());
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
//! use bytes::Bytes;
//! use std::sync::mpsc;
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
//!
//! let host = AudioHost::new();
//! let (_tx, rx) = mpsc::channel::<Bytes>();
//!
//! // Create an audio output
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .volume(1.0)
//!     .source(MpscSource::new(rx))
//!     .build(&host)
//!     .unwrap();
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
//! use bytes::Bytes;
//! use std::sync::mpsc;
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
//!
//! let host = AudioHost::new();
//!
//! // Create multiple outputs for different audio sources
//! let (_tx1, rx1) = mpsc::channel::<Bytes>();
//! let output1 = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .source(MpscSource::new(rx1))
//!     .build(&host)
//!     .unwrap();
//!
//! let (_tx2, rx2) = mpsc::channel::<Bytes>();
//! let output2 = AudioOutputBuilder::new()
//!     .sample_rate(44100)  // Different sample rate
//!     .source(MpscSource::new(rx2))
//!     .build(&host)
//!     .unwrap();
//! ```
//!
//! ## With Codec Support
//!
//! ```rust,no_run
//! use bytes::Bytes;
//! use std::sync::mpsc;
//! use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
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
//! let (_tx, rx) = mpsc::channel::<Bytes>();
//! let output = AudioOutputBuilder::new()
//!     .codec(true)  // Enable codec decoding
//!     .source(MpscSource::new(rx))
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
//! use std::sync::Arc;
//! use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
//! use atomic_float::AtomicF32;
//! use bytes::Bytes;
//! use std::sync::mpsc;
//! use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
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
//! let (_tx, rx) = mpsc::channel::<Bytes>();
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .output_volume_shared(&output_volume)
//!     .deafened_shared(&deafened)
//!     .source(MpscSource::new(rx))
//!     .build(&host)
//!     .unwrap();
//!
//! // Changes to shared state immediately affect audio processing
//! input_volume.store(0.5, Relaxed);  // Reduce input volume
//! muted.store(true, Relaxed);        // Mute input
//! output_volume.store(0.8, Relaxed); // Reduce output volume
//! deafened.store(true, Relaxed);     // Deafen output
//! ```

pub mod adapters;
pub mod devices;
pub mod error;
pub mod io;
pub mod player;

#[doc(hidden)]
pub mod constants;
#[doc(hidden)]
pub mod internal;
#[doc(hidden)]
pub mod sea;

mod platform;

pub use constants::FRAME_SIZE;
pub use error::Error;

// Re-export web audio wrapper for WASM consumers
#[cfg(target_family = "wasm")]
pub use platform::web_audio::WebAudioWrapper;

pub use cpal::Host;
pub use nnnoiseless::RnnModel;
