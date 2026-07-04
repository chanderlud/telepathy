#[cfg(target_family = "wasm")]
use flutter_rust_bridge::for_generated::futures::channel::oneshot::Canceled;
use iroh::KeyParsingError;
#[cfg(not(target_family = "wasm"))]
use iroh::endpoint::InvalidSocketAddr;
use iroh::endpoint::{BindError, ConnectionError};
use std::fmt::{Display, Formatter};
use std::net::AddrParseError;
use telepathy_audio::devices::DeviceError;
use tokio::task::JoinError;
use tokio::time::error::Elapsed;

/// Reason used in [`ProtocolMessage::error_goodbye`] and
/// [`AudioStreamError::remote_reason`] when the underlying error is an audio
/// device error. Defined once and reused so the wire-level reason and the
/// `Display` wording stay in sync.
pub(crate) const AUDIO_DEVICE_ERROR_REMOTE_REASON: &str = "audio device error";

/// generic error type for Telepathy
#[derive(Debug)]
pub struct Error {
    pub(crate) kind: ErrorKind,
}

#[derive(Debug)]
pub(crate) enum ErrorKind {
    Io(std::io::Error),
    MessageCodec(speedy::Error),
    KanalSend(kanal::SendError),
    KanalReceive(kanal::ReceiveError),
    KanalClose(kanal::CloseError),
    Join(JoinError),
    AddrParse(AddrParseError),
    Timeout(Elapsed),
    #[cfg(target_family = "wasm")]
    WasmTimeout(wasmtimer::tokio::error::Elapsed),
    AudioError(telepathy_audio::Error),
    AudioInputStream(String),
    AudioOutputStream(String),
    #[cfg(target_family = "wasm")]
    Canceled(Canceled),
    DeviceError(DeviceError),
    BindError(BindError),
    KeyParsing(KeyParsingError),
    Connection(ConnectionError),
    #[cfg(not(target_family = "wasm"))]
    InvalidSocketAddr(InvalidSocketAddr),
    Poison(&'static str),
    InvalidContactFormat,
    TransportSend,
    TransportRecv,
    #[cfg(not(target_family = "wasm"))]
    InvalidEncoder,
    RoomStateMissing,
    NoEncoderAvailable,
    NoIdentityAvailable,
    CallAlreadyActive,
    SessionStopped,
    NoSessionForContact,
    ManagerRestartDuringCall,
    AttachmentsTooLarge,
    MpscSend,
    InvalidModel,
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self {
            kind: ErrorKind::Io(err),
        }
    }
}

impl From<speedy::Error> for Error {
    fn from(err: speedy::Error) -> Self {
        Self {
            kind: ErrorKind::MessageCodec(err),
        }
    }
}

impl From<kanal::SendError> for Error {
    fn from(err: kanal::SendError) -> Self {
        Self {
            kind: ErrorKind::KanalSend(err),
        }
    }
}

impl From<kanal::ReceiveError> for Error {
    fn from(err: kanal::ReceiveError) -> Self {
        Self {
            kind: ErrorKind::KanalReceive(err),
        }
    }
}

impl From<kanal::CloseError> for Error {
    fn from(err: kanal::CloseError) -> Self {
        Self {
            kind: ErrorKind::KanalClose(err),
        }
    }
}

impl From<JoinError> for Error {
    fn from(err: JoinError) -> Self {
        Self {
            kind: ErrorKind::Join(err),
        }
    }
}

impl From<AddrParseError> for Error {
    fn from(err: AddrParseError) -> Self {
        Self {
            kind: ErrorKind::AddrParse(err),
        }
    }
}

impl From<Elapsed> for Error {
    fn from(err: Elapsed) -> Self {
        Self {
            kind: ErrorKind::Timeout(err),
        }
    }
}

#[cfg(target_family = "wasm")]
impl From<Canceled> for Error {
    fn from(err: Canceled) -> Self {
        Self {
            kind: ErrorKind::Canceled(err),
        }
    }
}

#[cfg(target_family = "wasm")]
impl From<wasmtimer::tokio::error::Elapsed> for Error {
    fn from(err: wasmtimer::tokio::error::Elapsed) -> Self {
        Self {
            kind: ErrorKind::WasmTimeout(err),
        }
    }
}

impl From<telepathy_audio::Error> for Error {
    fn from(err: telepathy_audio::Error) -> Self {
        Self {
            kind: ErrorKind::AudioError(err),
        }
    }
}

impl From<DeviceError> for Error {
    fn from(err: DeviceError) -> Self {
        Self {
            kind: ErrorKind::DeviceError(err),
        }
    }
}

impl From<BindError> for Error {
    fn from(err: BindError) -> Self {
        Self {
            kind: ErrorKind::BindError(err),
        }
    }
}

impl From<KeyParsingError> for Error {
    fn from(err: KeyParsingError) -> Self {
        Self {
            kind: ErrorKind::KeyParsing(err),
        }
    }
}

impl From<ConnectionError> for Error {
    fn from(err: ConnectionError) -> Self {
        Self {
            kind: ErrorKind::Connection(err),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl From<InvalidSocketAddr> for Error {
    fn from(err: InvalidSocketAddr) -> Self {
        Self {
            kind: ErrorKind::InvalidSocketAddr(err),
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self { kind }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self.kind {
                ErrorKind::Io(ref err) => format!("IO error: {}", err),
                ErrorKind::MessageCodec(ref err) => format!("Message codec error: {}", err),
                ErrorKind::KanalSend(ref err) => format!("Kanal send error: {}", err),
                ErrorKind::KanalReceive(ref err) => format!("Kanal receive error: {}", err),
                ErrorKind::KanalClose(ref err) => format!("Kanal close error: {}", err),
                ErrorKind::Join(ref err) => format!("Join error: {}", err),
                ErrorKind::Timeout(_) => "The connection timed out".to_string(),
                #[cfg(target_family = "wasm")]
                ErrorKind::WasmTimeout(_) => "The connection timed out".to_string(),
                ErrorKind::AddrParse(ref err) => err.to_string(),
                ErrorKind::AudioError(ref err) => format!("Audio error: {err}"),
                ErrorKind::AudioInputStream(ref err) => format!("Input stream error: {err}"),
                ErrorKind::AudioOutputStream(ref err) => format!("Output stream error: {err}"),
                #[cfg(target_family = "wasm")]
                ErrorKind::Canceled(ref err) => format!("Canceled: {}", err),
                ErrorKind::DeviceError(ref err) => format!("Device error: {}", err),
                ErrorKind::BindError(ref err) => format!("Bind error: {}", err),
                ErrorKind::KeyParsing(ref err) => format!("Key parsing error: {}", err),
                ErrorKind::Connection(ref err) => format!("Connection error: {}", err),
                ErrorKind::Poison(msg) => format!("Poison error: {}", msg),
                #[cfg(not(target_family = "wasm"))]
                ErrorKind::InvalidSocketAddr(ref err) => format!("Invalid socket address: {}", err),
                ErrorKind::InvalidContactFormat => "Invalid contact format".to_string(),
                ErrorKind::TransportSend => "Transport failed on send".to_string(),
                ErrorKind::TransportRecv => "Transport failed on receive".to_string(),
                #[cfg(not(target_family = "wasm"))]
                ErrorKind::InvalidEncoder => "Invalid encoder".to_string(),
                ErrorKind::RoomStateMissing => "Room state missing".to_string(),
                ErrorKind::NoEncoderAvailable => "No encoder available".to_string(),
                ErrorKind::NoIdentityAvailable => "No identity available".to_string(),
                ErrorKind::CallAlreadyActive => "A call is already active".to_string(),
                ErrorKind::SessionStopped => "Session stopped".to_string(),
                ErrorKind::NoSessionForContact => "No session found for contact".to_string(),
                ErrorKind::ManagerRestartDuringCall =>
                    "Cannot restart manager while a call is active".to_string(),
                ErrorKind::AttachmentsTooLarge => "Attachments too large".to_string(),
                ErrorKind::MpscSend => "Channel closed (mpsc send failed)".to_string(),
                ErrorKind::InvalidModel => "Invalid RNN model".to_string(),
            }
        )
    }
}

impl Error {
    pub(crate) fn is_session_critical(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::KanalReceive(_) | ErrorKind::TransportRecv | ErrorKind::TransportSend
        )
    }

    pub(crate) fn is_session_stopped(&self) -> bool {
        matches!(self.kind, ErrorKind::SessionStopped)
    }

    pub(crate) fn is_audio_error(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::AudioError(_)
                | ErrorKind::AudioInputStream(_)
                | ErrorKind::AudioOutputStream(_)
                | ErrorKind::DeviceError(_)
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AudioStreamDirection {
    Input,
    Output,
}

#[derive(Debug, Clone)]
pub(crate) struct AudioStreamError {
    direction: AudioStreamDirection,
    message: String,
}

impl AudioStreamError {
    pub(crate) fn input(message: String) -> Self {
        Self {
            direction: AudioStreamDirection::Input,
            message,
        }
    }

    pub(crate) fn output(message: String) -> Self {
        Self {
            direction: AudioStreamDirection::Output,
            message,
        }
    }

    pub(crate) fn user_message(&self) -> String {
        Error::from(self.clone().into_error_kind()).to_string()
    }

    pub(crate) fn remote_reason(&self) -> &'static str {
        AUDIO_DEVICE_ERROR_REMOTE_REASON
    }

    pub(crate) fn into_error_kind(self) -> ErrorKind {
        match self.direction {
            AudioStreamDirection::Input => ErrorKind::AudioInputStream(self.message),
            AudioStreamDirection::Output => ErrorKind::AudioOutputStream(self.message),
        }
    }
}
