//! tuirealm [`Model`] implementation for telepathy-tui.
//!
//! The model owns the tuirealm `Application`, the terminal bridge, the shared
//! [`AppState`], persistent [`AppConfig`] / [`SecretStore`] handles, and a
//! redraw/quit flag. All state mutation in response to a [`Msg`] happens here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use telepathy_core::native::NativeTelepathy;
use telepathy_core::types::{CallState, Contact, SessionStatus};
use tokio::runtime::Handle;
use tuirealm::ratatui::layout::{Constraint, Layout};
use tuirealm::ratatui::widgets::Paragraph;
use tuirealm::terminal::{CrosstermTerminalAdapter, TerminalAdapter, TerminalBridge};
use tuirealm::{Application, AttrValue, Attribute, Sub, SubClause, SubEventClause, Update};

use crate::components::{
    CallControlsData, ConfirmDialog, ConfirmDialogData, ContactsPaneData, IncomingCallDialog,
    IncomingCallDialogData, PlaceholderComponent, SessionBadge, payload_attr,
};
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
    pub pending_confirm: bool,
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
            pending_confirm: false,
        }
    }
}

impl<T> Model<T>
where
    T: TerminalAdapter,
{
    pub fn view(&mut self) {
        self.push_state_to_components();
        let _ = self.terminal.draw(|frame| {
            let area = frame.area();
            if area.width < 80 || area.height < 24 {
                frame.render_widget(Paragraph::new("Terminal too small (min 80x24)"), area);
                return;
            }

            let [main_area, status_area] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
            let [contacts_area, call_area, chat_area] = Layout::horizontal([
                Constraint::Percentage(30),
                Constraint::Percentage(30),
                Constraint::Percentage(40),
            ])
            .areas(main_area);

            self.app.view(&Id::ContactsPane, frame, contacts_area);
            self.app.view(&Id::CallControlsPane, frame, call_area);
            self.app.view(&Id::ChatPane, frame, chat_area);
            self.app.view(&Id::StatusBar, frame, status_area);

            if self.app.mounted(&Id::IncomingCallDialog) {
                self.app.view(&Id::IncomingCallDialog, frame, area);
            }
            if self.app.mounted(&Id::ConfirmDialog) {
                self.app.view(&Id::ConfirmDialog, frame, area);
            }
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

    fn mount_incoming_call_dialog(&mut self) {
        let _ = self.app.umount(&Id::IncomingCallDialog);
        let _ = self.app.mount(
            Id::IncomingCallDialog,
            Box::new(IncomingCallDialog::default()),
            vec![Sub::new(
                SubEventClause::Any,
                SubClause::IsMounted(Id::IncomingCallDialog),
            )],
        );
    }

    fn mount_confirm_dialog(&mut self, message: String, confirm_msg: Msg) {
        let _ = self.app.umount(&Id::ConfirmDialog);
        let _ = self.app.mount(
            Id::ConfirmDialog,
            Box::new(ConfirmDialog::default()),
            vec![Sub::new(
                SubEventClause::Any,
                SubClause::IsMounted(Id::ConfirmDialog),
            )],
        );
        let _ = self.app.attr(
            &Id::ConfirmDialog,
            Attribute::Content,
            payload_attr(ConfirmDialogData {
                message,
                confirm_msg,
            }),
        );
    }

    fn push_state_to_components(&mut self) {
        let (
            profile_name,
            call_active,
            call_state_label,
            contacts,
            rooms,
            sessions,
            muted,
            deafened,
            manager_active,
            manager_restartable,
            chat_messages,
            active_peer,
            incoming_prompt,
        ) = {
            let guard = self.lock_state();
            let call_active = !matches!(guard.call_state.as_ref(), CallState::Waiting);
            let call_state_label = match guard.call_state.as_ref() {
                CallState::Connected => "Connected".to_string(),
                CallState::Waiting => "Waiting".to_string(),
                CallState::RoomJoin(name) => format!("RoomJoin({name})"),
                CallState::RoomLeave(name) => format!("RoomLeave({name})"),
                CallState::CallEnded(peer, timeout) => format!("Ended({peer}, timeout={timeout})"),
            };
            (
                guard.active_profile.nickname.clone(),
                call_active,
                call_state_label,
                guard.contacts.clone(),
                guard
                    .rooms
                    .iter()
                    .map(|room| room.nickname.clone())
                    .collect::<Vec<_>>(),
                guard.sessions.clone(),
                guard.muted,
                guard.deafened,
                guard.manager_active,
                guard.manager_restartable,
                guard.chat_messages.clone(),
                guard.active_peer.clone(),
                guard.incoming_prompt.clone(),
            )
        };
        let (output_vol, input_vol, sound_vol, sensitivity) = {
            let config_guard = self.lock_config();
            (
                config_guard.preferences.output_volume_db,
                config_guard.preferences.input_volume_db,
                config_guard.preferences.sound_volume_db,
                config_guard.preferences.input_sensitivity_db,
            )
        };

        let call_state_label = match call_state_label.as_str() {
            "Connected" => "Connected".to_string(),
            "Waiting" => "Waiting".to_string(),
            _ => call_state_label,
        };
        let status_text = format!(
            "[Profile: {}]  [Status: {}]  [s] Settings  [l] Logs  [q] Quit",
            profile_name, call_state_label
        );
        let _ = self
            .app
            .attr(&Id::StatusBar, Attribute::Text, AttrValue::String(status_text));

        let mut session_badges = HashMap::new();
        for (peer, status) in sessions {
            let badge = match status.as_ref() {
                SessionStatus::Connecting => SessionBadge::Connecting,
                SessionStatus::Connected { relayed: true, .. } => SessionBadge::ConnectedRelayed,
                SessionStatus::Connected { relayed: false, .. } => SessionBadge::ConnectedDirect,
                SessionStatus::Inactive | SessionStatus::Unknown => SessionBadge::Inactive,
            };
            session_badges.insert(peer, badge);
        }
        let _ = self.app.attr(
            &Id::ContactsPane,
            Attribute::Content,
            payload_attr(ContactsPaneData {
                contacts,
                rooms,
                sessions: session_badges,
                call_active,
            }),
        );

        let _ = self.app.attr(
            &Id::CallControlsPane,
            Attribute::Content,
            payload_attr(CallControlsData {
                muted,
                deafened,
                call_active,
                manager_active,
                manager_restartable,
                output_vol,
                input_vol,
                sound_vol,
                sensitivity,
            }),
        );

        let _ = self.app.attr(
            &Id::ChatPane,
            Attribute::Content,
            payload_attr(crate::components::ChatPaneData {
                entries: chat_messages,
                active_peer,
                call_active,
            }),
        );

        if self.app.mounted(&Id::IncomingCallDialog)
            && let Some(prompt) = incoming_prompt
        {
            let nickname = self
                .lock_state()
                .contacts
                .iter()
                .find(|contact| contact.id == prompt.contact_id)
                .map(|contact| contact.nickname.clone())
                .unwrap_or_else(|| prompt.contact_id.clone());
            let _ = self.app.attr(
                &Id::IncomingCallDialog,
                Attribute::Content,
                payload_attr(IncomingCallDialogData {
                    request_id: prompt.request_id,
                    contact_name: nickname,
                }),
            );
        }
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
            Msg::None => {
                if self.app.mounted(&Id::ConfirmDialog) {
                    self.pending_confirm = false;
                    self.unmount(&Id::ConfirmDialog);
                }
            }

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
            Msg::OpenLogs => {
                self.mount_overlay(Id::LogsOverlay);
            }
            Msg::CloseLogs => {
                self.unmount(&Id::LogsOverlay);
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
                if self.pending_confirm && self.app.mounted(&Id::ConfirmDialog) {
                    self.pending_confirm = false;
                    self.unmount(&Id::ConfirmDialog);
                } else {
                    self.pending_confirm = true;
                    let nickname = self
                        .lock_state()
                        .contacts
                        .iter()
                        .find(|contact| contact.id == contact_id)
                        .map(|contact| contact.nickname.clone())
                        .unwrap_or_else(|| contact_id.clone());
                    self.mount_confirm_dialog(
                        format!("Delete contact \"{nickname}\"?"),
                        Msg::ContactDelete(contact_id),
                    );
                    return None;
                }
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
                if self.pending_confirm && self.app.mounted(&Id::ConfirmDialog) {
                    self.pending_confirm = false;
                    self.unmount(&Id::ConfirmDialog);
                } else {
                    self.pending_confirm = true;
                    self.mount_confirm_dialog(
                        format!("Delete room \"{nickname}\"?"),
                        Msg::RoomDelete(nickname),
                    );
                    return None;
                }
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
                CoreEvent::IncomingCall {
                    request_id,
                    contact_id,
                    ..
                } => {
                    self.mount_incoming_call_dialog();
                    let nickname = {
                        let guard = self.lock_state();
                        guard
                            .contacts
                            .iter()
                            .find(|contact| contact.id == contact_id)
                            .map(|contact| contact.nickname.clone())
                            .unwrap_or(contact_id)
                    };
                    let _ = self.app.attr(
                        &Id::IncomingCallDialog,
                        Attribute::Content,
                        payload_attr(IncomingCallDialogData {
                            request_id,
                            contact_name: nickname,
                        }),
                    );
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

