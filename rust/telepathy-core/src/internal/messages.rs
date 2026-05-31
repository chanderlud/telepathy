use crate::internal::error::Error;
use crate::internal::state::EarlyCallState;
use iroh::PublicKey;
use iroh::endpoint::Connection;
use serde::Serialize;
use speedy::{Readable, Writable};

pub(crate) const SESSION_STOPPED_REASON: &str = "session stopped";

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
        reason: Option<String>,
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
            reason: Some(
                if error.is_session_stopped() {
                    SESSION_STOPPED_REASON
                } else if error.is_audio_error() {
                    "audio device error"
                } else {
                    "an error occurred"
                }
                .to_string(),
            ),
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
    },
    Leave {
        peer: PublicKey,
        /// [`Connection::stable_id`] for the transport being torn down.
        connection_id: usize,
    },
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
