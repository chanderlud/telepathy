/// flutter_rust_bridge:ignore
pub(crate) mod callbacks;
pub mod logging;
pub mod utils;

use crate::AudioDevice;
use crate::internal::TelepathyHandle;
use crate::internal::state::PreparedIdentitySwitch as InternalPreparedIdentitySwitch;
use crate::overlay::Overlay;
pub use crate::types::*;
use flutter_rust_bridge::{DartFnFuture, frb};
use std::sync::Arc;
pub use telepathy_audio::Host;
use telepathy_audio::devices::CpalAudioHost;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

type DartVoid<A> = Arc<Mutex<dyn Fn(A) -> DartFnFuture<()> + Send>>;
type DartMethod<A, R> = Arc<Mutex<dyn Fn(A) -> DartFnFuture<R> + Send>>;
type AcceptCallArgs = (String, Option<Vec<u8>>, FrontendNotify);
type SessionStatusArgs = (String, SessionStatus);
type ManagerActiveArgs = ManagerState;

#[frb(opaque)]
pub struct StartOperation {
    cancel: CancellationToken,
}

impl StartOperation {
    #[frb(sync)]
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

#[frb(opaque)]
pub struct PreparedIdentitySwitch {
    prepared: Option<InternalPreparedIdentitySwitch>,
}

impl PreparedIdentitySwitch {
    pub async fn commit(&mut self) -> Result<(), DartError> {
        if let Some(prepared) = self.prepared.take() {
            prepared.commit().await.map_err(DartError::from)?;
        }
        Ok(())
    }
}

/// Rust API for FRB frontend. Mirrors `impl NativeTelepathy` 1:1; both
/// forward to `impl TelepathyHandle`.
#[frb(opaque)]
pub struct Telepathy {
    handle: TelepathyHandle<FlutterCallbacks, CpalAudioHost>,
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
                host.into(),
                network_config,
                screenshare_config,
                overlay,
                codec_config,
                callbacks,
            ),
        }
    }

    /// Non-blocking: spawns the manager task and returns. The Dart side observes
    /// the eventual `Active` transition via the `managerActive` callback. The
    /// non-blocking contract is validated by the CLI system test
    /// `test_start_manager_ack_precedes_active_event`; the `()` return type
    /// prevents silent reintroduction of blocking semantics.
    pub async fn start_manager(&mut self) {
        self.handle.start_manager().await;
    }

    /// Tries to start a session for a contact
    pub async fn start_session(&self, contact: &Contact) -> Result<(), DartError> {
        self.handle
            .try_start_session(contact)
            .await
            .map_err(DartError::from)
    }

    /// Creates an operation token that can cancel one pending call or room start.
    #[frb(sync)]
    pub fn new_start_operation(&self) -> StartOperation {
        StartOperation {
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Attempts to start a call through an existing session.
    pub async fn start_call(
        &self,
        contact: &Contact,
        operation: &StartOperation,
    ) -> Result<(), DartError> {
        self.handle
            .start_call_with_operation(contact, &operation.cancel)
            .await
            .map_err(DartError::from)
    }

    /// Ends the current audio test, room, or call in that order
    pub async fn end_call(&self) {
        self.handle.end_call().await;
    }

    /// The only entry point into participating in a room.
    pub async fn join_room(
        &self,
        member_strings: Vec<String>,
        operation: &StartOperation,
    ) -> Result<(), DartError> {
        self.handle
            .join_room_with_operation(member_strings, &operation.cancel)
            .await
            .map_err(DartError::from)
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> Result<(), DartError> {
        self.handle.restart_manager().await.map_err(DartError::from)
    }

    pub async fn prepare_identity_switch(
        &self,
        target_key: Vec<u8>,
        target_contacts: Vec<Contact>,
    ) -> Result<PreparedIdentitySwitch, DartError> {
        let target_key: [u8; 32] = target_key
            .try_into()
            .map_err(|_| DartError::from(IDENTITY_KEY_LENGTH_MESSAGE.to_string()))?;
        self.handle
            .prepare_identity_switch(target_key, target_contacts)
            .await
            .map(|prepared| PreparedIdentitySwitch {
                prepared: Some(prepared),
            })
            .map_err(DartError::from)
    }

    /// shuts down the entire rust backend
    pub async fn shutdown(&self) {
        self.handle.shutdown().await;
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: Vec<u8>) -> Result<(), DartError> {
        self.handle
            .set_identity(
                &(key
                    .try_into()
                    .map_err(|_| DartError::from(IDENTITY_KEY_LENGTH_MESSAGE.to_string()))?),
            )
            .await
            .map_err(DartError::from)
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
        self.handle.stop_session(contact).await
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> Result<(), DartError> {
        self.handle.audio_test().await.map_err(DartError::from)
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
    pub async fn send_chat(&self, message: &mut ChatMessage) -> Result<(), DartError> {
        self.handle
            .send_chat(message)
            .await
            .map_err(DartError::from)
    }

    pub async fn request_video_source(
        &self,
        contact: &Contact,
        source: VideoSource,
    ) -> VideoStartOutcome {
        self.handle.request_video_source(contact, source).await
    }

    pub async fn stop_video_source(&self, identity: VideoSessionIdentity) -> VideoStopOutcome {
        self.handle.stop_video_source(identity).await
    }

    pub async fn video_capabilities(&self) -> VideoCapabilities {
        self.handle.video_capabilities().await
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
    pub fn set_output_volume(&self, decibel: f32) -> Result<(), DartError> {
        self.handle
            .set_output_volume(decibel)
            .map_err(DartError::from)
    }

    #[frb(sync)]
    pub fn set_contact_output_volume(&self, contact: &Contact) -> Result<(), DartError> {
        self.handle
            .set_contact_output_volume(contact)
            .map_err(DartError::from)
    }

    #[frb(sync)]
    pub fn set_deafened(&self, deafened: bool) {
        self.handle.set_deafened(deafened)
    }

    #[frb(sync)]
    pub fn set_muted(&self, muted: bool) {
        self.handle.set_muted(muted)
    }

    /// Denoise is set on the processor; the current call is not reconfigured.
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
    pub fn list_devices(&self) -> Result<(Vec<AudioDevice>, Vec<AudioDevice>), DartError> {
        self.handle.list_devices().map_err(DartError::from)
    }

    pub async fn set_model(&self, model: Option<Vec<u8>>) -> Result<(), DartError> {
        self.handle.set_model(model).await.map_err(DartError::from)
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

    #[allow(dead_code)]
    video_lifecycle: DartVoid<VideoLifecycleEvent>,
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
        video_lifecycle: impl Fn(VideoLifecycleEvent) -> DartFnFuture<()> + Send + 'static,
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
            video_lifecycle: Arc::new(Mutex::new(video_lifecycle)),
        }
    }
}

#[frb(ignore)]
pub struct FlutterStatisticsCallback {
    inner: DartVoid<Statistics>,
}

async fn notify<A>(void: &DartVoid<A>, args: A) {
    (void.lock().await)(args).await
}

async fn invoke<A, R>(method: &DartMethod<A, R>, args: A) -> R {
    (method.lock().await)(args).await
}
