//! Persistence layer for Telepathy TUI.
//!
//! This module provides:
//! - JSON config persistence
//! - OS keychain secret storage
//! - opt-in local fallback secret storage

pub mod config;
pub mod fallback;
pub mod keychain;

use thiserror::Error;

use crate::storage::config::{AppConfig, StorageMode};
use crate::storage::fallback::FallbackStore;

/// Service name used by keyring-backed storage.
pub const KEYRING_SERVICE_NAME: &str = "telepathy-tui";
/// Directory name under the OS config directory where app files are persisted.
pub const CONFIG_DIR_NAME: &str = "telepathy-tui";
/// Config file name persisted under [`CONFIG_DIR_NAME`].
pub const CONFIG_FILE_NAME: &str = "config.json";
/// Fallback secrets file name persisted under [`CONFIG_DIR_NAME`].
pub const FALLBACK_SECRETS_FILE_NAME: &str = "secrets.json";

/// Errors returned by storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("config directory is unavailable on this platform")]
    ConfigDirectoryUnavailable,
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("contact not found: {0}")]
    ContactNotFound(String),
    #[error("room not found: {0}")]
    RoomNotFound(String),
    #[error("secret not found")]
    SecretNotFound,
    #[error("keyring unavailable: {0}")]
    KeyringUnavailable(String),
    #[error("fallback secret store requires explicit acknowledgement")]
    FallbackNotAcknowledged,
    #[error("fallback store is disabled for this configuration")]
    FallbackDisabled,
    #[error("background task join error: {0}")]
    Join(String),
}

/// Runtime-selected secret storage backend.
#[derive(Debug, Clone)]
pub enum SecretStore {
    /// Secure OS keyring backend.
    Keychain,
    /// Local file fallback backend.
    Fallback(FallbackStore),
}

impl SecretStore {
    /// Constructs a secret store based on persisted app configuration.
    pub fn from_config(config: &AppConfig) -> Result<Self, StorageError> {
        match config.storage_mode {
            StorageMode::SecureKeyring => Ok(Self::Keychain),
            StorageMode::FallbackFile => Ok(Self::Fallback(FallbackStore::from_config(config)?)),
        }
    }

    /// Stores keypair bytes for a profile.
    pub async fn store_keypair(
        &self,
        profile_id: &str,
        keypair_bytes: &[u8],
    ) -> Result<(), StorageError> {
        match self {
            Self::Keychain => keychain::store_keypair(profile_id, keypair_bytes).await,
            Self::Fallback(store) => store.store_keypair(profile_id, keypair_bytes),
        }
    }

    /// Loads keypair bytes for a profile.
    pub async fn load_keypair(&self, profile_id: &str) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::Keychain => keychain::load_keypair(profile_id).await,
            Self::Fallback(store) => store.load_keypair(profile_id),
        }
    }

    /// Deletes all secrets known for a profile.
    pub async fn delete_profile_keys(&self, profile_id: &str) -> Result<(), StorageError> {
        match self {
            Self::Keychain => keychain::delete_profile_keys(profile_id).await,
            Self::Fallback(store) => store.delete_profile_keys(profile_id),
        }
    }

    /// Stores a contact peer ID for a profile/contact pair.
    pub async fn store_contact_peer_id(
        &self,
        profile_id: &str,
        contact_id: &str,
        peer_id: &str,
    ) -> Result<(), StorageError> {
        match self {
            Self::Keychain => {
                keychain::store_contact_peer_id(profile_id, contact_id, peer_id).await
            }
            Self::Fallback(store) => store.store_contact_peer_id(profile_id, contact_id, peer_id),
        }
    }

    /// Loads a contact peer ID for a profile/contact pair.
    pub async fn load_contact_peer_id(
        &self,
        profile_id: &str,
        contact_id: &str,
    ) -> Result<String, StorageError> {
        match self {
            Self::Keychain => keychain::load_contact_peer_id(profile_id, contact_id).await,
            Self::Fallback(store) => store.load_contact_peer_id(profile_id, contact_id),
        }
    }

    /// Deletes a contact peer ID for a profile/contact pair.
    pub async fn delete_contact_peer_id(
        &self,
        profile_id: &str,
        contact_id: &str,
    ) -> Result<(), StorageError> {
        match self {
            Self::Keychain => keychain::delete_contact_peer_id(profile_id, contact_id).await,
            Self::Fallback(store) => store.delete_contact_peer_id(profile_id, contact_id),
        }
    }
}

pub(crate) fn resolve_app_config_dir() -> Result<std::path::PathBuf, StorageError> {
    dirs::config_dir()
        .map(|path| path.join(CONFIG_DIR_NAME))
        .ok_or(StorageError::ConfigDirectoryUnavailable)
}
