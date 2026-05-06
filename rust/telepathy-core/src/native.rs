mod callbacks;

use crate::audio::player::SoundPlayer;
use crate::error::DartError;
use crate::internal::TelepathyHandle;
use crate::overlay::overlay::Overlay;
use crate::types::{CallState, ChatMessage, Contact, FrontendNotify, SessionStatus, Statistics};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{oneshot, watch};

type NativeFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type NativeVoid<A> = Arc<dyn Fn(A) -> NativeFuture<()> + Send + Sync + 'static>;
type NativeMethod<A, R> = Arc<dyn Fn(A) -> NativeFuture<R> + Send + Sync + 'static>;
type NativeAcceptCall = Arc<
    dyn Fn(String, Option<Vec<u8>>, oneshot::Sender<bool>, watch::Receiver<bool>) + Send + Sync,
>;

/// Statistics callback adapter for non-FRB clients.
#[derive(Clone)]
pub struct NativeStatisticsCallback {
    inner: NativeVoid<Statistics>,
}

/// Rust-native callback surface for `telepathy-core`.
///
/// This mirrors `FlutterCallbacks` but replaces FRB function wrappers with plain
/// Rust closures/futures so native consumers (like `telepathy-tui`) can depend on
/// `telepathy-core` without FRB runtime semantics.
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
