use std::collections::BTreeSet;

use base64::Engine;
use keyring::Entry;
use tokio::task;

use crate::storage::{KEYRING_SERVICE_NAME, StorageError};

/// Stores keypair bytes for a profile in the OS keyring.
pub async fn store_keypair(profile_id: &str, keypair_bytes: &[u8]) -> Result<(), StorageError> {
    let profile_id = profile_id.to_owned();
    let encoded = base64::engine::general_purpose::STANDARD.encode(keypair_bytes);
    task::spawn_blocking(move || {
        let entry = entry_for(&keypair_account(&profile_id))?;
        entry.set_password(&encoded).map_err(map_keyring_error)
    })
    .await
    .map_err(|error| StorageError::Join(error.to_string()))?
}

/// Loads keypair bytes for a profile from the OS keyring.
pub async fn load_keypair(profile_id: &str) -> Result<Vec<u8>, StorageError> {
    let profile_id = profile_id.to_owned();
    task::spawn_blocking(move || {
        let entry = entry_for(&keypair_account(&profile_id))?;
        let encoded = entry.get_password().map_err(map_keyring_error)?;
        let decoded = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        Ok(decoded)
    })
    .await
    .map_err(|error| StorageError::Join(error.to_string()))?
}

/// Deletes profile keypair and all tracked contact peer IDs for a profile.
pub async fn delete_profile_keys(profile_id: &str) -> Result<(), StorageError> {
    let profile_id = profile_id.to_owned();
    task::spawn_blocking(move || {
        let mut index = load_contact_index(&profile_id)?;

        for contact_id in index.iter() {
            let account = contact_account(&profile_id, contact_id);
            let entry = entry_for(&account)?;
            if let Err(error) = entry.delete_credential()
                && !matches!(error, keyring::Error::NoEntry)
            {
                return Err(map_keyring_error(error));
            }
        }
        index.clear();
        save_contact_index(&profile_id, &index)?;

        let keypair = entry_for(&keypair_account(&profile_id))?;
        if let Err(error) = keypair.delete_credential()
            && !matches!(error, keyring::Error::NoEntry)
        {
            return Err(map_keyring_error(error));
        }

        let index_entry = entry_for(&contacts_index_account(&profile_id))?;
        if let Err(error) = index_entry.delete_credential()
            && !matches!(error, keyring::Error::NoEntry)
        {
            return Err(map_keyring_error(error));
        }
        Ok(())
    })
    .await
    .map_err(|error| StorageError::Join(error.to_string()))?
}

/// Stores a contact peer ID in the OS keyring.
pub async fn store_contact_peer_id(
    profile_id: &str,
    contact_id: &str,
    peer_id: &str,
) -> Result<(), StorageError> {
    let profile_id = profile_id.to_owned();
    let contact_id = contact_id.to_owned();
    let peer_id = peer_id.to_owned();
    task::spawn_blocking(move || {
        let entry = entry_for(&contact_account(&profile_id, &contact_id))?;
        entry.set_password(&peer_id).map_err(map_keyring_error)?;

        let mut index = load_contact_index(&profile_id)?;
        index.insert(contact_id);
        save_contact_index(&profile_id, &index)
    })
    .await
    .map_err(|error| StorageError::Join(error.to_string()))?
}

/// Loads a contact peer ID from the OS keyring.
pub async fn load_contact_peer_id(
    profile_id: &str,
    contact_id: &str,
) -> Result<String, StorageError> {
    let profile_id = profile_id.to_owned();
    let contact_id = contact_id.to_owned();
    task::spawn_blocking(move || {
        let entry = entry_for(&contact_account(&profile_id, &contact_id))?;
        entry.get_password().map_err(map_keyring_error)
    })
    .await
    .map_err(|error| StorageError::Join(error.to_string()))?
}

/// Deletes a contact peer ID from the OS keyring.
pub async fn delete_contact_peer_id(
    profile_id: &str,
    contact_id: &str,
) -> Result<(), StorageError> {
    let profile_id = profile_id.to_owned();
    let contact_id = contact_id.to_owned();
    task::spawn_blocking(move || {
        let entry = entry_for(&contact_account(&profile_id, &contact_id))?;
        if let Err(error) = entry.delete_credential()
            && !matches!(error, keyring::Error::NoEntry)
        {
            return Err(map_keyring_error(error));
        }

        let mut index = load_contact_index(&profile_id)?;
        index.remove(&contact_id);
        save_contact_index(&profile_id, &index)
    })
    .await
    .map_err(|error| StorageError::Join(error.to_string()))?
}

fn entry_for(account: &str) -> Result<Entry, StorageError> {
    Entry::new(KEYRING_SERVICE_NAME, account).map_err(map_keyring_error)
}

fn map_keyring_error(error: keyring::Error) -> StorageError {
    match error {
        keyring::Error::NoEntry => StorageError::SecretNotFound,
        other => StorageError::KeyringUnavailable(other.to_string()),
    }
}

fn keypair_account(profile_id: &str) -> String {
    format!("telepathy/{profile_id}/keypair")
}

fn contact_account(profile_id: &str, contact_id: &str) -> String {
    format!("telepathy/{profile_id}/contact/{contact_id}")
}

fn contacts_index_account(profile_id: &str) -> String {
    format!("telepathy/{profile_id}/contacts-index")
}

fn load_contact_index(profile_id: &str) -> Result<BTreeSet<String>, StorageError> {
    let entry = entry_for(&contacts_index_account(profile_id))?;
    match entry.get_password() {
        Ok(payload) => serde_json::from_str(&payload).map_err(StorageError::from),
        Err(keyring::Error::NoEntry) => Ok(BTreeSet::new()),
        Err(error) => Err(map_keyring_error(error)),
    }
}

fn save_contact_index(profile_id: &str, index: &BTreeSet<String>) -> Result<(), StorageError> {
    let entry = entry_for(&contacts_index_account(profile_id))?;
    let payload = serde_json::to_string(index)?;
    entry.set_password(&payload).map_err(map_keyring_error)
}
