//! tuirealm [`Model`] implementation for telepathy-tui.
//!
//! The model owns the tuirealm `Application`, the terminal bridge, the shared
//! [`AppState`], persistent [`AppConfig`] / [`SecretStore`] handles, and a
//! redraw/quit flag. All state mutation in response to a [`Msg`] happens here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use telepathy_core::native::NativeTelepathy;
use telepathy_core::types::{CallState, Contact};
use tokio::runtime::Handle;
use tuirealm::terminal::{CrosstermTerminalAdapter, TerminalAdapter, TerminalBridge};
use tuirealm::{Application, Sub, SubClause, SubEventClause, Update};

use crate::components::PlaceholderComponent;
use crate::events::{CoreEvent, Id, Msg, SettingKey, SettingValue, VolumeKind};
use crate::state::{AppState, ChatEntry};
use crate::storage::SecretStore;
use crate::storage::config::{self, AppConfig, ContactMeta};

/// Debounce window used before persisting a [`Msg::VolumeChanged`].
pub const VOLUME_DEBOUNCE: Duration = Duration::from_millis(200);

/// Owns every piece of state required to drive the tuirealm event loop.
pub struct Model<T>
where
    T: TerminalAdapter,
{
    pub app: Application<Id, Msg, CoreEvent>,
    pub terminal: TerminalBridge<T>,
    pub state: Arc<Mutex<AppState>>,
    pub config: Arc<Mutex<AppConfig>>,
    pub secret_store: SecretStore,
    pub handle: Handle,
    pub core: Arc<NativeTelepathy>,
    pub quit: bool,
    pub redraw: bool,
    pub volume_debounce: HashMap<VolumeKind, Instant>,
}

impl Model<CrosstermTerminalAdapter> {
    /// Construct a new model wired to the supplied tuirealm application,
    /// terminal adapter, state and persistence handles.
    pub fn new(
        app: Application<Id, Msg, CoreEvent>,
        terminal: TerminalBridge<CrosstermTerminalAdapter>,
        state: Arc<Mutex<AppState>>,
        config: AppConfig,
        secret_store: SecretStore,
        handle: Handle,
        core: Arc<NativeTelepathy>,
    ) -> Self {
        Self {
            app,
            terminal,
            state,
            config: Arc::new(Mutex::new(config)),
            secret_store,
            handle,
            core,
            quit: false,
            redraw: true,
            volume_debounce: HashMap::new(),
        }
    }
}

impl<T> Model<T>
where
    T: TerminalAdapter,
{
    /// Render the current frame. T5 will replace this with the proper layout.
    pub fn view(&mut self) {
        let _ = self.terminal.draw(|frame| {
            let area = frame.area();
            self.app.view(&Id::StatusBar, frame, area);
        });
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, AppState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn lock_config(&self) -> std::sync::MutexGuard<'_, AppConfig> {
        self.config.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn save_config(&self) {
        let config = self.lock_config();
        if let Err(error) = config::save_config(&config) {
            log::error!("failed to persist config: {error}");
        }
    }

    fn refresh_active_profile_view(&mut self) {
        let profile = {
            let config = self.lock_config();
            config
                .profiles
                .iter()
                .find(|p| p.id == config.active_profile_id)
                .cloned()
        };
        if let Some(profile) = profile {
            let mut guard = self.lock_state();
            guard.active_profile = profile.clone();
            guard.contacts = profile.contacts;
            guard.rooms = profile.rooms;
        }
    }

    fn apply_setting(&mut self, key: SettingKey, value: SettingValue) {
        let mut config = self.lock_config();
        let prefs = &mut config.preferences;
        match (key, value) {
            (SettingKey::RelayAddress, SettingValue::Str(v)) => prefs.relay_address = v,
            (SettingKey::RelayId, SettingValue::Str(v)) => prefs.relay_id = v,
            (SettingKey::OutputVolumeDb, SettingValue::Float(v)) => prefs.output_volume_db = v,
            (SettingKey::InputVolumeDb, SettingValue::Float(v)) => prefs.input_volume_db = v,
            (SettingKey::SoundVolumeDb, SettingValue::Float(v)) => prefs.sound_volume_db = v,
            (SettingKey::InputSensitivityDb, SettingValue::Float(v)) => {
                prefs.input_sensitivity_db = v
            }
            (SettingKey::OutputDeviceId, SettingValue::OptStr(v)) => prefs.output_device_id = v,
            (SettingKey::InputDeviceId, SettingValue::OptStr(v)) => prefs.input_device_id = v,
            (SettingKey::UseDenoise, SettingValue::Bool(v)) => prefs.use_denoise = v,
            (SettingKey::DenoiseModel, SettingValue::OptStr(v)) => prefs.denoise_model = v,
            (SettingKey::PlayCustomRingtones, SettingValue::Bool(v)) => {
                prefs.play_custom_ringtones = v
            }
            (SettingKey::CustomRingtonePath, SettingValue::OptStr(v)) => {
                prefs.custom_ringtone_path = v
            }
            (SettingKey::EfficiencyMode, SettingValue::Bool(v)) => prefs.efficiency_mode = v,
            (SettingKey::CodecEnabled, SettingValue::Bool(v)) => prefs.codec_enabled = v,
            (SettingKey::CodecVbr, SettingValue::Bool(v)) => prefs.codec_vbr = v,
            (SettingKey::CodecResidualBits, SettingValue::Float(v)) => prefs.codec_residual_bits = v,
            (key, value) => {
                log::warn!("setting/value mismatch: key={key:?} value={value:?}");
            }
        }
    }

    fn mount_overlay(&mut self, id: Id) {
        let _ = self.app.umount(&id);
        let _ = self.app.mount(
            id,
            Box::new(PlaceholderComponent::default()),
            vec![Sub::new(SubEventClause::Tick, SubClause::Always)],
        );
    }

    fn unmount(&mut self, id: &Id) {
        let _ = self.app.umount(id);
    }
}

impl<T> Update<Msg> for Model<T>
where
    T: TerminalAdapter,
{
    fn update(&mut self, msg: Option<Msg>) -> Option<Msg> {
        let msg = match msg {
            Some(m) => m,
            None => return None,
        };
        self.redraw = true;

        match msg {
            Msg::Quit => {
                self.quit = true;
            }
            Msg::None => {}

            Msg::FocusContacts => {
                let _ = self.app.active(&Id::ContactsPane);
            }
            Msg::FocusCallControls => {
                let _ = self.app.active(&Id::CallControlsPane);
            }
            Msg::FocusChat => {
                let _ = self.app.active(&Id::ChatPane);
            }
            Msg::OpenSettings => {
                self.mount_overlay(Id::SettingsOverlay);
            }
            Msg::CloseSettings => {
                self.unmount(&Id::SettingsOverlay);
            }

            // Contacts ---------------------------------------------------
            Msg::ContactSelected(peer_id) => {
                self.lock_state().active_peer = Some(peer_id);
            }
            Msg::ContactAdd(nickname, peer_id) => {
                let profile_id = self.lock_config().active_profile_id.clone();
                let contact_id = {
                    let mut config = self.lock_config();
                    match config::add_contact(&mut config, &profile_id, nickname.clone()) {
                        Ok(contact_id) => contact_id,
                        Err(error) => {
                            log::error!("contact add failed: {error}");
                            return None;
                        }
                    }
                };
                let secret_store = self.secret_store.clone();
                let config = self.config.clone();
                let state = self.state.clone();
                let profile_id_async = profile_id.clone();
                let contact_id_async = contact_id.clone();
                self.handle.spawn(async move {
                    let store_result = secret_store
                        .store_contact_peer_id(&profile_id_async, &contact_id_async, &peer_id)
                        .await;

                    if let Err(error) = store_result {
                        log::error!(
                            "failed to store peer id for contact {} in profile {}: {error}",
                            contact_id_async,
                            profile_id_async
                        );
                        let rollback_result = {
                            let mut config_guard = config.lock().unwrap_or_else(|p| p.into_inner());
                            config::remove_contact(
                                &mut config_guard,
                                &profile_id_async,
                                &contact_id_async,
                            )
                        };
                        if let Err(rollback_error) = rollback_result {
                            log::error!(
                                "failed to rollback contact add for {} in profile {}: {rollback_error}",
                                contact_id_async,
                                profile_id_async
                            );
                        }
                    } else {
                        let config_guard = config.lock().unwrap_or_else(|p| p.into_inner());
                        if let Err(error) = config::save_config(&config_guard) {
                            log::error!("failed to persist config: {error}");
                        }
                    }

                    let active_profile = {
                        let config_guard = config.lock().unwrap_or_else(|p| p.into_inner());
                        config_guard
                            .profiles
                            .iter()
                            .find(|profile| profile.id == config_guard.active_profile_id)
                            .cloned()
                    };
                    if let Some(active_profile) = active_profile {
                        let mut state_guard = state.lock().unwrap_or_else(|p| p.into_inner());
                        state_guard.active_profile = active_profile.clone();
                        state_guard.contacts = active_profile.contacts;
                        state_guard.rooms = active_profile.rooms;
                    }
                });
            }
            Msg::ContactDelete(contact_id) => {
                let profile_id = self.lock_config().active_profile_id.clone();
                let result = {
                    let mut config = self.lock_config();
                    config::remove_contact(&mut config, &profile_id, &contact_id)
                };
                if let Err(error) = result {
                    log::error!("contact delete failed: {error}");
                } else {
                    let secret_store = self.secret_store.clone();
                    let profile_id_async = profile_id.clone();
                    let contact_id_async = contact_id.clone();
                    self.handle.spawn(async move {
                        if let Err(error) = secret_store
                            .delete_contact_peer_id(&profile_id_async, &contact_id_async)
                            .await
                        {
                            log::error!("secret delete failed: {error}");
                        }
                    });
                    self.save_config();
                    self.refresh_active_profile_view();
                }
            }
            Msg::ContactRename(contact_id, nickname) => {
                let profile_id = self.lock_config().active_profile_id.clone();
                let result = {
                    let mut config = self.lock_config();
                    config::rename_contact(&mut config, &profile_id, &contact_id, nickname)
                };
                if let Err(error) = result {
                    log::error!("contact rename failed: {error}");
                } else {
                    self.save_config();
                    self.refresh_active_profile_view();
                }
            }

            // Rooms ------------------------------------------------------
            Msg::RoomSelected(_) => {}
            Msg::RoomAdd(nickname, peer_ids) => {
                let profile_id = self.lock_config().active_profile_id.clone();
                let result = {
                    let mut config = self.lock_config();
                    config::add_room(&mut config, &profile_id, nickname, peer_ids)
                };
                if let Err(error) = result {
                    log::error!("room add failed: {error}");
                } else {
                    self.save_config();
                    self.refresh_active_profile_view();
                }
            }
            Msg::RoomDelete(nickname) => {
                let profile_id = self.lock_config().active_profile_id.clone();
                let result = {
                    let mut config = self.lock_config();
                    config::remove_room(&mut config, &profile_id, &nickname)
                };
                if let Err(error) = result {
                    log::error!("room delete failed: {error}");
                } else {
                    self.save_config();
                    self.refresh_active_profile_view();
                }
            }
            Msg::RoomJoin(room_name) => {
                let room_members = {
                    let guard = self.lock_state();
                    guard
                        .rooms
                        .iter()
                        .find(|room| room.nickname == room_name)
                        .map(|room| room.peer_ids.clone())
                        .unwrap_or_default()
                };
                let core = self.core.clone();
                self.handle.spawn(async move {
                    if room_members.is_empty() {
                        log::warn!("room '{room_name}' has no members");
                        return;
                    }
                    if let Err(error) = core.join_room(room_members).await {
                        log::error!("room join failed: {error:?}");
                    }
                });
            }

            // Call -------------------------------------------------------
            Msg::StartCall => {
                let (active_peer, contacts) = {
                    let guard = self.lock_state();
                    (guard.active_peer.clone(), guard.contacts.clone())
                };
                let profile_id = self.lock_config().active_profile_id.clone();
                let secret_store = self.secret_store.clone();
                let core = self.core.clone();
                self.handle.spawn(async move {
                    let Some(contact) = resolve_core_contact(
                        profile_id,
                        active_peer,
                        contacts,
                        secret_store,
                    )
                    .await
                    else {
                        log::warn!("cannot start call without an active contact");
                        return;
                    };
                    core.start_session(&contact).await;
                    if let Err(error) = core.start_call(&contact).await {
                        log::error!("start call failed: {error:?}");
                    }
                });
            }
            Msg::EndCall => {
                let core = self.core.clone();
                self.handle.spawn(async move {
                    core.end_call().await;
                });
            }
            Msg::ToggleMute => {
                let muted = {
                    let mut guard = self.lock_state();
                    guard.muted = !guard.muted;
                    guard.muted
                };
                self.core.set_muted(muted);
            }
            Msg::ToggleDeafen => {
                let deafened = {
                    let mut guard = self.lock_state();
                    guard.deafened = !guard.deafened;
                    guard.deafened
                };
                self.core.set_deafened(deafened);
            }
            Msg::VolumeChanged(kind, value) => {
                let now = Instant::now();
                self.volume_debounce.insert(kind, now);
                {
                    let mut guard = self.lock_state();
                    guard.volume_debounce.insert(kind, now);
                }
                {
                    let mut config = self.lock_config();
                    let prefs = &mut config.preferences;
                    match kind {
                        VolumeKind::Output => prefs.output_volume_db = value,
                        VolumeKind::Input => prefs.input_volume_db = value,
                        VolumeKind::Sound => prefs.sound_volume_db = value,
                        VolumeKind::InputSensitivity => prefs.input_sensitivity_db = value,
                    }
                }
                let state = self.state.clone();
                let config = self.config.clone();
                let core = self.core.clone();
                let handle = self.handle.clone();
                handle.spawn(async move {
                    tokio::time::sleep(VOLUME_DEBOUNCE).await;
                    let still_latest = {
                        let guard = state.lock().unwrap_or_else(|p| p.into_inner());
                        guard
                            .volume_debounce
                            .get(&kind)
                            .map(|stored| *stored == now)
                            .unwrap_or(false)
                    };
                    if still_latest {
                        match kind {
                            VolumeKind::Output => core.set_output_volume(value),
                            VolumeKind::Input => core.set_input_volume(value),
                            VolumeKind::InputSensitivity => core.set_rms_threshold(value),
                            VolumeKind::Sound => {}
                        }
                        let guard = config.lock().unwrap_or_else(|p| p.into_inner());
                        if let Err(error) = config::save_config(&guard) {
                            log::error!("volume persist failed: {error}");
                        }
                    }
                });
            }
            Msg::AudioTestToggle => {
                let in_call = {
                    let guard = self.lock_state();
                    !matches!(guard.call_state.as_ref(), CallState::Waiting)
                };
                let core = self.core.clone();
                self.handle.spawn(async move {
                    if in_call {
                        core.end_call().await;
                    } else if let Err(error) = core.audio_test().await {
                        log::error!("audio test failed: {error:?}");
                    }
                });
            }
            Msg::RestartManager => {
                let core = self.core.clone();
                self.handle.spawn(async move {
                    if let Err(error) = core.restart_manager().await {
                        log::error!("manager restart failed: {error:?}");
                    }
                });
            }

            // Chat -------------------------------------------------------
            Msg::SendMessage(text) => {
                let (active_peer, contacts) = {
                    let guard = self.lock_state();
                    (guard.active_peer.clone(), guard.contacts.clone())
                };
                let profile_id = self.lock_config().active_profile_id.clone();
                let secret_store = self.secret_store.clone();
                let core = self.core.clone();
                let message_text = text.clone();
                if let Some(peer_id) = active_peer.clone() {
                    self.lock_state().chat_messages.push(ChatEntry {
                        peer_id,
                        text: message_text.clone(),
                    });
                }
                self.handle.spawn(async move {
                    let Some(contact) = resolve_core_contact(
                        profile_id,
                        active_peer,
                        contacts,
                        secret_store,
                    )
                    .await
                    else {
                        log::warn!("cannot send message without an active contact");
                        return;
                    };

                    let mut chat = core.build_chat(&contact, message_text, Vec::new());
                    if let Err(error) = core.send_chat(&mut chat).await {
                        log::error!("send chat failed: {error:?}");
                    }
                });
            }

            // Settings ---------------------------------------------------
            Msg::SettingChanged(key, value) => {
                self.apply_setting(key, value);
                self.save_config();
            }

            // Profiles ---------------------------------------------------
            Msg::ProfileCreate(nickname) => {
                {
                    let mut config = self.lock_config();
                    let _ = config::create_profile(&mut config, nickname);
                }
                self.save_config();
            }
            Msg::ProfileDelete(profile_id) => {
                let result = {
                    let mut config = self.lock_config();
                    config::delete_profile(&mut config, &profile_id)
                };
                if let Err(error) = result {
                    log::error!("profile delete failed: {error}");
                } else {
                    let secret_store = self.secret_store.clone();
                    let profile_id_async = profile_id.clone();
                    self.handle.spawn(async move {
                        if let Err(error) =
                            secret_store.delete_profile_keys(&profile_id_async).await
                        {
                            log::error!("profile secret delete failed: {error}");
                        }
                    });
                    self.save_config();
                }
            }
            Msg::ProfileSwitch(profile_id) => {
                let result = {
                    let mut config = self.lock_config();
                    config::switch_profile(&mut config, &profile_id)
                };
                if let Err(error) = result {
                    log::error!("profile switch failed: {error}");
                } else {
                    self.save_config();
                    let profile = {
                        let config = self.lock_config();
                        config
                            .profiles
                            .iter()
                            .find(|p| p.id == config.active_profile_id)
                            .cloned()
                    };
                    if let Some(profile) = profile {
                        self.lock_state().replace_active_profile(profile);
                    }
                }
            }

            // Incoming call response ------------------------------------
            Msg::AcceptCall {
                request_id,
                accepted,
            } => {
                let response = {
                    let mut guard = self.lock_state();
                    if guard
                        .incoming_prompt
                        .as_ref()
                        .map(|p| p.request_id == request_id)
                        .unwrap_or(false)
                    {
                        guard.incoming_prompt = None;
                        guard.pending_accept_cancel = None;
                        guard.pending_accept_response.take()
                    } else {
                        None
                    }
                };
                if let Some(tx) = response {
                    self.handle.spawn(async move {
                        let _ = tx.send(accepted);
                    });
                }
                self.unmount(&Id::IncomingCallDialog);
            }

            // Forwarded core events --------------------------------------
            Msg::CoreEvent(core) => match core {
                CoreEvent::CallStateChanged(state) => {
                    self.lock_state().call_state = state;
                }
                CoreEvent::SessionStatusChanged(peer, status) => {
                    self.lock_state().sessions.insert(peer, status);
                }
                CoreEvent::MessageReceived(peer_id, text) => {
                    self.lock_state()
                        .chat_messages
                        .push(ChatEntry { peer_id, text });
                }
                CoreEvent::StatisticsUpdated(stats) => {
                    self.lock_state().statistics = stats;
                }
                CoreEvent::ManagerActiveChanged(active, restartable) => {
                    let mut guard = self.lock_state();
                    guard.manager_active = active;
                    guard.manager_restartable = restartable;
                }
                CoreEvent::IncomingCall { .. } => {
                    self.mount_overlay(Id::IncomingCallDialog);
                }
                CoreEvent::IncomingCallCancelled { request_id } => {
                    let response = {
                        let mut guard = self.lock_state();
                        if guard
                            .incoming_prompt
                            .as_ref()
                            .map(|p| p.request_id == request_id)
                            .unwrap_or(false)
                        {
                            guard.incoming_prompt = None;
                            guard.pending_accept_cancel = None;
                            guard.pending_accept_response.take()
                        } else {
                            None
                        }
                    };
                    if let Some(tx) = response {
                        self.handle.spawn(async move {
                            let _ = tx.send(false);
                        });
                    }
                    self.unmount(&Id::IncomingCallDialog);
                }
                CoreEvent::LogLine(line) => {
                    self.lock_state().push_log(line);
                }
            },
        }

        None
    }
}

async fn resolve_core_contact(
    profile_id: String,
    active_peer: Option<String>,
    contacts: Vec<ContactMeta>,
    secret_store: SecretStore,
) -> Option<Contact> {
    let active_peer = active_peer?;

    if let Some(meta) = contacts.iter().find(|contact| contact.id == active_peer) {
        let peer_id = match secret_store.load_contact_peer_id(&profile_id, &meta.id).await {
            Ok(peer_id) => peer_id,
            Err(error) => {
                log::error!("failed to load peer id for active contact {}: {error}", meta.id);
                return None;
            }
        };
        return Contact::from_parts(meta.id.clone(), meta.nickname.clone(), peer_id)
            .map_err(|error| {
                log::error!("failed to build core contact {}: {error:?}", meta.id);
                error
            })
            .ok();
    }

    Contact::new(active_peer.clone(), active_peer)
        .map_err(|error| {
            log::error!("active peer is not a valid peer id: {error:?}");
            error
        })
        .ok()
}

