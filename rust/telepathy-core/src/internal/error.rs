#[cfg(target_family = "wasm")]
use flutter_rust_bridge::for_generated::futures::channel::oneshot::Canceled;
use std::fmt::{Display, Formatter};
use std::net::AddrParseError;
use iroh::endpoint::{BindError, ConnectionError};
use iroh::KeyParsingError;
use telepathy_audio::devices::DeviceError;
use tokio::task::JoinError;
use tokio::time::error::Elapsed;

/// generic error type for Telepathy
#[derive(Debug)]
pub(crate) struct Error {
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
    #[cfg(target_family = "wasm")]
    Canceled(Canceled),
    DeviceError(DeviceError),
    BindError(BindError),
    KeyParsing(KeyParsingError),
    Connection(ConnectionError),
    InvalidContactFormat,
    TransportSend,
    TransportRecv,
    UnexpectedSwarmEvent,
    SwarmBuild,
    SwarmEnded,
    #[cfg(not(target_family = "wasm"))]
    InvalidEncoder,
    RoomStateMissing,
    StreamsEnded,
    NoEncoderAvailable,
    NoIdentityAvailable,
    NoStream,
    CallAlreadyActive,
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
            kind: ErrorKind::BindError(err)
        }
    }
}

impl From<KeyParsingError> for Error {
    fn from(err: KeyParsingError) -> Self {
        Self {
            kind: ErrorKind::KeyParsing(err)
        }
    }
}

impl From<ConnectionError> for Error {
    fn from(err: ConnectionError) -> Self {
        Self {
            kind: ErrorKind::Connection(err)
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
                #[cfg(target_family = "wasm")]
                ErrorKind::Canceled(ref err) => format!("Canceled: {}", err),
                ErrorKind::DeviceError(ref err) => format!("Device error: {}", err),
                ErrorKind::BindError(ref err) => format!("Bind error: {}", err),
                ErrorKind::KeyParsing(ref err) => format!("Key parsing error: {}", err),
                ErrorKind::Connection(ref err) => format!("Connection error: {}", err),
                ErrorKind::InvalidContactFormat => "Invalid contact format".to_string(),
                ErrorKind::TransportSend => "Transport failed on send".to_string(),
                ErrorKind::TransportRecv => "Transport failed on receive".to_string(),
                ErrorKind::UnexpectedSwarmEvent => "Unexpected swarm event".to_string(),
                ErrorKind::SwarmBuild => "Swarm build error".to_string(),
                ErrorKind::SwarmEnded => "Swarm ended".to_string(),
                #[cfg(not(target_family = "wasm"))]
                ErrorKind::InvalidEncoder => "Invalid encoder".to_string(),
                ErrorKind::RoomStateMissing => "Room state missing".to_string(),
                ErrorKind::StreamsEnded => "Streams ended".to_string(),
                ErrorKind::NoEncoderAvailable => "No encoder available".to_string(),
                ErrorKind::NoIdentityAvailable => "No identity available".to_string(),
                ErrorKind::NoStream => "Did not get a stream".to_string(),
                ErrorKind::CallAlreadyActive => "A call is already active".to_string(),
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

    pub(crate) fn is_audio_error(&self) -> bool {
        matches!(self.kind, ErrorKind::DeviceError(_))
    }
}
