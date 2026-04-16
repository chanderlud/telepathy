use crate::BehaviourEvent;
#[cfg(target_family = "wasm")]
use flutter_rust_bridge::for_generated::futures::channel::oneshot::Canceled;
use libp2p::identity::{DecodingError, ParseError};
use libp2p::swarm::{DialError, SwarmEvent};
use libp2p::{TransportBuilderError, TransportError};
use libp2p_stream::{AlreadyRegistered, OpenStreamError};
use std::fmt::{Display, Formatter};
use std::net::AddrParseError;
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
    IdentityDecode(DecodingError),
    OpenStream(OpenStreamError),
    Dial(DialError),
    IdentityParse(ParseError),
    Transport(TransportError<std::io::Error>),
    AlreadyRegistered(AlreadyRegistered),
    AudioError(telepathy_audio::Error),
    #[cfg(target_family = "wasm")]
    Canceled(Canceled),
    TransportBuildError(TransportBuilderError),
    DeviceError(DeviceError),
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

impl From<DecodingError> for Error {
    fn from(err: DecodingError) -> Self {
        Self {
            kind: ErrorKind::IdentityDecode(err),
        }
    }
}

impl From<OpenStreamError> for Error {
    fn from(err: OpenStreamError) -> Self {
        Self {
            kind: ErrorKind::OpenStream(err),
        }
    }
}

impl From<DialError> for Error {
    fn from(err: DialError) -> Self {
        Self {
            kind: ErrorKind::Dial(err),
        }
    }
}

impl From<ParseError> for Error {
    fn from(err: ParseError) -> Self {
        Self {
            kind: ErrorKind::IdentityParse(err),
        }
    }
}

impl From<SwarmEvent<BehaviourEvent>> for Error {
    fn from(_: SwarmEvent<BehaviourEvent>) -> Self {
        Self {
            kind: ErrorKind::UnexpectedSwarmEvent,
        }
    }
}

impl From<TransportError<std::io::Error>> for Error {
    fn from(err: TransportError<std::io::Error>) -> Self {
        Self {
            kind: ErrorKind::Transport(err),
        }
    }
}

impl From<AlreadyRegistered> for Error {
    fn from(err: AlreadyRegistered) -> Self {
        Self {
            kind: ErrorKind::AlreadyRegistered(err),
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

impl From<TransportBuilderError> for Error {
    fn from(err: TransportBuilderError) -> Self {
        Self {
            kind: ErrorKind::TransportBuildError(err),
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
                ErrorKind::IdentityDecode(ref err) => format!("Identity decode error: {}", err),
                ErrorKind::OpenStream(ref err) => format!("Open stream error: {}", err),
                ErrorKind::Dial(ref err) => format!("Dial error: {}", err),
                ErrorKind::IdentityParse(ref err) => format!("Identity parse error: {}", err),
                ErrorKind::Transport(ref err) => format!("Transport error: {}", err),
                ErrorKind::AlreadyRegistered(ref err) => format!("Already registered: {}", err),
                ErrorKind::AudioError(ref err) => format!("Audio error: {err}"),
                #[cfg(target_family = "wasm")]
                ErrorKind::Canceled(ref err) => format!("Canceled: {}", err),
                ErrorKind::TransportBuildError(ref err) =>
                    format!("Transport build error: {}", err),
                ErrorKind::DeviceError(ref err) => format!("Device error: {}", err),
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

#[derive(Debug)]
pub struct DartError {
    pub message: String,
}

impl From<Error> for DartError {
    fn from(err: Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl From<ErrorKind> for DartError {
    fn from(kind: ErrorKind) -> Self {
        Self {
            message: Error { kind }.to_string(),
        }
    }
}

impl From<String> for DartError {
    fn from(message: String) -> Self {
        Self { message }
    }
}
