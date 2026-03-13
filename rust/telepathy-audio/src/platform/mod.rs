//! Platform-specific implementations.
//!
//! **This module is not part of the public API.**
//!
//! This module contains platform-specific audio implementations that are
//! conditionally compiled based on the target platform.
//!
//! ## Modules
//!
//! - `web_audio` (WASM only) - Web Audio API integration for browser audio

#[cfg(target_family = "wasm")]
pub(crate) mod web_audio;
