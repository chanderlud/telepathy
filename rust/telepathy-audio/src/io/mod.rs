//! Audio I/O module.
//!
//! This module provides the high-level API for audio capture and playback.
//! It handles device selection, resampling, codec encoding/decoding, and
//! delivers/receives audio via trait-based sinks and sources.
//!
//! ## Modules
//!
//! - [`input`](mod@crate::io::input) - Audio input capture with noise suppression and codec encoding
//! - [`output`](mod@crate::io::output) - Audio output playback with codec decoding and volume control
//!
//! ## Example
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
//! use bytes::Bytes;
//! use std::sync::mpsc;
//!
//! let host = AudioHost::new();
//!
//! // Create audio input with callback
//! let input = AudioInputBuilder::new()
//!     .volume(1.0)
//!     .callback(|data| {
//!         // Process or transmit audio data
//!     })
//!     .build(&host)
//!     .unwrap();
//!
//! // Create audio output using a custom source (here: std::sync::mpsc)
//! let (tx, rx) = mpsc::channel::<Bytes>();
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .source(MpscSource::new(rx))
//!     .build(&host)
//!     .unwrap();
//! let _ = tx; // feed Bytes frames via tx
//! ```

pub mod input;
pub mod output;
pub mod traits;

// Re-export main types for convenience
pub use input::{AudioInputBuilder, AudioInputConfig, AudioInputHandle};
pub use output::{AudioOutputBuilder, AudioOutputConfig, AudioOutputHandle};
pub use traits::{AudioDataSink, AudioDataSource};

/// cpal::Stream is not yet send and sync on WASM
///
/// SendStream allows the Stream to be used in spawned tasks
struct SendStream(cpal::Stream);
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}
