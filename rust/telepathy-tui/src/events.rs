//! Event and message types exchanged between `telepathy-core` callbacks,
//! the tuirealm event listener, components, and the [`Model`](crate::app::model::Model).
//!
//! - [`CoreEvent`] is the user-event type carried by the tuirealm event loop
//!   (delivered via [`crate::app::port::CoreEventPort`]).
//! - [`Msg`] is the message type produced by components and consumed by
//!   [`Model::update`](crate::app::model::Model::update).
//! - [`Id`] enumerates every component the tuirealm `Application` may mount.

use std::sync::Arc;

use telepathy_core::types::{CallState, SessionStatus, Statistics};

/// User events emitted by `telepathy-core` callbacks and forwarded to the
/// tuirealm event loop via the [`CoreEventPort`](crate::app::port::CoreEventPort).
///
/// The non-`Clone`/`Eq`/`Debug` core types are wrapped in [`Arc`] so the variant
/// can be cheaply cloned and routed through the tuirealm worker. `Debug` and
/// `PartialEq` are implemented by hand because some of those wrapped core
/// types (notably `Statistics`) do not implement them upstream.
#[derive(Clone)]
pub enum CoreEvent {
    CallStateChanged(Arc<CallState>),
    SessionStatusChanged(String, Arc<SessionStatus>),
    MessageReceived(String, String),
    StatisticsUpdated(Arc<Statistics>),
    ManagerActiveChanged(bool, bool),
    IncomingCall {
        request_id: String,
        contact_id: String,
        ringtone: Option<Vec<u8>>,
    },
    IncomingCallCancelled {
        request_id: String,
    },
    LogLine(String),
}

impl std::fmt::Debug for CoreEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CallStateChanged(state) => {
                f.debug_tuple("CallStateChanged").field(&**state).finish()
            }
            Self::SessionStatusChanged(peer, status) => f
                .debug_tuple("SessionStatusChanged")
                .field(peer)
                .field(&**status)
                .finish(),
            Self::MessageReceived(peer, text) => f
                .debug_tuple("MessageReceived")
                .field(peer)
                .field(text)
                .finish(),
            Self::StatisticsUpdated(_) => f.debug_tuple("StatisticsUpdated").field(&"..").finish(),
            Self::ManagerActiveChanged(active, restartable) => f
                .debug_tuple("ManagerActiveChanged")
                .field(active)
                .field(restartable)
                .finish(),
            Self::IncomingCall {
                request_id,
                contact_id,
                ringtone,
            } => f
                .debug_struct("IncomingCall")
                .field("request_id", request_id)
                .field("contact_id", contact_id)
                .field("ringtone_bytes", &ringtone.as_ref().map(|r| r.len()))
                .finish(),
            Self::IncomingCallCancelled { request_id } => f
                .debug_struct("IncomingCallCancelled")
                .field("request_id", request_id)
                .finish(),
            Self::LogLine(line) => f.debug_tuple("LogLine").field(line).finish(),
        }
    }
}

// tuirealm's event listener requires `UserEvent: Eq + PartialEq + Clone`.
// The wrapped core types do not implement `PartialEq`/`Eq`, so we compare by
// discriminant only — this matches the pattern used by the tuirealm
// `async_ports` example for `UserEvent`.
impl PartialEq for CoreEvent {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl Eq for CoreEvent {}

/// Identifies which audio gain a [`Msg::VolumeChanged`] applies to.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum VolumeKind {
    Output,
    Input,
    Sound,
    InputSensitivity,
}

/// Identifies a single field on
/// [`crate::storage::config::Preferences`] that may be mutated through a
/// [`Msg::SettingChanged`].
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum SettingKey {
    RelayAddress,
    RelayId,
    OutputVolumeDb,
    InputVolumeDb,
    SoundVolumeDb,
    InputSensitivityDb,
    OutputDeviceId,
    InputDeviceId,
    UseDenoise,
    DenoiseModel,
    PlayCustomRingtones,
    CustomRingtonePath,
    EfficiencyMode,
    CodecEnabled,
    CodecVbr,
    CodecResidualBits,
}

/// Type-erased value associated with a [`SettingKey`].
#[derive(Debug, Clone)]
pub enum SettingValue {
    Str(String),
    Float(f32),
    Bool(bool),
    OptStr(Option<String>),
}

/// Application messages produced by components and consumed by
/// [`Model::update`](crate::app::model::Model::update).
#[derive(Debug, Clone)]
pub enum Msg {
    // Navigation
    FocusContacts,
    FocusCallControls,
    FocusChat,
    OpenSettings,
    CloseSettings,
    Quit,

    // Contacts
    ContactSelected(String),
    ContactAdd(String, String),
    ContactDelete(String),
    ContactRename(String, String),

    // Rooms
    RoomSelected(String),
    RoomAdd(String, Vec<String>),
    RoomDelete(String),
    RoomJoin(String),

    // Call
    StartCall,
    EndCall,
    ToggleMute,
    ToggleDeafen,
    VolumeChanged(VolumeKind, f32),
    AudioTestToggle,
    RestartManager,

    // Chat
    SendMessage(String),

    // Settings
    SettingChanged(SettingKey, SettingValue),

    // Profiles
    ProfileCreate(String),
    ProfileDelete(String),
    ProfileSwitch(String),

    // Incoming call response
    AcceptCall {
        request_id: String,
        accepted: bool,
    },

    // Forwarding of a tuirealm `Event::User`
    CoreEvent(CoreEvent),

    None,
}

// `tuirealm::Update` requires `Msg: PartialEq`. `SettingValue::Float(f32)` is
// not `Eq`, so we compare by discriminant only — variant identity is the
// only thing tuirealm needs in practice (e.g. for subscription matching).
impl PartialEq for Msg {
    fn eq(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// Component identifiers used by the tuirealm `Application`.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub enum Id {
    ContactsPane,
    CallControlsPane,
    ChatPane,
    StatusBar,
    CoreEventBridge,
    SettingsOverlay,
    IncomingCallDialog,
    ConfirmDialog,
    LogsOverlay,
}
