//! Audio module providing audio processing functionality for Telepathy.

// iOS audio session management
/// flutter_rust_bridge:ignore
#[cfg(target_os = "ios")]
pub(crate) mod ios;

// Sound player module (Flutter wrapper for telepathy-audio player)
pub mod player;
