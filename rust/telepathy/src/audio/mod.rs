//! Audio module providing audio processing functionality for Telepathy.
//!
//! This module re-exports types from the telepathy_audio library
//! and provides the player module for sound playback.

// Re-export FRAME_SIZE from telepathy_audio (originally from nnnoiseless)
pub use telepathy_audio::FRAME_SIZE;

// Re-export web audio types for WASM
#[cfg(target_family = "wasm")]
pub(crate) mod web_audio {
    pub use telepathy_audio::{WebAudioInput, WebAudioWrapper};
}

// iOS audio session management (not in telepathy_audio)
/// flutter_rust_bridge:ignore
#[cfg(target_os = "ios")]
pub(crate) mod ios;

// Sound player module (Flutter wrapper for telepathy_audio player)
pub mod player;
