use crate::BehaviourEvent;
#[cfg(target_os = "ios")]
use crate::audio::ios::{configure_audio_session, deactivate_audio_session};
#[cfg(target_family = "wasm")]
use crate::audio::web_audio::WebAudioWrapper;
use crate::error::ErrorKind;
use crate::flutter::callbacks::{FrbCallbacks, FrbStatisticsCallback};
use crate::flutter::{
    CallState, ChatMessage, CodecConfig, Contact, NetworkConfig, ScreenshareConfig, SessionStatus,
};
use crate::overlay::CONNECTED;
use crate::overlay::overlay::Overlay;
use crate::telepathy::Result;
use crate::telepathy::messages::Message;
use crate::telepathy::sockets::{
    ConstSocket, SendingSockets, SharedSockets, Transport, TransportStream, audio_input,
    audio_output,
};
use crate::telepathy::utils::{
    loopback, read_message, statistics_collector, stream_to_audio_transport, write_message,
};
use crate::telepathy::{
    CHAT_PROTOCOL, DeviceName, EarlyCallState, HELLO_TIMEOUT, KEEP_ALIVE, OptionalCallArgs,
    RoomConnection, RoomMessage, RoomState, SessionState, StartScreenshare,
    StatisticsCollectorState,
};
use atomic_float::AtomicF32;
use chrono::Local;
use cpal::Host;
use cpal::traits::StreamTrait;
#[cfg(target_family = "wasm")]
use flutter_rust_bridge::JoinHandle;
use flutter_rust_bridge::for_generated::futures::StreamExt;
use flutter_rust_bridge::spawn;
use libp2p::core::ConnectedPoint;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{ConnectionId, SwarmEvent};
use libp2p::{PeerId, Stream, dcutr::Event as DcutrEvent, identify::Event as IdentifyEvent};
use libp2p_stream::Control;
use log::{debug, error, info, trace, warn};
use nnnoiseless::RnnModel;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};
use tokio::select;
use tokio::sync::mpsc::{Receiver as MReceiver, Sender as MSender, channel};
use tokio::sync::{Mutex, Notify, RwLock};
#[cfg(not(target_family = "wasm"))]
use tokio::task::JoinHandle;
#[cfg(not(target_family = "wasm"))]
use tokio::time::{Interval, interval, timeout};
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::{Interval, interval, timeout};

pub(crate) struct TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    /// The audio host
    pub(crate) host: Arc<Host>,

    /// Core state for telepathy
    pub(crate) core_state: CoreState,

    /// Tracks state for the current room
    pub(crate) room_state: Arc<RwLock<Option<RoomState>>>,

    /// Keeps track of and controls the sessions
    pub(crate) session_states: Arc<RwLock<HashMap<PeerId, Arc<SessionState>>>>,

    /// Signals the session manager to start a new session
    pub(crate) start_session: Option<MSender<PeerId>>,

    /// Signals the session manager to start a screenshare
    pub(crate) start_screenshare: Option<MSender<StartScreenshare>>,

    /// Restarts the session manager when needed
    pub(crate) restart_manager: Arc<Notify>,

    /// A reference to the object that controls the call overlay
    pub(crate) overlay: Overlay,

    /// A wrapper to provide audio input on the web
    #[cfg(target_family = "wasm")]
    pub(crate) web_input: Arc<Mutex<Option<WebAudioWrapper>>>,

    /// callback methods provided by the flutter frontend
    pub(crate) callbacks: Arc<C>,

    phantom: PhantomData<Arc<S>>,
}

impl<C, S> TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    pub(crate) fn new(
        host: Arc<Host>,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: C,
    ) -> TelepathyCore<C, S> {
        Self {
            host,
            core_state: CoreState {
                network_config: network_config.clone(),
                screenshare_config: screenshare_config.clone(),
                codec_config: codec_config.clone(),
                ..CoreState::default()
            },
            room_state: Default::default(),
            session_states: Default::default(),
            start_session: None,
            start_screenshare: None,
            restart_manager: Default::default(),
            overlay: overlay.clone(),
            #[cfg(target_family = "wasm")]
            web_input: Default::default(),
            callbacks: Arc::new(callbacks),
            phantom: Default::default(),
        }
    }

    /// Spawns the manager & returns the handle if no manager exists yet
    pub(crate) async fn start_manager(&mut self) -> Option<JoinHandle<()>> {
        // only allow one manager
        if self.start_screenshare.is_some() || self.start_session.is_some() {
            return None;
        }

        let (start_session, mut receive_session) = channel(8);
        let (start_screenshare, mut receive_screenshare) = channel(8);

        self.start_session = Some(start_session);
        self.start_screenshare = Some(start_screenshare);

        // start the session manager
        let manager_clone = self.clone();
        Some(spawn(async move {
            // break when stop_manager==true
            while !manager_clone.core_state.stop_manager.load(Relaxed) {
                // run the session manager to completion
                let result = manager_clone
                    .session_manager(&mut receive_session, &mut receive_screenshare)
                    .await;

                if let Err(error) = result {
                    error!("Session manager failed: {}", error);
                }
            }
        }))
    }

    /// Ends all sessions & restores session_states to default
    pub(crate) async fn reset_sessions(&self) {
        for (_, session) in self.session_states.write().await.drain() {
            // stops any call
            session.end_call.notify_one();
            // stops the session loop
            session.stop_session.cancel();
            // stops any active screenshare threads
            if let Some(notify) = session.stop_screenshare.lock().await.take() {
                notify.notify_waiters();
            }
        }
    }

    /// Builds the libp2p swarm, handles session start requests, screenshare messages, and libp2p events.
    /// spawns outgoing sessions & screenshare threads
    pub(crate) async fn session_manager(
        &self,
        start: &mut MReceiver<PeerId>,
        screenshare: &mut MReceiver<StartScreenshare>,
    ) -> Result<()> {
        // build the swarm & connect to relay
        let (mut swarm, relay_address) = self.setup_swarm().await?;
        // contains the state needed for negotiating sessions
        let mut peer_states: HashMap<PeerId, PeerState> = HashMap::new();
        // handles to threads spawned by the session manager
        let mut handles = Vec::new();
        // preload public identity
        let public_identity = self.peer_id().await;
        // preload the relay identity
        let relay_identity = *self.core_state.network_config.relay_id.read().await;

        // handle incoming streams
        let control = swarm.behaviour().stream.new_control();
        let stop_handler = Arc::new(Notify::new());
        let stop_handler_clone = stop_handler.clone();
        let self_clone = self.clone();
        let stream_handler_handle = spawn(async move {
            self_clone
                .incoming_stream_handler(control, stop_handler_clone)
                .await
        });

        // alerts the UI that the manager is active
        self.callbacks.manager_active(true, true).await;
        // the manager is about to start processing events
        self.core_state.manager_active.notify_waiters();

        loop {
            // extract peers with single connection session states
            let single_connections: Vec<_> = peer_states
                .iter()
                .filter(|(_, s)| s.connections.len() == 1)
                .filter_map(|(p, s)| {
                    s.connections
                        .iter()
                        .next()
                        .map(|(_, c)| (*p, s.selected_connection, c.relayed))
                })
                .collect();

            for (peer, selected, relayed) in single_connections {
                if selected {
                    // open a session control stream and start the session controller
                    self.open_session(
                        peer,
                        swarm.behaviour().stream.new_control(),
                        &mut peer_states,
                        &mut handles,
                        relayed,
                    )
                    .await;
                } else if let Some(session) = self.session_states.read().await.get(&peer) {
                    // only the non-dialing peer will reach this branch
                    // this peer state is no longer needed
                    peer_states.remove(&peer);
                    // set the real relayed status for the session
                    session.relayed.store(relayed, Relaxed);
                    // update the relayed status in the frontend
                    self.callbacks
                        .session_status(SessionStatus::Connected { relayed }, peer)
                        .await;
                }
            }

            let event = select! {
                // restart the manager
                _ = self.restart_manager.notified() => {
                    break;
                }
                // events are handled outside the select to help with spagetification
                Some(event) = swarm.next() => event,
                // start a new session
                Some(peer_id) = start.recv() => {
                    if peer_id == public_identity {
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
                        // insert a dialer peer state right away
                        peer_states.insert(peer_id, PeerState::dialer());
                        SessionStatus::Connecting
                    };

                    self.callbacks.session_status(status, peer_id).await;
                    continue;
                }
                // starts a stream for outgoing screen shares
                Some(message) = screenshare.recv() => {
                    info!("starting screenshare for {message:?}");

                    #[cfg(not(target_family = "wasm"))]
                    {
                        // when the header is some, a control is required to open the stream
                        let control_option = message.header.is_some()
                            .then(|| swarm.behaviour().stream.new_control());
                        let self_clone = self.clone();
                        spawn(async move {
                            let result = self_clone.start_screenshare(message, control_option).await;
                            if let Err(error) = result {
                                error!("failed to start screenshare: {error:?}");
                            }
                        });
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
                    if peer_id == relay_identity {
                        // ignore the relay connection
                        continue;
                    } else if self.session_states.read().await.contains_key(&peer_id) {
                        // TODO does this case ever hit in normal operation or does it only occur when the session is invalidated by a crash or other failure?
                        // ignore connections with peers who have a session
                        warn!("ignored connection from {} (EDGE CASE DETECTED)", peer_id);
                        continue;
                    }

                    let contact = self.callbacks.get_contact(peer_id.to_bytes()).await;
                    let listener = endpoint.is_listener();

                    if contact.is_none() && !self.is_in_room(&peer_id).await {
                        warn!("received a connection from an unknown peer: {:?}", peer_id);
                        if swarm.disconnect_peer_id(peer_id).is_err() {
                            warn!("unknown peer was no longer connected");
                        }
                    } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        // if two clients dial each other at the same time, one switches to non-dialer
                        if listener && peer_state.dialer {
                            debug!("dialer got incoming listener connection");
                            if peer_id < public_identity {
                                info!("one client switching to non-dialer");
                                peer_state.dialer = false;
                            }
                        }

                        // track the new connection
                        peer_state
                            .connections
                            .insert(connection_id, endpoint.into());
                    } else if listener {
                        info!("non-dialer established first connection with {peer_id}");
                        // insert initial non-dialer state
                        peer_states.insert(peer_id, PeerState::non_dialer(endpoint, connection_id));
                        // alert the frontend that the session is connecting
                        self.callbacks
                            .session_status(SessionStatus::Connecting, peer_id)
                            .await;
                    } else {
                        warn!("potential edge case; unreachable branch");
                    }
                }
                SwarmEvent::OutgoingConnectionError {
                    peer_id: Some(peer_id),
                    error,
                    connection_id,
                } => {
                    let has_session = self.session_states.read().await.contains_key(&peer_id);
                    let remove_state = if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        peer_state.connections.remove(&connection_id);
                        peer_state.connections.is_empty()
                    } else {
                        false
                    };

                    warn!(
                        "outgoing connection failed for {peer_id} because {error} has_session={has_session} remove_state={remove_state}"
                    );

                    // session initialization failed
                    if !has_session {
                        self.callbacks
                            .session_status(SessionStatus::Inactive, peer_id)
                            .await;
                    }

                    // clean up peer states
                    if remove_state {
                        peer_states.remove(&peer_id);
                    }
                }
                SwarmEvent::ConnectionClosed {
                    peer_id,
                    cause,
                    connection_id,
                    ..
                } => {
                    let remove_state = if !swarm.is_connected(&peer_id) {
                        // if there is no connection to the peer, the session initialization failed
                        self.callbacks
                            .session_status(SessionStatus::Inactive, peer_id)
                            .await;
                        true
                    } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        // untrack the connection
                        peer_state.connections.remove(&connection_id);
                        peer_state.connections.is_empty()
                    } else {
                        warn!("unexpected ConnectionClosed id={connection_id}: {cause:?}");
                        continue;
                    };

                    if remove_state {
                        info!("removing unused peer state for {peer_id}");
                        peer_states.remove(&peer_id);
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Ping(event)) => {
                    let latency = event.result.map(|duration| duration.as_millis()).ok();

                    // update the latency for the peer's session
                    if let Some(state) = self.session_states.read().await.get(&event.peer) {
                        state.latency.store(latency.unwrap_or(0) as usize, Relaxed);
                        continue; // the remaining logic is not needed while a session is active
                    }

                    // if we are the dialer & the session is still connecting, update the latency and try to choose a connection
                    let peer_state = if let Some(state) = peer_states.get_mut(&event.peer)
                        && state.dialer
                    {
                        state
                    } else {
                        continue;
                    };

                    // update the latency for the peer's connections
                    if let Some(state) = peer_state.connections.get_mut(&event.connection) {
                        state.latency = latency;
                    } else {
                        warn!(
                            "received ping for untracked connection: {}",
                            event.connection
                        );
                    }

                    info!("connection states: {:?}", peer_state.connections);

                    if peer_state.latencies_missing() {
                        // only start a session if all connections have latency
                        debug!("{} waiting for all latencies", event.peer);
                        continue;
                    } else if peer_state.relayed_only() && !peer_state.ductr_failed {
                        // only start a session if there is a non-relayed connection
                        // if ductr fails, fall back to relayed
                        debug!("{} is all relayed", event.peer);
                        continue;
                    }

                    // choose the connection with the lowest latency, prioritizing non-relay connections
                    let connection = peer_state.connections.iter().min_by(|a, b| {
                        match (a.1.relayed, b.1.relayed) {
                            (false, true) => std::cmp::Ordering::Less, // prioritize non-relay connections
                            (true, false) => std::cmp::Ordering::Greater, // prioritize non-relay connections
                            _ => a.1.latency.cmp(&b.1.latency), // compare latencies if both have the same relay status
                        }
                    });

                    let Some((id, state)) = connection else {
                        warn!("no connection available for {}", event.peer);
                        continue;
                    };

                    info!("using connection {state:?} [id:{id}] for {}", event.peer);
                    peer_state.selected_connection = true;
                    // close the other connections
                    for other_id in peer_state.connections.keys() {
                        if id != other_id {
                            swarm.close_connection(*other_id);
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(IdentifyEvent::Received {
                    peer_id,
                    info,
                    ..
                })) => {
                    let Some(peer_state) = peer_states.get_mut(&peer_id) else {
                        // the relay server sends identity events which will be caught here
                        continue;
                    };

                    if peer_state.dialed || !peer_state.dialer {
                        continue;
                    } else {
                        peer_state.dialed = true;
                    }

                    info!("Identify event from {peer_id}: {info:?}");

                    // for address in info.listen_addrs {
                    //     // checks for relayed addresses which are not useful
                    //     if address.ends_with(&Protocol::P2p(peer_id).into()) {
                    //         continue;
                    //     }
                    //
                    //     // dials the non-relayed addresses to attempt direct connections
                    //     info!("dialing {} from identify event", address);
                    //     if let Err(error) = swarm.dial(address) {
                    //         error!("Error dialing {}: {}", peer_id, error);
                    //     }
                    // }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(DcutrEvent {
                    remote_peer_id,
                    result,
                })) => {
                    let Some(state) = peer_states.get_mut(&remote_peer_id) else {
                        warn!("ductr event with unknown peer {remote_peer_id}: {result:?}");
                        continue;
                    };

                    let failed = result.is_err();
                    info!("setting ductr_failed to {failed} for {remote_peer_id}");
                    state.ductr_failed = failed;
                }
                event => {
                    trace!("other swarm event: {:?}", event);
                }
            }
        }

        debug!("tearing down old swarm");
        self.callbacks.manager_active(false, false).await;
        // stop the stream handler
        stop_handler.notify_one();
        // reset room state
        if let Some(state) = self.room_state.write().await.take() {
            state.end_call.notify_one();
            state.cancel.cancel();
        }
        // stream handler won't join until all sessions it created have finished
        stream_handler_handle.await??;
        debug!("joined stream handler");
        // join all sessions created in manager
        for handle in handles {
            handle.await??;
        }
        debug!("joined handles in session manager");
        Ok(())
    }

    /// Handles incoming streams for the libp2p swarm. spawns incoming sessions
    pub(crate) async fn incoming_stream_handler(
        &self,
        mut control: Control,
        stop: Arc<Notify>,
    ) -> Result<()> {
        let mut incoming_streams = control.accept(CHAT_PROTOCOL)?;
        let mut handles = Vec::new();

        let result = loop {
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

                    handles.push(self.initialize_session(peer, None, stream, false).await);
                }
                else => break Err(ErrorKind::StreamsEnded.into())
            }
        };

        for handle in handles {
            handle.await??;
        }

        result
    }

    /// Called by the dialer to open a stream and session
    async fn open_session(
        &self,
        peer: PeerId,
        mut control: Control,
        peer_states: &mut HashMap<PeerId, PeerState>,
        handles: &mut Vec<JoinHandle<Result<()>>>,
        relayed: bool,
    ) {
        match control.open_stream(peer, CHAT_PROTOCOL).await {
            Ok(stream) => {
                info!("opened stream with {}, starting new session", peer);
                handles.push(
                    self.initialize_session(peer, Some(control), stream, relayed)
                        .await,
                );
                // the peer state is no longer needed
                peer_states.remove(&peer);
            }
            Err(error) => {
                warn!("OpenStreamError for {peer}: {error}");
            }
        }
    }

    /// Entry point to a session that sets up state and spawns session outer
    pub(crate) async fn initialize_session(
        &self,
        peer: PeerId,
        control: Option<Control>,
        stream: Stream,
        relayed: bool,
    ) -> JoinHandle<Result<()>> {
        let contact_option = self.callbacks.get_contact(peer.to_bytes()).await;
        // sends messages to the session from elsewhere in the program
        let message_channel = channel::<Message>(8);
        // create the state and a clone of it for the session
        let state = Arc::new(SessionState::new(&message_channel.0, relayed));
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
            old_state.stop_session.cancel();
        }

        let contact = if let Some(contact) = contact_option {
            // alert the UI that this session is now connected
            self.callbacks
                .session_status(SessionStatus::Connected { relayed }, peer)
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

            Ok(())
        })
    }

    /// Runs session_inner as many times as needed, performs cleanup if needed
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
                (Ok(true), true) => {
                    info!("session for {} was room only", contact.peer_id);
                    break Ok(true);
                }
                // normal session continue
                (Ok(true), false) => {
                    // the session is not in a call
                    state.in_call.store(false, Relaxed);
                }
                (Err(error), room_only) => {
                    // if an error occurred during a non-room call, it is ended now
                    if state.in_call.swap(false, Relaxed)
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
            _ = state.stop_session.cancelled() => {
                info!("session for {} stopped", contact.nickname);
                Ok(false)
            },
            result = read_message::<Message, _>(transport) => {
                let mut other_ringtone = None;
                let remote_audio_header;
                let room_hash_option;

                info!("received {:?} from {}", result, contact.nickname);

                match result? {
                    Message::Hello { ringtone, audio_header, room_hash } => {
                        remote_audio_header = audio_header;
                        room_hash_option = room_hash;
                        if self.core_state.play_custom_ringtones.load(Relaxed) {
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
                } else if self.core_state.in_call.load(Relaxed) {
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
                    _ = state.stop_session.cancelled() => {
                        info!("session for {} stopped during accept prompt", contact.nickname);
                        if let Some(cancel) = cancel_prompt {
                            cancel.notify_one();
                        }
                        return Ok(false);
                    }
                    accepted = accept_future => {
                        if !accepted? {
                            // reject the call if not accepted
                            write_message(transport, &Message::Reject).await?;
                            return Ok(true);
                        }

                        match self.setup_call(contact.peer_id).await {
                            Ok(mut call_state) => {
                                // respond with hello ack containing audio header
                                call_state.remote_configuration = remote_audio_header;
                                write_message(transport, &Message::HelloAck { audio_header: call_state.local_configuration.clone() }).await?;

                                if is_in_room {
                                    self.room_handshake(transport, control, state, call_state).await?;
                                } else {
                                    // normal call handshake
                                    self.call_handshake(transport, control, &mut message_channel.1, state, call_state).await?;
                                }

                                keep_alive.reset(); // start sending normal keep alive messages
                            }
                            Err(error) => {
                                // if the audio input setup fails, other client will be left hanging
                                write_message(transport, &Message::Goodbye {
                                    reason: Some("audio device error".to_string())
                                }).await?;
                                // still propagate the error
                                return Err(error);
                            }
                        }
                    }
                    result = read_message::<Message, _>(transport) => {
                        // always cancel prompt because there is no chance of the call succeeding now
                        if let Some(cancel) = cancel_prompt {
                            cancel.notify_one();
                        }
                        // propagate errors for handling
                        let message = result?;
                        // log message
                        warn!("received {message:?} from {} while accept call was pending", contact.nickname);
                    }
                }

                Ok(true)
            }
            _ = state.start_call.notified() => {
                // limits session restarts
                state.in_call.store(true, Relaxed);

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
                        _ = state.stop_session.cancelled() => {
                            info!("session for {} stopped while waiting for HelloAck", contact.nickname);
                            return Ok(false);
                        }
                        _ = state.end_call.notified() => {
                            // gracefully end the call & continue the session
                            info!("end call notified while waiting for hello ack");
                            write_message(transport, &Message::Goodbye { reason: None }).await?;
                            break;
                        }
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
                                Message::Goodbye { reason: Some(m) } => {
                                    Some(format!("{} did not accept the call because of {m}", contact.nickname))
                                }
                                Message::Reject | Message::Busy if is_in_room => None,
                                Message::Goodbye { .. } | Message::Reject => {
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
                    }
                }

                Ok(true)
            }
            _ = keep_alive.tick() => {
                debug!("sending keep alive to {}", contact.nickname);
                write_message(transport, &Message::KeepAlive).await?;
                Ok(true)
            },
        }
    }

    /// Gets everything ready for the call
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
        self.core_state.in_call.store(true, Relaxed);
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
        self.core_state.in_call.store(false, Relaxed);
        // hide the overlay
        self.overlay.hide();
        // send a goodbye message on errors
        if let Err(error) = result.as_ref() {
            let message = Message::error_goodbye(error);
            write_message(transport, &message).await?;
        }

        result
    }

    /// Normal call & self-test logic
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

        let mut input_helper = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                codec_config,
                &statistics_state,
                false,
            )
            .await?;

        let mut output_helper = self
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
            self.setup_input_stream(&call_state, input_helper.sender(), end_call.clone())?;

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

        let statistics_handle = spawn(statistics_collector(
            statistics_state,
            self.callbacks.statistics_callback(),
            stop_io.clone(),
        ));

        if let Some(o) = optional {
            let (write, read) = o.audio_transport.split();

            let input_handle = spawn(audio_input(
                input_helper.receiver(),
                ConstSocket::new(write),
                stop_io.clone(),
                upload_bandwidth,
            ));

            let output_handle = spawn(audio_output(
                output_helper.sender(),
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
            loopback(
                input_helper.receiver(),
                output_helper.sender(),
                stop_io,
                end_call,
            )
            .await;
            stop_io.cancel();
        }

        // on ios the audio session must be deactivated
        #[cfg(target_os = "ios")]
        deactivate_audio_session();

        #[cfg(target_family = "wasm")]
        {
            // drop the web input to free resources and stop the input processor
            *self.web_input.lock().await = None;
        }

        debug!("starting call teardown");
        statistics_handle.await?;
        input_helper.join().await?;
        output_helper.join().await?;
        debug!("finished call teardown");

        Ok(())
    }

    /// Controller for normal calls
    pub(crate) async fn call_controller(
        &self,
        transport: &mut Transport<TransportStream>,
        receiver: &mut MReceiver<Message>,
        peer: PeerId,
        end_call: &Arc<Notify>,
    ) -> Result<(Option<String>, bool)> {
        let identity = self.peer_id().await;

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

    /// Manages connection with one room peer
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
                _ = cancel.cancelled() => {
                    // try to say goodbye
                    _ = write_message(transport, &Message::Goodbye { reason: None }).await;
                    break
                }
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
            }
        }

        // sender may already be closed at this point
        _ = sender.send(RoomMessage::Leave(peer_id)).await;
        Ok(())
    }

    /// The controller for rooms
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

        let mut input_helper = self
            .setup_input(
                call_state.local_configuration.sample_rate as f64,
                (true, true, 5_f32), // hard coded room codec options
                &statistics_state,
                true,
            )
            .await?;

        #[cfg(not(target_family = "wasm"))]
        let input_stream =
            self.setup_input_stream(&call_state, input_helper.sender(), end_call.clone())?;

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
            input_helper.receiver(),
            SendingSockets::new(new_sockets.clone()),
            stop_io.clone(),
            statistics_state.upload_bandwidth.clone(),
        ));

        let statistics_handle = spawn(statistics_collector(
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
                            let mut helper = self
                                .setup_output(
                                    state.remote_configuration.sample_rate as f64,
                                    true,
                                    &statistics_state,
                                    true,
                                    end_call.clone(),
                                )
                                .await?;
                            // begin sending
                            let handle = spawn(audio_output(
                                helper.sender(),
                                read,
                                stop_io.clone(),
                                statistics_state.download_bandwidth.clone(),
                                statistics_state.loss.clone(),
                            ));

                            connections.insert(state.peer, RoomConnection {
                                output: helper,
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
        // join input IO task
        input_handle.await??;
        // join output tasks
        for connection in connections.into_values() {
            connection.handle.await??;
            connection.output.join().await?;
        }
        debug!("finished tearing down room processing stack");
        // cleanup room state
        self.room_state.write().await.take();
        // cleanup sessions blocked by room
        end_sessions.cancel();
        // join statistics collector
        statistics_handle.await?;
        // join input tasks
        input_helper.join().await?;
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
            core_state: self.core_state.clone(),
            room_state: Arc::clone(&self.room_state),
            session_states: Arc::clone(&self.session_states),
            start_session: self.start_session.clone(),
            start_screenshare: self.start_screenshare.clone(),
            restart_manager: Arc::clone(&self.restart_manager),
            overlay: self.overlay.clone(),
            #[cfg(target_family = "wasm")]
            web_input: Arc::clone(&self.web_input),
            callbacks: Arc::clone(&self.callbacks),
            phantom: self.phantom,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct CoreState {
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
    pub(crate) identity: Arc<RwLock<Option<Keypair>>>,

    /// Keeps track of whether the user is in a call
    pub(crate) in_call: Arc<AtomicBool>,

    /// used to end an audio test, if there is one
    pub(crate) end_audio_test: Arc<Mutex<Option<Arc<Notify>>>>,

    /// Disables the output stream
    pub(crate) deafened: Arc<AtomicBool>,

    /// Disables the input stream
    pub(crate) muted: Arc<AtomicBool>,

    /// Disables the playback of custom ringtones
    pub(crate) play_custom_ringtones: Arc<AtomicBool>,

    /// Enables sending your custom ringtone
    pub(crate) send_custom_ringtone: Arc<AtomicBool>,

    // TODO use efficiency mode for something again
    pub(crate) efficiency_mode: Arc<AtomicBool>,

    /// set to true at shutdown to break manager loop
    pub(crate) stop_manager: Arc<AtomicBool>,

    /// notifies when a manager starts
    pub(crate) manager_active: Arc<Notify>,

    /// Network configuration for p2p connections
    pub(crate) network_config: NetworkConfig,

    /// Configuration for the screenshare functionality
    #[allow(dead_code)]
    pub(crate) screenshare_config: ScreenshareConfig,

    /// configuration for audio codec, or lack thereof
    pub(crate) codec_config: CodecConfig,
}

/// a state used for session negotiation
#[derive(Debug, Default)]
struct PeerState {
    /// set to true after dialing peer's identity addresses
    dialed: bool,

    /// when true the peer is the dialer
    dialer: bool,

    /// a map of connections and their latencies
    connections: HashMap<ConnectionId, ConnectionState>,

    selected_connection: bool,

    ductr_failed: bool,
}

impl PeerState {
    fn dialer() -> Self {
        Self {
            dialer: true,
            ..Default::default()
        }
    }

    fn non_dialer(endpoint: ConnectedPoint, connection_id: ConnectionId) -> Self {
        Self {
            dialer: false,
            connections: HashMap::from([(connection_id, endpoint.into())]),
            ..Default::default()
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

    /// the underlying connection details
    _endpoint: ConnectedPoint,
}

impl From<ConnectedPoint> for ConnectionState {
    fn from(endpoint: ConnectedPoint) -> Self {
        Self {
            latency: None,
            relayed: endpoint.is_relayed(),
            _endpoint: endpoint,
        }
    }
}
