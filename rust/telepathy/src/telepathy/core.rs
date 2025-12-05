#[cfg(target_os = "ios")]
use crate::audio::ios::{configure_audio_session, deactivate_audio_session};
use crate::error::ErrorKind;
use crate::flutter::callbacks::{FrbCallbacks, FrbStatisticsCallback};
use crate::flutter::{
    CallState, ChatMessage, CodecConfig, Contact, DartNotify, NetworkConfig, ScreenshareConfig,
    SessionStatus,
};
use crate::overlay::CONNECTED;
use crate::overlay::overlay::Overlay;
use crate::telepathy::Result;
use crate::telepathy::messages::Message;
#[cfg(not(target_family = "wasm"))]
use crate::telepathy::screenshare;
use crate::telepathy::sockets::{
    ConstSocket, SendingSockets, SharedSockets, Transport, TransportStream, audio_input,
    audio_output,
};
use crate::telepathy::utils::{read_message, write_message};
use crate::telepathy::{
    CHAT_PROTOCOL, ConnectionState, DeviceName, EarlyCallState, HELLO_TIMEOUT, KEEP_ALIVE,
    OptionalCallArgs, PeerState, RoomConnection, RoomMessage, RoomState, SessionState,
    StartScreenshare, StatisticsCollectorState, loopback, statistics_collector,
    stream_to_audio_transport,
};
use crate::{Behaviour, BehaviourEvent};
use atomic_float::AtomicF32;
use chrono::Local;
use cpal::Host;
use cpal::traits::StreamTrait;
use flutter_rust_bridge::for_generated::futures::StreamExt;
use flutter_rust_bridge::spawn;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
#[cfg(not(target_family = "wasm"))]
use libp2p::tcp;
use libp2p::{
    Multiaddr, PeerId, Stream, autonat, dcutr, dcutr::Event as DcutrEvent, identify,
    identify::Event as IdentifyEvent, noise, ping, yamux,
};
use libp2p_stream::Control;
use log::{debug, error, info, trace, warn};
use nnnoiseless::RnnModel;
use std::collections::HashMap;
use std::marker::PhantomData;
#[cfg(not(target_family = "wasm"))]
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::mpsc::{Receiver as MReceiver, Sender as MSender, channel};
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::{Interval, interval, sleep, timeout};
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) struct TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    /// The audio host
    pub(crate) host: Arc<Host>,

    /// Controls the threshold for silence detection
    pub(crate) rms_threshold: Arc<AtomicF32>,

    /// The factor to adjust the input volume by
    pub(crate) input_volume: Arc<AtomicF32>,

    /// The factor to adjust the output volume by
    pub(crate) output_volume: Arc<AtomicF32>,

    /// Enables rnnoise denoising
    pub(crate) denoise: Arc<AtomicBool>,

    /// The rnnoise model
    pub(crate) denoise_model: Arc<RwLock<RnnModel>>,

    /// Manually set the input device
    pub(crate) input_device: DeviceName,

    /// Manually set the output device
    pub(crate) output_device: DeviceName,

    /// The current libp2p private key
    pub(crate) identity: Arc<RwLock<Keypair>>,

    /// Keeps track of whether the user is in a call
    pub(crate) in_call: Arc<AtomicBool>,

    /// used to end an audio test, if there is one
    pub(crate) end_audio_test: Arc<Mutex<Option<Arc<Notify>>>>,

    /// Tracks state for the current room
    pub(crate) room_state: Arc<RwLock<Option<RoomState>>>,

    /// Disables the output stream
    pub(crate) deafened: Arc<AtomicBool>,

    /// Disables the input stream
    pub(crate) muted: Arc<AtomicBool>,

    /// Disables the playback of custom ringtones
    pub(crate) play_custom_ringtones: Arc<AtomicBool>,

    /// Enables sending your custom ringtone
    pub(crate) send_custom_ringtone: Arc<AtomicBool>,

    pub(crate) efficiency_mode: Arc<AtomicBool>,

    /// Keeps track of and controls the sessions
    pub(crate) session_states: Arc<RwLock<HashMap<PeerId, Arc<SessionState>>>>,

    /// Signals the session manager to start a new session
    pub(crate) start_session: MSender<PeerId>,

    /// Signals the session manager to start a screenshare
    pub(crate) start_screenshare: MSender<StartScreenshare>,

    /// Restarts the session manager when needed
    pub(crate) restart_manager: Arc<Notify>,

    /// Network configuration for p2p connections
    pub(crate) network_config: NetworkConfig,

    /// Configuration for the screenshare functionality
    #[allow(dead_code)]
    pub(crate) screenshare_config: ScreenshareConfig,

    /// A reference to the object that controls the call overlay
    pub(crate) overlay: Overlay,

    pub(crate) codec_config: CodecConfig,

    #[cfg(target_family = "wasm")]
    pub(crate) web_input: Arc<Mutex<Option<crate::audio::web_audio::WebAudioWrapper>>>,

    /// callback methods provided by the flutter frontend
    pub(crate) callbacks: Arc<C>,

    phantom: PhantomData<Arc<S>>,
}

impl<C, S> TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    /// main entry point to Telepathy. must be async to use `spawn`
    pub(crate) async fn new(
        identity: Vec<u8>,
        host: Arc<Host>,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: C,
    ) -> TelepathyCore<C, S> {
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
            callbacks: Arc::new(callbacks),
            phantom: Default::default(),
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

        chat
    }

    /// Starts new sessions
    pub(crate) async fn session_manager(
        &self,
        start: &mut MReceiver<PeerId>,
        screenshare: &mut MReceiver<StartScreenshare>,
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
        self.callbacks.manager_active(true, true).await;

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
                Some(event) = swarm.next() => event,
                // start a new session
                Some(peer_id) = start.recv() => {
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
                        SessionStatus::Connecting
                    };

                    self.callbacks.session_status(status, peer_id).await;
                    continue;
                }
                // starts a stream for outgoing screen shares
                Some(message) = screenshare.recv() => {
                    info!("starting screenshare for {message:?}");

                    #[cfg(not(target_family = "wasm"))]
                    if let Some(state) = self.session_states.read().await.get(&message.peer) {
                        let stop = Arc::new(Notify::new());
                        let dart_stop = DartNotify { inner: stop.clone() };

                        if let Some(Message::ScreenshareHeader { encoder_name }) = message.header {
                            let stream_result = swarm.behaviour().stream.new_control().open_stream(message.peer, CHAT_PROTOCOL).await;
                            match stream_result {
                                Ok(stream) => {
                                    let width = self.screenshare_config.width.load(Relaxed);
                                    let height = self.screenshare_config.height.load(Relaxed);
                                    let bandwidth = state.download_bandwidth.clone();
                                    spawn(screenshare::playback(stream, stop, bandwidth, encoder_name, width, height));
                                    self.callbacks.screenshare_started(dart_stop, false).await;
                                }
                                Err(error) => {
                                    error!("failed to open stream for screenshare playback {error}");
                                }
                            }
                        } else if let Some(config) = self.screenshare_config.recording_config.read().await.clone() {
                            _ = state.message_sender.send(Message::ScreenshareHeader { encoder_name: config.encoder.to_string() }).await;
                            match state.receive_stream().await {
                                Ok(stream) => {
                                    spawn(screenshare::record(stream, stop, state.upload_bandwidth.clone(), config));
                                    self.callbacks.screenshare_started(dart_stop, true).await;
                                }
                                Err(error) => {
                                    error!("failed to receive sub-stream for screenshare broadcast {error}");
                                }
                            }
                        } else {
                            // the frontend blocks this case
                            warn!("screenshare started without recording configuration");
                        }
                    } else {
                        warn!("screenshare started for a peer without a session: {}", message.peer);
                    }

                    continue;
                }
                else => {
                    warn!("session manager hit else");
                    break;
                },
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

                    let contact = self.callbacks.get_contact(peer_id.to_bytes()).await;
                    let relayed = endpoint.is_relayed();
                    let listener = endpoint.is_listener();

                    if contact.is_none() && !self.is_in_room(&peer_id).await {
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
                                .session_status(SessionStatus::Connecting, peer_id)
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
                            .session_status(SessionStatus::Inactive, peer_id)
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
                            .session_status(SessionStatus::Inactive, peer_id)
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
        self.callbacks.manager_active(false, false).await;
        stop_handler.notify_one();
        stream_handler_handle.await??;
        debug!("joined stream handler");

        Ok(())
    }

    /// Handles incoming streams for the libp2p swarm
    pub(crate) async fn incoming_stream_handler(
        &self,
        mut control: Control,
        stop: Arc<Notify>,
    ) -> Result<()> {
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

                    self.initialize_session(peer, None, stream).await;
                }
                else => break Err(ErrorKind::StreamsEnded.into())
            }
        }
    }

    /// Called by the dialer to open a stream and session
    pub(crate) async fn open_session(
        &self,
        peer: PeerId,
        mut control: Control,
        connection_count: usize,
        peer_states: &mut HashMap<PeerId, PeerState>,
    ) {
        // it may take multiple tries to open the stream because the of the RNG in the stream handler
        for _ in 0..connection_count {
            match control.open_stream(peer, CHAT_PROTOCOL).await {
                Ok(stream) => {
                    info!("opened stream with {}, starting new session", peer);
                    self.initialize_session(peer, Some(control), stream).await;
                    // the peer state is no longer needed
                    peer_states.remove(&peer);
                    return;
                }
                Err(error) => {
                    warn!("OpenStreamError for {peer}: {error}");
                }
            }
        }

        error!("failed to open stream for {peer} after {connection_count} tries");
    }

    /// Entry point to a session that sets up state and spawns session outer
    pub(crate) async fn initialize_session(
        &self,
        peer: PeerId,
        control: Option<Control>,
        stream: Stream,
    ) {
        let contact_option = self.callbacks.get_contact(peer.to_bytes()).await;
        // sends messages to the session from elsewhere in the program
        let message_channel = channel::<Message>(8);
        // create the state and a clone of it for the session
        let state = Arc::new(SessionState::new(&message_channel.0));
        // insert the new state
        let old_state_option = self
            .session_states
            .write()
            .await
            .insert(peer, state.clone());

        if let Some(old_state) = old_state_option {
            warn!("{peer} already had a session");
            // if the session was in a call, end it so the session can end
            old_state.end_call.notify_one();
            // stop the session
            old_state.stop_session.notify_one();
        }

        let contact = if let Some(contact) = contact_option {
            // alert the UI that this session is now connected
            self.callbacks
                .session_status(SessionStatus::Connected, peer)
                .await;
            contact
        } else {
            // there may be no contact for members of a group
            Contact {
                id: Uuid::new_v4().to_string(),
                nickname: String::from("GroupContact"),
                peer_id: peer,
                is_room_only: true,
            }
        };

        let self_clone = self.clone();
        spawn(async move {
            self_clone
                .session_outer(peer, control, stream, state, contact, message_channel)
                .await;
        });
    }

    /// Runs session inner as many times as needed, performs cleanup if needed
    pub(crate) async fn session_outer(
        &self,
        peer: PeerId,
        mut control: Option<Control>,
        stream: Stream,
        state: Arc<SessionState>,
        contact: Contact,
        mut message_channel: (MSender<Message>, MReceiver<Message>),
    ) {
        // controls keep alive messages
        let mut keep_alive = interval(KEEP_ALIVE);
        // the length delimited transport used for the session
        let mut transport = LengthDelimitedCodec::builder()
            .max_frame_length(usize::MAX)
            .length_field_type::<u64>()
            .new_framed(stream.compat());

        // the dialer for room sessions always starts a call
        if self.is_in_room(&peer).await && control.is_some() {
            state.start_call.notify_one();
        }

        let result = loop {
            let result = self
                .session_inner(
                    &contact,
                    control.as_mut(),
                    &mut transport,
                    &state,
                    &mut message_channel,
                    &mut keep_alive,
                )
                .await;

            match (result, contact.is_room_only) {
                // the session was stopped
                (Ok(false), _) => break Ok(false),
                // room only sessions never continue
                (Ok(true), true) => break Ok(true),
                // normal session continue
                (Ok(true), false) => {
                    // the session is not in a call
                    state.in_call.store(false, Relaxed);
                }
                (Err(error), room_only) => {
                    // if an error occurred during a non-room call, it is ended now
                    if state.in_call.load(Relaxed)
                        && !room_only
                        && !self.is_in_room(&contact.peer_id).await
                    {
                        warn!("session error while call active, alerting ui (e={error:?})");
                        self.callbacks
                            .call_state(CallState::CallEnded(error.to_string(), false))
                            .await;
                    }

                    if room_only || error.is_session_critical() {
                        // session cannot recover from these errors
                        error!("Session error for {}: {error:?}", contact.nickname);
                        break Err(error);
                    } else {
                        warn!("recoverable session failure: {:?}", error);
                        // the session is not in a call
                        state.in_call.store(false, Relaxed);
                    }
                }
            }
        };

        match result {
            // session cleanup required
            Ok(true) | Err(_) => (),
            // the session has already been cleaned up
            Ok(false) => {
                warn!("Session for {} stopped", contact.nickname);
                return;
            }
        }

        // cleanup
        self.session_states.write().await.remove(&peer);

        // avoid sending session statuses for dummy contacts
        if !contact.is_room_only {
            self.callbacks
                .session_status(SessionStatus::Inactive, peer)
                .await;
        }

        info!("Session for {} cleaned up", contact.nickname);
    }

    /// The inner logic of a session that may execute many times
    /// Returns true if the session should continue
    pub(crate) async fn session_inner(
        &self,
        contact: &Contact,
        control: Option<&mut Control>,
        transport: &mut Transport<TransportStream>,
        state: &Arc<SessionState>,
        message_channel: &mut (MSender<Message>, MReceiver<Message>),
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
                    let cancel = Arc::new(Notify::new());
                    accept_handle = Some(self.callbacks.get_accept_handle(&contact.id, other_ringtone, &cancel));
                    cancel_prompt = Some(cancel);
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
                                self.call_handshake(transport, control, &mut message_channel.1, state, call_state).await?;
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
                                        self.call_handshake(transport, control, &mut message_channel.1, state, call_state).await?;
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
                                self.callbacks.call_state(CallState::CallEnded(message, true)).await;
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

    /// gets everything ready for the call
    pub(crate) async fn call_handshake(
        &self,
        transport: &mut Transport<TransportStream>,
        control: Option<&mut Control>,
        message_receiver: &mut MReceiver<Message>,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
        let stream = state.open_stream(control, &call_state).await?;

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
                    message_receiver,
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
            Err(error) => {
                let message = Message::Goodbye {
                    reason: Some(if error.is_audio_error() {
                        "Audio device error".to_string()
                    } else {
                        error.to_string()
                    }),
                };
                write_message(transport, &message).await?;
                Err(error)
            }
        }
    }

    /// normal call & self-test logic
    pub(crate) async fn call(
        &self,
        stop_io: &CancellationToken,
        call_state: EarlyCallState,
        end_call: &Arc<Notify>,
        optional: Option<OptionalCallArgs<'_>>,
    ) -> Result<()> {
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // shared statistics values
        let statistics_state = StatisticsCollectorState::new(optional.as_ref().map(|o| o.state));
        // references for use in networking threads
        let upload_bandwidth = statistics_state.upload_bandwidth.clone();
        let download_bandwidth = statistics_state.download_bandwidth.clone();
        let loss = statistics_state.loss.clone();

        // the two clients agree on these codec options
        let codec_config = call_state.codec_config();

        let input_channel = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                codec_config,
                &statistics_state,
                false,
            )
            .await?;

        let (output_sender, output_stream) = self
            .setup_output(
                call_state.remote_configuration.sample_rate as f64,
                codec_config.0,
                &statistics_state,
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
            statistics_state,
            self.callbacks.statistics_callback(),
            stop_io.clone(),
        ));

        if let Some(o) = optional {
            let (write, read) = o.audio_transport.split();

            let input_handle = spawn(audio_input(
                input_channel.0,
                ConstSocket::new(write),
                stop_io.clone(),
                upload_bandwidth,
            ));

            let output_handle = spawn(audio_output(
                output_sender,
                read,
                stop_io.clone(),
                download_bandwidth,
                loss,
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
                self.callbacks
                    .call_state(CallState::CallEnded(message, true))
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
    pub(crate) async fn call_controller(
        &self,
        transport: &mut Transport<TransportStream>,
        receiver: &mut MReceiver<Message>,
        peer: PeerId,
        end_call: &Arc<Notify>,
    ) -> Result<(Option<String>, bool)> {
        let identity = self.identity.read().await.public().to_peer_id();

        CONNECTED.store(true, Relaxed);
        self.callbacks.call_state(CallState::Connected).await;

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
                            self.callbacks.message_received(ChatMessage {
                                text,
                                receiver: identity,
                                timestamp: Local::now(),
                                attachments,
                            }).await;
                        }
                        Message::ScreenshareHeader { .. } => {
                            info!("received screenshare header {:?}", message);
                            self.send_start_screenshare(peer, Some(message)).await;
                        }
                        _ => error!("call controller unexpected message: {:?}", message),
                    }
                },
                // sends messages to the callee
                result = receiver.recv() => {
                    if let Some(message) = result {
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

    /// manages connection with one room peer
    pub(crate) async fn room_handshake(
        &self,
        transport: &mut Transport<TransportStream>,
        control: Option<&mut Control>,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
        let stream = state.open_stream(control, &call_state).await?;
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
            .await
            .map_err(|_| ErrorKind::RoomStateMissing)?;

        loop {
            select! {
                result = read_message::<Message, _>(transport) => {
                    match result {
                        Ok(Message::Goodbye { .. }) | Err(_) => {
                            break;
                        }
                        Ok(Message::Chat { .. }) => {
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
            }
        }

        // sender may already be closed at this point
        _ = sender.send(RoomMessage::Leave(peer_id)).await;
        Ok(())
    }

    /// the controller for rooms
    pub(crate) async fn room_controller(
        &self,
        mut receiver: MReceiver<RoomMessage>,
        end_sessions: CancellationToken,
        call_state: EarlyCallState,
        stop_io: &CancellationToken,
        end_call: Arc<Notify>,
    ) -> Result<()> {
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // moves sockets to audio_input
        let new_sockets = SharedSockets::default();
        // shared statistics
        let statistics_state = StatisticsCollectorState::new(None);
        // tracks connection state for peers
        let mut connections = HashMap::new();

        let input_channel = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                (true, true, 5_f32), // hard coded room codec options
                &statistics_state,
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
            SendingSockets::new(new_sockets.clone()),
            stop_io.clone(),
            statistics_state.upload_bandwidth.clone(),
        ));

        spawn(statistics_collector(
            statistics_state.clone(),
            self.callbacks.statistics_callback(),
            stop_io.clone(),
        ));

        // kick the UI out of connecting mode
        self.callbacks.call_state(CallState::Waiting).await;

        loop {
            select! {
                Some(message) = receiver.recv() => {
                    match message{
                        RoomMessage::Join { audio_transport, state } => {
                            info!("received room Join [p={}]", state.peer);

                            // first connection
                            if connections.is_empty() {
                                CONNECTED.store(true, Relaxed);
                                self.callbacks.call_state(CallState::Connected).await;
                            }

                            let (write, read) = (*audio_transport).split();
                            // this unwrap is safe because audio_input never panics
                            new_sockets.lock().unwrap().push((write, Instant::now()));
                            // setup output stack
                            let (output_sender, output_stream) = self
                                .setup_output(
                                    state.remote_configuration.sample_rate as f64,
                                    true,
                                    &statistics_state,
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
                                statistics_state.download_bandwidth.clone(),
                                statistics_state.loss.clone(),
                            ));

                            connections.insert(state.peer, RoomConnection {
                                stream: output_stream,
                                handle,
                            });
                            self.callbacks.call_state(CallState::RoomJoin(state.peer.to_string())).await;
                        }
                        RoomMessage::Leave(peer) => {
                            self.callbacks.call_state(CallState::RoomLeave(peer.to_string())).await;

                            if let Some(connection) = connections.remove(&peer) {
                                connection.handle.await??;
                                info!("successfully cleaned up room connection [p={peer}]");
                            } else {
                                warn!("Leave for peer without room connection [p={peer}]");
                            }
                        }
                    }
                }
                _ = end_call.notified() => {
                    break;
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
}

impl<C, S> Clone for TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            host: Arc::clone(&self.host),
            rms_threshold: Arc::clone(&self.rms_threshold),
            input_volume: Arc::clone(&self.input_volume),
            output_volume: Arc::clone(&self.output_volume),
            denoise: Arc::clone(&self.denoise),
            denoise_model: Arc::clone(&self.denoise_model),
            input_device: self.input_device.clone(),
            output_device: self.output_device.clone(),
            identity: Arc::clone(&self.identity),
            in_call: Arc::clone(&self.in_call),
            end_audio_test: Arc::clone(&self.end_audio_test),
            room_state: Arc::clone(&self.room_state),
            deafened: Arc::clone(&self.deafened),
            muted: Arc::clone(&self.muted),
            play_custom_ringtones: Arc::clone(&self.play_custom_ringtones),
            send_custom_ringtone: Arc::clone(&self.send_custom_ringtone),
            efficiency_mode: Arc::clone(&self.efficiency_mode),
            session_states: Arc::clone(&self.session_states),
            start_session: self.start_session.clone(),
            start_screenshare: self.start_screenshare.clone(),
            restart_manager: Arc::clone(&self.restart_manager),
            network_config: self.network_config.clone(),
            screenshare_config: self.screenshare_config.clone(),
            overlay: self.overlay.clone(),
            codec_config: self.codec_config.clone(),
            #[cfg(target_family = "wasm")]
            web_input: Arc::clone(&self.web_input),
            callbacks: Arc::clone(&self.callbacks),
            phantom: self.phantom,
        }
    }
}
