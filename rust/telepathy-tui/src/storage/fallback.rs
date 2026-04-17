use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::config::{AppConfig, StorageMode};
use crate::storage::{FALLBACK_SECRETS_FILE_NAME, StorageError, resolve_app_config_dir};

/// File-backed fallback secret store for environments where keyring is unavailable.
#[derive(Debug, Clone)]
pub struct FallbackStore {
    path: PathBuf,
}

impl FallbackStore {
    /// Creates a fallback store only when config explicitly enables and acknowledges it.
    pub fn from_config(config: &AppConfig) -> Result<Self, StorageError> {
        if config.storage_mode != StorageMode::FallbackFile {
            return Err(StorageError::FallbackDisabled);
        }
        if !config.fallback_acknowledged {
            return Err(StorageError::FallbackNotAcknowledged);
        }
        let path = resolve_app_config_dir()?.join(FALLBACK_SECRETS_FILE_NAME);
        Ok(Self { path })
    }

    /// Stores keypair bytes for a profile in fallback JSON.
    pub fn store_keypair(
        &self,
        profile_id: &str,
        keypair_bytes: &[u8],
    ) -> Result<(), StorageError> {
        let mut secrets = self.load_all()?;
        let profile = secrets
            .profiles
            .entry(profile_id.to_owned())
            .or_insert_with(ProfileSecrets::default);
        profile.keypair_b64 = Some(base64::engine::general_purpose::STANDARD.encode(keypair_bytes));
        self.save_all(&secrets)
    }

    /// Loads keypair bytes for a profile from fallback JSON.
    pub fn load_keypair(&self, profile_id: &str) -> Result<Vec<u8>, StorageError> {
        let secrets = self.load_all()?;
        let encoded = secrets
            .profiles
            .get(profile_id)
            .and_then(|profile| profile.keypair_b64.as_ref())
            .ok_or(StorageError::SecretNotFound)?;
        let decoded = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        Ok(decoded)
    }

    /// Deletes all fallback secrets for a profile.
    pub fn delete_profile_keys(&self, profile_id: &str) -> Result<(), StorageError> {
        let mut secrets = self.load_all()?;
        secrets.profiles.remove(profile_id);
        self.save_all(&secrets)
    }

    /// Stores a contact peer ID in fallback JSON.
    pub fn store_contact_peer_id(
        &self,
        profile_id: &str,
        contact_id: &str,
        peer_id: &str,
    ) -> Result<(), StorageError> {
        let mut secrets = self.load_all()?;
        let profile = secrets
            .profiles
            .entry(profile_id.to_owned())
            .or_insert_with(ProfileSecrets::default);
        profile
            .contacts
            .insert(contact_id.to_owned(), peer_id.to_owned());
        self.save_all(&secrets)
    }

    /// Loads a contact peer ID from fallback JSON.
    pub fn load_contact_peer_id(
        &self,
        profile_id: &str,
        contact_id: &str,
    ) -> Result<String, StorageError> {
        let secrets = self.load_all()?;
        secrets
            .profiles
            .get(profile_id)
            .and_then(|profile| profile.contacts.get(contact_id))
            .cloned()
            .ok_or(StorageError::SecretNotFound)
    }

    /// Deletes a contact peer ID from fallback JSON.
    pub fn delete_contact_peer_id(
        &self,
        profile_id: &str,
        contact_id: &str,
    ) -> Result<(), StorageError> {
        let mut secrets = self.load_all()?;
        if let Some(profile) = secrets.profiles.get_mut(profile_id) {
            profile.contacts.remove(contact_id);
        }
        self.save_all(&secrets)
    }

    fn load_all(&self) -> Result<FallbackSecretsFile, StorageError> {
        if !self.path.exists() {
            return Ok(FallbackSecretsFile::default());
        }
        let bytes = fs::read(&self.path)?;
        let secrets = serde_json::from_slice(&bytes)?;
        Ok(secrets)
    }

    fn save_all(&self, secrets: &FallbackSecretsFile) -> Result<(), StorageError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let payload = serde_json::to_vec_pretty(secrets)?;
        let temp_path = self
            .path
            .with_extension(format!("json.tmp-{}", Uuid::new_v4()));
        let write_result = (|| -> Result<(), StorageError> {
            let mut file = fs::File::create(&temp_path)?;
            file.write_all(&payload)?;
            file.sync_all()?;
            fs::rename(&temp_path, &self.path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FallbackSecretsFile {
    profiles: HashMap<String, ProfileSecrets>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProfileSecrets {
    keypair_b64: Option<String>,
    contacts: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    fn store_for_test() -> FallbackStore {
        let root = env::temp_dir().join(format!("telepathy-tui-fallback-test-{}", Uuid::new_v4()));
        let path = root.join("secrets.json");
        FallbackStore { path }
    }

    #[test]
    fn fallback_gating_requires_acknowledgement() {
        let config = AppConfig {
            storage_mode: StorageMode::FallbackFile,
            fallback_acknowledged: false,
            ..AppConfig::default()
        };
        let result = FallbackStore::from_config(&config);
        assert!(matches!(result, Err(StorageError::FallbackNotAcknowledged)));
    }

    #[test]
    fn fallback_store_roundtrip() {
        let store = store_for_test();
        let profile_id = "profile-1";
        let contact_id = "contact-1";

        store
            .store_keypair(profile_id, b"\x01\x02\x03")
            .expect("keypair should store");
        store
            .store_contact_peer_id(profile_id, contact_id, "12D3KooWPeer")
            .expect("peer id should store");

        let keypair = store.load_keypair(profile_id).expect("keypair should load");
        assert_eq!(keypair, b"\x01\x02\x03");

        let peer_id = store
            .load_contact_peer_id(profile_id, contact_id)
            .expect("peer id should load");
        assert_eq!(peer_id, "12D3KooWPeer");

        store
            .delete_contact_peer_id(profile_id, contact_id)
            .expect("contact should delete");
        assert!(matches!(
            store.load_contact_peer_id(profile_id, contact_id),
            Err(StorageError::SecretNotFound)
        ));

        store
            .delete_profile_keys(profile_id)
            .expect("profile secrets should delete");
        assert!(matches!(
            store.load_keypair(profile_id),
            Err(StorageError::SecretNotFound)
        ));
    }
}
