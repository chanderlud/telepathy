use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem;
#[cfg(not(target_family = "wasm"))]
use std::net::Ipv4Addr;
pub use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;

use crate::api::audio::codec::{decoder, encoder};
#[cfg(target_os = "ios")]
use crate::api::audio::ios::{configure_audio_session, deactivate_audio_session};
#[cfg(target_family = "wasm")]
use crate::api::audio::web_audio::{WebAudioWrapper, WebInput};
use crate::api::audio::{input_processor, output_processor};
use crate::api::error::{DartError, Error, ErrorKind};
use crate::api::flutter::*;
use crate::api::overlay::overlay::Overlay;
use crate::api::overlay::{CONNECTED, LATENCY, LOSS};
use crate::api::screenshare;
use crate::api::utils::*;
use crate::frb_generated::FLUTTER_RUST_BRIDGE_HANDLER;
use crate::{Behaviour, BehaviourEvent};
use atomic_float::AtomicF32;
use chrono::Local;
#[cfg(not(target_family = "wasm"))]
use cpal::Device;
pub use cpal::Host;
use cpal::SupportedStreamConfig;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use flutter_rust_bridge::for_generated::futures::SinkExt;
use flutter_rust_bridge::for_generated::futures::stream::{SplitSink, SplitStream};
use flutter_rust_bridge::{frb, spawn, spawn_blocking_with};
pub use kanal::AsyncReceiver;
use kanal::bounded;
use kanal::{AsyncSender, Sender, unbounded_async};
use libp2p::futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{ConnectionId, SwarmEvent};
#[cfg(not(target_family = "wasm"))]
use libp2p::tcp;
use libp2p::{
    Multiaddr, PeerId, Stream, StreamProtocol, autonat, dcutr, dcutr::Event as DcutrEvent,
    identify, identify::Event as IdentifyEvent, noise, ping, yamux,
};
use libp2p_stream::Control;
use log::{debug, error, info, trace, warn};
use messages::{Attachment, AudioHeader, Message};
use nnnoiseless::{DenoiseState, FRAME_SIZE, RnnModel};
use sea_codec::ProcessorMessage;
use sea_codec::codec::file::SeaFileHeader;
#[cfg(not(target_family = "wasm"))]
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::select;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
#[cfg(not(target_family = "wasm"))]
use tokio::time::{Interval, interval, timeout};
use tokio_util::bytes::Bytes;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::std::Instant;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::{Interval, interval, sleep_until, timeout};

type Result<T> = std::result::Result<T, Error>;
pub(crate) type DeviceName = Arc<Mutex<Option<String>>>;
type TransportStream = Compat<Stream>;
pub type Transport<T> = Framed<T, LengthDelimitedCodec>;
type StartScreenshare = (PeerId, Option<Message>);
type AudioSocket = SplitSink<Transport<TransportStream>, Bytes>;

/// The number of bytes in a single network audio frame
const TRANSFER_BUFFER_SIZE: usize = FRAME_SIZE * size_of::<i16>();
/// A timeout used when initializing the call
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to keep-alive libp2p streams
const KEEP_ALIVE: Duration = Duration::from_secs(10);
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

    /// Private key for signing the handshake
    identity: Arc<RwLock<Keypair>>,

    /// Keeps track of whether the user is in a call
    in_call: Arc<AtomicBool>,

    /// used to end an audio test, if there is one
    end_audio_test: Arc<Mutex<Option<Arc<Notify>>>>,

    /// Tracks state for the current room
    room_state: Arc<RwLock<Option<RoomState>>>,

    /// Keeps the early call state the same across the whole room
    early_room_state: Arc<RwLock<Option<EarlyCallState>>>,

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

    /// callback methods provided by the flutter frontend
    callbacks: TelepathyCallbacks,
}

impl Telepathy {
    // this function must be async to use `spawn`
    pub async fn new(
        identity: Vec<u8>,
        host: Arc<Host>,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: TelepathyCallbacks,
    ) -> Telepathy {
        let (start_session, receive_session) = unbounded_async::<PeerId>();
        let (start_screenshare, receive_screenshare) = unbounded_async::<StartScreenshare>();

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
            early_room_state: Default::default(),
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
                    .session_manager(&receive_session, &receive_screenshare)
                    .await
                {
                    error!("Session manager failed: {}", error);
                }

                // just for safety
                sleep(Duration::from_millis(250)).await;
            }
        });

        // start the sessions
        notify(&chat.callbacks.start_sessions, chat.clone()).await;
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

        // message channel
        let (sender, receiver) = unbounded_async();
        let cancel = CancellationToken::new();
        let end_call = Arc::new(Notify::new());
        // parse members
        let members: Vec<_> = member_strings
            .into_iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        // set room state
        self.room_state.write().await.replace(RoomState {
            peers: members.clone(),
            sender,
            cancel: cancel.clone(),
            end_call: end_call.clone(),
        });

        // the same early call state is used throughout the room, the real peer ids are set later
        let call_state = self.setup_call(PeerId::random()).await?;
        *self.early_room_state.write().await = Some(call_state.clone());

        let identity = self.identity.read().await.public().to_peer_id();
        for member in members {
            if let Some(state) = self.session_states.read().await.get(&member) {
                state.start_call.notify_one();
            } else if member == identity {
                continue;
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
            notify(&self.callbacks.start_sessions, self.clone()).await;
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
        notify(&self.callbacks.manager_active, (true, true)).await;

        // handle incoming streams
        let self_clone = self.clone();
        let control = swarm.behaviour().stream.new_control();
        let stop_handler = Arc::new(Notify::new());
        let stop_handler_clone = stop_handler.clone();
        let stream_handler_handle = spawn(async move {
            self_clone
                .incoming_stream_handler(control, stop_handler_clone)
                .await
        });

        // handles the state needed for negotiating sessions
        // it is cleared each time a peer successfully connects
        let mut peer_states: HashMap<PeerId, PeerState> = HashMap::new();

        loop {
            let event = select! {
                // restart the manager
                _ = self.restart_manager.notified() => {
                    break;
                }
                // events are handled outside the select to help with spagetification
                event = swarm.select_next_some() => event,
                // start a new session
                result = start.recv() => {
                    let peer_id = result?;

                    if peer_id == self.identity.read().await.public().to_peer_id() {
                        // prevents dialing yourself
                        continue;
                    } else if swarm.is_connected(&peer_id) {
                        // TODO is it possible that this check can result in invalid states where two peers cannot get into a session?
                        // prevents dialing a peer who is already connected
                        warn!("{} is already connected (EDGE CASE DETECTED)", peer_id);
                        continue;
                    }

                    debug!("initial dial for {}", peer_id);

                    // dial the peer through the relay
                    let status = if let Err(error) = swarm.dial(relay_address.clone().with(Protocol::P2p(peer_id))) {
                        error!("dial error for {}: {}", peer_id, error);
                        SessionStatus::Inactive
                    } else {
                        SessionStatus::Connected
                    };

                    self.callbacks.update_status(status, peer_id).await;
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
                                notify(&self.callbacks.screenshare_started, (dart_stop, false)).await;
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
                            notify(&self.callbacks.screenshare_started, (dart_stop, true)).await;
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
                        // TODO does this case ever hit in normal operation or does it only occur when the session is invalidated by a crash or other failure?
                        // ignore connections with peers who have a session
                        warn!("ignored connection from {} (EDGE CASE DETECTED)", peer_id);
                        continue;
                    }

                    let contact_option =
                        invoke(&self.callbacks.get_contact, peer_id.to_bytes()).await;
                    let relayed = endpoint.is_relayed();
                    let listener = endpoint.is_listener();

                    if contact_option.is_none() && !self.is_in_room(&peer_id).await {
                        warn!("received a connection from an unknown peer: {:?}", peer_id);
                        if swarm.disconnect_peer_id(peer_id).is_err() {
                            warn!("unknown peer was no longer connected");
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
                            self.callbacks
                                .update_status(SessionStatus::Connecting, peer_id)
                                .await;
                        }
                    }
                }
                SwarmEvent::OutgoingConnectionError {
                    peer_id: Some(peer_id),
                    error,
                    connection_id,
                } => {
                    warn!("outgoing connection failed for {peer_id} because {error}",);

                    if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        peer_state.connections.remove(&connection_id);
                    } else if !self.session_states.read().await.contains_key(&peer_id) {
                        // if an outgoing error occurs when no connection is active, the session initialization failed
                        self.callbacks
                            .update_status(SessionStatus::Inactive, peer_id)
                            .await;
                    }
                }
                SwarmEvent::ConnectionClosed {
                    peer_id,
                    cause,
                    connection_id,
                    ..
                } => {
                    warn!("connection {connection_id} closed with {peer_id} cause={cause:?}",);

                    // if there is no connection to the peer, the session initialization failed
                    if !swarm.is_connected(&peer_id) {
                        peer_states.remove(&peer_id);
                        self.callbacks
                            .update_status(SessionStatus::Inactive, peer_id)
                            .await;
                    } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
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
                            debug!(
                                "not trying to establish a session with {} because not all connections have latency",
                                event.peer
                            );
                            continue;
                        } else if peer_state.relayed_only() {
                            // only start a session if there is a non-relayed connection
                            debug!(
                                "not trying to establish a session with {} because all connections are relayed",
                                event.peer
                            );
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
                                .keys()
                                .filter(|id| id != &connection_id)
                                .for_each(|id| {
                                    swarm.close_connection(*id);
                                });

                            // open a session control stream and start the session controller
                            self.open_session(
                                event.peer,
                                swarm.behaviour().stream.new_control(),
                                peer_state.connections.len(),
                                &mut peer_states,
                            )
                            .await;
                        } else {
                            warn!("no connection available for {}", event.peer);
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(IdentifyEvent::Received {
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
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(DcutrEvent {
                    remote_peer_id,
                    result,
                })) => {
                    debug!("ductr event with {}: {:?}", remote_peer_id, result);

                    if let Some(peer_state) = peer_states.get(&remote_peer_id)
                        && peer_state.relayed_only()
                        && result.is_err()
                    {
                        info!("ductr failed while relayed_only, falling back to relay");
                        self.open_session(
                            remote_peer_id,
                            swarm.behaviour().stream.new_control(),
                            peer_state.connections.len(),
                            &mut peer_states,
                        )
                        .await;
                    }
                }
                event => {
                    trace!("other swarm event: {:?}", event);
                }
            }
        }

        debug!("tearing down old swarm");
        notify(&self.callbacks.manager_active, (false, false)).await;
        stop_handler.notify_one();
        stream_handler_handle.await??;
        debug!("joined stream handler");

        Ok(())
    }

    /// Handles incoming streams for the libp2p swarm
    async fn incoming_stream_handler(&self, mut control: Control, stop: Arc<Notify>) -> Result<()> {
        let mut incoming_streams = control.accept(CHAT_PROTOCOL)?;

        loop {
            select! {
                _ = stop.notified() => break Ok(()),
                Some((peer, stream)) = incoming_streams.next() => {
                    let state_option = self.session_states.read().await.get(&peer).cloned();

                    if let Some(state) = state_option {
                        if state.wants_stream.load(Relaxed) {
                            info!("sub-stream accepted for {}", peer);

                            if let Err(error) = state.stream_sender.send(stream).await {
                                error!("error sending sub-stream to {}: {}", peer, error);
                            }

                            continue;
                        } else {
                            warn!(
                                "received a stream while {} did not want sub-stream, starting new session",
                                peer
                            );
                        }
                    } else {
                        info!("stream accepted for new session with {}", peer);
                    }

                    self.session_outer(peer, None, stream).await;
                }
                else => break Err(ErrorKind::StreamsEnded.into())
            }
        }
    }

    /// Called by the dialer to open a stream and session
    async fn open_session(
        &self,
        peer_id: PeerId,
        mut control: Control,
        connection_count: usize,
        peer_states: &mut HashMap<PeerId, PeerState>,
    ) {
        // it may take multiple tries to open the stream because the of the RNG in the stream handler
        for _ in 0..connection_count {
            match control.open_stream(peer_id, CHAT_PROTOCOL).await {
                Ok(stream) => {
                    info!("opened stream with {}, starting new session", peer_id);
                    self.session_outer(peer_id, Some(control), stream).await;
                    // the peer state is no longer needed
                    peer_states.remove(&peer_id);
                    return;
                }
                Err(error) => {
                    warn!("OpenStreamError for {peer_id}: {error}");
                }
            }
        }

        error!("failed to open stream for {peer_id} after {connection_count} tries");
    }

    /// Manages a session throughout its lifetime
    async fn session_outer(&self, peer_id: PeerId, mut control: Option<Control>, stream: Stream) {
        let contact_option = invoke(&self.callbacks.get_contact, peer_id.to_bytes()).await;
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
                state.end_call.notify_one();
            }

            // stop the session
            old_state.stop_session.notify_one();
        }

        let contact = if let Some(contact) = contact_option {
            // alert the UI that this session is now connected
            self.callbacks
                .update_status(SessionStatus::Connected, peer_id)
                .await;
            contact
        } else {
            // there may be no contact for members of a group
            Contact {
                id: Uuid::new_v4().to_string(),
                nickname: String::from("GroupContact"),
                peer_id,
                is_room_only: true,
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
            let mut keep_alive = interval(KEEP_ALIVE);

            let result = loop {
                let result = self_clone
                    .session_inner(
                        &contact,
                        control.as_mut(),
                        &mut transport,
                        &state,
                        &message_channel,
                        &mut keep_alive,
                    )
                    .await;

                match (result, contact.is_room_only) {
                    (Ok(true), true) | (Ok(false), _) => {
                        break Ok(());
                    }
                    (Ok(true), false) => {
                        // the session is not in a call
                        state.in_call.store(false, Relaxed);
                    }
                    (Err(error), room_only) => {
                        // if an error occurred during a non-room call, it is ended now
                        if state.in_call.load(Relaxed)
                            && !room_only
                            && !self_clone.is_in_room(&contact.peer_id).await
                        {
                            warn!("session error while call active, alerting ui (e={error:?})");
                            notify(
                                &self_clone.callbacks.call_state,
                                CallState::CallEnded(error.to_string(), false),
                            )
                            .await;
                        }

                        if room_only || error.is_session_critical() {
                            // session cannot recover from these errors
                            break Err(error);
                        } else {
                            warn!("recoverable session failure: {:?}", error);
                            // the session is not in a call
                            state.in_call.store(false, Relaxed);
                        }
                    }
                }
            };

            if let Err(error) = result {
                // session failed & requires cleanup
                error!("Session error for {}: {error:?}", contact.nickname);
            } else if !contact.is_room_only {
                // the session has already been cleaned up
                warn!("Session for {} stopped", contact.nickname);
                return;
            }

            // cleanup
            self_clone.session_states.write().await.remove(&peer_id);

            // avoid sending session statuses for dummy contacts
            if !contact.is_room_only {
                self_clone
                    .callbacks
                    .update_status(SessionStatus::Inactive, peer_id)
                    .await;
            }

            info!("Session for {} cleaned up", contact.nickname);
        });
    }

    /// The inner logic of a session that may execute many times
    async fn session_inner(
        &self,
        contact: &Contact,
        control: Option<&mut Control>,
        transport: &mut Transport<TransportStream>,
        state: &Arc<SessionState>,
        message_channel: &(AsyncSender<Message>, AsyncReceiver<Message>),
        keep_alive: &mut Interval,
    ) -> Result<bool> {
        info!("[{}] session waiting for event", contact.nickname);

        select! {
            result = read_message::<Message, _>(transport) => {
                let mut other_ringtone = None;
                let remote_audio_header;
                let room_hash_option;

                info!("received {:?} from {}", result, contact.nickname);

                match result? {
                    Message::Hello { ringtone, audio_header, room_hash } => {
                        remote_audio_header = audio_header;
                        room_hash_option = room_hash;
                        if self.play_custom_ringtones.load(Relaxed) {
                            other_ringtone = ringtone;
                        }
                    },
                    Message::KeepAlive => return Ok(true),
                    message => {
                        warn!("received unexpected {:?} from {}", message, contact.nickname);
                        return Ok(true);
                    }
                }

                let is_in_room = self.is_in_room(&contact.peer_id).await;
                let mut cancel_prompt = None;
                let mut accept_handle = None;

                if is_in_room && room_hash_option == self.room_hash().await {
                    // automatically accept calls from member of current room
                } else if room_hash_option.is_some() {
                    // the call is part of a room, but the client is not in the room
                    write_message(transport, &Message::Reject).await?;
                    return Ok(true);
                } else if self.in_call.load(Relaxed) {
                    // do not accept another call if already in one
                    write_message(transport, &Message::Busy).await?;
                    return Ok(true);
                } else {
                    let other_cancel_prompt = Arc::new(Notify::new());
                    // a cancel Notify that can be used in the frontend
                    let dart_cancel = DartNotify { inner: Arc::clone(&other_cancel_prompt) };
                    cancel_prompt = Some(other_cancel_prompt);

                    let accept_call_clone = Arc::clone(&self.callbacks.accept_call);
                    let contact_id = contact.id.clone();
                    accept_handle = Some(spawn(async move {
                        invoke(&accept_call_clone, (contact_id, other_ringtone, dart_cancel)).await
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

                Ok(true)
            }
            _ = state.start_call.notified() => {
                state.in_call.store(true, Relaxed); // blocks the session from being restarted

                let room_hash = self.room_hash().await;
                let is_in_room = room_hash.is_some();
                // load custom ringtone if enabled
                let other_ringtone = self.load_ringtone().await;
                // initialize call state
                let mut call_state = self.setup_call(contact.peer_id).await?;
                // when custom ringtone is used wait longer for a response to account for extra data being sent in Hello
                let hello_timeout = HELLO_TIMEOUT + if other_ringtone.is_some() { Duration::from_secs(10) } else { Default::default() };
                // queries the other client for a call
                write_message(transport, &Message::Hello { ringtone: other_ringtone, audio_header: call_state.local_configuration.clone(), room_hash }).await?;

                loop {
                    select! {
                        result = timeout(hello_timeout, read_message(transport)) => {
                            // handles a variety of outcomes in response to Hello
                            let message_option = match result?? {
                                Message::HelloAck { audio_header } => {
                                    call_state.remote_configuration = audio_header;

                                    if is_in_room {
                                        self.room_handshake(transport, control, state, call_state).await?;
                                    } else {
                                        // normal call handshake
                                        self.call_handshake(transport, control, &message_channel.1, state, call_state).await?;
                                    }

                                    keep_alive.reset(); // start sending normal keep alive messages
                                    None
                                }
                                Message::Reject | Message::Busy if is_in_room => None,
                                Message::Reject => {
                                    Some(format!("{} did not accept the call", contact.nickname))
                                },
                                Message::Busy => {
                                    Some(format!("{} is busy", contact.nickname))
                                },
                                // keep alive messages are sometimes received here
                                Message::KeepAlive => continue,
                                message => {
                                    // the front end needs to know that the call ended here
                                    warn!("received unexpected {:?} from {} [stopped call process]", message, contact.nickname);
                                    Some(format!("Received an unexpected message from {}", contact.nickname))
                                }
                            };

                            if let Some(message) = message_option {
                                invoke(&self.callbacks.call_state, CallState::CallEnded(message, true)).await;
                            }

                            break;
                        }
                        _ = state.end_call.notified() => {
                            info!("end call notified while waiting for hello ack");
                            write_message(transport, &Message::Goodbye { reason: None }).await?;
                        }
                    }
                }

                Ok(true)
            }
            // state will never notify while a call is active
            _ = state.stop_session.notified() => {
                info!("session state stop notified for {}", contact.nickname);
                Ok(false)
            },
            _ = keep_alive.tick() => {
                debug!("sending keep alive to {}", contact.nickname);
                write_message(transport, &Message::KeepAlive).await?;
                Ok(true)
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
        let stop_io = CancellationToken::new();

        let result = self
            .call(
                &stop_io,
                call_state,
                &state.end_call,
                Some(OptionalCallArgs {
                    audio_transport: stream_to_audio_transport(stream),
                    control_transport: transport,
                    message_receiver: message_receiver.clone(),
                    state,
                }),
            )
            .await;

        info!("call ended in handshake");
        // ensure that all background i/o threads are stopped
        stop_io.cancel();
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

    /// The bulk of the normal call logic
    async fn call(
        &self,
        stop_io: &CancellationToken,
        call_state: EarlyCallState,
        end_call: &Arc<Notify>,
        optional: Option<OptionalCallArgs<'_>>,
    ) -> Result<()> {
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // shared values for various statistics
        let latency = optional
            .as_ref()
            .map(|o| Arc::clone(&o.state.latency))
            .unwrap_or_default();
        let upload_bandwidth = optional
            .as_ref()
            .map(|o| Arc::clone(&o.state.upload_bandwidth))
            .unwrap_or_default();
        let download_bandwidth = optional
            .as_ref()
            .map(|o| Arc::clone(&o.state.download_bandwidth))
            .unwrap_or_default();
        // shared values used to move rms to statistics collector
        let input_rms_sender: Arc<AtomicF32> = Default::default();
        let output_rms_sender: Arc<AtomicF32> = Default::default();

        // the two clients agree on these codec options
        let codec_config = call_state.codec_config();

        let input_channel = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                codec_config,
                input_rms_sender.clone(),
                false,
            )
            .await?;

        let (output_sender, output_stream) = self
            .setup_output(
                call_state.remote_configuration.sample_rate as f64,
                codec_config.0,
                output_rms_sender.clone(),
                false,
                end_call.clone(),
            )
            .await?;

        #[cfg(not(target_family = "wasm"))]
        let input_stream =
            self.setup_input_stream(&call_state, input_channel.1, end_call.clone())?;

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
            input_rms_sender,
            output_rms_sender,
            latency,
            Arc::clone(&upload_bandwidth),
            Arc::clone(&download_bandwidth),
            Arc::clone(&self.callbacks.statistics),
            stop_io.clone(),
        ));

        if let Some(o) = optional {
            let (socket_sender, socket_receiver) = unbounded_async();
            let (write, read) = o.audio_transport.split();
            socket_sender.send(write).await?;

            let input_handle = spawn(audio_input(
                input_channel.0,
                socket_receiver,
                stop_io.clone(),
                upload_bandwidth,
            ));

            let output_handle = spawn(audio_output(
                output_sender,
                read,
                stop_io.clone(),
                download_bandwidth,
            ));

            let controller_future = self.call_controller(
                o.control_transport,
                o.message_receiver,
                call_state.peer,
                end_call,
            );

            info!("call controller starting");

            let message_option = match controller_future.await {
                Ok((message, notify)) if notify => Some(message.unwrap_or_default()),
                Err(error) => Some(error.to_string()),
                _ => None,
            };

            if let Some(message) = message_option {
                invoke(
                    &self.callbacks.call_state,
                    CallState::CallEnded(message, true),
                )
                .await;
            }

            info!("call controller done, notifying stop_io");
            stop_io.cancel();

            match input_handle.await {
                Ok(Ok(())) => info!("input handle joined"),
                Ok(Err(error)) => {
                    error!("audio_input failed: {}", error);
                }
                Err(error) => {
                    error!("audio_input failed: {}", error);
                }
            }

            match output_handle.await {
                Ok(Ok(())) => info!("output handle joined"),
                Ok(Err(error)) => {
                    error!("audio_output failed: {}", error);
                }
                Err(error) => {
                    error!("audio_output failed: {}", error);
                }
            }

            info!("call controller returned and was handled, call returning");
        } else {
            loopback(input_channel.0, output_sender, stop_io, end_call).await;
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
        peer: PeerId,
        end_call: &Arc<Notify>,
    ) -> Result<(Option<String>, bool)> {
        let identity = self.identity.read().await.public().to_peer_id();

        CONNECTED.store(true, Relaxed);
        invoke(&self.callbacks.call_state, CallState::Connected).await;

        loop {
            select! {
                // receives and handles messages from the callee
                result = read_message(transport) => {
                    let message: Message = result?;

                    match message {
                        Message::Goodbye { reason } => {
                            debug!("received goodbye, reason = {:?}", reason);
                            break Ok((reason, true));
                        },
                        Message::Chat { text, attachments } => {
                            invoke(&self.callbacks.message_received, ChatMessage {
                                text,
                                receiver: identity,
                                timestamp: Local::now(),
                                attachments,
                            }).await;
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
                        break Ok((None, true));
                    }
                },
                // ends the call
                _ = end_call.notified() => {
                    write_message(transport, &Message::Goodbye { reason: None }).await?;
                    break Ok((None, false));
                },
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
        let audio_transport = stream_to_audio_transport(stream);
        let peer_id = call_state.peer;
        let (sender, cancel) = self
            .room_state
            .read()
            .await
            .as_ref()
            .map(|s| (s.sender.clone(), s.cancel.clone()))
            .ok_or(ErrorKind::RoomStateMissing)?;

        sender
            .send(RoomMessage::Join {
                audio_transport: Box::new(audio_transport),
                state: call_state,
            })
            .await?;

        loop {
            select! {
                Ok(result) = read_message::<Message, _>(transport) => {
                    match result {
                        Message::Goodbye { .. } => {
                            break;
                        }
                        Message::Chat { .. } => {
                            // TODO handle chat messages
                        }
                        _ => ()
                    }
                }
                _ = cancel.cancelled() => {
                    // try to say goodbye
                    _ = write_message(transport, &Message::Goodbye { reason: None }).await;
                    break
                }
                else => break,
            }
        }

        // sender may already be closed at this point
        _ = sender.send(RoomMessage::Leave(peer_id)).await;
        Ok(())
    }

    /// controller for rooms
    async fn room_controller(
        &self,
        receiver: AsyncReceiver<RoomMessage>,
        end_sessions: CancellationToken,
        call_state: EarlyCallState,
        stop_io: &CancellationToken,
        end_call: Arc<Notify>,
    ) -> Result<()> {
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // moves new sockets to audio_input
        let (socket_sender, socket_receiver) = unbounded_async();
        // shared values used for moving values to the statistics collector
        let upload_bandwidth: Arc<AtomicUsize> = Default::default();
        let download_bandwidth: Arc<AtomicUsize> = Default::default();
        let input_rms_sender: Arc<AtomicF32> = Default::default();
        let output_rms_sender: Arc<AtomicF32> = Default::default();
        // tracks connection state for peers
        let mut connections = HashMap::new();

        let input_channel = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                (true, true, 5_f32), // hard coded room codec options
                input_rms_sender.clone(),
                true,
            )
            .await?;

        #[cfg(not(target_family = "wasm"))]
        let input_stream =
            self.setup_input_stream(&call_state, input_channel.1, end_call.clone())?;

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

        let input_handle = spawn(audio_input(
            input_channel.0,
            socket_receiver,
            stop_io.clone(),
            upload_bandwidth.clone(),
        ));

        spawn(statistics_collector(
            input_rms_sender,
            output_rms_sender.clone(),
            Default::default(), // TODO decide what to do with room latencies
            Arc::clone(&upload_bandwidth),
            Arc::clone(&download_bandwidth),
            Arc::clone(&self.callbacks.statistics),
            stop_io.clone(),
        ));

        // kick the UI out of connecting mode
        invoke(&self.callbacks.call_state, CallState::Waiting).await;

        loop {
            select! {
                _ = end_call.notified() => {
                    break;
                }
                Ok(message) = receiver.recv() => {
                    match message{
                        RoomMessage::Join { audio_transport, state } => {
                            info!("received room Join [p={}]", state.peer);

                            // first connection
                            if connections.is_empty() {
                                CONNECTED.store(true, Relaxed);
                                invoke(&self.callbacks.call_state, CallState::Connected).await;
                            }

                            let (write, read) = (*audio_transport).split();
                            // begin sending audio to transport
                            socket_sender.send(write).await?;
                            // setup output stack
                            let (output_sender, output_stream) = self
                                .setup_output(
                                    state.remote_configuration.sample_rate as f64,
                                    true,
                                    output_rms_sender.clone(),
                                    true,
                                    end_call.clone(),
                                )
                                .await?;
                            // begin playing audio
                            output_stream.stream.play()?;
                            // begin sending
                            let handle = spawn(audio_output(
                                output_sender,
                                read,
                                stop_io.clone(),
                                download_bandwidth.clone(),
                            ));

                            connections.insert(state.peer, RoomConnection {
                                stream: output_stream,
                                handle,
                            });
                            invoke(&self.callbacks.call_state, CallState::RoomJoin(state.peer.to_string())).await;
                        }
                        RoomMessage::Leave(peer) => {
                            invoke(&self.callbacks.call_state, CallState::RoomLeave(peer.to_string())).await;

                            if let Some(connection) = connections.remove(&peer) {
                                connection.handle.await??;
                                info!("successfully cleaned up room connection [p={peer}]");
                            } else {
                                warn!("Leave for peer without room connection [p={peer}]");
                            }
                        }
                    }
                }
            }
        }

        // tear down processing stack
        debug!("starting to tear down room processing stack");
        stop_io.cancel();
        input_handle.await??;
        for connection in connections.into_values() {
            connection.handle.await??;
            drop(connection.stream);
        }
        debug!("finished tearing down room processing stack");

        // clean up room state
        self.room_state.write().await.take();
        // clean up sessions blocked by room
        end_sessions.cancel();
        Ok(())
    }

    /// helper method to set up audio input stack between the network and device layers
    async fn setup_input(
        &self,
        sample_rate: f64,
        codec_options: (bool, bool, f32),
        rms_sender: Arc<AtomicF32>,
        is_room: bool,
    ) -> Result<(AsyncReceiver<ProcessorMessage>, Sender<f32>)> {
        // input stream -> input processor
        let (input_sender, input_receiver) = bounded::<f32>(CHANNEL_SIZE);

        #[cfg(target_family = "wasm")]
        let input_receiver = {
            // normal channel is unused on the web
            drop(input_receiver);

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
                    rms_sender,
                    codec_enabled,
                )
            },
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        // if using codec, spawn extra encoder thread
        if codec_enabled {
            spawn_blocking_with(
                move || {
                    encoder(
                        processed_input_receiver.to_sync(),
                        encoded_input_sender.to_sync(),
                        if denoise { 48_000 } else { sample_rate as u32 },
                        vbr,
                        residual_bits,
                        is_room,
                    );
                },
                FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
            );

            Ok((encoded_input_receiver, input_sender))
        } else {
            Ok((processed_input_receiver, input_sender))
        }
    }

    /// helper method to set up audio output stack above network layer
    async fn setup_output(
        &self,
        remote_sample_rate: f64,
        codec_enabled: bool,
        rms_sender: Arc<AtomicF32>,
        is_room: bool,
        end_call: Arc<Notify>,
    ) -> Result<(AsyncSender<ProcessorMessage>, SendStream)> {
        // receiving socket -> output processor or decoder
        let (network_output_sender, network_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // decoder -> output processor
        let (decoded_output_sender, decoded_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // output processor -> output stream
        #[cfg(not(target_family = "wasm"))]
        let (output_sender, output_receiver) = bounded::<f32>(CHANNEL_SIZE * 4);

        // output processor -> output stream
        #[cfg(target_family = "wasm")]
        let output_sender = Arc::new(wasm_sync::Mutex::new(Vec::new()));
        #[cfg(target_family = "wasm")]
        let web_output = output_sender.clone();

        // get the output device and its default configuration
        let output_device = get_output_device(&self.output_device, &self.host).await?;
        let output_config = output_device.default_output_config()?;
        info!("output device: {:?}", output_device.name());

        // in rooms, the SEA header is hard coded
        let header = is_room.then_some(SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 960,
            frames_per_chunk: 480,
            sample_rate: remote_sample_rate as u32,
        });
        // the ratio of the output sample rate to the remote input sample rate
        let ratio = output_config.sample_rate().0 as f64 / remote_sample_rate;
        // get a reference to output volume for the processor
        let output_volume = Arc::clone(&self.output_volume);
        // do this outside the output processor thread
        let output_processor_receiver = if codec_enabled {
            spawn_blocking_with(
                move || {
                    decoder(
                        network_output_receiver.to_sync(),
                        decoded_output_sender.to_sync(),
                        header,
                    );
                },
                FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
            );

            decoded_output_receiver.to_sync()
        } else {
            network_output_receiver.to_sync()
        };

        // spawn the output processor thread
        spawn_blocking_with(
            move || {
                output_processor(
                    output_processor_receiver,
                    output_sender,
                    ratio,
                    output_volume,
                    rms_sender,
                )
            },
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        // get the output channels for chunking the output
        let output_channels = output_config.channels() as usize;
        // a reference to the flag for use in the output callback
        let deafened = Arc::clone(&self.deafened);

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
        end_call: Arc<Notify>,
    ) -> Result<SendStream> {
        let input_channels = call_state.local_configuration.channels as usize;

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

    /// helper method to load pre-encoded ringtone bytes
    async fn load_ringtone(&self) -> Option<Vec<u8>> {
        #[cfg(not(target_family = "wasm"))]
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

        #[cfg(target_family = "wasm")]
        None
    }

    /// helper method to check if a peer is in the current room
    async fn is_in_room(&self, peer_id: &PeerId) -> bool {
        self.room_state
            .read()
            .await
            .as_ref()
            .map(|m| m.peers.contains(peer_id))
            .unwrap_or(false)
    }

    async fn room_hash(&self) -> Option<Vec<u8>> {
        self.room_state
            .read()
            .await
            .as_ref()
            .map(|state| {
                state.peers.iter().fold(0u64, |acc, peer| {
                    let mut hasher = DefaultHasher::new();
                    peer.hash(&mut hasher);
                    acc ^ hasher.finish()
                })
            })
            .map(|hash| hash.to_le_bytes().to_vec())
    }

    async fn is_call_active(&self) -> bool {
        self.in_call.load(Relaxed)
            || self.room_state.read().await.is_some()
            || self.end_audio_test.lock().await.is_some()
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

    end_call: Arc<Notify>,
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
            end_call: Default::default(),
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

struct RoomState {
    peers: Vec<PeerId>,

    sender: AsyncSender<RoomMessage>,

    cancel: CancellationToken,

    end_call: Arc<Notify>,
}

enum RoomMessage {
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

struct OptionalCallArgs<'a> {
    audio_transport: Transport<TransportStream>,
    control_transport: &'a mut Transport<TransportStream>,
    message_receiver: AsyncReceiver<Message>,
    state: &'a Arc<SessionState>,
}

/// Receives frames of audio data from the input processor and sends them to the socket
async fn audio_input(
    input_receiver: AsyncReceiver<ProcessorMessage>,
    socket_receiver: AsyncReceiver<AudioSocket>,
    cancel: CancellationToken,
    bandwidth: Arc<AtomicUsize>,
) -> Result<()> {
    // static signal bytes
    let keep_alive = Bytes::from_static(&[1]);
    let mut sockets: Vec<AudioSocket> = Vec::new();

    loop {
        select! {
            message = socket_receiver.recv() => {
                if let Ok(socket) = message {
                    sockets.push(socket); // new connection established
                } else {
                    // in theory, this is dead code, the socket_sender isn't dropped
                    debug!("audio_input ended with socket shutdown");
                    break Ok(());
                }
            }
            message = timeout(KEEP_ALIVE, input_receiver.recv()) => {
                let bytes = match message {
                    Ok(Ok(ProcessorMessage::Data(bytes))) => bytes,
                    // shutdown
                    Ok(_) => {
                        debug!("audio_input ended with input shutdown");
                        break Ok(())
                    },
                    // send keep alive during extended silence
                    Err(_) => keep_alive.clone(),
                };

                // send the bytes to all connections, dropping any that error
                let mut i = 0;
                let mut successful_sends = 0;

                while i < sockets.len() {
                    let send_result = {
                        // limit the &mut borrow to this block
                        let socket = &mut sockets[i];
                        socket.send(bytes.clone()).await
                    };

                    if send_result.is_err() {
                        // remove this socket, do NOT increment i
                        _ = sockets.remove(i);
                        info!("audio_input dropping socket [remaining={}]", sockets.len());
                    } else {
                        successful_sends += 1;
                        i += 1;
                    }
                }

                // update bandwidth based on successful sends only
                if successful_sends > 0 {
                    bandwidth.fetch_add(bytes.len() * successful_sends, Relaxed);
                }
            }
            _ = cancel.cancelled() => {
                debug!("audio_input ended with cancellation");
                break Ok(());
            }
        }
    }
}

/// Receives audio data from the socket and sends it to the output processor
async fn audio_output(
    sender: AsyncSender<ProcessorMessage>,
    mut socket: SplitStream<Transport<TransportStream>>,
    cancel: CancellationToken,
    bandwidth: Arc<AtomicUsize>,
) -> Result<()> {
    loop {
        select! {
            message = socket.next() => {
                match message {
                    Some(Ok(message)) => {
                        let len = message.len();
                        bandwidth.fetch_add(len, Relaxed);

                        if len != 1 {
                            sender.try_send(ProcessorMessage::bytes(message.freeze()))?;
                        } else {
                            debug!("audio_output received keep alive");
                        }
                    }
                    Some(Err(error)) => {
                        error!("audio_output error: {}", error);
                        break Err(error.into());
                    }
                    None => {
                        debug!("audio_output ended with None");
                        break Ok(());
                    }
                }
            }
            _ = cancel.cancelled() => {
                debug!("audio_output ended with cancellation");
                break Ok(());
            },
        }
    }
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
            _ = end_call.notified() => {
                break;
            }
            _ = cancel.cancelled() => {
                break;
            },
            message = input_receiver.recv() => {
                if let Ok(message) = message {
                    if output_sender.try_send(message).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            },
        }
    }
}

/// Collects statistics from throughout the application, processes them, and provides them to the frontend
async fn statistics_collector(
    input_rms: Arc<AtomicF32>,
    output_rms: Arc<AtomicF32>,
    latency: Arc<AtomicUsize>,
    upload_bandwidth: Arc<AtomicUsize>,
    download_bandwidth: Arc<AtomicUsize>,
    callback: DartVoid<Statistics>,
    cancel: CancellationToken,
) -> Result<()> {
    // the interval for statistics updates
    let mut update_interval = interval(Duration::from_millis(100));
    // the interval for the input_max and output_max to decrease
    let mut reset_interval = interval(Duration::from_secs(5));

    let mut input_max = 0_f32;
    let mut output_max = 0_f32;

    loop {
        select! {
            _ = update_interval.tick() => {
                let latency = latency.load(Relaxed);
                LATENCY.store(latency, Relaxed);

                 invoke(&callback, Statistics {
                    input_level: level_from_window(input_rms.load(Relaxed), &mut input_max),
                    output_level: level_from_window(output_rms.load(Relaxed), &mut output_max),
                    latency,
                    upload_bandwidth: upload_bandwidth.load(Relaxed),
                    download_bandwidth: download_bandwidth.load(Relaxed),
                    loss: LOSS.load(Relaxed),
                }).await;

                input_rms.store(0_f32, Relaxed);
                output_rms.store(0_f32, Relaxed);
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
    let statistics = Statistics::default();
    invoke(&callback, statistics).await;

    LATENCY.store(0, Relaxed);
    LOSS.store(0_f64, Relaxed);
    CONNECTED.store(false, Relaxed);

    info!("statistics collector returning");
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
    use crate::api::audio::input_processor;
    use fast_log::Config;
    use kanal::unbounded;
    use log::LevelFilter::Trace;
    use rand::Rng;
    use rand::prelude::SliceRandom;
    use std::fs::read;
    use std::io::Write;
    use std::thread::{sleep, spawn};
    use std::time::Instant;

    const HOGWASH_BYTES: &[u8] = include_bytes!("../../../../assets/models/hogwash.rnn");

    struct BenchmarkResult {
        average: Duration,
        min: Duration,
        max: Duration,
        end: Duration,
    }

    #[ignore]
    #[test]
    fn benchmark() {
        fast_log::init(Config::new().file("bench.log").level(Trace)).unwrap();

        let sample_rate = 44_100;

        let mut samples = Vec::new();
        let bytes = read("../bench.raw").unwrap();

        for chunk in bytes.chunks(4) {
            let sample = f32::from_ne_bytes(chunk.try_into().unwrap());
            samples.push(sample);
        }

        // warmup
        for _ in 0..5 {
            simulate_input_stack(false, false, sample_rate, &samples, 2400);
        }

        let num_iterations = 10;
        let mut results: HashMap<(bool, bool), (Vec<Duration>, Duration)> = HashMap::new();

        for _ in 0..num_iterations {
            let mut cases = vec![(false, false), (false, true), (true, false), (true, true)];
            cases.shuffle(&mut rand::thread_rng()); // Shuffle for each iteration

            for (denoise, codec_enabled) in cases {
                let (durations, end, _) =
                    simulate_input_stack(denoise, codec_enabled, sample_rate, &samples, 2400);

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

    #[test]
    fn packet_burst_simulation() {
        fast_log::init(Config::new().file("burst_simulation.log").level(Trace)).unwrap();

        let sample_rate = 44_100;
        let codec_enabled = true;

        let mut samples = Vec::new();
        let bytes = read("../bench.raw").unwrap();

        let mut duration = 0.0;
        let length = 1_f64 / sample_rate as f64;
        for chunk in bytes.chunks(4) {
            let sample = f32::from_ne_bytes(chunk.try_into().unwrap());
            samples.push(sample);
            duration += length;
        }
        let audio_duration = Duration::from_secs_f64(duration);
        info!(
            "loaded audio with length {:?} samples_len={}",
            audio_duration,
            samples.len()
        );

        let now = Instant::now();
        // use the input stack simulator to construct realistic stream of ProcessorMessage
        let (_, _, messages) =
            simulate_input_stack(true, codec_enabled, sample_rate, &samples, CHANNEL_SIZE);
        info!(
            "processed {} messages in {:?}",
            messages.len(),
            now.elapsed()
        );

        let now = Instant::now();
        // use the output stack simulator to process the messages in a burst situation
        let received_samples = simulate_output_stack(
            messages,
            CHANNEL_SIZE,
            codec_enabled,
            sample_rate as f64,
            sample_rate as f64 / 48_000_f64,
        );
        info!(
            "received {} samples in {:?} aprox {}",
            received_samples.len(),
            now.elapsed(),
            received_samples.len() as f64 / sample_rate as f64
        );

        // save processed samples to output file
        let mut output = std::fs::File::create("../bench-out.raw").unwrap();
        for sample in received_samples {
            output.write(sample.to_ne_bytes().as_slice()).unwrap();
        }
    }

    fn simulate_input_stack(
        denoise: bool,
        codec_enabled: bool,
        sample_rate: u32,
        samples: &[f32],
        channel_size: usize,
    ) -> (Vec<Duration>, Duration, Vec<ProcessorMessage>) {
        // input stream -> input processor
        let (input_sender, input_receiver) = bounded(channel_size);

        // input processor -> encoder or dummy
        let (processed_input_sender, processed_input_receiver) = unbounded::<ProcessorMessage>();

        // encoder -> dummy
        let (encoded_input_sender, encoded_input_receiver) = unbounded::<ProcessorMessage>();

        let model = RnnModel::from_bytes(HOGWASH_BYTES).unwrap();
        let denoiser = denoise.then_some(DenoiseState::from_model(model));

        spawn(move || {
            let result = input_processor(
                input_receiver,
                processed_input_sender,
                sample_rate as f64,
                Arc::new(AtomicF32::new(1_f32)),
                Arc::new(AtomicF32::new(db_to_multiplier(50_f32))),
                Arc::new(AtomicBool::new(false)),
                denoiser,
                Default::default(),
                codec_enabled,
            );

            if let Err(error) = result {
                error!("{}", error);
            }
        });

        let output_receiver = if codec_enabled {
            spawn(move || {
                encoder(
                    processed_input_receiver,
                    encoded_input_sender,
                    if denoise { 48_000 } else { sample_rate },
                    true,
                    5.0,
                    false,
                );
            });

            encoded_input_receiver
        } else {
            processed_input_receiver
        };

        let handle = spawn(move || {
            let start = Instant::now();
            let mut now = Instant::now();
            let mut durations = Vec::new();
            let mut messages = Vec::new();

            while let Ok(message) = output_receiver.recv() {
                durations.push(now.elapsed());
                now = Instant::now();
                messages.push(message);
            }

            let end = start.elapsed();
            (durations, end, messages)
        });

        for sample in samples {
            input_sender.send(*sample).unwrap();
        }
        _ = input_sender.close();
        handle.join().unwrap()
    }

    fn simulate_output_stack(
        input: Vec<ProcessorMessage>,
        channel_size: usize,
        codec_enabled: bool,
        sample_rate: f64,
        ratio: f64,
    ) -> Vec<f32> {
        // receiving socket -> output processor or decoder
        let (network_output_sender, network_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // decoder -> output processor
        let (decoded_output_sender, decoded_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // output processor -> dummy output stream
        let (output_sender, output_receiver) = bounded::<f32>(channel_size * 4);

        let output_processor_receiver = if codec_enabled {
            spawn(move || {
                decoder(
                    network_output_receiver.to_sync(),
                    decoded_output_sender.to_sync(),
                    None,
                );
            });

            decoded_output_receiver.to_sync()
        } else {
            network_output_receiver.to_sync()
        };

        spawn(move || {
            output_processor(
                output_processor_receiver,
                output_sender,
                ratio,
                Arc::new(AtomicF32::new(1_f32)),
                Default::default(),
            )
        });

        // simulate network dumping burst of packets into sender
        let sender = network_output_sender.to_sync();
        spawn(move || {
            let interval = Duration::from_secs_f64(FRAME_SIZE as f64 / sample_rate);
            let mut c = 0;

            for i in input {
                _ = sender.send(i);
                c += 1;

                // big ol lag spike + packet dump
                if c < 525 || c > 550 {
                    sleep(interval);
                } else if c == 500 {
                    sleep(Duration::from_millis(250));
                }
            }
        });

        let mut result = Vec::new();

        // mildly accurate simulation of an output stream reading at sample_rate
        let interval = Duration::from_secs_f64(2048_f64 / sample_rate);
        'outer: loop {
            for _ in 0..2048 {
                if let Ok(sample) = output_receiver.recv() {
                    result.push(sample);
                } else {
                    break 'outer;
                }
            }

            sleep(interval);
        }

        result
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
    pub(crate) fn dummy_frame() -> [f32; FRAME_SIZE] {
        let mut frame = [0_f32; FRAME_SIZE];
        let mut rng = rand::thread_rng();
        rng.fill(&mut frame[..]);

        for x in &mut frame {
            *x = x.clamp(i16::MIN as f32, i16::MAX as f32);
            *x /= i16::MAX as f32;
        }

        frame
    }

    pub(crate) fn dummy_int_frame() -> [i16; FRAME_SIZE] {
        let mut frame = [0_i16; FRAME_SIZE];
        let mut rng = rand::thread_rng();
        rng.fill(&mut frame[..]);
        frame
    }
}
