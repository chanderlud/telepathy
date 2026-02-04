//! Audio module providing audio processing functionality for Telepathy.
//!
//! This module re-exports audio processing functions from the telepathy_audio
//! library and provides the player module for sound playback.

// Re-export FRAME_SIZE from telepathy_audio (originally from nnnoiseless)
pub use telepathy_audio::FRAME_SIZE;

// Re-export state types
pub(crate) use telepathy_audio::{InputProcessorState, OutputProcessorState};

// Re-export processor functions
pub(crate) use telepathy_audio::{input_processor, output_processor};

// Re-export traits and channel implementations
#[cfg(not(target_family = "wasm"))]
pub(crate) use telepathy_audio::{ChannelInput, ChannelOutput};
#[cfg(target_family = "wasm")]
pub(crate) use telepathy_audio::WebOutput;

// Re-export codec functions
pub(crate) mod codec {
    pub use telepathy_audio::{decoder, encoder};
}

// Re-export web audio types for WASM
#[cfg(target_family = "wasm")]
pub(crate) mod web_audio {
    pub use telepathy_audio::{WebAudioInput, WebAudioWrapper};
}

// iOS audio session management (not in telepathy_audio)
/// flutter_rust_bridge:ignore
#[cfg(target_os = "ios")]
pub(crate) mod ios;

// Sound player module (specific to telepathy)
pub mod player;
