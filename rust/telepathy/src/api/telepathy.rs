#![allow(clippy::type_complexity)]

use std::collections::{HashMap, VecDeque};
use std::mem;
#[cfg(not(target_family = "wasm"))]
use std::net::Ipv4Addr;
pub use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;

use crate::api::codec::{decoder, encoder};
use crate::api::contact::Contact;
use crate::api::error::{DartError, Error, ErrorKind};
#[cfg(target_os = "ios")]
use crate::api::ios::{configure_audio_session, deactivate_audio_session};
use crate::api::overlay::overlay::Overlay;
use crate::api::overlay::{CONNECTED, LATENCY, LOSS};
use crate::api::screenshare;
use crate::api::screenshare::{Decoder, Encoder};
use crate::api::utils::*;
#[cfg(target_family = "wasm")]
use crate::api::web_audio::{WebAudioWrapper, WebInput};
use crate::frb_generated::FLUTTER_RUST_BRIDGE_HANDLER;
use crate::{Behaviour, BehaviourEvent};
use atomic_float::AtomicF32;
use chrono::{DateTime, Local};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(not(target_family = "wasm"))]
use cpal::Device;
pub use cpal::Host;
use cpal::SupportedStreamConfig;
use flutter_rust_bridge::for_generated::futures::stream::{SplitSink, SplitStream};
use flutter_rust_bridge::for_generated::futures::SinkExt;
use flutter_rust_bridge::{frb, spawn, spawn_blocking_with, DartFnFuture};
#[cfg(not(target_family = "wasm"))]
use kanal::bounded;
pub use kanal::AsyncReceiver;
use kanal::{bounded_async, unbounded_async, AsyncSender, Receiver, Sender};
use libp2p::futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{ConnectionId, SwarmEvent};
#[cfg(not(target_family = "wasm"))]
use libp2p::tcp;
use libp2p::{
    autonat, dcutr, identify, noise, ping, yamux, Multiaddr, PeerId, Stream, StreamProtocol,
};
use libp2p_stream::Control;
use log::{debug, error, info, warn};
use messages::{Attachment, AudioHeader, Message};
use nnnoiseless::{DenoiseState, RnnModel, FRAME_SIZE};
use rubato::Resampler;
use sea_codec::ProcessorMessage;
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
#[cfg(not(target_family = "wasm"))]
use tokio::net::lookup_host;
use tokio::select;
use tokio::sync::{Mutex, Notify, RwLock};
#[cfg(not(target_family = "wasm"))]
use tokio::time::{interval, sleep_until, timeout, Instant, Interval};
use tokio_util::bytes::Bytes;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::std::Instant;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::{interval, sleep_until, timeout, Interval};

type Result<T> = std::result::Result<T, Error>;
pub(crate) type DeviceName = Arc<Mutex<Option<String>>>;
type TransportStream = Compat<Stream>;
pub type Transport<T> = Framed<T, LengthDelimitedCodec>;
type StartScreenshare = (PeerId, Option<Message>);
type AudioSocket = SplitSink<Transport<TransportStream>, Bytes>;
type RoomJoin = (Transport<TransportStream>, EarlyCallState);

/// The number of bytes in a single network audio frame
const TRANSFER_BUFFER_SIZE: usize = FRAME_SIZE * size_of::<i16>();
/// A timeout used when initializing the call
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// A timeout used to detect temporary network issues
const TIMEOUT_DURATION: Duration = Duration::from_millis(100);
/// the number of frames to hold in a channel
pub(crate) const CHANNEL_SIZE: usize = 2_400;
/// the protocol identifier for Telepathy
const CHAT_PROTOCOL: StreamProtocol = StreamProtocol::new("/telepathy/0.0.1");
#[cfg(target_family = "wasm")]
const SILENCE: [f32; FRAME_SIZE] = [0_f32; FRAME_SIZE];

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

    /// Notifies the call to end
    end_call: Arc<Notify>,

    /// Manually set the input device
    input_device: DeviceName,

    /// Manually set the output device
    output_device: DeviceName,

    /// Private key for signing the handshake
    identity: Arc<RwLock<Keypair>>,

    /// Keeps track of whether the user is in a call
    in_call: Arc<AtomicBool>,

    /// Tracks whether the user is in a room
    in_room: Arc<RwLock<Option<Vec<PeerId>>>>,

    /// Keeps the early call state the same across the whole room
    early_room_state: Arc<RwLock<Option<EarlyCallState>>>,

    room_control_sender: AsyncSender<RoomJoin>,

    room_control_receiver: AsyncReceiver<RoomJoin>,

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
    start_session: AsyncSender<PeerId>,

    /// Signals the session manager to start a screenshare
    start_screenshare: AsyncSender<StartScreenshare>,

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

    /// Prompts the user to accept a call
    accept_call:
        Arc<Mutex<dyn Fn(String, Option<Vec<u8>>, DartNotify) -> DartFnFuture<bool> + Send>>,

    /// Alerts the UI that a call has ended
    call_ended: Arc<Mutex<dyn Fn(String, bool) -> DartFnFuture<()> + Send>>,

    /// Fetches a contact from the front end
    get_contact: Arc<Mutex<dyn Fn(Vec<u8>) -> DartFnFuture<Option<Contact>> + Send>>,

    /// Notifies the frontend that the call has disconnected or reconnected
    call_state: Arc<Mutex<dyn Fn(bool) -> DartFnFuture<()> + Send>>,

    /// Alerts the UI when the status of a session changes
    session_status: Arc<Mutex<dyn Fn(String, String) -> DartFnFuture<()> + Send>>,

    /// Starts a session for each of the UI's contacts
    start_sessions: Arc<Mutex<dyn Fn(Telepathy) -> DartFnFuture<()> + Send>>,

    /// Used to report statistics to the frontend
    statistics: Arc<Mutex<dyn Fn(Statistics) -> DartFnFuture<()> + Send>>,

    /// Used to send chat messages to the frontend
    message_received: Arc<Mutex<dyn Fn(ChatMessage) -> DartFnFuture<()> + Send>>,

    /// Alerts the UI when the manager is active and restartable
    manager_active: Arc<Mutex<dyn Fn(bool, bool) -> DartFnFuture<()> + Send>>,

    /// Called when a screenshare starts
    #[allow(dead_code)]
    screenshare_started: Arc<Mutex<dyn Fn(DartNotify, bool) -> DartFnFuture<()> + Send>>,
}

impl Telepathy {
    // this function must be async to use `spawn`
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        identity: Vec<u8>,
        host: Arc<Host>,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        accept_call: impl Fn(String, Option<Vec<u8>>, DartNotify) -> DartFnFuture<bool> + Send + 'static,
        call_ended: impl Fn(String, bool) -> DartFnFuture<()> + Send + 'static,
        get_contact: impl Fn(Vec<u8>) -> DartFnFuture<Option<Contact>> + Send + 'static,
        call_state: impl Fn(bool) -> DartFnFuture<()> + Send + 'static,
        session_status: impl Fn(String, String) -> DartFnFuture<()> + Send + 'static,
        start_sessions: impl Fn(Telepathy) -> DartFnFuture<()> + Send + 'static,
        statistics: impl Fn(Statistics) -> DartFnFuture<()> + Send + 'static,
        message_received: impl Fn(ChatMessage) -> DartFnFuture<()> + Send + 'static,
        manager_active: impl Fn(bool, bool) -> DartFnFuture<()> + Send + 'static,
        screenshare_started: impl Fn(DartNotify, bool) -> DartFnFuture<()> + Send + 'static,
    ) -> Telepathy {
        let (start_session, session) = unbounded_async::<PeerId>();
        let (start_screenshare, screenshare) = unbounded_async::<StartScreenshare>();
        let (room_control_sender, room_control_receiver) = unbounded_async();

        let chat = Self {
            host,
            rms_threshold: Default::default(),
            input_volume: Default::default(),
            output_volume: Default::default(),
            denoise: Default::default(),
            denoise_model: Default::default(),
            end_call: Default::default(),
            input_device: Default::default(),
            output_device: Default::default(),
            identity: Arc::new(RwLock::new(
                Keypair::from_protobuf_encoding(&identity).unwrap(),
            )),
            in_call: Default::default(),
            in_room: Default::default(),
            early_room_state: Default::default(),
            room_control_sender,
            room_control_receiver,
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
            accept_call: Arc::new(Mutex::new(accept_call)),
            call_ended: Arc::new(Mutex::new(call_ended)),
            get_contact: Arc::new(Mutex::new(get_contact)),
            call_state: Arc::new(Mutex::new(call_state)),
            session_status: Arc::new(Mutex::new(session_status)),
            start_sessions: Arc::new(Mutex::new(start_sessions)),
            statistics: Arc::new(Mutex::new(statistics)),
            message_received: Arc::new(Mutex::new(message_received)),
            manager_active: Arc::new(Mutex::new(manager_active)),
            screenshare_started: Arc::new(Mutex::new(screenshare_started)),
        };

        // start the session manager
        let chat_clone = chat.clone();
        spawn(async move {
            let mut interval = interval(Duration::from_millis(100));

            loop {
                loop {
                    // retry the session manager if it fails, but not too fast
                    interval.tick().await;

                    if let Err(error) = chat_clone.session_manager(&session, &screenshare).await {
                        (chat_clone.manager_active.lock().await)(false, false).await;
                        error!("Session manager failed: {}", error);
                    } else {
                        // if the session manager succeeds, wait for restart signal
                        break;
                    }
                }

                debug!("Session manager waiting for restart signal");
                (chat_clone.manager_active.lock().await)(false, true).await;
                chat_clone.restart_manager.notified().await;
            }
        });

        // start the sessions
        (chat.start_sessions.lock().await)(chat.clone()).await;

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
        if self.in_call.load(Relaxed) || self.in_room.read().await.is_some() {
            return Ok(());
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

    /// Ends the call (if there is one)
    #[frb(sync)]
    pub fn end_call(&self) {
        self.end_call.notify_one();
    }

    pub async fn join_room(
        &self,
        member_strings: Vec<String>,
    ) -> std::result::Result<(), DartError> {
        if self.in_call.load(Relaxed) || self.in_room.read().await.is_some() {
            return Ok(());
        }

        #[cfg(target_family = "wasm")]
        self.init_web_audio()
            .await
            .map_err::<Error, _>(Error::into)?;

        // set the members of the room
        let members: Vec<_> = member_strings
            .into_iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        self.in_room.write().await.replace(members.clone());

        // the same early call state is used throughout the room, the real peer ids are set later
        let call_state = self.setup_call(PeerId::random()).await?;
        *self.early_room_state.write().await = Some(call_state.clone());

        for member in members {
            if let Some(state) = self.session_states.read().await.get(&member) {
                state.start_call.notify_one();
            } else {
                // when the session opens, start_call will be notified
                if self.start_session.send(member).await.is_err() {
                    error!("start_session channel is closed");
                }
            }
        }

        let self_clone = self.clone();
        spawn(async move {
            let stop_io = Default::default();
            if let Err(error) = self_clone.room_controller(call_state, &stop_io).await {
                error!("error in room controller: {:?}", error);
            }

            stop_io.notify_waiters();
        });

        Ok(())
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> std::result::Result<(), DartError> {
        if self.in_call.load(Relaxed) {
            Err(ErrorKind::InCall.into())
        } else {
            self.restart_manager.notify_one();
            (self.start_sessions.lock().await)(self.clone()).await;
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

            // clean up the session state
            self.session_states.write().await.remove(&contact.peer_id);
        }
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> std::result::Result<(), DartError> {
        self.in_call.store(true, Relaxed);

        let stop_io = Arc::new(Notify::new());

        #[cfg(target_family = "wasm")]
        self.init_web_audio()
            .await
            .map_err::<Error, _>(Error::into)?;

        let mut audio_config = self.setup_call(PeerId::random()).await?;
        audio_config.remote_configuration = audio_config.local_configuration.clone();

        let result = self
            .call(&stop_io, audio_config, None, None, None, None)
            .await
            .map_err(Into::into);

        stop_io.notify_waiters();
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
                .map_err(Error::from)?;
        }

        Ok(())
    }

    pub async fn start_screenshare(&self, contact: &Contact) -> std::result::Result<(), DartError> {
        self.start_screenshare
            .send((contact.peer_id, None))
            .await
            .map_err(Error::from)?;
        Ok(())
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

    #[cfg(target_family = "wasm")]
    async fn init_web_audio(&self) -> Result<()> {
        let wrapper = WebAudioWrapper::new().await?;
        *self.web_input.lock().await = Some(wrapper);
        Ok(())
    }

    /// Starts new sessions
    async fn session_manager(
        &self,
        start: &AsyncReceiver<PeerId>,
        screenshare: &AsyncReceiver<StartScreenshare>,
    ) -> Result<()> {
        let builder =
            libp2p::SwarmBuilder::with_existing_identity(self.identity.read().await.clone());

        let provider_phase;

        #[cfg(not(target_family = "wasm"))]
        {
            provider_phase = builder
                .with_tokio()
                .with_tcp(
                    tcp::Config::default().nodelay(true),
                    noise::Config::new,
                    yamux::Config::default,
                )
                .map_err(|_| ErrorKind::SwarmBuild)?
                .with_quic();
        }

        #[cfg(target_family = "wasm")]
        {
            provider_phase = builder
                .with_wasm_bindgen()
                .with_other_transport(|id_keys| {
                    Ok(libp2p_webtransport_websys::Transport::new(
                        libp2p_webtransport_websys::Config::new(id_keys),
                    ))
                })?;
        }

        let mut swarm = provider_phase
            .with_relay_client(noise::Config::new, yamux::Config::default)
            .map_err(|_| ErrorKind::SwarmBuild)?
            .with_behaviour(|keypair, relay_behaviour| Behaviour {
                relay_client: relay_behaviour,
                ping: ping::Behaviour::new(ping::Config::new()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/telepathy/0.0.1".to_string(),
                    keypair.public(),
                )),
                dcutr: dcutr::Behaviour::new(keypair.public().to_peer_id()),
                stream: libp2p_stream::Behaviour::new(),
                auto_nat: autonat::Behaviour::new(
                    keypair.public().to_peer_id(),
                    autonat::Config {
                        ..Default::default()
                    },
                ),
            })
            .map_err(|_| ErrorKind::SwarmBuild)?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();

        #[cfg(not(target_family = "wasm"))]
        let listen_port = *self.network_config.listen_port.read().await;

        #[cfg(not(target_family = "wasm"))]
        let listen_addr_quic = Multiaddr::empty()
            .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
            .with(Protocol::Udp(listen_port))
            .with(Protocol::QuicV1);

        #[cfg(not(target_family = "wasm"))]
        swarm.listen_on(listen_addr_quic)?;

        #[cfg(not(target_family = "wasm"))]
        let listen_addr_tcp = Multiaddr::empty()
            .with(Protocol::from(Ipv4Addr::UNSPECIFIED))
            .with(Protocol::Tcp(listen_port));

        #[cfg(not(target_family = "wasm"))]
        swarm.listen_on(listen_addr_tcp)?;

        let socket_address = *self.network_config.relay_address.read().await;
        let relay_identity = *self.network_config.relay_id.read().await;

        #[cfg(not(target_family = "wasm"))]
        let relay_address_udp = Multiaddr::from(socket_address.ip())
            .with(Protocol::Udp(socket_address.port()))
            .with(Protocol::QuicV1)
            .with_p2p(relay_identity)
            .map_err(|_| ErrorKind::SwarmBuild)?;

        #[cfg(not(target_family = "wasm"))]
        let relay_address_tcp = Multiaddr::from(socket_address.ip())
            .with(Protocol::Tcp(socket_address.port()))
            .with_p2p(relay_identity)
            .map_err(|_| ErrorKind::SwarmBuild)?;

        // TODO the relay currently does not support WebTransport
        #[cfg(target_family = "wasm")]
        let relay_address_web = Multiaddr::from(socket_address.ip())
            .with(Protocol::Udp(socket_address.port()))
            .with(Protocol::QuicV1)
            .with(Protocol::WebTransport)
            .with_p2p(relay_identity)
            .map_err(|_| ErrorKind::SwarmBuild)?;

        let relay_address;

        #[cfg(not(target_family = "wasm"))]
        if swarm.dial(relay_address_udp.clone()).is_err() {
            if let Err(error) = swarm.dial(relay_address_tcp.clone()) {
                return Err(error.into());
            } else {
                info!("connected to relay with tcp");
                relay_address = relay_address_tcp.with(Protocol::P2pCircuit);
            }
        } else {
            info!("connected to relay with udp");
            relay_address = relay_address_udp.with(Protocol::P2pCircuit);
        }

        #[cfg(target_family = "wasm")]
        if let Err(error) = swarm.dial(relay_address_web.clone()) {
            return Err(error.into());
        } else {
            info!("connected to relay with webtransport");
            relay_address = relay_address_web.with(Protocol::P2pCircuit);
        }

        let mut learned_observed_addr = false;
        let mut told_relay_observed_addr = false;

        loop {
            match swarm.next().await.ok_or(ErrorKind::SwarmEnded)? {
                SwarmEvent::NewListenAddr { .. } => (),
                SwarmEvent::Dialing { .. } => (),
                SwarmEvent::ConnectionEstablished { .. } => (),
                SwarmEvent::Behaviour(BehaviourEvent::Ping(_)) => (),
                SwarmEvent::NewExternalAddrCandidate { .. } => (),
                SwarmEvent::NewExternalAddrOfPeer { .. } => (),
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Sent {
                    ..
                })) => {
                    info!("Told relay its public address");
                    told_relay_observed_addr = true;
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                    info: identify::Info { .. },
                    ..
                })) => {
                    info!("Relay told us our observed address");
                    learned_observed_addr = true;
                }
                // no other event occurs during a successful initialization
                event => {
                    error!("Unexpected event during initialization {:?}", event);
                    return Err(ErrorKind::UnexpectedSwarmEvent.into());
                }
            }

            if learned_observed_addr && told_relay_observed_addr {
                break;
            }
        }

        swarm.listen_on(relay_address.clone())?;

        // alerts the UI that the manager is active
        (self.manager_active.lock().await)(true, true).await;

        // handle incoming streams
        let self_clone = self.clone();
        let control = swarm.behaviour().stream.new_control();
        spawn(async move {
            self_clone.incoming_stream_handler(control).await;
        });

        // handles the state needed for negotiating sessions
        // it is cleared each time a peer successfully connects
        let mut peer_states: HashMap<PeerId, PeerState> = HashMap::new();

        loop {
            let event = select! {
                // events are handled outside the select to help with spagetification
                event = swarm.select_next_some() => event,
                // restart the manager
                _ = self.restart_manager.notified() => {
                    break Err(ErrorKind::ManagerRestarted.into());
                }
                // start a new session
                result = start.recv() => {
                    let peer_id = result?;

                    if peer_id == self.identity.read().await.public().to_peer_id() {
                        // prevents dialing yourself
                        continue;
                    } else if swarm.is_connected(&peer_id) {
                        // TODO is it possible that this check can result in invalid states where two peers cannot get into a session?
                        // prevents dialing a peer who is already connected
                        warn!("{} is already connected", peer_id);
                        continue;
                    }

                    debug!("initial dial for {}", peer_id);

                    // dial the peer through the relay
                    if let Err(error) = swarm.dial(relay_address.clone().with(Protocol::P2p(peer_id))) {
                        error!("dial error for {}: {}", peer_id, error);
                        (self.session_status.lock().await)(peer_id.to_string(), "Inactive".to_string()).await;
                    } else {
                        (self.session_status.lock().await)(peer_id.to_string(), "Connecting".to_string()).await;
                    }

                    continue;
                }
                // starts a stream for outgoing screen shares
                result = screenshare.recv() => {
                    let (peer_id, header_option) = result?;
                    info!("starting screenshare for {} {:?}", peer_id, header_option);

                    #[cfg(not(target_family = "wasm"))]
                    if let Some(state) = self.session_states.read().await.get(&peer_id) {
                        let stop = Arc::new(Notify::new());
                        let dart_stop = DartNotify { inner: stop.clone() };

                        if let Some(Message::ScreenshareHeader { encoder_name }) = header_option {
                            if let Ok(stream) = swarm
                                .behaviour()
                                .stream
                                .new_control()
                                .open_stream(peer_id, CHAT_PROTOCOL)
                                .await {
                                let width = self.screenshare_config.width.load(Relaxed);
                                let height = self.screenshare_config.height.load(Relaxed);

                                spawn(screenshare::playback(stream, stop, state.download_bandwidth.clone(), encoder_name, width, height));
                                (self.screenshare_started.lock().await)(dart_stop, false).await;
                            }
                        } else if let Some(config) = self.screenshare_config.recording_config.read().await.clone() {
                            let message = Message::ScreenshareHeader { encoder_name: config.encoder.to_string() };

                            state
                                .message_sender
                                .send(message)
                                .await
                                .map_err(Error::from)?;

                            state.wants_stream.store(true, Relaxed);

                            if let Ok(stream) = state.stream_receiver.recv().await {
                                spawn(screenshare::record(stream, stop, state.upload_bandwidth.clone(), config));
                            }

                            state.wants_stream.store(false, Relaxed);
                            (self.screenshare_started.lock().await)(dart_stop, true).await;
                        } else {
                            // TODO this should be blocked from occurring via the frontend i think
                            warn!("screenshare started without recording configuration");
                        }
                    } else {
                        warn!("screenshare started for a peer without a session: {}", peer_id);
                    }

                    continue;
                }
            };

            match event {
                SwarmEvent::ConnectionEstablished {
                    peer_id,
                    endpoint,
                    connection_id,
                    ..
                } => {
                    if peer_id == *self.network_config.relay_id.read().await {
                        // ignore the relay connection
                        continue;
                    } else if self.session_states.read().await.contains_key(&peer_id) {
                        // ignore connections with peers who have a session
                        continue;
                    }

                    let contact_option = (self.get_contact.lock().await)(peer_id.to_bytes()).await;

                    let relayed = endpoint.is_relayed();
                    let listener = endpoint.is_listener();

                    if contact_option.is_none() && !self.is_in_room(&peer_id).await {
                        warn!("received a connection from an unknown peer: {:?}", peer_id);
                        if swarm.disconnect_peer_id(peer_id).is_err() {
                            error!("error disconnecting from unknown peer");
                        }
                    } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        // insert the new connection
                        peer_state
                            .connections
                            .insert(connection_id, ConnectionState::new(relayed));
                    } else {
                        info!(
                            "connection {} established with {} endpoint={:?} relayed={}",
                            connection_id, peer_id, endpoint, relayed
                        );

                        // insert the new state and new connection
                        peer_states
                            .insert(peer_id, PeerState::new(!listener, connection_id, relayed));

                        if listener {
                            // a stream will be established by the other client
                            // the dialer already has the connecting status set
                            (self.session_status.lock().await)(
                                peer_id.to_string(),
                                "Connecting".to_string(),
                            )
                            .await;
                        }
                    }
                }
                SwarmEvent::OutgoingConnectionError {
                    peer_id,
                    error,
                    connection_id,
                } => {
                    if let Some(peer_id) = peer_id {
                        if self.session_states.read().await.contains_key(&peer_id) {
                            warn!(
                                "outgoing connection failed for {} because {}",
                                peer_id, error
                            );
                        } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                            warn!(
                                "outgoing connection failed for {} because {}",
                                peer_id, error
                            );
                            peer_state.connections.remove(&connection_id);
                        } else {
                            // if an outgoing error occurs when no connection is active, the session initialization failed
                            error!(
                                "session initialization failed for {} because {}",
                                peer_id, error
                            );
                            (self.session_status.lock().await)(
                                peer_id.to_string(),
                                "Inactive".to_string(),
                            )
                            .await;
                        }
                    }
                }
                SwarmEvent::ConnectionClosed {
                    peer_id,
                    cause,
                    connection_id,
                    ..
                } => {
                    warn!(
                        "connection {} closed with {} cause={:?}",
                        connection_id, peer_id, cause
                    );

                    if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        peer_state.connections.remove(&connection_id);
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Ping(event)) => {
                    let latency = event.result.map(|duration| duration.as_millis()).ok();

                    // update the latency for the peer's session
                    if let Some(state) = self.session_states.read().await.get(&event.peer) {
                        state.latency.store(latency.unwrap_or(0) as usize, Relaxed);
                        continue; // the remaining logic is not needed while a session is active
                    }

                    // if the session is still connecting, update the latency and try to choose a connection
                    if let Some(peer_state) = peer_states.get_mut(&event.peer) {
                        // the dialer chooses the connection
                        if !peer_state.dialer {
                            continue;
                        }

                        // update the latency for the peer's connections
                        if let Some(connection_latency) =
                            peer_state.connections.get_mut(&event.connection)
                        {
                            connection_latency.latency = latency;
                        } else {
                            warn!(
                                "received a ping for an unknown connection id={}",
                                event.connection
                            );
                        }

                        info!("connection states: {:?}", peer_state.connections);

                        if peer_state.latencies_missing() {
                            // only start a session if all connections have latency
                            debug!("not trying to establish a session with {} because not all connections have latency", event.peer);
                            continue;
                        } else if peer_state.relayed_only() {
                            // only start a session if there is a non-relayed connection
                            debug!("not trying to establish a session with {} because all connections are relayed", event.peer);
                            continue;
                        }

                        // choose the connection with the lowest latency, prioritizing non-relay connections
                        let connection = peer_state
                            .connections
                            .iter()
                            .min_by(|a, b| {
                                match (a.1.relayed, b.1.relayed) {
                                    (false, true) => std::cmp::Ordering::Less, // prioritize non-relay connections
                                    (true, false) => std::cmp::Ordering::Greater, // prioritize non-relay connections
                                    _ => a.1.latency.cmp(&b.1.latency), // compare latencies if both have the same relay status
                                }
                            })
                            .map(|(id, _)| id);

                        if let Some(connection_id) = connection {
                            info!("using connection id={} for {}", connection_id, event.peer);

                            // close the other connections
                            peer_state
                                .connections
                                .iter()
                                .filter(|(id, _)| *id != connection_id)
                                .for_each(|(id, _)| {
                                    swarm.close_connection(*id);
                                });

                            let control = swarm.behaviour().stream.new_control();

                            // open a session control stream and start the session controller
                            self.open_stream(event.peer, control, &mut peer_states)
                                .await;
                        } else {
                            warn!("no connection available for {}", event.peer);
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                    peer_id,
                    info,
                    ..
                })) => {
                    if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        if peer_state.dialed || !peer_state.dialer {
                            continue;
                        } else {
                            peer_state.dialed = true;
                        }
                    } else {
                        // the relay server sends identity events which will be caught here
                        continue;
                    }

                    info!("Received first identify event from {}", peer_id);

                    for address in info.listen_addrs {
                        // checks for relayed addresses which are not useful
                        if address.ends_with(&Protocol::P2p(peer_id).into()) {
                            continue;
                        }

                        // dials the non-relayed addresses to attempt direct connections
                        if let Err(error) = swarm.dial(address) {
                            error!("Error dialing {}: {}", peer_id, error);
                        }
                    }
                }
                // TODO validate that this logic successfully handles cases where the relay is the only available connection
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(dcutr::Event {
                    remote_peer_id,
                    result,
                })) => {
                    debug!("ductr event with {}: {:?}", remote_peer_id, result);

                    if let Some(peer_state) = peer_states.get(&remote_peer_id) {
                        if peer_state.relayed_only() && result.is_err() {
                            info!("ductr failed while relayed_only, falling back to relay");
                            let control = swarm.behaviour().stream.new_control();
                            self.open_stream(remote_peer_id, control, &mut peer_states)
                                .await;
                        }
                    }
                }
                event => {
                    debug!("other swarm event: {:?}", event);
                }
            }
        }
    }

    /// Handles incoming streams for the libp2p swarm
    async fn incoming_stream_handler(&self, mut control: Control) {
        while let Ok(mut incoming_streams) = control.accept(CHAT_PROTOCOL) {
            while let Some((peer, stream)) = incoming_streams.next().await {
                let state_option = self.session_states.read().await.get(&peer).cloned();

                if let Some(state) = state_option {
                    if state.wants_stream.load(Relaxed) {
                        info!("sub-stream accepted for {}", peer);

                        if let Err(error) = state.stream_sender.send(stream).await {
                            error!("error sending sub-stream to {}: {}", peer, error);
                        }

                        continue;
                    } else {
                        warn!("received a stream while {} did not want sub-stream, starting new session", peer);
                    }
                } else {
                    info!("stream accepted for new session with {}", peer);
                }

                if let Err(error) = self._start_session(peer, None, stream).await {
                    error!("error starting session with {}: {}", peer, error);
                }
            }

            info!("incoming streams ended, trying to restart");
        }

        warn!("stopped accepting incoming streams; restarting controller");
        self.restart_manager.notify_one();
    }

    /// Called by the dialer to open a stream and session
    async fn open_stream(
        &self,
        peer_id: PeerId,
        mut control: Control,
        peer_states: &mut HashMap<PeerId, PeerState>,
    ) {
        // it may take multiple tries to open the stream because the of the (dumb) RNG in the stream handler
        loop {
            match control.open_stream(peer_id, CHAT_PROTOCOL).await {
                Ok(stream) => {
                    info!("opened stream with {}, starting new session", peer_id);

                    if let Err(error) = self._start_session(peer_id, Some(control), stream).await {
                        error!("error starting session with {}: {}", peer_id, error);
                    }

                    // the peer state is no longer needed
                    peer_states.remove(&peer_id);
                    break;
                }
                Err(error) => {
                    error!("error opening stream {}: {}", peer_id, error);
                }
            }
        }
    }

    /// A wrapper which manages the session throughout its lifetime
    async fn _start_session(
        &self,
        peer_id: PeerId,
        mut control: Option<Control>,
        stream: Stream,
    ) -> Result<()> {
        let contact_option = (self.get_contact.lock().await)(peer_id.to_bytes()).await;
        // sends messages to the session from elsewhere in the program
        let message_channel = unbounded_async::<Message>();
        // create the state and a clone of it for the session
        let state = Arc::new(SessionState::new(&message_channel.0));
        // insert the new state
        let old_state_option = self
            .session_states
            .write()
            .await
            .insert(peer_id, state.clone());

        if let Some(old_state) = old_state_option {
            warn!("{} already has a session", peer_id);

            if old_state.in_call.load(Relaxed) {
                // if the session was in a call, end it so the session can end
                self.end_call.notify_one();
            }

            // stop the session
            old_state.stop_session.notify_one();
        }

        let contact = if let Some(contact) = contact_option {
            // alert the UI that this session is now connected
            (self.session_status.lock().await)(peer_id.to_string(), "Connected".to_string()).await;
            contact
        } else {
            // there may be no contact for members of a group
            Contact {
                id: Uuid::new_v4().to_string(),
                nickname: String::from("GroupContact"),
                peer_id,
            }
        };

        let self_clone = self.clone();
        spawn(async move {
            // the length delimited transport used for the session
            let mut transport = LengthDelimitedCodec::builder()
                .max_frame_length(usize::MAX)
                .length_field_type::<u64>()
                .new_framed(stream.compat());

            // the dialer for room sessions always starts a call
            if self_clone.is_in_room(&peer_id).await && control.is_some() {
                state.start_call.notify_one();
            }

            // controls keep alive messages
            let mut keep_alive = interval(Duration::from_secs(10));

            let result = loop {
                let future = self_clone.session(
                    &contact,
                    control.as_mut(),
                    &mut transport,
                    &state,
                    &message_channel,
                    &mut keep_alive,
                );

                if let Err(error) = future.await {
                    if state.in_call.load(Relaxed) {
                        info!("session error while call active, alerting ui");
                        (self_clone.call_ended.lock().await)(error.to_string(), false).await;
                    }

                    match error.kind {
                        ErrorKind::KanalReceive(_)
                        | ErrorKind::TransportRecv
                        | ErrorKind::TransportSend => break Err(error),
                        ErrorKind::SessionStopped => break Ok(()),
                        _ => error!("Session error: {:?}", error),
                    }
                }

                // the session is now safe to restart
                state.in_call.store(false, Relaxed);
            };

            // result is *only* Ok when the session has been stopped
            if let Err(error) = result {
                error!("Session error for {}: {}", contact.nickname, error);
            } else {
                warn!("Session for {} stopped", contact.nickname);
                return; // the session has already been cleaned up
            }

            // cleanup
            self_clone
                .session_states
                .write()
                .await
                .remove(&contact.peer_id);

            (self_clone.session_status.lock().await)(contact.peer_id(), "Inactive".to_string())
                .await;

            info!("Session for {} cleaned up", contact.nickname);
        });

        Ok(())
    }

    /// The session logic
    async fn session(
        &self,
        contact: &Contact,
        control: Option<&mut Control>,
        transport: &mut Transport<TransportStream>,
        state: &Arc<SessionState>,
        message_channel: &(AsyncSender<Message>, AsyncReceiver<Message>),
        keep_alive: &mut Interval,
    ) -> Result<()> {
        info!("[{}] session waiting for event", contact.nickname);

        select! {
            result = read_message::<Message, _>(transport) => {
                let mut other_ringtone = None;
                let remote_audio_header;
                // let remote_in_room;

                info!("received {:?} from {}", result, contact.nickname);

                match result? {
                    Message::Hello { ringtone, audio_header, .. } => {
                        remote_audio_header = audio_header;
                        // remote_in_room = room;
                        if self.play_custom_ringtones.load(Relaxed) {
                            other_ringtone = ringtone;
                        }
                    },
                    Message::KeepAlive | Message::ConnectionInterrupted | Message::ConnectionRestored => {
                        warn!("session for {} ending expected Hello", contact.nickname);
                        return Ok::<(), Error>(());
                    },
                    message => {
                        warn!("received unexpected {:?} from {}", message, contact.nickname);
                        return Ok::<(), Error>(());
                    }
                }

                let is_in_room = self.is_in_room(&contact.peer_id).await;
                let mut cancel_prompt = None;
                let mut accept_handle = None;

                if is_in_room {
                    // automatically accept calls from member of current room
                } else if self.in_call.load(Relaxed) {
                    // do not accept another call if already in one
                    write_message(transport, &Message::Busy).await?;
                    return Ok(());
                } else {
                    // TODO when another user in a room tries to call the room, any user not in the room already will hit this case & not have the state required to successfully join the room
                    let other_cancel_prompt = Arc::new(Notify::new());
                    // a cancel Notify that can be used in the frontend
                    let dart_cancel = DartNotify { inner: Arc::clone(&other_cancel_prompt) };
                    cancel_prompt = Some(other_cancel_prompt);

                    let accept_call_clone = Arc::clone(&self.accept_call);
                    let contact_id = contact.id.clone();
                    accept_handle = Some(spawn(async move {
                        (accept_call_clone.lock().await)(contact_id, other_ringtone, dart_cancel).await
                    }));
                }

                state.in_call.store(true, Relaxed); // blocks the session from being restarted

                let accept_future = async {
                    if let Some(accept_handle) = accept_handle {
                        accept_handle.await
                    } else {
                        Ok(true)
                    }
                };

                select! {
                    accepted = accept_future => {
                        if accepted? {
                            // respond with hello ack containing audio header
                            let mut call_state = self.setup_call(contact.peer_id).await?;
                            call_state.remote_configuration = remote_audio_header;
                            write_message(transport, &Message::HelloAck { audio_header: call_state.local_configuration.clone() }).await?;

                            if is_in_room {
                                self.room_handshake(transport, control, state, call_state).await?;
                            } else {
                                // normal call handshake
                                self.call_handshake(transport, control, &message_channel.1, state, call_state).await?;
                            }

                            keep_alive.reset(); // start sending normal keep alive messages
                        } else {
                            // reject the call if not accepted
                            write_message(transport, &Message::Reject).await?;
                        }
                    }
                    result = read_message::<Message, _>(transport) => {
                        info!("received message while accept call was pending");

                        match result {
                            Ok(Message::Goodbye { .. }) => {
                                info!("received goodbye from {} while prompting for call", contact.nickname);
                                if let Some(cancel) = cancel_prompt {
                                    cancel.notify_one();
                                }
                            }
                            Ok(message) => {
                                warn!("received unexpected {:?} from {} while prompting for call", message, contact.nickname);
                            }
                            Err(error) => {
                                error!("Error reading message while prompting for call from {}: {}", contact.nickname, error);
                            }
                        }
                    }
                }

                Ok(())
            }
            _ = state.start_call.notified() => {
                state.in_call.store(true, Relaxed); // blocks the session from being restarted

                let is_in_room = self.is_in_room(&contact.peer_id).await;
                // load custom ringtone if enabled
                let other_ringtone = self.load_ringtone().await;
                // initialize call state
                let mut call_state = self.setup_call(contact.peer_id).await?;
                // when custom ringtone is used wait longer for a response to account for extra data being sent in Hello
                let hello_timeout = HELLO_TIMEOUT + if other_ringtone.is_some() { Duration::from_secs(10) } else { Default::default() };
                // queries the other client for a call
                write_message(transport, &Message::Hello { ringtone: other_ringtone, audio_header: call_state.local_configuration.clone(), room: is_in_room }).await?;

                loop {
                    select! {
                        result = timeout(hello_timeout, read_message(transport)) => {
                            // handles a variety of outcomes in response to Hello
                            match result?? {
                                Message::HelloAck { audio_header } => {
                                    call_state.remote_configuration = audio_header;

                                    if is_in_room {
                                        self.room_handshake(transport, control, state, call_state).await?;
                                    } else {
                                        // normal call handshake
                                        self.call_handshake(transport, control, &message_channel.1, state, call_state).await?;
                                    }

                                    keep_alive.reset(); // start sending normal keep alive messages
                                }
                                Message::Reject => {
                                    (self.call_ended.lock().await)(format!("{} did not accept the call", contact.nickname), true).await;
                                },
                                Message::Busy => {
                                    (self.call_ended.lock().await)(format!("{} is busy", contact.nickname), true).await;
                                }
                                // keep alive messages are sometimes received here
                                Message::KeepAlive => continue,
                                message => {
                                    // the front end needs to know that the call ended here
                                    (self.call_ended.lock().await)(format!("Received an unexpected message from {}", contact.nickname), true).await;
                                    warn!("received unexpected {:?} from {} [stopped call process]", message, contact.nickname);
                                }
                            }

                            break;
                        }
                        _ = self.end_call.notified() => {
                            info!("end call notified while waiting for hello ack");
                            write_message(transport, &Message::Goodbye { reason: None }).await?;
                        }
                    }
                }

                Ok(())
            }
            // state will never notify while a call is active
            _ = state.stop_session.notified() => {
                info!("session state stop notified for {}", contact.nickname);
                Err(ErrorKind::SessionStopped.into())
            },
            _ = keep_alive.tick() => {
                debug!("sending keep alive to {}", contact.nickname);
                write_message(transport, &Message::KeepAlive).await?;
                Ok(())
            },
        }
    }

    /// Gets everything ready for the call
    async fn call_handshake(
        &self,
        transport: &mut Transport<TransportStream>,
        control: Option<&mut Control>,
        message_receiver: &AsyncReceiver<Message>,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
        let stream = state.open_stream(transport, control, &call_state).await?;

        // change the app call state
        self.in_call.store(true, Relaxed);
        // show the overlay
        self.overlay.show();

        // stop_io must notify when the call ends, so it is external to the call function
        let stop_io = Arc::new(Notify::new());

        let result = self
            .call(
                &stop_io,
                call_state,
                Some(stream_to_audio_transport(stream)),
                Some(transport),
                Some(message_receiver.clone()),
                Some(state),
            )
            .await;

        info!("call ended in handshake");

        // ensure that all background i/o threads are stopped
        stop_io.notify_waiters();
        info!("notified stop_io");

        // the call has ended
        self.in_call.store(false, Relaxed);
        // hide the overlay
        self.overlay.hide();

        match result {
            Ok(()) => Ok(()),
            Err(error) => match error.kind {
                ErrorKind::NoInputDevice
                | ErrorKind::NoOutputDevice
                | ErrorKind::BuildStream(_)
                | ErrorKind::StreamConfig(_) => {
                    let message = Message::Goodbye {
                        reason: Some("Audio device error".to_string()),
                    };
                    write_message(transport, &message).await?;
                    Err(error)
                }
                _ => {
                    let message = Message::Goodbye {
                        reason: Some(error.to_string()),
                    };
                    write_message(transport, &message).await?;
                    Err(error)
                }
            },
        }
    }

    /// The bulk of the call logic
    async fn call(
        &self,
        stop_io: &Arc<Notify>,
        call_state: EarlyCallState,
        audio_transport: Option<Transport<TransportStream>>,
        control_transport: Option<&mut Transport<TransportStream>>,
        message_receiver: Option<AsyncReceiver<Message>>,
        state: Option<&Arc<SessionState>>,
    ) -> Result<()> {
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // channels used for moving values to the statistics collector
        let (input_rms_sender, input_rms_receiver) = self.efficiency_channel();
        let (output_rms_sender, output_rms_receiver) = self.efficiency_channel();

        // shared values for various statistics
        let latency = state
            .map(|state| Arc::clone(&state.latency))
            .unwrap_or_default();
        let upload_bandwidth = state
            .map(|state| Arc::clone(&state.upload_bandwidth))
            .unwrap_or_default();
        let download_bandwidth = state
            .map(|state| Arc::clone(&state.download_bandwidth))
            .unwrap_or_default();

        // the two clients agree on these codec options
        let codec_config = call_state.codec_config();

        let (input_receiver, input_sender) = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                codec_config,
                input_rms_sender,
            )
            .await?;

        let (output_sender, output_stream) = self
            .setup_output(
                call_state.remote_configuration.sample_rate as f64,
                codec_config.0,
                output_rms_sender,
            )
            .await?;

        #[cfg(not(target_family = "wasm"))]
        let input_stream = self.setup_input_stream(&call_state, input_sender)?;

        // play the output stream
        output_stream.stream.play()?;
        // play the input stream (non web)
        #[cfg(not(target_family = "wasm"))]
        input_stream.stream.play()?;
        // play the input stream (web)
        #[cfg(target_family = "wasm")]
        if let Some(web_input) = self.web_input.lock().await.as_ref() {
            web_input.resume();
        } else {
            return Err(ErrorKind::NoInputDevice.into());
        }

        spawn(statistics_collector(
            input_rms_receiver,
            output_rms_receiver,
            latency,
            Arc::clone(&upload_bandwidth),
            Arc::clone(&download_bandwidth),
            Arc::clone(&self.statistics),
            Arc::clone(stop_io),
        ));

        match (audio_transport, control_transport, message_receiver) {
            (Some(audio_transport), Some(transport), Some(message_receiver)) => {
                let (socket_sender, socket_receiver) = unbounded_async();
                let (receiving_sender, receiving_receiver) = unbounded_async::<bool>();
                let (write, read) = audio_transport.split();
                socket_sender.send(write).await?;

                spawn(audio_input(
                    input_receiver,
                    socket_receiver,
                    Arc::clone(stop_io),
                    upload_bandwidth,
                ));

                spawn(audio_output(
                    output_sender,
                    read,
                    Arc::clone(stop_io),
                    download_bandwidth,
                    Some(receiving_sender),
                ));

                let controller_future = self.call_controller(
                    transport,
                    message_receiver,
                    receiving_receiver,
                    call_state.peer,
                );

                info!("call controller starting");

                match controller_future.await {
                    Ok(message) => {
                        (self.call_ended.lock().await)(message.unwrap_or_default(), true).await
                    }
                    Err(error) => match error.kind {
                        ErrorKind::CallEnded => (), // when the call is ended externally, no UI notification is needed
                        _ => (self.call_ended.lock().await)(error.to_string(), false).await,
                    },
                }

                stop_io.notify_waiters();
                info!("call controller returned and was handled, call returning");
            }
            _ => {
                spawn(loopback(input_receiver, output_sender, Arc::clone(stop_io)));
                self.end_call.notified().await;
            }
        }

        // on ios the audio session must be deactivated
        #[cfg(target_os = "ios")]
        deactivate_audio_session();

        #[cfg(target_family = "wasm")]
        {
            // drop the web input to free resources & stop input processor
            *self.web_input.lock().await = None;
        }

        Ok(())
    }

    /// controller for normal calls
    async fn call_controller(
        &self,
        transport: &mut Transport<TransportStream>,
        receiver: AsyncReceiver<Message>,
        receiving: AsyncReceiver<bool>,
        peer: PeerId,
    ) -> Result<Option<String>> {
        let identity = self.identity.read().await.public().to_peer_id();

        // in case this value has been used in a previous call, reset to false
        CONNECTED.store(false, Relaxed);

        // whether the session is currently receiving audio
        let mut is_receiving = false;
        // whether the remote peer is currently receiving audio
        let mut remote_is_receiving = true;

        // the instant the UI will be notified that the session is not receiving audio
        let mut notify_ui = Instant::now() + Duration::from_secs(2);

        // the instant the session stopped receiving audio
        let mut disconnected_at = Instant::now();

        // the instant the disconnect started and the duration of the disconnect
        let mut disconnect_durations: VecDeque<(Instant, Duration)> = VecDeque::new();
        // ticks to update the connection quality and remove old entries from `disconnect_durations`
        let mut update_durations = interval(Duration::from_secs(1));

        // constant durations used in the connection quality algorithm
        let window_duration = Duration::from_secs(10);
        let disconnect_duration = Duration::from_millis(1_500);

        let (state_sender, state_receiver) = unbounded_async::<bool>();

        loop {
            select! {
                // receives and handles messages from the callee
                result = read_message(transport) => {
                    let message: Message = result?;

                    match message {
                        Message::Goodbye { reason } => {
                            debug!("received goodbye, reason = {:?}", reason);
                            break Ok(reason);
                        },
                        Message::Chat { text, attachments } => {
                            (self.message_received.lock().await)(ChatMessage {
                                text,
                                receiver: identity,
                                timestamp: Local::now(),
                                attachments,
                            }).await;
                        }
                        Message::ConnectionInterrupted => {
                            // info!("received connection interrupted message r={} rr={}", is_receiving, remote_is_receiving);

                            let receiving = is_receiving && remote_is_receiving;
                            remote_is_receiving = false;
                            state_sender.send(receiving).await?;
                        }
                        Message::ConnectionRestored => {
                            // info!("received connection restored message r={} rr={}", is_receiving, remote_is_receiving);

                            if remote_is_receiving {
                                warn!("received connection restored message while already receiving");
                                continue;
                            }

                            remote_is_receiving = true;
                            state_sender.send(false).await?;
                        }
                        Message::ScreenshareHeader { .. } => {
                            info!("received screenshare header {:?}", message);
                            self.start_screenshare.send((peer, Some(message))).await?;
                        }
                        _ => error!("call controller unexpected message: {:?}", message),
                    }
                },
                // sends messages to the callee
                result = receiver.recv() => {
                    if let Ok(message) = result {
                        write_message(transport, &message).await?;
                    } else {
                        // if the channel closes, the call has ended
                        break Ok(None);
                    }
                },
                // ends the call
                _ = self.end_call.notified() => {
                    write_message(transport, &Message::Goodbye { reason: None }).await?;
                    break Err(ErrorKind::CallEnded.into());
                },
                receiving = state_receiver.recv() => {
                    if receiving? {
                        // the instant the disconnect began
                        disconnected_at = Instant::now();
                        // notify the ui in 2 seconds if the disconnect hasn't ended
                        notify_ui = disconnected_at + disconnect_duration;
                    } else if is_receiving && remote_is_receiving {
                        let elapsed = disconnected_at.elapsed();

                        // update the overlay to connected
                        if !CONNECTED.swap(true, Relaxed) {
                            // update the call state in the UI
                            (self.call_state.lock().await)(false).await;
                        }

                        // record the disconnect
                        disconnect_durations.push_back((disconnected_at, elapsed));
                        // prevents any notification to the ui as audio is being received
                        notify_ui = Instant::now() + Duration::from_secs(86400 * 365 * 30);
                    }
                },
                // receives when the receiving state changes
                Ok(receiving) = receiving.recv() => {
                    if receiving != is_receiving {
                        state_sender.send(is_receiving && remote_is_receiving).await?;

                        is_receiving = receiving;

                        let message = if is_receiving {
                            Message::ConnectionRestored
                        } else {
                            Message::ConnectionInterrupted
                        };

                        if let Err(error) = write_message(transport, &message).await {
                            error!("Error sending connection notification message: {}", error);
                        }
                    } else {
                        // TODO it appears that this is happening occasionally
                        warn!("received duplicate receiving state: {}", receiving);
                    }
                },
                // if the session doesn't reconnect within the time limit, notify the UI
                _ = sleep_until(notify_ui) => {
                    (self.call_state.lock().await)(true).await;
                    // the UI does not need to be notified until the session reconnects
                    notify_ui = Instant::now() + Duration::from_secs(86400 * 365 * 30);
                    // set the overlay to disconnected
                    CONNECTED.store(false, Relaxed);
                },
                _ = update_durations.tick() => {
                    let now = Instant::now();

                    // check for disconnects outside the 10-second window
                    while let Some((start, _)) = disconnect_durations.front() {
                        if now - *start > window_duration {
                            disconnect_durations.pop_front();
                        } else {
                            break;
                        }
                    }

                    let mut total_disconnect = disconnect_durations.iter().fold(Duration::default(), |acc, (_, duration)| acc + *duration).as_millis();

                    // if not receiving, add the current disconnect duration
                    if !is_receiving || !remote_is_receiving {
                        total_disconnect += disconnected_at.elapsed().as_millis();
                    }

                    LOSS.store(total_disconnect as f64 / 10_000_f64, Relaxed);
                }
            }
        }
    }

    async fn room_handshake(
        &self,
        transport: &mut Transport<TransportStream>,
        control: Option<&mut Control>,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
        let stream = state.open_stream(transport, control, &call_state).await?;
        let transport = stream_to_audio_transport(stream);
        self.room_control_sender
            .send((transport, call_state))
            .await?;
        Ok(())
    }

    async fn room_controller(
        &self,
        call_state: EarlyCallState,
        stop_io: &Arc<Notify>,
    ) -> Result<()> {
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // the two clients agree on these codec options
        let (socket_sender, socket_receiver) = unbounded_async();
        let upload_bandwidth: Arc<AtomicUsize> = Default::default();
        let download_bandwidth: Arc<AtomicUsize> = Default::default();
        let mut output_streams = Vec::new();

        let (input_receiver, input_sender) = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                (true, true, 5_f32),
                None,
            )
            .await?;

        #[cfg(not(target_family = "wasm"))]
        let input_stream = self.setup_input_stream(&call_state, input_sender)?;

        // play the input stream (non web)
        #[cfg(not(target_family = "wasm"))]
        input_stream.stream.play()?;
        // play the input stream (web)
        #[cfg(target_family = "wasm")]
        if let Some(web_input) = self.web_input.lock().await.as_ref() {
            web_input.resume();
        } else {
            return Err(ErrorKind::NoInputDevice.into());
        }

        spawn(audio_input(
            input_receiver,
            socket_receiver,
            Arc::clone(stop_io),
            upload_bandwidth.clone(),
        ));

        while let Ok((transport, state)) = self.room_control_receiver.recv().await {
            info!("received room control message for {:?}", state.peer);

            let (write, read) = transport.split();
            socket_sender.send(write).await?; // TODO write always needs a SEA header sent to it i think

            let (output_sender, output_stream) = self
                .setup_output(state.remote_configuration.sample_rate as f64, true, None)
                .await?;

            output_stream.stream.play()?;
            output_streams.push(output_stream);

            spawn(audio_output(
                output_sender,
                read,
                Arc::clone(stop_io),
                download_bandwidth.clone(),
                None,
            ));
        }

        Ok(())
    }

    /// helper method to set up audio input stack between the network and device layers
    async fn setup_input(
        &self,
        sample_rate: f64,
        codec_options: (bool, bool, f32),
        input_rms_sender: Option<Sender<f32>>,
    ) -> Result<(AsyncReceiver<ProcessorMessage>, Sender<f32>)> {
        // input stream -> input processor
        #[cfg(not(target_family = "wasm"))]
        let (input_sender, input_receiver) = bounded::<f32>(CHANNEL_SIZE);

        #[cfg(target_family = "wasm")]
        let input_receiver = {
            if let Some(web_input) = self.web_input.lock().await.as_ref() {
                WebInput::from(web_input)
            } else {
                return Err(ErrorKind::NoInputDevice.into());
            }
        };

        // input processor -> encoder or sending socket
        let (processed_input_sender, processed_input_receiver) =
            unbounded_async::<ProcessorMessage>();

        // encoder -> sending socket
        let (encoded_input_sender, encoded_input_receiver) = unbounded_async::<ProcessorMessage>();

        let (codec_enabled, vbr, residual_bits) = codec_options;
        let denoise = self.denoise.load(Relaxed);
        // get a reference to input volume for the processor
        let input_volume = Arc::clone(&self.input_volume);
        // get a reference to the rms threshold for the processor
        let rms_threshold = Arc::clone(&self.rms_threshold);
        // get a reference to the muted flag for the processor
        let muted = Arc::clone(&self.muted);
        // get a sync version of the processed input sender
        let processed_input_sender = processed_input_sender.to_sync();
        // the rnnoise denoiser
        let denoiser = denoise.then_some(DenoiseState::from_model(
            self.denoise_model.read().await.clone(),
        ));

        // spawn the input processor thread
        spawn_blocking_with(
            move || {
                input_processor(
                    input_receiver,
                    processed_input_sender,
                    sample_rate,
                    input_volume,
                    rms_threshold,
                    muted,
                    denoiser,
                    input_rms_sender,
                    codec_enabled,
                )
            },
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        // if using codec, spawn extra encoder thread
        if codec_enabled {
            let encoder_receiver = processed_input_receiver.clone_sync();
            let encoder_sender = encoded_input_sender.clone_sync();

            spawn_blocking_with(
                move || {
                    encoder(
                        encoder_receiver,
                        encoder_sender,
                        if denoise { 48_000 } else { sample_rate as u32 },
                        vbr,
                        residual_bits,
                    );
                },
                FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
            );
        }

        Ok((
            if codec_enabled {
                encoded_input_receiver
            } else {
                processed_input_receiver
            },
            input_sender,
        ))
    }

    /// helper method to set up audio output stack above network layer
    async fn setup_output(
        &self,
        remote_sample_rate: f64,
        codec_enabled: bool,
        output_rms_sender: Option<Sender<f32>>,
    ) -> Result<(AsyncSender<ProcessorMessage>, SendStream)> {
        // receiving socket -> output processor or decoder
        let (network_output_sender, network_output_receiver) =
            bounded_async::<ProcessorMessage>(CHANNEL_SIZE / FRAME_SIZE);

        // decoder -> output processor
        let (decoded_output_sender, decoded_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // output processor -> output stream
        #[cfg(not(target_family = "wasm"))]
        let (output_sender, output_receiver) = bounded::<f32>(CHANNEL_SIZE);

        // output processor -> output stream
        #[cfg(target_family = "wasm")]
        let output_sender = Arc::new(wasm_sync::Mutex::new(Vec::new()));
        #[cfg(target_family = "wasm")]
        let web_output = output_sender.clone();

        // get the output device and its default configuration
        let output_device = get_output_device(&self.output_device, &self.host).await?;
        let output_config = output_device.default_output_config()?;
        info!("output device: {:?}", output_device.name());

        // the ratio of the output sample rate to the remote input sample rate
        let ratio = output_config.sample_rate().0 as f64 / remote_sample_rate;
        // get a reference to output volume for the processor
        let output_volume = Arc::clone(&self.output_volume);
        // do this outside the output processor thread
        let output_processor_receiver = if codec_enabled {
            decoded_output_receiver.to_sync()
        } else {
            network_output_receiver.clone_sync()
        };

        // spawn the output processor thread
        spawn_blocking_with(
            move || {
                output_processor(
                    output_processor_receiver,
                    output_sender,
                    ratio,
                    output_volume,
                    output_rms_sender,
                )
            },
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        if codec_enabled {
            let decoder_receiver = network_output_receiver.to_sync();
            let decoder_sender = decoded_output_sender.clone_sync();

            spawn_blocking_with(
                move || {
                    decoder(decoder_receiver, decoder_sender);
                },
                FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
            );
        }

        // get the output channels for chunking the output
        let output_channels = output_config.channels() as usize;
        // a reference to the flag for use in the output callback
        let deafened = Arc::clone(&self.deafened);
        let end_call = Arc::clone(&self.end_call);

        let output_stream = SendStream {
            stream: output_device.build_output_stream(
                &output_config.into(),
                move |output: &mut [f32], _: &_| {
                    if deafened.load(Relaxed) {
                        output.fill(0_f32);
                        return;
                    }

                    // unwrap is safe because this mutex should never be poisoned
                    #[cfg(target_family = "wasm")]
                    let mut data = web_output.lock().unwrap();
                    // get the len before moving data
                    #[cfg(target_family = "wasm")]
                    let data_len = data.len();
                    // get enough samples to fill the output if possible
                    #[cfg(target_family = "wasm")]
                    let mut samples = data.drain(..(output.len() / output_channels).min(data_len));

                    for frame in output.chunks_mut(output_channels) {
                        #[cfg(not(target_family = "wasm"))]
                        let sample = output_receiver.recv().unwrap_or(0_f32);
                        #[cfg(target_family = "wasm")]
                        let sample = samples.next().unwrap_or(0_f32);

                        // write the sample to all the channels
                        for channel in frame.iter_mut() {
                            *channel = sample;
                        }
                    }
                },
                move |err| {
                    error!("Error in output stream: {}", err);
                    end_call.notify_one();
                },
                None,
            )?,
        };

        Ok((network_output_sender, output_stream))
    }

    /// Helper method to set up non-web audio input stream
    #[cfg(not(target_family = "wasm"))]
    fn setup_input_stream(
        &self,
        call_state: &EarlyCallState,
        input_sender: Sender<f32>,
    ) -> Result<SendStream> {
        let input_channels = call_state.local_configuration.channels as usize;
        let end_call = Arc::clone(&self.end_call);

        Ok(SendStream {
            stream: call_state.input_device.build_input_stream(
                &call_state.input_config.clone().into(),
                move |input, _: &_| {
                    for frame in input.chunks(input_channels) {
                        _ = input_sender.try_send(frame[0]);
                    }
                },
                move |err| {
                    error!("Error in input stream: {}", err);
                    end_call.notify_one();
                },
                None,
            )?,
        })
    }

    /// helper method to set up EarlyCallState
    async fn setup_call(&self, peer: PeerId) -> Result<EarlyCallState> {
        // if there is an early room state, use it w/ the real peer id
        if let Some(mut state) = self.early_room_state.read().await.clone() {
            state.peer = peer;
            return Ok(state);
        }

        #[cfg(not(target_family = "wasm"))]
        let input_device;
        #[cfg(not(target_family = "wasm"))]
        let input_config;

        let input_sample_rate;
        let input_sample_format;
        let input_channels;

        #[cfg(not(target_family = "wasm"))]
        {
            // get the input device and its default configuration
            input_device = self.get_input_device().await?;
            input_config = input_device.default_input_config()?;
            info!("input_device: {:?}", input_device.name());
            input_sample_rate = input_config.sample_rate().0;
            input_sample_format = input_config.sample_format().to_string();
            input_channels = input_config.channels() as usize;
        }

        #[cfg(target_family = "wasm")]
        {
            if let Some(web_input) = self.web_input.lock().await.as_ref() {
                input_sample_rate = web_input.sample_rate as u32;
            } else {
                return Err(ErrorKind::NoInputDevice.into());
            }

            input_sample_format = String::from("f32");
            input_channels = 1; // only ever 1 channel on web
        }

        // load the shared codec config values
        let config_codec_enabled = self.codec_config.enabled.load(Relaxed);
        let config_vbr = self.codec_config.vbr.load(Relaxed);
        let config_residual_bits = self.codec_config.residual_bits.load(Relaxed);

        let mut local_configuration = AudioHeader {
            channels: input_channels as u32,
            sample_rate: input_sample_rate,
            sample_format: input_sample_format,
            codec_enabled: config_codec_enabled,
            vbr: config_vbr,
            residual_bits: config_residual_bits as f64,
        };

        // rnnoise requires a 48kHz sample rate
        if self.denoise.load(Relaxed) {
            local_configuration.sample_rate = 48_000;
        }

        Ok(EarlyCallState {
            peer,
            local_configuration,
            remote_configuration: AudioHeader::default(),
            #[cfg(not(target_family = "wasm"))]
            input_config,
            #[cfg(not(target_family = "wasm"))]
            input_device,
        })
    }

    /// helper method to get the user specified device or default as fallback
    #[cfg(not(target_family = "wasm"))]
    async fn get_input_device(&self) -> Result<Device> {
        match *self.input_device.lock().await {
            Some(ref name) => Ok(self
                .host
                .input_devices()?
                .find(|device| {
                    if let Ok(ref device_name) = device.name() {
                        name == device_name
                    } else {
                        false
                    }
                })
                .unwrap_or(
                    self.host
                        .default_input_device()
                        .ok_or(ErrorKind::NoInputDevice)?,
                )),
            None => self
                .host
                .default_input_device()
                .ok_or(ErrorKind::NoInputDevice.into()),
        }
    }

    /// helper method for building channels that can be disabled by efficiency mode
    fn efficiency_channel(&self) -> (Option<Sender<f32>>, Option<AsyncReceiver<f32>>) {
        if self.efficiency_mode.load(Relaxed) {
            (None, None)
        } else {
            let (sender, receiver) = unbounded_async();
            (Some(sender.to_sync()), Some(receiver))
        }
    }

    /// helper method to load pre-encoded ringtone bytes
    async fn load_ringtone(&self) -> Option<Vec<u8>> {
        if self.send_custom_ringtone.load(Relaxed) {
            if let Ok(mut file) = File::open("ringtone.sea").await {
                let mut buffer = Vec::new();

                if let Err(error) = file.read_to_end(&mut buffer).await {
                    warn!("failed to read ringtone: {:?}", error);
                    None
                } else {
                    Some(buffer)
                }
            } else {
                warn!("failed to find ringtone");
                None
            }
        } else {
            None
        }
    }

    /// helper method to check if a peer is in the current room
    async fn is_in_room(&self, peer_id: &PeerId) -> bool {
        self.in_room
            .read()
            .await
            .as_ref()
            .map(|m| m.contains(peer_id))
            .unwrap_or(false)
    }
}

/// state used early in the call before it starts
#[derive(Clone)]
struct EarlyCallState {
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
struct PeerState {
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
struct SessionState {
    /// signals the session to initiate a call
    start_call: Notify,

    /// stops the session normally
    stop_session: Notify,

    /// if the session is in a call
    in_call: AtomicBool,

    /// a reusable sender for messages while a call is active
    message_sender: AsyncSender<Message>,

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
}

impl SessionState {
    fn new(message_sender: &AsyncSender<Message>) -> Self {
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
        }
    }

    async fn open_stream(
        &self,
        transport: &mut Transport<TransportStream>,
        mut control: Option<&mut Control>,
        call_state: &EarlyCallState,
    ) -> Result<Stream> {
        // change the session state to accept incoming audio streams
        self.wants_stream.store(true, Relaxed);

        // TODO evaluate this loop's performance in handling unexpected messages
        loop {
            let future = async {
                let stream = if let Some(control) = control.as_mut() {
                    // if dialer, open stream
                    control.open_stream(call_state.peer, CHAT_PROTOCOL).await?
                } else {
                    // if listener, receive stream
                    self.stream_receiver.recv().await?
                };

                Ok::<_, Error>(stream)
            };

            select! {
                stream = future => {
                    // change the session state back
                    self.wants_stream.store(false, Relaxed);
                    break stream
                },
                // handle unexpected messages while waiting for the audio stream
                // these messages tend to be from previous calls close together
                result = read_message::<Message, _>(transport) => {
                    warn!("received unexpected message while waiting for audio stream: {:?}", result);
                    // return Err(ErrorKind::UnexpectedMessage.into());
                }
            }
        }
    }
}

/// processed statistics for the frontend
#[derive(Default)]
pub struct Statistics {
    /// a percentage of the max input volume in the window
    pub input_level: f32,

    /// a percentage of the max output volume in the window
    pub output_level: f32,

    /// the current call latency
    pub latency: usize,

    /// the approximate upload bandwidth used by the current call
    pub upload_bandwidth: usize,

    /// the approximate download bandwidth used by the current call
    pub download_bandwidth: usize,

    /// a value between 0 and 1 representing the percent of audio lost in a sliding window
    pub loss: f64,
}

#[frb(opaque)]
#[derive(Clone)]
pub struct NetworkConfig {
    /// the relay server's address
    relay_address: Arc<RwLock<SocketAddr>>,

    /// the relay server's peer id
    relay_id: Arc<RwLock<PeerId>>,

    /// the libp2p port for the swarm
    listen_port: Arc<RwLock<u16>>,
}

impl NetworkConfig {
    #[frb(sync)]
    pub fn new(relay_address: String, relay_id: String) -> std::result::Result<Self, DartError> {
        Ok(Self {
            relay_address: Arc::new(RwLock::new(relay_address.parse().map_err(Error::from)?)),
            relay_id: Arc::new(RwLock::new(
                PeerId::from_str(&relay_id).map_err(Error::from)?,
            )),
            listen_port: Arc::new(RwLock::new(0)),
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn set_relay_address(
        &self,
        relay_address: String,
    ) -> std::result::Result<(), DartError> {
        if let Some(address) = lookup_host(&relay_address)
            .await
            .map_err(Error::from)?
            .next()
        {
            *self.relay_address.write().await = address;
            Ok(())
        } else {
            Err("Failed to resolve address".to_string().into())
        }
    }

    #[cfg(target_family = "wasm")]
    pub async fn set_relay_address(
        &self,
        relay_address: String,
    ) -> std::result::Result<(), DartError> {
        *self.relay_address.write().await = SocketAddr::from_str(&relay_address)
            .map_err(|error| DartError::from(error.to_string()))?;
        Ok(())
    }

    pub async fn get_relay_address(&self) -> String {
        self.relay_address.read().await.to_string()
    }

    pub async fn set_relay_id(&self, relay_id: String) -> std::result::Result<(), DartError> {
        *self.relay_id.write().await = PeerId::from_str(&relay_id).map_err(Error::from)?;
        Ok(())
    }

    pub async fn get_relay_id(&self) -> String {
        self.relay_id.read().await.to_string()
    }
}

#[frb(opaque)]
#[derive(Clone, Serialize, Deserialize)]
pub struct ScreenshareConfig {
    /// the screenshare capabilities. default until loaded
    #[serde(skip)]
    capabilities: Arc<RwLock<Capabilities>>,

    /// a validated recording configuration
    #[serde(with = "rwlock_option_recording_config")]
    recording_config: Arc<RwLock<Option<RecordingConfig>>>,

    /// the default width of the playback window
    #[serde(
        serialize_with = "atomic_u32_serialize",
        deserialize_with = "atomic_u32_deserialize"
    )]
    width: Arc<AtomicU32>,

    /// the default height of the playback window
    #[serde(
        serialize_with = "atomic_u32_serialize",
        deserialize_with = "atomic_u32_deserialize"
    )]
    height: Arc<AtomicU32>,
}

impl Default for ScreenshareConfig {
    fn default() -> Self {
        Self {
            capabilities: Default::default(),
            recording_config: Default::default(),
            width: Arc::new(AtomicU32::new(1280)),
            height: Arc::new(AtomicU32::new(720)),
        }
    }
}

impl ScreenshareConfig {
    // this function must be async to use spawn
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub async fn new(config_str: String) -> Self {
        let config: ScreenshareConfig = serde_json::from_str(&config_str).unwrap_or_default();

        let capabilities_clone = Arc::clone(&config.capabilities);
        spawn(async move {
            let now = Instant::now();
            let c = Capabilities::new().await;
            *capabilities_clone.write().await = c;
            info!("Capabilities loaded in {:?}", now.elapsed());
        });

        config
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub async fn new(_config_str: String) -> Self {
        Self::default()
    }

    pub async fn capabilities(&self) -> Capabilities {
        self.capabilities.read().await.clone()
    }

    pub async fn recording_config(&self) -> Option<RecordingConfig> {
        self.recording_config.read().await.clone()
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub async fn update_recording_config(
        &self,
        encoder: String,
        device: String,
        bitrate: u32,
        framerate: u32,
        height: Option<u32>,
    ) -> std::result::Result<(), DartError> {
        let encoder = Encoder::from_str(&encoder).map_err(|_| ErrorKind::InvalidEncoder)?;

        let recording_config = RecordingConfig {
            encoder,
            device: screenshare::Device::from_str(&device)
                .map_err(|_| "Invalid device".to_string())?,
            bitrate,
            framerate,
            height,
        };

        if let Ok(status) = recording_config.test_config().await {
            if status.success() {
                *self.recording_config.write().await = Some(recording_config);
                return Ok(());
            }
        }

        Err("Invalid configuration".to_string().into())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub async fn update_recording_config(
        &self,
        _encoder: String,
        _device: String,
        _bitrate: u32,
        _framerate: u32,
        _height: Option<u32>,
    ) -> std::result::Result<(), DartError> {
        Ok(())
    }

    #[frb(sync)]
    pub fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

/// capabilities for ffmpeg and ffplay supported by this client
#[derive(Default, Debug, Clone)]
#[frb(opaque)]
pub struct Capabilities {
    pub(crate) _available: bool,

    pub(crate) encoders: Vec<Encoder>,

    pub(crate) _decoders: Vec<Decoder>,

    pub(crate) devices: Vec<screenshare::Device>,
}

impl Capabilities {
    #[frb(sync)]
    pub fn encoders(&self) -> Vec<String> {
        self.encoders.iter().map(|e| e.to_string()).collect()
    }

    #[frb(sync)]
    pub fn devices(&self) -> Vec<String> {
        self.devices.iter().map(|d| d.to_string()).collect()
    }
}

/// recording config for screenshare
#[derive(Debug, Clone, Serialize, Deserialize)]
#[frb(opaque)]
pub struct RecordingConfig {
    pub(crate) encoder: Encoder,

    pub(crate) device: screenshare::Device,

    pub(crate) bitrate: u32,

    pub(crate) framerate: u32,

    /// the height for the video output
    pub(crate) height: Option<u32>,
}

impl RecordingConfig {
    #[frb(sync)]
    pub fn encoder(&self) -> String {
        let encoder_str: &str = self.encoder.into();
        encoder_str.to_string()
    }

    #[frb(sync)]
    pub fn device(&self) -> String {
        self.device.to_string()
    }

    #[frb(sync)]
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    #[frb(sync)]
    pub fn framerate(&self) -> u32 {
        self.framerate
    }

    #[frb(sync)]
    pub fn height(&self) -> Option<u32> {
        self.height
    }
}

#[frb(opaque)]
#[derive(Clone)]
pub struct CodecConfig {
    /// whether to use the codec
    enabled: Arc<AtomicBool>,

    /// whether to use variable bitrate
    vbr: Arc<AtomicBool>,

    /// the compression level
    residual_bits: Arc<AtomicF32>,
}

impl CodecConfig {
    #[frb(sync)]
    pub fn new(enabled: bool, vbr: bool, residual_bits: f32) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            vbr: Arc::new(AtomicBool::new(vbr)),
            residual_bits: Arc::new(AtomicF32::new(residual_bits)),
        }
    }

    #[frb(sync)]
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Relaxed);
    }

    #[frb(sync)]
    pub fn set_vbr(&self, vbr: bool) {
        self.vbr.store(vbr, Relaxed);
    }

    #[frb(sync)]
    pub fn set_residual_bits(&self, residual_bits: f32) {
        self.residual_bits.store(residual_bits, Relaxed);
    }

    #[frb(sync)]
    pub fn to_values(&self) -> (bool, bool, f32) {
        (
            self.enabled.load(Relaxed),
            self.vbr.load(Relaxed),
            self.residual_bits.load(Relaxed),
        )
    }
}

/// a shared notifier that can be passed to dart code
#[frb(opaque)]
pub struct DartNotify {
    inner: Arc<Notify>,
}

impl DartNotify {
    /// public notified function for dart
    pub async fn notified(&self) {
        self.inner.notified().await;
    }

    /// notifies one waiter
    #[frb(sync)]
    pub fn notify(&self) {
        self.inner.notify_waiters();
    }
}

pub struct ChatMessage {
    pub text: String,

    receiver: PeerId,

    timestamp: DateTime<Local>,

    pub(crate) attachments: Vec<Attachment>,
}

impl ChatMessage {
    #[frb(sync)]
    pub fn is_sender(&self, identity: String) -> bool {
        self.receiver.to_string() != identity
    }

    #[frb(sync)]
    pub fn time(&self) -> String {
        self.timestamp.format("%l:%M %p").to_string()
    }

    #[frb(sync)]
    pub fn attachments(&self) -> Vec<(String, Vec<u8>)> {
        self.attachments
            .iter()
            .map(|a| (a.name.clone(), a.data.clone()))
            .collect()
    }

    #[frb(sync)]
    pub fn clear_attachments(&mut self) {
        for attachment in self.attachments.iter_mut() {
            attachment.data.truncate(0);
        }
    }
}

/// Receives frames of audio data from the input processor and sends them to the socket
async fn audio_input(
    input_receiver: AsyncReceiver<ProcessorMessage>,
    socket_receiver: AsyncReceiver<AudioSocket>,
    stop_io: Arc<Notify>,
    bandwidth: Arc<AtomicUsize>,
) -> Result<()> {
    // a static byte used as the silence signal
    let silence_byte = Bytes::from_static(&[0]);
    let mut sockets: Vec<AudioSocket> = Vec::new();

    let future = async {
        loop {
            select! {
                Ok(socket) = socket_receiver.recv() => {
                    sockets.push(socket); // new connection established
                }
                Ok(message) = input_receiver.recv() => {
                    let bytes = match message {
                        ProcessorMessage::Silence => silence_byte.clone(),
                        ProcessorMessage::Data(bytes) => bytes,
                        ProcessorMessage::Samples(_) => {
                            warn!("audio input received Samples");
                            continue;
                        },
                    };

                    // send the bytes to all connections
                    for socket in &mut sockets {
                        socket.send(bytes.clone()).await?;
                    }

                    // update bandwidth
                    bandwidth.fetch_add(bytes.len() * sockets.len(), Relaxed);
                }
                else => return Ok::<(), Error>(()),
            }
        }
    };

    select! {
        // this future should never complete
        result = future => result,
        _ = stop_io.notified() => {
            debug!("Input to socket ended");
            Ok(())
        },
    }
}

/// Receives audio data from the socket and sends it to the output processor
async fn audio_output(
    sender: AsyncSender<ProcessorMessage>,
    mut socket: SplitStream<Transport<TransportStream>>,
    stop_io: Arc<Notify>,
    bandwidth: Arc<AtomicUsize>,
    receiving: Option<AsyncSender<bool>>,
) -> Result<()> {
    let mut is_receiving = false;

    let future = async {
        loop {
            match timeout(TIMEOUT_DURATION, socket.next()).await {
                Ok(Some(Ok(message))) => {
                    if !is_receiving {
                        is_receiving = true;
                        if let Some(ref sender) = receiving {
                            _ = sender.send(is_receiving).await;
                        }
                    }

                    let len = message.len();
                    bandwidth.fetch_add(len, Relaxed);

                    match len {
                        1 => match message[0] {
                            0 => _ = sender.try_send(ProcessorMessage::silence())?, // silence
                            _ => error!("received unknown control signal {}", message[0]),
                        },
                        _ => {
                            sender.try_send(ProcessorMessage::bytes(message.freeze()))?;
                        }
                    }
                }
                Ok(Some(Err(error))) => {
                    error!("Socket output error: {}", error);
                    break Err(error.into());
                }
                Ok(None) => {
                    debug!("Socket output ended with None");
                    break Ok(());
                }
                Err(_) if is_receiving => {
                    is_receiving = false;
                    if let Some(ref sender) = receiving {
                        _ = sender.send(is_receiving).await;
                    }
                }
                Err(_) => (),
            }
        }
    };

    select! {
        result = future => result,
        _ = stop_io.notified() => {
            debug!("Socket output ended from stop_io");
            Ok(())
        },
    }
}

/// Used for audio tests, plays the input into the output
async fn loopback(
    input_receiver: AsyncReceiver<ProcessorMessage>,
    output_sender: AsyncSender<ProcessorMessage>,
    notify: Arc<Notify>,
) {
    let loopback_future = async {
        while let Ok(message) = input_receiver.recv().await {
            if output_sender.try_send(message).is_err() {
                break;
            }
        }
    };

    select! {
        result = loopback_future => result,
        _ = notify.notified() => {
            debug!("Loopback ended");
        },
    }
}

/// Collects statistics from throughout the application, processes them, and provides them to the frontend
async fn statistics_collector(
    input_receiver: Option<AsyncReceiver<f32>>,
    output_receiver: Option<AsyncReceiver<f32>>,
    latency: Arc<AtomicUsize>,
    upload_bandwidth: Arc<AtomicUsize>,
    download_bandwidth: Arc<AtomicUsize>,
    callback: Arc<Mutex<dyn Fn(Statistics) -> DartFnFuture<()> + Send>>,
    notify: Arc<Notify>,
) -> Result<()> {
    // the interval for statistics updates
    let mut update_interval = interval(Duration::from_millis(100));
    // the interval for the input_max and output_max to decrease
    let mut reset_interval = interval(Duration::from_secs(5));

    let stop = Arc::new(AtomicBool::new(false));

    let mut input_max = 0_f32;
    let mut output_max = 0_f32;

    let stop_clone = Arc::clone(&stop);
    spawn(async move {
        notify.notified().await;
        stop_clone.store(true, Relaxed);
        info!("statistics collector set stop=true");
    });

    while !stop.load(Relaxed) {
        select! {
            _ = update_interval.tick() => {
                let statistics = Statistics {
                    input_level: if let Some(r) = input_receiver.as_ref() {
                        level_from_window(r, &mut input_max).await
                    } else {
                        0_f32
                    },
                    output_level: if let Some(r) = output_receiver.as_ref() {
                        level_from_window(r, &mut output_max).await
                    } else {
                        0_f32
                    },
                    latency: latency.load(Relaxed),
                    upload_bandwidth: upload_bandwidth.load(Relaxed),
                    download_bandwidth: download_bandwidth.load(Relaxed),
                    loss: LOSS.load(Relaxed),
                };

                LATENCY.store(statistics.latency, Relaxed);
                (callback.lock().await)(statistics).await;
            }
            _ = reset_interval.tick() => {
                input_max /= 2_f32;
                output_max /= 2_f32;
            }
        }
    }

    // zero out the statistics when the collector ends
    let statistics = Statistics::default();
    (callback.lock().await)(statistics).await;

    LATENCY.store(0, Relaxed);
    LOSS.store(0_f64, Relaxed);
    CONNECTED.store(false, Relaxed);

    info!("statistics collector returning");
    Ok(())
}

/// Processes the audio input and sends it to the sending socket
#[allow(clippy::too_many_arguments)]
fn input_processor(
    #[cfg(not(target_family = "wasm"))] receiver: Receiver<f32>,
    #[cfg(target_family = "wasm")] web_input: WebInput,
    sender: Sender<ProcessorMessage>,
    sample_rate: f64,
    input_factor: Arc<AtomicF32>,
    rms_threshold: Arc<AtomicF32>,
    muted: Arc<AtomicBool>,
    mut denoiser: Option<Box<DenoiseState>>,
    rms_sender: Option<Sender<f32>>,
    codec_enabled: bool,
) -> Result<()> {
    // the maximum and minimum values for i16 as f32
    let max_i16_f32 = i16::MAX as f32;
    let min_i16_f32 = i16::MIN as f32;
    let i16_size = size_of::<i16>();

    let ratio = if denoiser.is_some() {
        // rnnoise requires a 48kHz sample rate
        48_000_f64 / sample_rate
    } else {
        // do not resample if not using rnnoise
        1_f64
    };

    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 + 10_f64) as usize;
    let in_len = (FRAME_SIZE as f64 / ratio).ceil() as usize;

    let mut resampler = resampler_factory(ratio, 1, in_len)?;

    // the input for the resampler
    let mut pre_buf = [vec![0_f32; in_len]];
    // the output for the resampler
    let mut post_buf = [vec![0_f32; post_len]];
    // the output for rnnoise
    let mut out_buf = [0_f32; FRAME_SIZE];

    // output for 16 bit samples. the compiler does not recognize that it is used
    #[allow(unused_assignments)]
    let mut int_buffer = [0; FRAME_SIZE];

    // the position in pre_buf
    let mut position = 0;
    // a counter user for short silence detection
    let mut silence_length = 0_u8;

    loop {
        #[cfg(not(target_family = "wasm"))]
        {
            if let Ok(sample) = receiver.recv() {
                pre_buf[0][position] = sample;
                position += 1;
            } else {
                break;
            }
        }

        #[cfg(target_family = "wasm")]
        {
            if web_input.finished.load(Relaxed) {
                break;
            }

            if let Ok(mut data) = web_input.pair.0.lock() {
                if data.is_empty() {
                    let c = |i: &mut Vec<f32>| i.is_empty() && !web_input.finished.load(Relaxed);

                    if let Ok(d) = web_input.pair.1.wait_while(data, c) {
                        data = d;
                    } else {
                        break;
                    }
                }

                let data_len = data.len();
                for sample in data.drain(..(in_len - position).min(data_len)) {
                    pre_buf[0][position] = sample;
                    position += 1;
                }
            } else {
                break;
            }
        }

        if position < in_len {
            continue;
        }

        position = 0;

        // sends a silence signal if the input is muted
        if muted.load(Relaxed) {
            sender.try_send(ProcessorMessage::silence())?;
            continue;
        }

        let (target_buffer, len) = if let Some(resampler) = &mut resampler {
            // resample the data
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;
            (&mut post_buf[0], processed.1)
        } else {
            (&mut pre_buf[0], FRAME_SIZE)
        };

        // the first frame may be smaller than FRAME_SIZE
        if len != FRAME_SIZE {
            warn!("input_processor: len != FRAME_SIZE: {}", len);
            continue;
        }

        // apply the input volume & scale the samples to -32768.0 to 32767.0
        let factor = max_i16_f32 * input_factor.load(Relaxed);

        // rescale the samples to -32768.0 to 32767.0 for rnnoise
        target_buffer.iter_mut().for_each(|x| {
            *x *= factor;
            *x = x.trunc().clamp(min_i16_f32, max_i16_f32);
        });

        if let Some(ref mut denoiser) = denoiser {
            // denoise the frame
            denoiser.process_frame(&mut out_buf, &target_buffer[..len]);
        } else {
            out_buf = target_buffer[..len].try_into()?;
        };

        // calculate the rms
        let rms = calculate_rms(&out_buf);
        // send the rms to the statistics collector
        rms_sender.as_ref().map(|s| s.send(rms));

        // check if the frame is below the rms threshold
        if rms < rms_threshold.load(Relaxed) {
            if silence_length < 80 {
                silence_length += 1; // short silences are ignored
            } else {
                sender.try_send(ProcessorMessage::silence())?;
                continue;
            }
        } else {
            silence_length = 0;
        }

        // cast the f32 samples to i16
        int_buffer = out_buf.map(|x| x as i16);

        if codec_enabled {
            sender.send(ProcessorMessage::samples(int_buffer))?;
        } else {
            // convert the i16 samples to bytes
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    int_buffer.as_ptr() as *const u8,
                    int_buffer.len() * i16_size,
                )
            };

            sender.send(ProcessorMessage::slice(bytes))?;
        }
    }

    debug!("Input processor ended");
    Ok(())
}

/// Processes the audio data and sends it to the output stream
fn output_processor(
    receiver: Receiver<ProcessorMessage>,
    #[cfg(target_family = "wasm")] web_output: Arc<wasm_sync::Mutex<Vec<f32>>>,
    #[cfg(not(target_family = "wasm"))] sender: Sender<f32>,
    ratio: f64,
    output_volume: Arc<AtomicF32>,
    rms_sender: Option<Sender<f32>>,
) -> Result<()> {
    let scale = 1_f32 / i16::MAX as f32;
    let i16_size = size_of::<i16>();

    let mut resampler = resampler_factory(ratio, 1, FRAME_SIZE)?;

    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 * ratio + 10_f64) as usize;

    // the input for the resampler
    let pre_buf = [&mut [0_f32; FRAME_SIZE]];
    // the output for the resampler
    let mut post_buf = [vec![0_f32; post_len]];

    while let Ok(message) = receiver.recv() {
        match message {
            ProcessorMessage::Silence => {
                #[cfg(not(target_family = "wasm"))]
                for _ in 0..FRAME_SIZE {
                    sender.try_send(0_f32)?;
                }

                #[cfg(target_family = "wasm")]
                web_output
                    .lock()
                    .map(|mut data| {
                        if data.len() < CHANNEL_SIZE {
                            data.extend(SILENCE)
                        }
                    })
                    .unwrap();

                continue;
            }
            ProcessorMessage::Data(bytes) => {
                // convert the bytes to 16-bit integers
                let ints = unsafe {
                    std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / i16_size)
                };

                // convert the frame to f32s
                for (out, &x) in pre_buf[0].iter_mut().zip(ints.iter()) {
                    *out = x as f32 * scale;
                }
            }
            ProcessorMessage::Samples(samples) => {
                // convert the frame to f32s
                for (out, &x) in pre_buf[0].iter_mut().zip(samples.iter()) {
                    *out = x as f32 * scale;
                }
            }
        }

        // apply the output volume
        mul(pre_buf[0], output_volume.load(Relaxed));

        rms_sender
            .as_ref()
            .map(|s| s.send(calculate_rms(pre_buf[0])));

        if let Some(resampler) = &mut resampler {
            // resample the data
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;

            // send the resampled data to the output stream
            #[cfg(not(target_family = "wasm"))]
            for sample in &post_buf[0][..processed.1] {
                sender.try_send(*sample)?;
            }

            #[cfg(target_family = "wasm")]
            web_output
                .lock()
                .map(|mut data| {
                    if data.len() < CHANNEL_SIZE {
                        data.extend(&post_buf[0][..processed.1])
                    }
                })
                .unwrap();
        } else {
            // if no resampling is needed, send the data to the output stream
            #[cfg(not(target_family = "wasm"))]
            for sample in *pre_buf[0] {
                sender.try_send(sample)?;
            }

            #[cfg(target_family = "wasm")]
            web_output
                .lock()
                .map(|mut data| {
                    if data.len() < CHANNEL_SIZE {
                        data.extend(*pre_buf[0])
                    }
                })
                .unwrap();
        }
    }

    debug!("Output processor ended");
    Ok(())
}

fn stream_to_audio_transport(stream: Stream) -> Transport<TransportStream> {
    LengthDelimitedCodec::builder()
        .max_frame_length(TRANSFER_BUFFER_SIZE)
        .length_field_type::<u16>()
        .new_framed(stream.compat())
}

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
pub(crate) mod tests {
    use super::*;
    use kanal::unbounded;
    use log::LevelFilter;
    use rand::prelude::SliceRandom;
    use rand::Rng;
    use std::fs::read;
    use std::thread::spawn;

    struct BenchmarkResult {
        average: Duration,
        min: Duration,
        max: Duration,
        end: Duration,
    }

    #[ignore]
    #[test]
    fn benchmark() {
        simple_logging::log_to_file("bench.log", LevelFilter::Trace).unwrap();

        let sample_rate = 44_100;

        let mut samples = Vec::new();
        let bytes = read("../bench.raw").unwrap();

        for chunk in bytes.chunks(4) {
            let sample = f32::from_ne_bytes(chunk.try_into().unwrap());
            samples.push(sample);
        }

        // warmup
        for _ in 0..5 {
            benchmark_input_stack(false, false, sample_rate, &samples, 2400);
        }

        let num_iterations = 10;
        let mut results: HashMap<(bool, bool), (Vec<Duration>, Duration)> = HashMap::new();

        for _ in 0..num_iterations {
            let mut cases = vec![(false, false), (false, true), (true, false), (true, true)];
            cases.shuffle(&mut rand::thread_rng()); // Shuffle for each iteration

            for (denoise, codec_enabled) in cases {
                let (durations, end) =
                    benchmark_input_stack(denoise, codec_enabled, sample_rate, &samples, 2400);

                // Update the results in a cumulative way
                results
                    .entry((denoise, codec_enabled))
                    .and_modify(|(all_durations, total_time)| {
                        all_durations.extend(durations.clone());
                        *total_time += end;
                    })
                    .or_insert((durations, end));
            }
        }

        // compute final averages
        for ((_denoise, _codec_enabled), (_durations, total_time)) in results.iter_mut() {
            *total_time /= num_iterations as u32; // Average total runtime
        }

        compare_runs(results);
    }

    fn benchmark_input_stack(
        denoise: bool,
        codec_enabled: bool,
        sample_rate: u32,
        samples: &[f32],
        channel_size: usize,
    ) -> (Vec<Duration>, Duration) {
        // input stream -> input processor
        let (input_sender, input_receiver) = bounded(channel_size);

        // input processor -> encoder or dummy
        let (processed_input_sender, processed_input_receiver) = unbounded::<ProcessorMessage>();

        // encoder -> dummy
        let (encoded_input_sender, encoded_input_receiver) = unbounded::<ProcessorMessage>();

        let model = RnnModel::default();

        let denoiser = denoise.then_some(DenoiseState::from_model(model));

        spawn(move || {
            input_processor(
                input_receiver,
                processed_input_sender,
                sample_rate as f64,
                Arc::new(AtomicF32::new(1_f32)),
                Arc::new(AtomicF32::new(15_f32)),
                Arc::new(AtomicBool::new(false)),
                denoiser,
                None,
                codec_enabled,
            )
        });

        let output_receiver = if codec_enabled {
            spawn(move || {
                encoder(
                    processed_input_receiver,
                    encoded_input_sender,
                    if denoise { 48_000 } else { sample_rate },
                    true,
                    5.0,
                );
            });

            encoded_input_receiver
        } else {
            processed_input_receiver
        };

        let samples = samples.to_vec();
        spawn(move || {
            for sample in samples {
                input_sender.send(sample).unwrap();
            }
        });

        let start = Instant::now();
        let mut now = Instant::now();
        let mut durations = Vec::new();

        while let Ok(_) = output_receiver.recv() {
            durations.push(now.elapsed());
            now = Instant::now();
        }

        let end = start.elapsed();
        (durations, end)
    }

    fn compute_statistics(durations: &[Duration]) -> (Duration, Duration, Duration) {
        let sum: Duration = durations.iter().sum();
        let average = sum / durations.len() as u32;

        let min = *durations.iter().min().unwrap();
        let max = *durations.iter().max().unwrap();

        (average, min, max)
    }

    fn compare_runs(benchmark_results: HashMap<(bool, bool), (Vec<Duration>, Duration)>) {
        let mut summary: HashMap<(bool, bool), BenchmarkResult> = HashMap::new();

        for ((denoise, codec_enabled), (durations, end)) in benchmark_results {
            let (average, min, max) = compute_statistics(&durations);
            summary.insert(
                (denoise, codec_enabled),
                BenchmarkResult {
                    average,
                    min,
                    max,
                    end,
                },
            );
        }

        info!("\nComparison of Runs:");
        info!("===================================================");
        info!(" Denoise | Codec Enabled | Avg Duration | Min Duration | Max Duration | Runtime ");
        info!("---------------------------------------------------");

        for ((denoise, codec_enabled), result) in summary {
            info!(
                " {}   | {}     | {:?} | {:?} | {:?} | {:?}",
                denoise, codec_enabled, result.average, result.min, result.max, result.end
            );
        }
    }

    /// returns a frame of random samples
    pub(crate) fn dummy_frame() -> [f32; 4096] {
        let mut frame = [0_f32; 4096];
        let mut rng = rand::thread_rng();
        rng.fill(&mut frame[..]);

        for x in &mut frame {
            *x /= i16::MAX as f32;
            *x = x.trunc().clamp(i16::MIN as f32, i16::MAX as f32);
        }

        frame
    }
}
