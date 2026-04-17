//! Volatile, in-memory application state held by [`Model`](crate::app::model::Model).
//!
//! Persisted state lives in [`crate::storage::config::AppConfig`]; this struct
//! mirrors only the bits required to render the UI and to coordinate
//! interactions with `telepathy-core`.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use telepathy_core::types::{CallState, SessionStatus, Statistics};
use tokio::sync::{oneshot, watch};

use crate::events::VolumeKind;
use crate::storage::config::{ContactMeta, ProfileMeta, RoomConfig};

/// Maximum number of log lines retained in [`AppState::log_lines`].
pub const LOG_RING_CAPACITY: usize = 1000;

/// Per-message chat entry rendered by the chat pane.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatEntry {
    pub peer_id: String,
    pub text: String,
}

/// State backing the modal "incoming call" dialog.
#[derive(Debug, Clone)]
pub struct IncomingPromptState {
    pub request_id: String,
    pub contact_id: String,
}

/// Volatile application state shared between the [`Model`](crate::app::model::Model)
/// and the `telepathy-core` callback closures (via `Arc<Mutex<_>>`).
pub struct AppState {
    pub active_profile: ProfileMeta,
    pub contacts: Vec<ContactMeta>,
    pub rooms: Vec<RoomConfig>,
    pub sessions: HashMap<String, Arc<SessionStatus>>,
    pub call_state: Arc<CallState>,
    pub active_peer: Option<String>,
    pub muted: bool,
    pub deafened: bool,
    pub manager_active: bool,
    pub manager_restartable: bool,
    pub chat_messages: Vec<ChatEntry>,
    pub statistics: Arc<Statistics>,
    pub log_lines: VecDeque<String>,
    pub incoming_prompt: Option<IncomingPromptState>,
    pub pending_accept_response: Option<oneshot::Sender<bool>>,
    pub pending_accept_cancel: Option<watch::Receiver<bool>>,
    pub volume_debounce: HashMap<VolumeKind, Instant>,
}

impl AppState {
    /// Build initial state for the supplied active profile.
    pub fn new(active_profile: ProfileMeta) -> Self {
        let contacts = active_profile.contacts.clone();
        let rooms = active_profile.rooms.clone();
        Self {
            active_profile,
            contacts,
            rooms,
            sessions: HashMap::new(),
            call_state: Arc::new(CallState::Waiting),
            active_peer: None,
            muted: false,
            deafened: false,
            manager_active: false,
            manager_restartable: false,
            chat_messages: Vec::new(),
            statistics: Arc::new(Statistics::default()),
            log_lines: VecDeque::with_capacity(LOG_RING_CAPACITY),
            incoming_prompt: None,
            pending_accept_response: None,
            pending_accept_cancel: None,
            volume_debounce: HashMap::new(),
        }
    }

    /// Append a log line, evicting the oldest entry once the ring is full.
    pub fn push_log(&mut self, line: String) {
        if self.log_lines.len() >= LOG_RING_CAPACITY {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(line);
    }

    /// Switch the active profile, replacing derived contact/room views.
    pub fn replace_active_profile(&mut self, profile: ProfileMeta) {
        self.contacts = profile.contacts.clone();
        self.rooms = profile.rooms.clone();
        self.active_profile = profile;
        self.chat_messages.clear();
        self.sessions.clear();
        self.active_peer = None;
        self.incoming_prompt = None;
        self.pending_accept_response = None;
        self.pending_accept_cancel = None;
    }
}
