//! Audio I/O module.
//!
//! This module provides the high-level API for audio capture and playback.
//! It handles device selection, resampling, codec encoding/decoding, and
//! delivers processed audio through callbacks or channels.
//!
//! ## Modules
//!
//! - [`input`] - Audio input capture with noise suppression and codec encoding
//! - [`output`] - Audio output playback with codec decoding and volume control
//!
//! ## Example
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};
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
//! // Create audio output
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .build(&host)
//!     .unwrap();
//!
//! let sender = output.sender();
//! ```

pub mod input;
pub mod output;

// Re-export main types for convenience
pub use input::{AudioInputBuilder, AudioInputConfig, AudioInputHandle};
pub use output::{AudioOutputBuilder, AudioOutputConfig, AudioOutputHandle};
