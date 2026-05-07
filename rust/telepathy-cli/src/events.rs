use serde::Serialize;
use telepathy_core::types::{CallState, ChatMessage, SessionStatus, Statistics};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ready {
        version: String,
    },
    ManagerActive {
        active: bool,
        restartable: bool,
    },
    SessionStatus {
        peer: String,
        status: SessionStatus,
    },
    CallState {
        state: CallState,
    },
    Statistics {
        input_level: f32,
        output_level: f32,
        latency: usize,
        upload_bandwidth: usize,
        download_bandwidth: usize,
        loss: usize,
    },
    MessageReceived {
        #[serde(flatten)]
        message: ChatMessage,
    },
    ScreenshareStarted {
        sender: bool,
    },
    AcceptCallPrompt {
        request_id: String,
        contact_id: String,
        has_ringtone: bool,
    },
    AcceptCallCanceled {
        request_id: String,
    },
    Error {
        id: Option<String>,
        message: String,
    },
}

impl From<Statistics> for Event {
    fn from(value: Statistics) -> Self {
        Self::Statistics {
            input_level: value.input_level,
            output_level: value.output_level,
            latency: value.latency,
            upload_bandwidth: value.upload_bandwidth,
            download_bandwidth: value.download_bandwidth,
            loss: value.loss,
        }
    }
}

impl From<ChatMessage> for Event {
    fn from(value: ChatMessage) -> Self {
        Self::MessageReceived { message: value }
    }
}
