/// flutter_rust_bridge:ignore
pub(crate) mod callbacks;

use crate::AudioDevice;
use crate::error::DartError;
use crate::frb_generated::StreamSink;
use crate::internal::TelepathyHandle;
use crate::overlay::overlay::Overlay;
#[cfg(not(target_family = "wasm"))]
use fast_log::Config;
#[cfg(not(target_family = "wasm"))]
use fast_log::appender::{FastLogRecord, LogAppender};
use flutter_rust_bridge::{DartFnFuture, frb};
use lazy_static::lazy_static;
use libp2p::PeerId;
use libp2p::identity::Keypair;
use log::{LevelFilter, info, warn};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, Once};
pub use telepathy_audio::Host;
#[cfg(not(target_family = "wasm"))]
use tokio::process::Command;
use tokio::sync::Mutex;

pub use crate::types::*;

static INIT_LOGGER_ONCE: Once = Once::new();

lazy_static! {
    static ref SEND_TO_DART_LOGGER_STREAM_SINK: std::sync::RwLock<Option<StreamSink<String>>> =
        std::sync::RwLock::new(None);
}

pub(crate) type DartVoid<A> = Arc<Mutex<dyn Fn(A) -> DartFnFuture<()> + Send>>;
pub(crate) type DartMethod<A, R> = Arc<Mutex<dyn Fn(A) -> DartFnFuture<R> + Send>>;
pub(crate) type AcceptCallArgs = (String, Option<Vec<u8>>, FrontendNotify);
pub(crate) type SessionStatusArgs = (String, SessionStatus);
pub(crate) type ScreenshareStartedArgs = (FrontendNotify, bool);
pub(crate) type ManagerActiveArgs = (bool, bool);

/// Rust API for FRB frontend.
///
/// Every public method here forwards to a same-named method on `TelepathyHandle`.
/// Keep this `impl` in sync with `impl NativeTelepathy` and `impl TelepathyHandle`.
#[frb(opaque)]
pub struct Telepathy {
    handle: TelepathyHandle<FlutterCallbacks, FlutterStatisticsCallback>,
}

impl Telepathy {
    #[frb(sync)]
    pub fn new(
        host: Arc<Host>,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: FlutterCallbacks,
    ) -> Telepathy {
        Self {
            handle: TelepathyHandle::new(
                host,
                network_config,
                screenshare_config,
                overlay,
                codec_config,
                callbacks,
            ),
        }
    }

    pub async fn start_manager(&mut self) {
        self.handle.start_manager().await;
    }

    /// Tries to start a session for a contact
    pub async fn start_session(&self, contact: &Contact) {
        self.handle.start_session(contact).await;
    }

    /// Attempts to start a call through an existing session
    pub async fn start_call(&self, contact: &Contact) -> std::result::Result<(), DartError> {
        self.handle.start_call(contact).await
    }

    /// Ends the current audio test, room, or call in that order
    pub async fn end_call(&self) {
        self.handle.end_call().await;
    }

    /// The only entry point into participating in a room
    pub async fn join_room(
        &self,
        member_strings: Vec<String>,
    ) -> std::result::Result<(), DartError> {
        self.handle.join_room(member_strings).await
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> std::result::Result<(), DartError> {
        self.handle.restart_manager().await
    }

    /// shuts down the entire rust backend
    pub async fn shutdown(&self) {
        self.handle.shutdown().await;
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: Vec<u8>) -> std::result::Result<(), DartError> {
        self.handle.set_identity(key).await
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
        self.handle.stop_session(contact).await
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> std::result::Result<(), DartError> {
        self.handle.audio_test().await
    }

    #[frb(sync)]
    pub fn build_chat(
        &self,
        contact: &Contact,
        text: String,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> ChatMessage {
        self.handle.build_chat(contact, text, attachments)
    }

    /// Sends a chat message
    pub async fn send_chat(&self, message: &mut ChatMessage) -> std::result::Result<(), DartError> {
        self.handle.send_chat(message).await
    }

    pub async fn start_screenshare(&self, contact: &Contact) {
        self.handle.start_screenshare(contact).await
    }

    #[frb(sync)]
    pub fn set_rms_threshold(&self, decimal: f32) {
        self.handle.set_rms_threshold(decimal)
    }

    #[frb(sync)]
    pub fn set_input_volume(&self, decibel: f32) {
        self.handle.set_input_volume(decibel)
    }

    #[frb(sync)]
    pub fn set_output_volume(&self, decibel: f32) {
        self.handle.set_output_volume(decibel)
    }

    #[frb(sync)]
    pub fn set_deafened(&self, deafened: bool) {
        self.handle.set_deafened(deafened)
    }

    #[frb(sync)]
    pub fn set_muted(&self, muted: bool) {
        self.handle.set_muted(muted)
    }

    /// Changing the denoise flag will not affect the current call
    #[frb(sync)]
    pub fn set_denoise(&self, denoise: bool) {
        self.handle.set_denoise(denoise)
    }

    #[frb(sync)]
    pub fn set_play_custom_ringtones(&self, play: bool) {
        self.handle.set_play_custom_ringtones(play)
    }

    #[frb(sync)]
    pub fn set_send_custom_ringtone(&self, send: bool) {
        self.handle.set_send_custom_ringtone(send)
    }

    #[frb(sync)]
    pub fn set_efficiency_mode(&self, enabled: bool) {
        self.handle.set_efficiency_mode(enabled)
    }

    #[frb(sync)]
    pub fn pause_statistics(&self) {
        self.handle.pause_statistics()
    }

    #[frb(sync)]
    pub fn resume_statistics(&self) {
        self.handle.resume_statistics()
    }

    pub async fn set_input_device(&self, device_id: Option<String>) {
        self.handle.set_input_device(device_id).await
    }

    pub async fn set_output_device(&self, device_id: Option<String>) {
        self.handle.set_output_device(device_id).await
    }

    /// Lists the input and output devices
    pub fn list_devices(
        &self,
    ) -> std::result::Result<(Vec<AudioDevice>, Vec<AudioDevice>), DartError> {
        self.handle.list_devices()
    }

    pub async fn set_model(&self, model: Option<Vec<u8>>) -> std::result::Result<(), DartError> {
        self.handle.set_model(model).await
    }
}

#[frb(opaque)]
pub struct FlutterCallbacks {
    /// Prompts the user to accept a call
    accept_call: DartMethod<AcceptCallArgs, bool>,

    /// Fetches a contact from the front end
    get_contact: DartMethod<Vec<u8>, Option<Contact>>,

    /// Notifies the frontend that the call has disconnected or reconnected
    call_state: DartVoid<CallState>,

    /// Alerts the UI when the status of a session changes
    session_status: DartVoid<SessionStatusArgs>,

    /// Starts a session for each of the UI's contacts
    get_contacts: DartMethod<(), Vec<Contact>>,

    /// Used to report statistics to the frontend
    statistics: DartVoid<Statistics>,

    /// Used to send chat messages to the frontend
    message_received: DartVoid<ChatMessage>,

    /// Alerts the UI when the manager is active and restartable
    manager_active: DartVoid<ManagerActiveArgs>,

    /// Called when a screenshare starts
    #[allow(dead_code)]
    screenshare_started: DartVoid<ScreenshareStartedArgs>,
}

impl FlutterCallbacks {
    #[frb(sync)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        accept_call: impl Fn(AcceptCallArgs) -> DartFnFuture<bool> + Send + 'static,
        get_contact: impl Fn(Vec<u8>) -> DartFnFuture<Option<Contact>> + Send + 'static,
        call_state: impl Fn(CallState) -> DartFnFuture<()> + Send + 'static,
        session_status: impl Fn(SessionStatusArgs) -> DartFnFuture<()> + Send + 'static,
        get_contacts: impl Fn(()) -> DartFnFuture<Vec<Contact>> + Send + 'static,
        statistics: impl Fn(Statistics) -> DartFnFuture<()> + Send + 'static,
        message_received: impl Fn(ChatMessage) -> DartFnFuture<()> + Send + 'static,
        manager_active: impl Fn(ManagerActiveArgs) -> DartFnFuture<()> + Send + 'static,
        screenshare_started: impl Fn(ScreenshareStartedArgs) -> DartFnFuture<()> + Send + 'static,
    ) -> Self {
        Self {
            accept_call: Arc::new(Mutex::new(accept_call)),
            get_contact: Arc::new(Mutex::new(get_contact)),
            call_state: Arc::new(Mutex::new(call_state)),
            session_status: Arc::new(Mutex::new(session_status)),
            get_contacts: Arc::new(Mutex::new(get_contacts)),
            statistics: Arc::new(Mutex::new(statistics)),
            message_received: Arc::new(Mutex::new(message_received)),
            manager_active: Arc::new(Mutex::new(manager_active)),
            screenshare_started: Arc::new(Mutex::new(screenshare_started)),
        }
    }
}

pub(crate) struct FlutterStatisticsCallback {
    inner: DartVoid<Statistics>,
}

// The following is a modified version of the code found at
// https://github.com/fzyzcjy/flutter_rust_bridge/issues/486

pub struct SendToDartLogger {}

impl SendToDartLogger {
    pub fn set_stream_sink(stream_sink: StreamSink<String>) {
        let mut guard = SEND_TO_DART_LOGGER_STREAM_SINK.write().unwrap();
        let overriding = guard.is_some();

        *guard = Some(stream_sink);

        drop(guard);

        if overriding {
            warn!(
                "SendToDartLogger::set_stream_sink but already exist a sink, thus overriding. \
                (This may or may not be a problem. It will happen normally if hot-reload Flutter app.)"
            );
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl LogAppender for SendToDartLogger {
    fn do_logs(&mut self, records: &[FastLogRecord]) {
        if let Some(stream) = SEND_TO_DART_LOGGER_STREAM_SINK.read().unwrap().as_ref() {
            for record in records {
                _ = stream.add(record.formated.clone());
            }
        }
    }
}

#[frb(sync)]
pub fn create_log_stream(s: StreamSink<String>) {
    SendToDartLogger::set_stream_sink(s);
}

#[frb(sync)]
pub fn rust_set_up() {
    // https://stackoverflow.com/questions/30177845/how-to-initialize-the-logger-for-integration-tests
    INIT_LOGGER_ONCE.call_once(|| {
        let level = if cfg!(debug_assertions) {
            LevelFilter::Debug
        } else {
            LevelFilter::Warn
        };

        assert!(
            level <= log::STATIC_MAX_LEVEL,
            "Should respect log::STATIC_MAX_LEVEL={:?}, which is done in compile time. level{:?}",
            log::STATIC_MAX_LEVEL,
            level
        );

        #[cfg(not(target_family = "wasm"))]
        fast_log::init(
            Config::new()
                .file("telepathy.log")
                .level(level)
                .add_appender(SendToDartLogger {}),
        )
            .unwrap();

        #[cfg(target_family = "wasm")]
        wasm_logger::init(wasm_logger::Config::default());

        log_panics::init();

        info!("init_logger finished");
    });
}

#[frb(sync)]
pub fn generate_keys() -> Result<(String, Vec<u8>), DartError> {
    let pair = Keypair::generate_ed25519();

    let peer_id = pair.public().to_peer_id();

    Ok((
        peer_id.to_string(),
        pair.to_protobuf_encoding()
            .map_err(|e| DartError::from(e.to_string()))?,
    ))
}

#[frb(sync)]
pub fn room_hash(peers: Vec<String>) -> Result<String, DartError> {
    let mut acc = 0;

    for peer in peers {
        if let Ok(peer) = PeerId::from_str(&peer) {
            let mut hasher = DefaultHasher::new();
            peer.hash(&mut hasher);
            acc ^= hasher.finish();
        } else {
            return Err(DartError::from(peer));
        }
    }

    Ok(format!("room-{}", acc))
}

#[frb(sync)]
pub fn validate_peer_id(peer_id: String) -> bool {
    PeerId::from_str(&peer_id).is_ok()
}

pub async fn screenshare_available() -> bool {
    #[cfg(target_family = "wasm")]
    return false;

    #[cfg(not(target_family = "wasm"))]
    if let Ok(status) = Command::new("ffmpeg").status().await {
        // ffmpeg with no arguments returns status 1
        status.code() == Some(1)
    } else {
        false
    }
}

async fn notify<A>(void: &DartVoid<A>, args: A) {
    (void.lock().await)(args).await
}

async fn invoke<A, R>(method: &DartMethod<A, R>, args: A) -> R {
    (method.lock().await)(args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ignore]
    #[tokio::test]
    async fn screenshare_available_returns_true() {
        let ffmpeg_available = screenshare_available().await;
        assert!(ffmpeg_available);
    }
}
