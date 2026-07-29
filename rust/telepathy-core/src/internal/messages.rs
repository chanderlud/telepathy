use crate::internal::error::Error;
use crate::internal::state::EarlyCallState;
use iroh::PublicKey;
use iroh::endpoint::Connection;
use serde::Serialize;
use speedy::{Readable, Writable};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Canonical reasons for a [`ProtocolMessage::Goodbye`]. Wire vocabulary
/// stays canonical; user-facing rendering lives in `CallEndMessage`.
#[derive(Readable, Writable, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoodbyeReason {
    SessionStopped,
    AudioDeviceError,
    Error,
    None,
}

impl std::fmt::Display for GoodbyeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{:?}", self))
    }
}

impl From<&Error> for GoodbyeReason {
    fn from(error: &Error) -> Self {
        if error.is_session_stopped() {
            Self::SessionStopped
        } else if error.is_audio_error() {
            Self::AudioDeviceError
        } else {
            Self::Error
        }
    }
}

#[derive(Readable, Writable, Debug, Clone)]
pub(crate) enum ProtocolMessage {
    Hello {
        ringtone: Option<Vec<u8>>,
        audio_header: AudioHeader,
        room_hash: Option<u64>,
    },
    HelloAck {
        audio_header: AudioHeader,
    },
    Reject,
    Busy,
    Goodbye {
        reason: GoodbyeReason,
    },
    Chat {
        text: String,
        attachments: Vec<Attachment>,
    },
    KeepAlive,
    ScreenshareHeader {
        encoder_name: String,
    },
}

impl ProtocolMessage {
    pub(crate) fn error_goodbye(error: &Error) -> Self {
        Self::Goodbye {
            reason: GoodbyeReason::from(error),
        }
    }

    pub(crate) fn goodbye() -> Self {
        Self::Goodbye {
            reason: GoodbyeReason::None,
        }
    }
}

#[derive(Readable, Writable, Debug, Clone, Default)]
pub(crate) struct AudioHeader {
    pub(crate) sample_rate: u32,
    pub(crate) codec_enabled: bool,
    pub(crate) vbr: bool,
    pub(crate) residual_bits: f64,
}

impl AudioHeader {
    pub(crate) fn is_valid(&self) -> bool {
        self.sample_rate < 128_000
            && self.sample_rate > 8_000
            && self.residual_bits <= 8_f64
            && self.residual_bits >= 2_f64
    }
}

#[derive(Readable, Writable, Debug, Clone, Serialize)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) data: Vec<u8>,
}

pub(crate) enum RoomMessage {
    Join {
        /// established audio transport
        connection: Connection,

        /// established early call state
        state: EarlyCallState,

        /// ID for the corresponding session
        session_id: Uuid,

        terminal_sender: UnboundedSender<RoomControl>,

        admission_sender: oneshot::Sender<RoomJoinAdmission>,
    },
    Leave {
        peer: PublicKey,
        /// [`Connection::stable_id`] for the transport being torn down.
        connection_id: usize,
    },
}

pub(crate) enum RoomControl {
    Goodbye(GoodbyeReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoomJoinAdmission {
    Admitted,
    Aborted,
}

#[derive(Debug)]
pub(crate) struct StartScreenshare {
    pub(crate) peer: PublicKey,
    pub(crate) header: Option<ProtocolMessage>,
    pub(crate) connection: Connection,
}

impl StartScreenshare {
    pub(crate) fn new_sender(peer: PublicKey, connection: Connection) -> Self {
        Self {
            peer,
            header: None,
            connection,
        }
    }

    pub(crate) fn new_receiver(
        peer: PublicKey,
        message: ProtocolMessage,
        connection: Connection,
    ) -> Self {
        Self {
            peer,
            header: Some(message),
            connection,
        }
    }
}
