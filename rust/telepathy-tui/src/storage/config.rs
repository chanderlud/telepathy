use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::{CONFIG_FILE_NAME, StorageError, resolve_app_config_dir};

/// Top-level application configuration persisted to JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub active_profile_id: String,
    pub profiles: Vec<ProfileMeta>,
    pub preferences: Preferences,
    pub storage_mode: StorageMode,
    pub fallback_acknowledged: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_profile_id: String::new(),
            profiles: Vec::new(),
            preferences: Preferences::default(),
            storage_mode: StorageMode::SecureKeyring,
            fallback_acknowledged: false,
        }
    }
}

/// Secure/fallback secret store selection persisted in config.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StorageMode {
    #[default]
    SecureKeyring,
    FallbackFile,
}

/// User-visible profile metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileMeta {
    pub id: String,
    pub nickname: String,
    pub contacts: Vec<ContactMeta>,
    pub rooms: Vec<RoomConfig>,
}

/// User-visible contact metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactMeta {
    pub id: String,
    pub nickname: String,
}

/// Room configuration for group calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomConfig {
    pub nickname: String,
    pub peer_ids: Vec<String>,
}

/// Non-secret application preferences.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Preferences {
    pub relay_address: String,
    pub relay_id: String,
    pub output_volume_db: f32,
    pub input_volume_db: f32,
    pub sound_volume_db: f32,
    pub input_sensitivity_db: f32,
    pub output_device_id: Option<String>,
    pub input_device_id: Option<String>,
    pub use_denoise: bool,
    pub denoise_model: Option<String>,
    pub play_custom_ringtones: bool,
    pub custom_ringtone_path: Option<String>,
    pub efficiency_mode: bool,
    pub codec_enabled: bool,
    pub codec_vbr: bool,
    pub codec_residual_bits: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            relay_address: String::new(),
            relay_id: String::new(),
            output_volume_db: 0.0,
            input_volume_db: 0.0,
            sound_volume_db: 0.0,
            input_sensitivity_db: 0.0,
            output_device_id: None,
            input_device_id: None,
            use_denoise: false,
            denoise_model: None,
            play_custom_ringtones: false,
            custom_ringtone_path: None,
            efficiency_mode: false,
            codec_enabled: false,
            codec_vbr: false,
            codec_residual_bits: 0.0,
        }
    }
}

/// Loads config from the default app path, returning defaults if no file exists.
pub fn load_config() -> Result<AppConfig, StorageError> {
    let path = config_file_path()?;
    load_config_from_path(&path)
}

/// Saves config to the default app path using atomic write/rename.
pub fn save_config(config: &AppConfig) -> Result<(), StorageError> {
    let path = config_file_path()?;
    save_config_to_path(config, &path)
}

/// Creates a profile and returns its generated profile ID.
pub fn create_profile(config: &mut AppConfig, nickname: impl Into<String>) -> String {
    let profile = ProfileMeta {
        id: Uuid::new_v4().to_string(),
        nickname: nickname.into(),
        contacts: Vec::new(),
        rooms: Vec::new(),
    };
    let profile_id = profile.id.clone();
    if config.active_profile_id.is_empty() {
        config.active_profile_id = profile_id.clone();
    }
    config.profiles.push(profile);
    profile_id
}

/// Deletes a profile and updates active profile if needed.
pub fn delete_profile(config: &mut AppConfig, profile_id: &str) -> Result<(), StorageError> {
    let original_len = config.profiles.len();
    config.profiles.retain(|profile| profile.id != profile_id);

    if config.profiles.len() == original_len {
        return Err(StorageError::ProfileNotFound(profile_id.to_owned()));
    }

    if config.active_profile_id == profile_id {
        config.active_profile_id = config
            .profiles
            .first()
            .map(|profile| profile.id.clone())
            .unwrap_or_default();
    }
    Ok(())
}

/// Switches the active profile.
pub fn switch_profile(config: &mut AppConfig, profile_id: &str) -> Result<(), StorageError> {
    if config
        .profiles
        .iter()
        .any(|profile| profile.id == profile_id)
    {
        config.active_profile_id = profile_id.to_owned();
        Ok(())
    } else {
        Err(StorageError::ProfileNotFound(profile_id.to_owned()))
    }
}

/// Renames an existing profile.
pub fn rename_profile(
    config: &mut AppConfig,
    profile_id: &str,
    nickname: impl Into<String>,
) -> Result<(), StorageError> {
    let profile = profile_mut(config, profile_id)?;
    profile.nickname = nickname.into();
    Ok(())
}

/// Adds a contact to the specified profile and returns generated contact ID.
pub fn add_contact(
    config: &mut AppConfig,
    profile_id: &str,
    nickname: impl Into<String>,
) -> Result<String, StorageError> {
    let profile = profile_mut(config, profile_id)?;
    let contact = ContactMeta {
        id: Uuid::new_v4().to_string(),
        nickname: nickname.into(),
    };
    let contact_id = contact.id.clone();
    profile.contacts.push(contact);
    Ok(contact_id)
}

/// Removes a contact from a profile.
pub fn remove_contact(
    config: &mut AppConfig,
    profile_id: &str,
    contact_id: &str,
) -> Result<(), StorageError> {
    let profile = profile_mut(config, profile_id)?;
    let original_len = profile.contacts.len();
    profile.contacts.retain(|contact| contact.id != contact_id);
    if profile.contacts.len() == original_len {
        return Err(StorageError::ContactNotFound(contact_id.to_owned()));
    }
    Ok(())
}

/// Renames a contact in a profile.
pub fn rename_contact(
    config: &mut AppConfig,
    profile_id: &str,
    contact_id: &str,
    nickname: impl Into<String>,
) -> Result<(), StorageError> {
    let profile = profile_mut(config, profile_id)?;
    let contact = profile
        .contacts
        .iter_mut()
        .find(|contact| contact.id == contact_id)
        .ok_or_else(|| StorageError::ContactNotFound(contact_id.to_owned()))?;
    contact.nickname = nickname.into();
    Ok(())
}

/// Adds a room to a profile.
pub fn add_room(
    config: &mut AppConfig,
    profile_id: &str,
    nickname: impl Into<String>,
    peer_ids: Vec<String>,
) -> Result<(), StorageError> {
    let profile = profile_mut(config, profile_id)?;
    profile.rooms.push(RoomConfig {
        nickname: nickname.into(),
        peer_ids,
    });
    Ok(())
}

/// Removes a room from a profile by room nickname.
pub fn remove_room(
    config: &mut AppConfig,
    profile_id: &str,
    room_nickname: &str,
) -> Result<(), StorageError> {
    let profile = profile_mut(config, profile_id)?;
    let original_len = profile.rooms.len();
    profile.rooms.retain(|room| room.nickname != room_nickname);
    if profile.rooms.len() == original_len {
        return Err(StorageError::RoomNotFound(room_nickname.to_owned()));
    }
    Ok(())
}

/// Updates non-secret preferences using a caller-provided closure.
pub fn update_preferences<F>(config: &mut AppConfig, updater: F)
where
    F: FnOnce(&mut Preferences),
{
    updater(&mut config.preferences);
}

/// Sets the storage mode and optional fallback acknowledgement state.
pub fn set_storage_mode(config: &mut AppConfig, mode: StorageMode, fallback_acknowledged: bool) {
    config.storage_mode = mode;
    config.fallback_acknowledged = fallback_acknowledged;
}

pub(crate) fn config_file_path() -> Result<PathBuf, StorageError> {
    Ok(resolve_app_config_dir()?.join(CONFIG_FILE_NAME))
}

pub(crate) fn load_config_from_path(path: &Path) -> Result<AppConfig, StorageError> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let data = fs::read(path)?;
    let config = serde_json::from_slice(&data)?;
    Ok(config)
}

pub(crate) fn save_config_to_path(config: &AppConfig, path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = serde_json::to_vec_pretty(config)?;
    let temp_path = path.with_extension(format!("json.tmp-{}", Uuid::new_v4()));
    let write_result = (|| -> Result<(), StorageError> {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&payload)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn profile_mut<'a>(
    config: &'a mut AppConfig,
    profile_id: &str,
) -> Result<&'a mut ProfileMeta, StorageError> {
    config
        .profiles
        .iter_mut()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| StorageError::ProfileNotFound(profile_id.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use super::*;

    fn temp_path(file_name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("telepathy-tui-storage-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("temp dir should be creatable");
        root.join(file_name)
    }

    #[test]
    fn load_missing_config_returns_default() {
        let path = temp_path("config.json");
        let config = load_config_from_path(&path).expect("load should succeed");
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_path("config.json");
        let mut config = AppConfig::default();
        let profile_id = create_profile(&mut config, "alice");
        let contact_id =
            add_contact(&mut config, &profile_id, "bob").expect("contact add should succeed");
        add_room(
            &mut config,
            &profile_id,
            "team-room",
            vec!["12D3KooWPeer1".to_owned(), "12D3KooWPeer2".to_owned()],
        )
        .expect("room add should succeed");
        rename_contact(&mut config, &profile_id, &contact_id, "bob-updated")
            .expect("contact rename should succeed");
        update_preferences(&mut config, |prefs| {
            prefs.relay_address = "relay.example.com:40142".to_owned();
            prefs.codec_enabled = true;
        });
        set_storage_mode(&mut config, StorageMode::FallbackFile, true);

        save_config_to_path(&config, &path).expect("save should succeed");
        let loaded = load_config_from_path(&path).expect("load should succeed");
        assert_eq!(loaded, config);
    }

    #[test]
    fn profile_crud_updates_state() {
        let mut config = AppConfig::default();
        let first = create_profile(&mut config, "first");
        let second = create_profile(&mut config, "second");
        assert_eq!(config.active_profile_id, first);

        switch_profile(&mut config, &second).expect("profile should switch");
        assert_eq!(config.active_profile_id, second);

        rename_profile(&mut config, &second, "second-renamed").expect("rename should work");
        assert_eq!(config.profiles[1].nickname, "second-renamed");

        delete_profile(&mut config, &second).expect("profile should delete");
        assert_eq!(config.active_profile_id, first);
    }
}
