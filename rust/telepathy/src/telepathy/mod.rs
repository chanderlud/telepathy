/// implementations for core telepathy functionality
/// flutter_rust_bridge:ignore
pub mod core;
/// helper methods used by telepathy core
/// flutter_rust_bridge:ignore
mod helpers;
/// flutter_rust_bridge:ignore
pub(crate) mod messages;
pub(crate) mod screenshare;
/// networking code for live audio streams
/// flutter_rust_bridge:ignore
mod sockets;
#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
pub(crate) mod tests;
pub(crate) mod utils;

use crate::error::{DartError, Error, ErrorKind};
use crate::flutter::callbacks::FrbCallbacks;
use crate::flutter::*;
use crate::overlay::overlay::Overlay;
use crate::telepathy::core::TelepathyCore;
use crate::telepathy::helpers::OutputHelper;
use atomic_float::AtomicF32;
use chrono::Local;
#[cfg(not(target_family = "wasm"))]
use cpal::Device;
use cpal::Host;
#[cfg(not(target_family = "wasm"))]
use cpal::SupportedStreamConfig;
use cpal::traits::{DeviceTrait, HostTrait};
#[cfg(target_family = "wasm")]
use flutter_rust_bridge::JoinHandle;
use flutter_rust_bridge::{frb, spawn};
use kanal::AsyncReceiver;
use kanal::{AsyncSender, unbounded_async};
use libp2p::core::ConnectedPoint;
use libp2p::identity::Keypair;
use libp2p::{PeerId, Stream, StreamProtocol};
use libp2p_stream::Control;
use log::{debug, error, info, warn};
use messages::{Attachment, AudioHeader, Message};
use nnnoiseless::{FRAME_SIZE, RnnModel};
use sockets::{Transport, TransportStream};
use std::mem;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc::{Receiver as MReceiver, Sender as MSender, channel};
use tokio::sync::{Mutex, Notify};
#[cfg(not(target_family = "wasm"))]
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use utils::*;
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::interval;

type Result<T> = std::result::Result<T, Error>;
pub(crate) type DeviceName = Arc<Mutex<Option<String>>>;

/// The number of bytes in a single network audio frame
const TRANSFER_BUFFER_SIZE: usize = FRAME_SIZE * size_of::<i16>();
/// A timeout used when initializing the call
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to keep-alive libp2p streams
pub(crate) const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// the number of samples to hold in a channel
pub(crate) const CHANNEL_SIZE: usize = 2_400;
/// the protocol identifier for Telepathy
const CHAT_PROTOCOL: StreamProtocol = StreamProtocol::new("/telepathy/0.0.1");
/// Maximum allowed size for a single length-delimited control/message frame on the session stream.
const SESSION_MAX_FRAME_LENGTH: usize = 1024 * 1024 * 1024;

/// the public API for flutter
#[frb(opaque)]
pub struct Telepathy {
    inner: TelepathyCore<FlutterCallbacks, FlutterStatisticsCallback>,

    /// contains handles to the manager thread & room managers
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
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
            inner: TelepathyCore::new(
                host,
                network_config,
                screenshare_config,
                overlay,
                codec_config,
                callbacks,
            ),
            handles: Default::default(),
        }
    }

    pub async fn start_manager(&mut self) {
        #[cfg(not(target_family = "wasm"))]
        if let Some(handle) = self.inner.start_manager().await {
            self.handles.lock().await.push(handle);
        }
    }

    /// Tries to start a session for a contact
    pub async fn start_session(&self, contact: &Contact) {
        debug!("start_session called for {}", contact.peer_id);

        if let Some(ref sender) = self.inner.start_session
            && sender.send(contact.peer_id).await.is_err()
        {
            error!("start_session channel is closed");
        }
    }

    /// Attempts to start a call through an existing session
    pub async fn start_call(&self, contact: &Contact) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            return Err("Cannot start call while a call is already active"
                .to_string()
                .into());
        }

        if let Some(state) = self.inner.session_states.read().await.get(&contact.peer_id) {
            #[cfg(target_family = "wasm")]
            self.inner
                .init_web_audio()
                .await
                .map_err::<Error, _>(Error::into)?;

            state.start_call.notify_one();
            Ok(())
        } else {
            Err(String::from("No session found for contact").into())
        }
    }

    /// Ends the current audio test, room, or call in that order
    pub async fn end_call(&self) {
        if let Some(end_audio_test) = self.inner.core_state.end_audio_test.lock().await.as_ref() {
            debug!("ending audio test");
            end_audio_test.notify_one();
        } else if let Some(room_state) = self.inner.room_state.read().await.as_ref() {
            debug!("ending room");
            room_state.end_call.notify_one();
        } else if let Some(session_state) = self
            .inner
            .session_states
            .read()
            .await
            .values()
            .find(|s| s.in_call.load(Relaxed))
        {
            debug!("ending call");
            session_state.end_call.notify_one();
        } else {
            warn!("end_call failed to end anything");
        }
    }

    /// The only entry point into participating in a room
    pub async fn join_room(
        &self,
        member_strings: Vec<String>,
    ) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            return Err("Cannot join room while a call is already active"
                .to_string()
                .into());
        }

        #[cfg(target_family = "wasm")]
        self.inner
            .init_web_audio()
            .await
            .map_err::<Error, _>(Error::into)?;

        // parse members
        let members: Vec<_> = member_strings
            .into_iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        // delivers messages from each session to the room controller
        let (sender, receiver) = channel(32);
        // cancels all processing threads
        let cancel = CancellationToken::new();
        // gracefully ends the room call
        let end_call = Arc::new(Notify::new());
        // the same early call state is used throughout the room, the real peer ids are set later
        let call_state = self.inner.setup_call(PeerId::random()).await?;
        // set room state
        let old_state_option = self.inner.room_state.write().await.replace(RoomState {
            peers: members.clone(),
            sender,
            cancel: cancel.clone(),
            end_call: end_call.clone(),
            early_state: call_state.clone(),
        });
        // clean up old state
        if let Some(old_state) = old_state_option {
            old_state.cancel.cancel();
            old_state.end_call.notify_one();
        }
        // tries to connect to every member of the room using existing sessions, or new ones if needed
        // note: sending your own identity to start_session is safe
        for member in members {
            if let Some(state) = self.inner.session_states.read().await.get(&member) {
                state.start_call.notify_one();
            } else if let Some(ref sender) = self.inner.start_session {
                // when the session opens, start_call will be notified
                _ = sender.send(member).await;
            }
        }
        // spawn room controller
        let self_clone = self.inner.clone();
        self.handles.lock().await.push(spawn(async move {
            let stop_io = Default::default();
            if let Err(error) = self_clone
                .room_controller(receiver, cancel, call_state, &stop_io, end_call)
                .await
            {
                error!("error in room controller: {:?}", error);
            }
            stop_io.cancel();
        }));

        Ok(())
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            Err("Cannot restart manager while call is active"
                .to_string()
                .into())
        } else {
            // reset sessions so manager can clean up
            self.inner.reset_sessions().await;
            // restart the manager
            self.inner.restart_manager.notify_one();
            // wait for a new manager to start
            self.inner.core_state.manager_active.notified().await;
            // start a session for all contacts
            for contact in self.inner.callbacks.get_contacts().await {
                self.start_session(&contact).await;
            }
            Ok(())
        }
    }

    /// shuts down the entire rust backend
    pub async fn shutdown(&self) {
        // stops sessions & manager
        self.inner.shutdown().await;
        // wait for manager & any room controllers to join
        for handle in self.handles.lock().await.drain(..) {
            handle.await.unwrap();
        }
        info!("shutdown complete");
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: Vec<u8>) -> std::result::Result<(), DartError> {
        *self.inner.core_state.identity.write().await =
            Some(Keypair::from_protobuf_encoding(&key).map_err(Error::from)?);
        Ok(())
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
        if let Some(state) = self
            .inner
            .session_states
            .write()
            .await
            .remove(&contact.peer_id)
        {
            state.stop_session.cancel();
        }
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            return Err("Cannot start test while call is active".to_string().into());
        }

        #[cfg(target_family = "wasm")]
        self.inner
            .init_web_audio()
            .await
            .map_err::<Error, _>(Error::into)?;

        let mut audio_config = self.inner.setup_call(PeerId::random()).await?;
        audio_config.remote_configuration = audio_config.local_configuration.clone();
        let stop_io = CancellationToken::new();
        let end_call = Arc::new(Notify::new());

        self.inner.core_state.in_call.store(true, Relaxed);
        *self.inner.core_state.end_audio_test.lock().await = Some(end_call.clone());
        let result = self
            .inner
            .call(&stop_io, audio_config, &end_call, None)
            .await
            .map_err(Into::into);
        stop_io.cancel();
        self.inner.core_state.end_audio_test.lock().await.take();
        self.inner.core_state.in_call.store(false, Relaxed);
        result
    }

    #[frb(sync)]
    pub fn build_chat(
        &self,
        contact: &Contact,
        text: String,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> ChatMessage {
        ChatMessage {
            text,
            receiver: contact.peer_id,
            timestamp: Local::now(),
            attachments: attachments
                .into_iter()
                .map(|(name, data)| Attachment { name, data })
                .collect(),
        }
    }

    /// Sends a chat message
    pub async fn send_chat(&self, message: &mut ChatMessage) -> std::result::Result<(), DartError> {
        if message
            .attachments
            .iter()
            .map(|a| a.data.len())
            .sum::<usize>()
            > SESSION_MAX_FRAME_LENGTH
        {
            return Err("attachments too large".to_string().into());
        }

        let Some(state) = self
            .inner
            .session_states
            .read()
            .await
            .get(&message.receiver)
            .cloned()
        else {
            warn!(
                "send_chat called for peer with no session {}",
                message.receiver
            );
            return Ok(());
        };

        // take the data out of each attachment. the frontend doesn't need it
        let attachments = message
            .attachments
            .iter_mut()
            .map(|attachment| Attachment {
                name: attachment.name.clone(),
                data: mem::take(&mut attachment.data),
            })
            .collect();

        let message = Message::Chat {
            text: message.text.clone(),
            attachments,
        };

        state
            .message_sender
            .send(message)
            .await
            .map_err(|_| "channel closed".to_string())?;
        Ok(())
    }

    pub async fn start_screenshare(&self, contact: &Contact) {
        self.inner
            .send_start_screenshare(contact.peer_id, None)
            .await;
    }

    #[frb(sync)]
    pub fn set_rms_threshold(&self, decimal: f32) {
        let threshold = db_to_multiplier(decimal);
        self.inner
            .core_state
            .rms_threshold
            .store(threshold, Relaxed);
    }

    #[frb(sync)]
    pub fn set_input_volume(&self, decibel: f32) {
        let multiplier = db_to_multiplier(decibel);
        self.inner
            .core_state
            .input_volume
            .store(multiplier, Relaxed);
    }

    #[frb(sync)]
    pub fn set_output_volume(&self, decibel: f32) {
        let multiplier = db_to_multiplier(decibel);
        self.inner
            .core_state
            .output_volume
            .store(multiplier, Relaxed);
    }

    #[frb(sync)]
    pub fn set_deafened(&self, deafened: bool) {
        self.inner.core_state.deafened.store(deafened, Relaxed);
    }

    #[frb(sync)]
    pub fn set_muted(&self, muted: bool) {
        self.inner.core_state.muted.store(muted, Relaxed);
    }

    /// Changing the denoise flag will not affect the current call
    #[frb(sync)]
    pub fn set_denoise(&self, denoise: bool) {
        self.inner.core_state.denoise.store(denoise, Relaxed);
    }

    #[frb(sync)]
    pub fn set_play_custom_ringtones(&self, play: bool) {
        self.inner
            .core_state
            .play_custom_ringtones
            .store(play, Relaxed);
    }

    #[frb(sync)]
    pub fn set_send_custom_ringtone(&self, send: bool) {
        self.inner
            .core_state
            .send_custom_ringtone
            .store(send, Relaxed);
    }

    #[frb(sync)]
    pub fn set_efficiency_mode(&self, enabled: bool) {
        self.inner
            .core_state
            .efficiency_mode
            .store(enabled, Relaxed);
    }

    pub async fn set_input_device(&self, device: Option<String>) {
        *self.inner.core_state.input_device.lock().await = device;
    }

    pub async fn set_output_device(&self, device: Option<String>) {
        *self.inner.core_state.output_device.lock().await = device;
    }

    /// Lists the input and output devices
    pub fn list_devices(&self) -> std::result::Result<(Vec<String>, Vec<String>), DartError> {
        let input_devices = self.inner.host.input_devices().map_err(Error::from)?;
        let output_devices = self.inner.host.output_devices().map_err(Error::from)?;

        let input_devices = input_devices
            .filter_map(|device| device.name().ok())
            .collect();

        let output_devices = output_devices
            .filter_map(|device| device.name().ok())
            .collect();

        Ok((input_devices, output_devices))
    }

    pub async fn set_model(&self, model: Option<Vec<u8>>) -> std::result::Result<(), DartError> {
        let model = if let Some(mode_bytes) = model {
            RnnModel::from_bytes(&mode_bytes).ok_or(String::from("invalid model"))?
        } else {
            RnnModel::default()
        };

        *self.inner.core_state.denoise_model.write().await = model;
        Ok(())
    }
}

/// state used early in the call before it starts
#[derive(Clone)]
pub(crate) struct EarlyCallState {
    peer: PeerId,
    local_configuration: AudioHeader,
    remote_configuration: AudioHeader,
    #[cfg(not(target_family = "wasm"))]
    input_config: SupportedStreamConfig,
    #[cfg(not(target_family = "wasm"))]
    input_device: Device,
    input_channels: usize,
}

impl EarlyCallState {
    fn codec_config(&self) -> (bool, bool, f32) {
        let codec_enabled =
            self.remote_configuration.codec_enabled || self.local_configuration.codec_enabled;
        let vbr = self.remote_configuration.vbr || self.local_configuration.vbr;
        let residual_bits = (self.remote_configuration.residual_bits as f32)
            .min(self.local_configuration.residual_bits as f32);
        (codec_enabled, vbr, residual_bits)
    }
}

/// shared values for a single session
#[derive(Debug)]
pub(crate) struct SessionState {
    /// identifies a unique session state
    id: Uuid,

    /// signals the session to initiate a call
    start_call: Notify,

    /// notifies during shutdown & manager restarts
    stop_session: CancellationToken,

    /// if the session is in a call
    in_call: AtomicBool,

    /// a reusable sender for messages while a call is active
    message_sender: MSender<Message>,

    /// forwards sub-streams to the session
    stream_sender: AsyncSender<Stream>,

    /// receives sub-streams for the session
    stream_receiver: AsyncReceiver<Stream>,

    /// a shared latency value for the session from libp2p ping
    latency: Arc<AtomicUsize>,

    /// a shared upload bandwidth value for the session
    upload_bandwidth: Arc<AtomicUsize>,

    /// a shared download bandwidth value for the session
    download_bandwidth: Arc<AtomicUsize>,

    /// whether the session wants a sub-stream
    wants_stream: Arc<AtomicBool>,

    end_call: Arc<Notify>,

    stop_screenshare: Arc<Mutex<Option<Arc<Notify>>>>,
}

impl SessionState {
    fn new(message_sender: &MSender<Message>) -> Self {
        let stream_channel = unbounded_async();

        Self {
            id: Uuid::new_v4(),
            start_call: Notify::new(),
            stop_session: Default::default(),
            in_call: AtomicBool::new(false),
            message_sender: message_sender.clone(),
            stream_sender: stream_channel.0,
            stream_receiver: stream_channel.1,
            latency: Default::default(),
            upload_bandwidth: Default::default(),
            download_bandwidth: Default::default(),
            wants_stream: Default::default(),
            end_call: Default::default(),
            stop_screenshare: Default::default(),
        }
    }

    async fn open_stream(
        &self,
        mut control: Option<&mut Control>,
        call_state: &EarlyCallState,
    ) -> Result<Stream> {
        // change the session state to accept incoming audio streams
        self.wants_stream.store(true, Relaxed);

        let stream_future = async {
            if let Some(control) = control.as_mut() {
                // if dialer, open stream
                control
                    .open_stream(call_state.peer, CHAT_PROTOCOL)
                    .await
                    .map_err(Error::from)
            } else {
                // if listener, receive stream
                self.stream_receiver.recv().await.map_err(Error::from)
            }
        };

        let stream_result = select! {
            _ = self.end_call.notified() => Ok(Err(ErrorKind::NoStream.into())),
            _ = self.stop_session.cancelled() => Ok(Err(ErrorKind::NoStream.into())),
            result = timeout(HELLO_TIMEOUT, stream_future) => result,
        };

        self.wants_stream.store(false, Relaxed);
        stream_result?
    }

    async fn receive_stream(&self) -> Result<Stream> {
        self.wants_stream.store(true, Relaxed);
        let result = self.stream_receiver.recv().await.map_err(Into::into);
        self.wants_stream.store(false, Relaxed);
        result
    }

    async fn teardown(&self) {
        // stops any call
        self.end_call.notify_one();
        // stops the session loop
        self.stop_session.cancel();
        // stops any active screenshare threads
        if let Some(notify) = self.stop_screenshare.lock().await.take() {
            notify.notify_waiters();
        }
    }
}

pub(crate) struct RoomState {
    peers: Vec<PeerId>,

    sender: MSender<RoomMessage>,

    cancel: CancellationToken,

    end_call: Arc<Notify>,

    early_state: EarlyCallState,
}

pub(crate) enum RoomMessage {
    Join {
        /// established audio transport
        audio_transport: Box<Transport<TransportStream>>,

        /// established early call state
        state: EarlyCallState,
    },
    Leave(PeerId),
}

struct RoomConnection {
    output: OutputHelper,
    handle: JoinHandle<Result<()>>,
}

pub(crate) struct OptionalCallArgs<'a> {
    audio_transport: Transport<TransportStream>,
    control_transport: &'a mut Transport<TransportStream>,
    message_receiver: &'a mut MReceiver<Message>,
    state: &'a Arc<SessionState>,
}

#[derive(Clone)]
pub(crate) struct StatisticsCollectorState {
    input_rms: Arc<AtomicF32>,
    output_rms: Arc<AtomicF32>,
    latency: Arc<AtomicUsize>,
    upload_bandwidth: Arc<AtomicUsize>,
    download_bandwidth: Arc<AtomicUsize>,
    loss: Arc<AtomicUsize>,
}

impl StatisticsCollectorState {
    fn new(state: Option<&Arc<SessionState>>) -> Self {
        Self {
            input_rms: Arc::new(Default::default()),
            output_rms: Arc::new(Default::default()),
            latency: state.map(|s| s.latency.clone()).unwrap_or_default(),
            upload_bandwidth: state
                .map(|s| s.upload_bandwidth.clone())
                .unwrap_or_default(),
            download_bandwidth: state
                .map(|s| s.download_bandwidth.clone())
                .unwrap_or_default(),
            loss: Arc::new(Default::default()),
        }
    }
}

#[derive(Debug)]
pub(crate) struct StartScreenshare {
    peer: PeerId,
    header: Option<Message>,
}

/// the state of a single connection during session negotiation
#[derive(Debug, Clone)]
pub(crate) struct ConnectionState {
    /// the latency is ms when available
    latency: Option<u128>,

    /// whether the connection is relayed
    pub(crate) relayed: bool,

    /// the underlying connection details
    pub(crate) endpoint: ConnectedPoint,

    /// tracks failed open stream attempts
    retries: Arc<AtomicUsize>,
}

impl From<ConnectedPoint> for ConnectionState {
    fn from(endpoint: ConnectedPoint) -> Self {
        Self {
            latency: None,
            relayed: endpoint.is_relayed(),
            endpoint,
            retries: Default::default(),
        }
    }
}
