use crate::error::Error;
use bincode::{Decode, Encode};

#[derive(Debug, Decode, Encode, Clone)]
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

#[derive(Debug, Decode, Encode, Clone, Default)]
pub(crate) struct AudioHeader {
    pub(crate) channels: u32,
    pub(crate) sample_rate: u32,
    pub(crate) sample_format: String,
    pub(crate) codec_enabled: bool,
    pub(crate) vbr: bool,
    pub(crate) residual_bits: f64,
}

impl AudioHeader {
    pub(crate) fn is_valid(&self) -> bool {
        self.channels < 10 && self.sample_rate < 128_000 && self.sample_format != "unknown"
    }
}

#[derive(Debug, Decode, Encode, Clone)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) data: Vec<u8>,
}

impl From<&[u8]> for AudioHeader {
    fn from(value: &[u8]) -> Self {
        let bits_per_sample = u16::from_le_bytes([value[34], value[35]]);
        let audio_format = u16::from_le_bytes([value[20], value[21]]);

        let sample_format = match (audio_format, bits_per_sample) {
            (1, 8) => "u8",
            (1, 16) => "i16",
            (1, 32) => "i32",
            (3, 32) => "f32",
            (3, 64) => "f64",
            _ => "unknown",
        };

        Self {
            channels: u16::from_le_bytes([value[22], value[23]]) as u32,
            sample_rate: u32::from_le_bytes([value[24], value[25], value[26], value[27]]),
            sample_format: String::from(sample_format),
            codec_enabled: false,
            vbr: false,
            residual_bits: 0_f64,
        }
    }
}
