pub use crate::internal::messages::GoodbyeReason;
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

pub const CALL_END_ALREADY_ACTIVE: &str = "A call is already active";
pub const CALL_END_AUDIO_DEVICE_FAILURE: &str = "Audio device error";
pub const CALL_END_AUDIO_INPUT_FAILURE: &str = "Microphone error";
pub const CALL_END_AUDIO_OUTPUT_FAILURE: &str = "Speaker error";
pub const CALL_END_GENERIC: &str = "The call ended unexpectedly";
pub const CALL_END_SESSION_STOPPED: &str = "The session was stopped";
pub const CALL_END_TIMEOUT: &str = "The connection timed out";

/// generic error type for Telepathy
#[derive(Debug)]
pub struct Error {
    pub(crate) kind: ErrorKind,
}

#[derive(Debug)]
pub enum ErrorKind {
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
pub struct AudioStreamError {
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

    pub(crate) fn remote_reason(&self) -> GoodbyeReason {
        GoodbyeReason::AudioDeviceError
    }

    pub(crate) fn into_error_kind(self) -> ErrorKind {
        match self.direction {
            AudioStreamDirection::Input => ErrorKind::AudioInputStream(self.message),
            AudioStreamDirection::Output => ErrorKind::AudioOutputStream(self.message),
        }
    }
}

/// User-visible copy for a `CallState::CallEnded` emission. All `CallEnded`
/// emissions must flow through this type so internal error wording and
/// `GoodbyeReason` wire strings are converted into stable, user-facing sentences
/// in exactly one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEndMessage {
    text: String,
}

impl CallEndMessage {
    pub fn from_text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Maps each `ErrorKind` to a user-facing sentence; never passes
    /// `error.to_string()` through to the frontend.
    pub fn from_error(error: &Error) -> Self {
        let text = match &error.kind {
            ErrorKind::SessionStopped => CALL_END_SESSION_STOPPED.to_string(),
            ErrorKind::CallAlreadyActive => CALL_END_ALREADY_ACTIVE.to_string(),
            ErrorKind::AudioError(_)
            | ErrorKind::AudioInputStream(_)
            | ErrorKind::AudioOutputStream(_)
            | ErrorKind::DeviceError(_) => CALL_END_AUDIO_DEVICE_FAILURE.to_string(),
            ErrorKind::Timeout(_) => CALL_END_TIMEOUT.to_string(),
            #[cfg(target_family = "wasm")]
            ErrorKind::WasmTimeout(_) => CALL_END_TIMEOUT.to_string(),
            _ => CALL_END_GENERIC.to_string(),
        };
        Self { text }
    }

    /// Audio stream errors render as direction-specific copy (microphone /
    /// speaker); the underlying cpal/driver wording is dropped.
    pub fn from_stream_error(error: &AudioStreamError) -> Self {
        let text = match error.direction {
            AudioStreamDirection::Input => CALL_END_AUDIO_INPUT_FAILURE.to_string(),
            AudioStreamDirection::Output => CALL_END_AUDIO_OUTPUT_FAILURE.to_string(),
        };
        Self { text }
    }

    /// Converts a transport [`GoodbyeReason`] to user-facing copy. Wire reasons
    /// remain canonical transport vocabulary; conversion happens only here.
    ///
    /// `GoodbyeReason::None` produces an empty string so the frontend dialog
    /// guard (`state.field0.isNotEmpty` in `lib/main.dart`) suppresses the
    /// failure dialog and the silent hangup tone plays instead.
    pub fn from_goodbye_reason(reason: GoodbyeReason) -> Self {
        let text = match reason {
            GoodbyeReason::None => String::new(),
            GoodbyeReason::SessionStopped => CALL_END_SESSION_STOPPED.to_string(),
            GoodbyeReason::AudioDeviceError => CALL_END_AUDIO_DEVICE_FAILURE.to_string(),
            GoodbyeReason::Error => CALL_END_GENERIC.to_string(),
        };
        Self { text }
    }

    pub fn into_string(self) -> String {
        self.text
    }
}

/// `nickname` is supplied by the caller; the wire contract deliberately
/// omits the peer nickname, so pulling it would require a re-resolution the dialer
/// has already done locally.
pub fn peer_busy_message(nickname: &str) -> String {
    format!("{nickname} is busy")
}

pub fn peer_no_response_message(nickname: &str) -> String {
    format!("{nickname} did not respond to the call")
}

pub fn peer_not_accepted_message(nickname: &str) -> String {
    format!("{nickname} did not accept the call")
}

/// Peer-facing rejection keyed on a typed [`GoodbyeReason`]. Each variant maps
/// to a natural English sentence so raw transport vocabulary (e.g. "session
/// stopped" or "AudioDeviceError") never reaches the dialer's UI.
pub fn peer_goodbye_reason_message(nickname: &str, reason: GoodbyeReason) -> String {
    match reason {
        GoodbyeReason::SessionStopped => {
            format!("{nickname} did not accept the call because the session was stopped")
        }
        GoodbyeReason::AudioDeviceError => {
            format!("{nickname} did not accept the call because of an audio device problem")
        }
        GoodbyeReason::Error => {
            format!("{nickname} did not accept the call because of an unexpected problem")
        }
        GoodbyeReason::None => format!("{nickname} did not accept the call"),
    }
}

pub fn peer_unexpected_message(nickname: &str) -> String {
    format!("Received an unexpected message from {nickname}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_text_passes_through_user_facing_copy() {
        let message = CallEndMessage::from_text("custom copy");
        assert_eq!(message.into_string(), "custom copy");
    }

    #[test]
    fn from_error_maps_session_stopped() {
        let error: Error = ErrorKind::SessionStopped.into();
        assert_eq!(
            CallEndMessage::from_error(&error).into_string(),
            CALL_END_SESSION_STOPPED
        );
    }

    #[test]
    fn from_error_maps_call_already_active() {
        let error: Error = ErrorKind::CallAlreadyActive.into();
        assert_eq!(
            CallEndMessage::from_error(&error).into_string(),
            CALL_END_ALREADY_ACTIVE
        );
    }

    #[test]
    fn from_error_maps_audio_error_kinds() {
        for kind in [
            ErrorKind::AudioInputStream("anything".to_string()),
            ErrorKind::AudioOutputStream("anything".to_string()),
        ] {
            let error: Error = kind.into();
            assert_eq!(
                CallEndMessage::from_error(&error).into_string(),
                CALL_END_AUDIO_DEVICE_FAILURE
            );
        }
    }

    #[tokio::test]
    async fn from_error_maps_timeout_to_user_facing_copy() {
        // Timeout errors are constructed via `From<Elapsed>` in production; we
        // cannot instantiate `Elapsed` directly, so delegate through `tokio::time::timeout`.
        let error: Error = match tokio::time::timeout(
            std::time::Duration::from_millis(1),
            std::future::pending::<()>(),
        )
        .await
        {
            Ok(_) => unreachable!("future is pending"),
            Err(elapsed) => elapsed.into(),
        };
        assert_eq!(
            CallEndMessage::from_error(&error).into_string(),
            CALL_END_TIMEOUT
        );
    }

    #[test]
    fn from_error_maps_other_kinds_to_generic_copy() {
        // Non-audio, non-session-stopped, non-already-active, non-timeout errors
        // must collapse to the generic copy.
        for kind in [
            ErrorKind::Poison("test poison"),
            ErrorKind::MpscSend,
            ErrorKind::TransportSend,
            ErrorKind::TransportRecv,
            ErrorKind::KanalSend(kanal::SendError::Closed),
        ] {
            let error: Error = kind.into();
            assert_eq!(
                CallEndMessage::from_error(&error).into_string(),
                CALL_END_GENERIC,
                "internal wording for {error:?} leaked to CallEnded"
            );
            // Sanity: the internal `Display` wording must contain text the
            // generic copy does NOT — proves the generic copy is genuinely free
            // of internal wording.
            assert_ne!(error.to_string(), CALL_END_GENERIC);
        }
    }

    #[test]
    fn from_stream_error_distinguishes_input_and_output_directions() {
        let input_error = AudioStreamError::input("cpal driver reset".to_string());
        assert_eq!(
            CallEndMessage::from_stream_error(&input_error).into_string(),
            CALL_END_AUDIO_INPUT_FAILURE,
            "input stream errors must surface as the microphone copy"
        );

        let output_error = AudioStreamError::output("device unplugged".to_string());
        assert_eq!(
            CallEndMessage::from_stream_error(&output_error).into_string(),
            CALL_END_AUDIO_OUTPUT_FAILURE,
            "output stream errors must surface as the speaker copy"
        );

        // Remote-side wire mapping must stay generic — direction-specific copy is
        // local-only and must not leak into the wire payload.
        assert_eq!(input_error.remote_reason(), GoodbyeReason::AudioDeviceError);
        assert_eq!(
            output_error.remote_reason(),
            GoodbyeReason::AudioDeviceError
        );

        // Raw cpal/driver wording must never appear in user-facing copy.
        assert!(
            !CallEndMessage::from_stream_error(&input_error)
                .into_string()
                .contains("cpal driver reset"),
            "raw cpal wording leaked through input CallEndMessage"
        );
        assert!(
            !CallEndMessage::from_stream_error(&output_error)
                .into_string()
                .contains("device unplugged"),
            "raw driver wording leaked through output CallEndMessage"
        );
    }

    #[test]
    fn from_goodbye_reason_converts_to_user_facing_copy() {
        assert_eq!(
            CallEndMessage::from_goodbye_reason(GoodbyeReason::SessionStopped).into_string(),
            CALL_END_SESSION_STOPPED
        );
        assert_eq!(
            CallEndMessage::from_goodbye_reason(GoodbyeReason::AudioDeviceError).into_string(),
            CALL_END_AUDIO_DEVICE_FAILURE
        );
        assert_eq!(
            CallEndMessage::from_goodbye_reason(GoodbyeReason::Error).into_string(),
            CALL_END_GENERIC
        );
        // Normal hangup must render to an EMPTY string so the frontend dialog guard
        // (`state.field0.isNotEmpty` in `lib/main.dart`) stays silent.
        assert_eq!(
            CallEndMessage::from_goodbye_reason(GoodbyeReason::None).into_string(),
            "",
            "normal hangup must not surface as a failure message"
        );
        assert!(
            CallEndMessage::from_goodbye_reason(GoodbyeReason::None)
                .into_string()
                .is_empty(),
            "normal hangup must produce an explicitly silent CallEnded message"
        );
    }

    #[test]
    fn peer_message_helpers_format_user_facing_copy() {
        assert_eq!(peer_busy_message("Alice"), "Alice is busy");
        assert_eq!(
            peer_no_response_message("Alice"),
            "Alice did not respond to the call"
        );
        assert_eq!(
            peer_not_accepted_message("Alice"),
            "Alice did not accept the call"
        );
        assert_eq!(
            peer_goodbye_reason_message("Alice", GoodbyeReason::SessionStopped),
            "Alice did not accept the call because the session was stopped"
        );
        assert_eq!(
            peer_goodbye_reason_message("Alice", GoodbyeReason::AudioDeviceError),
            "Alice did not accept the call because of an audio device problem"
        );
        assert_eq!(
            peer_goodbye_reason_message("Alice", GoodbyeReason::Error),
            "Alice did not accept the call because of an unexpected problem"
        );
        assert_eq!(
            peer_goodbye_reason_message("Alice", GoodbyeReason::None),
            "Alice did not accept the call"
        );
        assert_eq!(
            peer_unexpected_message("Alice"),
            "Received an unexpected message from Alice"
        );
    }
}
