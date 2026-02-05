//! Audio module providing audio processing functionality for Telepathy.
//!
//! This module re-exports types from the telepathy_audio library
//! and provides the player module for sound playback.

// iOS audio session management
/// flutter_rust_bridge:ignore
#[cfg(target_os = "ios")]
pub(crate) mod ios;

// Sound player module (Flutter wrapper for telepathy_audio player)
pub mod player;
