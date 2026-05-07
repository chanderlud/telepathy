use crate::internal::TelepathyHandle;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::{JoinHandle, spawn_task};
use crate::overlay::Overlay;
use crate::player::SoundPlayer;
use crate::types::{
    CallState, ChatMessage, Contact, DartError, FrontendNotify, SessionStatus, Statistics,
};
use libp2p::PeerId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Notify, oneshot, watch};

type NativeFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type NativeVoid<A> = Arc<dyn Fn(A) -> NativeFuture<()> + Send + Sync + 'static>;
type NativeMethod<A, R> = Arc<dyn Fn(A) -> NativeFuture<R> + Send + Sync + 'static>;
type NativeAcceptCall = Arc<
    dyn Fn(String, Option<Vec<u8>>, oneshot::Sender<bool>, watch::Receiver<bool>) + Send + Sync,
>;

/// Rust-native runtime client for `telepathy-core`.
///
/// This mirrors the Flutter-facing API but accepts [`NativeCallbacks`] and does
/// not depend on FRB runtime semantics.
pub struct NativeTelepathy {
    handle: TelepathyHandle<NativeCallbacks, NativeStatisticsCallback>,
}

impl NativeTelepathy {
    pub fn new(
        host: Arc<telepathy_audio::Host>,
        network_config: &crate::types::NetworkConfig,
        screenshare_config: &crate::types::ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &crate::types::CodecConfig,
        callbacks: NativeCallbacks,
    ) -> Self {
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

    /// Convenience constructor for native clients that do not provide a custom
    /// host/screenshare/overlay setup.
    pub fn new_default(
        network_config: &crate::types::NetworkConfig,
        codec_config: &crate::types::CodecConfig,
        callbacks: NativeCallbacks,
    ) -> Self {
        let host = SoundPlayer::new(0.0).host();
        let screenshare_config = crate::types::ScreenshareConfig::default();
        let overlay = Overlay::default();
        Self::new(
            host,
            network_config,
            &screenshare_config,
            &overlay,
            codec_config,
            callbacks,
        )
    }

    pub async fn start_manager(&mut self) {
        self.handle.start_manager().await;
    }

    pub async fn start_session(&self, contact: &Contact) {
        self.handle.start_session(contact).await;
    }

    pub async fn start_call(&self, contact: &Contact) -> std::result::Result<(), DartError> {
        self.handle.start_call(contact).await
    }

    pub async fn end_call(&self) {
        self.handle.end_call().await;
    }

    pub async fn join_room(
        &self,
        member_strings: Vec<String>,
    ) -> std::result::Result<(), DartError> {
        self.handle.join_room(member_strings).await
    }

    pub async fn restart_manager(&self) -> std::result::Result<(), DartError> {
        self.handle.restart_manager().await
    }

    pub async fn shutdown(&self) {
        self.handle.shutdown().await;
    }

    pub async fn set_identity(&self, key: Vec<u8>) -> std::result::Result<(), DartError> {
        self.handle.set_identity(key).await
    }

    pub async fn stop_session(&self, contact: &Contact) {
        self.handle.stop_session(contact).await;
    }

    pub async fn audio_test(&self) -> std::result::Result<(), DartError> {
        self.handle.audio_test().await
    }

    pub fn build_chat(
        &self,
        contact: &Contact,
        text: String,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> ChatMessage {
        self.handle.build_chat(contact, text, attachments)
    }

    pub async fn send_chat(&self, message: &mut ChatMessage) -> std::result::Result<(), DartError> {
        self.handle.send_chat(message).await
    }

    pub fn set_rms_threshold(&self, decimal: f32) {
        self.handle.set_rms_threshold(decimal);
    }

    pub fn set_input_volume(&self, decibel: f32) {
        self.handle.set_input_volume(decibel);
    }

    pub fn set_output_volume(&self, decibel: f32) {
        self.handle.set_output_volume(decibel);
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.handle.set_deafened(deafened);
    }

    pub fn set_muted(&self, muted: bool) {
        self.handle.set_muted(muted);
    }

    pub fn set_denoise(&self, denoise: bool) {
        self.handle.set_denoise(denoise);
    }

    pub fn set_play_custom_ringtones(&self, play: bool) {
        self.handle.set_play_custom_ringtones(play);
    }

    pub fn set_efficiency_mode(&self, enabled: bool) {
        self.handle.set_efficiency_mode(enabled);
    }

    pub async fn set_input_device(&self, device_id: Option<String>) {
        self.handle.set_input_device(device_id).await;
    }

    pub async fn set_output_device(&self, device_id: Option<String>) {
        self.handle.set_output_device(device_id).await;
    }

    pub async fn set_model(&self, model: Option<Vec<u8>>) -> std::result::Result<(), DartError> {
        self.handle.set_model(model).await
    }
}

/// Statistics callback adapter for non-FRB clients.
#[derive(Clone)]
pub struct NativeStatisticsCallback {
    inner: NativeVoid<Statistics>,
}

impl CoreStatisticsCallback for NativeStatisticsCallback {
    async fn post(&self, stats: Statistics) {
        (self.inner)(stats).await
    }
}

/// Rust-native callback surface for `telepathy-core`.
///
/// This mirrors `FlutterCallbacks` but replaces FRB function wrappers with plain
/// Rust closures/futures so native consumers can depend on `telepathy-core` without
/// FRB runtime semantics.
pub struct NativeCallbacks {
    /// Prompts the user to accept a call.
    ///
    /// - `response_tx`: send `true` to accept or `false` to reject
    /// - `cancel_rx`: core toggles this to `true` to dismiss the pending prompt
    accept_call: NativeAcceptCall,
    get_contact: NativeMethod<Vec<u8>, Option<Contact>>,
    call_state: NativeVoid<CallState>,
    session_status: NativeVoid<(String, SessionStatus)>,
    get_contacts: NativeMethod<(), Vec<Contact>>,
    statistics: NativeVoid<Statistics>,
    message_received: NativeVoid<ChatMessage>,
    manager_active: NativeVoid<(bool, bool)>,
    screenshare_started: NativeVoid<(FrontendNotify, bool)>,
}

impl NativeCallbacks {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        accept_call: impl Fn(String, Option<Vec<u8>>, oneshot::Sender<bool>, watch::Receiver<bool>)
        + Send
        + Sync
        + 'static,
        get_contact: impl Fn(Vec<u8>) -> NativeFuture<Option<Contact>> + Send + Sync + 'static,
        call_state: impl Fn(CallState) -> NativeFuture<()> + Send + Sync + 'static,
        session_status: impl Fn((String, SessionStatus)) -> NativeFuture<()> + Send + Sync + 'static,
        get_contacts: impl Fn(()) -> NativeFuture<Vec<Contact>> + Send + Sync + 'static,
        statistics: impl Fn(Statistics) -> NativeFuture<()> + Send + Sync + 'static,
        message_received: impl Fn(ChatMessage) -> NativeFuture<()> + Send + Sync + 'static,
        manager_active: impl Fn((bool, bool)) -> NativeFuture<()> + Send + Sync + 'static,
        screenshare_started: impl Fn((FrontendNotify, bool)) -> NativeFuture<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            accept_call: Arc::new(accept_call),
            get_contact: Arc::new(get_contact),
            call_state: Arc::new(call_state),
            session_status: Arc::new(session_status),
            get_contacts: Arc::new(get_contacts),
            statistics: Arc::new(statistics),
            message_received: Arc::new(message_received),
            manager_active: Arc::new(manager_active),
            screenshare_started: Arc::new(screenshare_started),
        }
    }
}

impl CoreCallbacks<NativeStatisticsCallback> for NativeCallbacks {
    async fn session_status(&self, status: SessionStatus, peer: PeerId) {
        (self.session_status)((peer.to_string(), status)).await
    }

    async fn call_state(&self, status: CallState) {
        (self.call_state)(status).await
    }

    async fn get_contacts(&self) -> Vec<Contact> {
        (self.get_contacts)(()).await
    }

    async fn manager_active(&self, active: bool, restartable: bool) {
        (self.manager_active)((active, restartable)).await
    }

    async fn screenshare_started(&self, stop: FrontendNotify, sender: bool) {
        (self.screenshare_started)((stop, sender)).await
    }

    async fn get_contact(&self, peer_id: Vec<u8>) -> Option<Contact> {
        (self.get_contact)(peer_id).await
    }

    fn get_accept_handle(
        &self,
        contact_id: &str,
        ringtone: Option<Vec<u8>>,
        cancel: &Arc<Notify>,
    ) -> JoinHandle<bool> {
        let accept_call = Arc::clone(&self.accept_call);
        let contact_id = contact_id.to_string();
        let cancel_signal = Arc::clone(cancel);
        spawn_task(async move {
            let (response_tx, response_rx) = oneshot::channel();
            let (cancel_tx, cancel_rx) = watch::channel(false);

            accept_call(contact_id, ringtone, response_tx, cancel_rx);

            tokio::select! {
                _ = cancel_signal.notified() => {
                    let _ = cancel_tx.send(true);
                    false
                }
                response = response_rx => response.unwrap_or(false),
            }
        })
    }

    async fn message_received(&self, chat_message: ChatMessage) {
        (self.message_received)(chat_message).await
    }

    fn statistics_callback(&self) -> NativeStatisticsCallback {
        NativeStatisticsCallback {
            inner: Arc::clone(&self.statistics),
        }
    }
}
