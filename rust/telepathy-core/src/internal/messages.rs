use crate::internal::error::Error;
use crate::internal::state::EarlyCallState;
use crate::internal::video::VideoControl;
use iroh::PublicKey;
use iroh::endpoint::Connection;
use serde::Serialize;
use speedy::{Readable, Writable};
use tokio::sync::mpsc::UnboundedSender;
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
    Video {
        control: VideoControl,
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

#[cfg(test)]
mod tests {
    use super::ProtocolMessage;
    use crate::internal::video::{
        VideoCodec, VideoControl, VideoMediaDescriptor, VideoRejectReason, VideoSessionId,
        VideoTerminalReason,
    };
    use speedy::{Readable, Writable};

    #[test]
    fn video_controls_round_trip_with_the_initiator_identity() {
        let session_id = VideoSessionId::new();
        let controls = [
            VideoControl::offer(
                session_id,
                VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080),
            ),
            VideoControl::ready(session_id),
            VideoControl::reject(session_id, VideoRejectReason::UnsupportedCodec),
            VideoControl::stop(session_id, VideoTerminalReason::Stopped),
        ];

        for control in controls {
            let message = ProtocolMessage::Video { control };
            let encoded = message.write_to_vec().expect("control encodes");
            let decoded = ProtocolMessage::read_from_buffer(&encoded).expect("control decodes");
            let ProtocolMessage::Video { control: decoded } = decoded else {
                panic!("video control must remain a video message");
            };
            assert_eq!(decoded, control);
            assert_eq!(decoded.session_id(), session_id);
        }
    }
}
