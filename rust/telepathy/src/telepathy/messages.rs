use crate::error::Error;
use speedy::{Readable, Writable};

#[derive(Readable, Writable, Debug, Clone)]
pub(crate) enum Message {
    Hello {
        ringtone: Option<Vec<u8>>,
        audio_header: AudioHeader,
        room_hash: Option<Vec<u8>>,
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

impl Message {
    pub(crate) fn error_goodbye(error: &Error) -> Self {
        Self::Goodbye {
            reason: Some(
                if error.is_audio_error() {
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
        self.sample_rate < 128_000 && self.residual_bits <= 8_f64 && self.residual_bits >= 1_f64
    }
}

#[derive(Readable, Writable, Debug, Clone)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) data: Vec<u8>,
}
