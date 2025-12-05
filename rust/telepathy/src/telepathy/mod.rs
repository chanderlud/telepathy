/// implementations for core telepathy functionality
mod core;
/// helper methods used by telepathy core
mod helpers;
pub(crate) mod messages;
pub(crate) mod screenshare;
/// networking code for live audio streams
/// flutter_rust_bridge:ignore
mod sockets;
#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
pub(crate) mod tests;
pub(crate) mod utils;

#[cfg(target_family = "wasm")]
use crate::audio::web_audio::WebAudioWrapper;
use crate::error::{DartError, Error};
use crate::flutter::*;
use crate::overlay::overlay::Overlay;
use crate::overlay::{CONNECTED, LATENCY, LOSS};
use atomic_float::AtomicF32;
use chrono::Local;
#[cfg(not(target_family = "wasm"))]
use cpal::Device;
pub use cpal::Host;
#[cfg(not(target_family = "wasm"))]
use cpal::SupportedStreamConfig;
use cpal::traits::{DeviceTrait, HostTrait};
#[cfg(target_family = "wasm")]
use flutter_rust_bridge::JoinHandle;
use flutter_rust_bridge::{frb, spawn};
pub use kanal::AsyncReceiver;
use kanal::{AsyncSender, unbounded_async};
use libp2p::identity::Keypair;
use libp2p::swarm::ConnectionId;
use libp2p::{PeerId, Stream, StreamProtocol};
use libp2p_stream::Control;
use log::{debug, error, warn};
use messages::{Attachment, AudioHeader, Message};
use nnnoiseless::{FRAME_SIZE, RnnModel};
use sea_codec::ProcessorMessage;
use sockets::{Transport, TransportStream};
use std::collections::HashMap;
use std::mem;
pub use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc::{Receiver as MReceiver, Sender as MSender, channel};
use tokio::sync::{Mutex, Notify, RwLock};
#[cfg(not(target_family = "wasm"))]
use tokio::task::JoinHandle;
#[cfg(not(target_family = "wasm"))]
use tokio::time::interval;
use tokio::time::sleep;
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;
use utils::*;
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

#[frb(opaque)]
#[derive(Clone)]
pub struct Telepathy {
    /// The audio host
    host: Arc<Host>,

    /// Controls the threshold for silence detection
    rms_threshold: Arc<AtomicF32>,

    /// The factor to adjust the input volume by
    input_volume: Arc<AtomicF32>,

    /// The factor to adjust the output volume by
    output_volume: Arc<AtomicF32>,

    /// Enables rnnoise denoising
    denoise: Arc<AtomicBool>,

    /// The rnnoise model
    denoise_model: Arc<RwLock<RnnModel>>,

    /// Manually set the input device
    input_device: DeviceName,

    /// Manually set the output device
    output_device: DeviceName,

    /// The current libp2p private key
    identity: Arc<RwLock<Keypair>>,

    /// Keeps track of whether the user is in a call
    in_call: Arc<AtomicBool>,

    /// used to end an audio test, if there is one
    end_audio_test: Arc<Mutex<Option<Arc<Notify>>>>,

    /// Tracks state for the current room
    room_state: Arc<RwLock<Option<RoomState>>>,

    /// Disables the output stream
    deafened: Arc<AtomicBool>,

    /// Disables the input stream
    muted: Arc<AtomicBool>,

    /// Disables the playback of custom ringtones
    play_custom_ringtones: Arc<AtomicBool>,

    /// Enables sending your custom ringtone
    send_custom_ringtone: Arc<AtomicBool>,

    efficiency_mode: Arc<AtomicBool>,

    /// Keeps track of and controls the sessions
    session_states: Arc<RwLock<HashMap<PeerId, Arc<SessionState>>>>,

    /// Signals the session manager to start a new session
    start_session: MSender<PeerId>,

    /// Signals the session manager to start a screenshare
    start_screenshare: MSender<StartScreenshare>,

    /// Restarts the session manager when needed
    restart_manager: Arc<Notify>,

    /// Network configuration for p2p connections
    network_config: NetworkConfig,

    /// Configuration for the screenshare functionality
    #[allow(dead_code)]
    screenshare_config: ScreenshareConfig,

    /// A reference to the object that controls the call overlay
    overlay: Overlay,

    codec_config: CodecConfig,

    #[cfg(target_family = "wasm")]
    web_input: Arc<Mutex<Option<WebAudioWrapper>>>,

    /// callback methods provided by the flutter frontend
    callbacks: TelepathyCallbacks,
}

impl Telepathy {
    /// main entry point to Telepathy. must be async to use `spawn`
    pub async fn new(
        identity: Vec<u8>,
        host: Arc<Host>,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: TelepathyCallbacks,
    ) -> Telepathy {
        let (start_session, mut receive_session) = channel(8);
        let (start_screenshare, mut receive_screenshare) = channel(8);

        let chat = Self {
            host,
            rms_threshold: Default::default(),
            input_volume: Default::default(),
            output_volume: Default::default(),
            denoise: Default::default(),
            denoise_model: Default::default(),
            input_device: Default::default(),
            output_device: Default::default(),
            identity: Arc::new(RwLock::new(
                Keypair::from_protobuf_encoding(&identity).unwrap(),
            )),
            in_call: Default::default(),
            end_audio_test: Default::default(),
            room_state: Default::default(),
            deafened: Default::default(),
            muted: Default::default(),
            play_custom_ringtones: Default::default(),
            send_custom_ringtone: Default::default(),
            efficiency_mode: Default::default(),
            session_states: Default::default(),
            start_session,
            start_screenshare,
            restart_manager: Default::default(),
            network_config: network_config.clone(),
            screenshare_config: screenshare_config.clone(),
            overlay: overlay.clone(),
            codec_config: codec_config.clone(),
            #[cfg(target_family = "wasm")]
            web_input: Default::default(),
            callbacks,
        };

        // start the session manager
        let chat_clone = chat.clone();
        spawn(async move {
            loop {
                if let Err(error) = chat_clone
                    .session_manager(&mut receive_session, &mut receive_screenshare)
                    .await
                {
                    error!("Session manager failed: {}", error);
                }

                // just for safety
                sleep(Duration::from_millis(250)).await;
            }
        });

        // start the sessions
        chat.callbacks.start_sessions(&chat).await;
        chat
    }

    /// Tries to start a session for a contact
    pub async fn start_session(&self, contact: &Contact) {
        debug!("start_session called for {}", contact.peer_id);

        if self.start_session.send(contact.peer_id).await.is_err() {
            error!("start_session channel is closed");
        }
    }

    /// Attempts to start a call through an existing session
    pub async fn start_call(&self, contact: &Contact) -> std::result::Result<(), DartError> {
        if self.is_call_active().await {
            return Err("Cannot start call while a call is already active"
                .to_string()
                .into());
        }

        if let Some(state) = self.session_states.read().await.get(&contact.peer_id) {
            #[cfg(target_family = "wasm")]
            self.init_web_audio()
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
        if let Some(end_audio_test) = self.end_audio_test.lock().await.as_ref() {
            debug!("ending audio test");
            end_audio_test.notify_one();
        } else if let Some(room_state) = self.room_state.read().await.as_ref() {
            debug!("ending room");
            room_state.end_call.notify_one();
        } else if let Some(session_state) = self
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
        if self.is_call_active().await {
            return Err("Cannot join room while a call is already active"
                .to_string()
                .into());
        }

        #[cfg(target_family = "wasm")]
        self.init_web_audio()
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
        let call_state = self.setup_call(PeerId::random()).await?;
        // set room state
        self.room_state.write().await.replace(RoomState {
            peers: members.clone(),
            sender,
            cancel: cancel.clone(),
            end_call: end_call.clone(),
            early_state: call_state.clone(),
        });

        // tries to connect to every member of the room using existing sessions, or new ones if needed
        // note: sending your own identity to start_session is safe
        for member in members {
            if let Some(state) = self.session_states.read().await.get(&member) {
                state.start_call.notify_one();
            } else {
                // when the session opens, start_call will be notified
                _ = self.start_session.send(member).await;
            }
        }

        let self_clone = self.clone();
        spawn(async move {
            let stop_io = Default::default();
            if let Err(error) = self_clone
                .room_controller(receiver, cancel, call_state, &stop_io, end_call)
                .await
            {
                error!("error in room controller: {:?}", error);
            }

            stop_io.cancel();
        });

        Ok(())
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> std::result::Result<(), DartError> {
        if self.is_call_active().await {
            Err("Cannot restart manager while call is active"
                .to_string()
                .into())
        } else {
            self.restart_manager.notify_one();
            self.callbacks.start_sessions(self).await;
            Ok(())
        }
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: Vec<u8>) -> std::result::Result<(), DartError> {
        *self.identity.write().await =
            Keypair::from_protobuf_encoding(&key).map_err(Error::from)?;
        Ok(())
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
        if let Some(state) = self.session_states.write().await.remove(&contact.peer_id) {
            state.stop_session.notify_one();
        }
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> std::result::Result<(), DartError> {
        if self.is_call_active().await {
            return Err("Cannot start test while call is active".to_string().into());
        }

        #[cfg(target_family = "wasm")]
        self.init_web_audio()
            .await
            .map_err::<Error, _>(Error::into)?;

        let mut audio_config = self.setup_call(PeerId::random()).await?;
        audio_config.remote_configuration = audio_config.local_configuration.clone();
        let stop_io = CancellationToken::new();
        let end_call = Arc::new(Notify::new());

        self.in_call.store(true, Relaxed);
        *self.end_audio_test.lock().await = Some(end_call.clone());
        let result = self
            .call(&stop_io, audio_config, &end_call, None)
            .await
            .map_err(Into::into);
        stop_io.cancel();
        self.end_audio_test.lock().await.take();
        self.in_call.store(false, Relaxed);
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
        if let Some(state) = self.session_states.read().await.get(&message.receiver) {
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
        }

        Ok(())
    }

    pub async fn start_screenshare(&self, contact: &Contact) {
        self.send_start_screenshare(contact.peer_id, None).await;
    }

    #[frb(sync)]
    pub fn set_rms_threshold(&self, decimal: f32) {
        let threshold = db_to_multiplier(decimal);
        self.rms_threshold.store(threshold, Relaxed);
    }

    #[frb(sync)]
    pub fn set_input_volume(&self, decibel: f32) {
        let multiplier = db_to_multiplier(decibel);
        self.input_volume.store(multiplier, Relaxed);
    }

    #[frb(sync)]
    pub fn set_output_volume(&self, decibel: f32) {
        let multiplier = db_to_multiplier(decibel);
        self.output_volume.store(multiplier, Relaxed);
    }

    #[frb(sync)]
    pub fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Relaxed);
    }

    #[frb(sync)]
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Relaxed);
    }

    /// Changing the denoise flag will not affect the current call
    #[frb(sync)]
    pub fn set_denoise(&self, denoise: bool) {
        self.denoise.store(denoise, Relaxed);
    }

    #[frb(sync)]
    pub fn set_play_custom_ringtones(&self, play: bool) {
        self.play_custom_ringtones.store(play, Relaxed);
    }

    #[frb(sync)]
    pub fn set_send_custom_ringtone(&self, send: bool) {
        self.send_custom_ringtone.store(send, Relaxed);
    }

    #[frb(sync)]
    pub fn set_efficiency_mode(&self, enabled: bool) {
        self.efficiency_mode.store(enabled, Relaxed);
    }

    pub async fn set_input_device(&self, device: Option<String>) {
        *self.input_device.lock().await = device;
    }

    pub async fn set_output_device(&self, device: Option<String>) {
        *self.output_device.lock().await = device;
    }

    /// Lists the input and output devices
    pub fn list_devices(&self) -> std::result::Result<(Vec<String>, Vec<String>), DartError> {
        let input_devices = self.host.input_devices().map_err(Error::from)?;
        let output_devices = self.host.output_devices().map_err(Error::from)?;

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

        *self.denoise_model.write().await = model;
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

/// a state used for session negotiation
#[derive(Debug)]
pub(crate) struct PeerState {
    /// when true the peer's identity addresses will not be dialed
    dialed: bool,

    /// when true the peer is the dialer
    dialer: bool,

    /// a map of connections and their latencies
    connections: HashMap<ConnectionId, ConnectionState>,
}

impl PeerState {
    fn new(dialer: bool, connection_id: ConnectionId, relayed: bool) -> Self {
        let mut connections = HashMap::new();
        connections.insert(connection_id, ConnectionState::new(relayed));

        Self {
            dialed: false,
            dialer,
            connections,
        }
    }

    fn relayed_only(&self) -> bool {
        self.connections.iter().all(|(_, state)| state.relayed)
    }

    fn latencies_missing(&self) -> bool {
        self.connections
            .iter()
            .any(|(_, state)| state.latency.is_none())
    }
}

/// the state of a single connection during session negotiation
#[derive(Debug)]
struct ConnectionState {
    /// the latency is ms when available
    latency: Option<u128>,

    /// whether the connection is relayed
    relayed: bool,
}

impl ConnectionState {
    fn new(relayed: bool) -> Self {
        Self {
            latency: None,
            relayed,
        }
    }
}

/// shared values for a single session
pub(crate) struct SessionState {
    /// signals the session to initiate a call
    start_call: Notify,

    /// stops the session normally
    stop_session: Notify,

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
}

impl SessionState {
    fn new(message_sender: &MSender<Message>) -> Self {
        let stream_channel = unbounded_async();

        Self {
            start_call: Notify::new(),
            stop_session: Notify::new(),
            in_call: AtomicBool::new(false),
            message_sender: message_sender.clone(),
            stream_sender: stream_channel.0,
            stream_receiver: stream_channel.1,
            latency: Default::default(),
            upload_bandwidth: Default::default(),
            download_bandwidth: Default::default(),
            wants_stream: Default::default(),
            end_call: Default::default(),
        }
    }

    async fn open_stream(
        &self,
        mut control: Option<&mut Control>,
        call_state: &EarlyCallState,
    ) -> Result<Stream> {
        // change the session state to accept incoming audio streams
        self.wants_stream.store(true, Relaxed);

        let stream_result = if let Some(control) = control.as_mut() {
            // if dialer, open stream
            control
                .open_stream(call_state.peer, CHAT_PROTOCOL)
                .await
                .map_err(Error::from)
        } else {
            // if listener, receive stream
            self.stream_receiver.recv().await.map_err(Error::from)
        };

        self.wants_stream.store(false, Relaxed);
        stream_result
    }

    async fn receive_stream(&self) -> Result<Stream> {
        self.wants_stream.store(true, Relaxed);
        let result = self.stream_receiver.recv().await.map_err(Into::into);
        self.wants_stream.store(false, Relaxed);
        result
    }
}

struct RoomState {
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
    stream: SendStream,
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

/// Used for audio tests, plays the input into the output
async fn loopback(
    input_receiver: AsyncReceiver<ProcessorMessage>,
    output_sender: AsyncSender<ProcessorMessage>,
    cancel: &CancellationToken,
    end_call: &Arc<Notify>,
) {
    loop {
        select! {
            message = input_receiver.recv() => {
                if let Ok(message) = message {
                    if output_sender.try_send(message).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            },
            _ = end_call.notified() => {
                break;
            }
            _ = cancel.cancelled() => {
                break;
            },
        }
    }
}

/// Collects statistics from throughout the application, processes them, and provides them to the frontend
async fn statistics_collector(
    state: StatisticsCollectorState,
    callback: StatisticsCallback,
    cancel: CancellationToken,
) {
    // the interval for statistics updates
    let mut update_interval = interval(Duration::from_millis(100));
    // the interval for the input_max and output_max to decrease
    let mut reset_interval = interval(Duration::from_secs(5));
    // max input RMS
    let mut input_max = 0_f32;
    // max output RMS
    let mut output_max = 0_f32;

    loop {
        select! {
            _ = update_interval.tick() => {
                let latency = state.latency.load(Relaxed);
                let loss = state.loss.swap(0, Relaxed);

                callback.post(Statistics {
                    input_level: level_from_window(state.input_rms.swap(0_f32, Relaxed), &mut input_max),
                    output_level: level_from_window(state.output_rms.swap(0_f32, Relaxed), &mut output_max),
                    latency,
                    upload_bandwidth: state.upload_bandwidth.load(Relaxed),
                    download_bandwidth: state.download_bandwidth.load(Relaxed),
                    loss,
                }).await;

                LATENCY.store(latency, Relaxed);
                LOSS.store(loss, Relaxed);
            }
            _ = reset_interval.tick() => {
                input_max /= 2_f32;
                output_max /= 2_f32;
            }
            _ = cancel.cancelled() => {
                break;
            }
        }
    }

    // zero out the statistics when the collector ends
    callback.post(Statistics::default()).await;
    LATENCY.store(0, Relaxed);
    LOSS.store(0, Relaxed);
    CONNECTED.store(false, Relaxed);
    debug!("statistics collector returning");
}

fn stream_to_audio_transport(stream: Stream) -> Transport<TransportStream> {
    LengthDelimitedCodec::builder()
        .max_frame_length(TRANSFER_BUFFER_SIZE)
        .length_field_type::<u16>()
        .new_framed(stream.compat())
}
