use crate::BehaviourEvent;
use crate::internal::Result;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::error::ErrorKind;
use crate::internal::helpers::OutputHelper;
use crate::internal::messages::{ProtocolMessage, RoomMessage, StartScreenshare};
use crate::internal::sockets::{
    ConstSocket, SendingSockets, SharedSockets, Transport, TransportStream, audio_input,
    audio_output,
};
use crate::internal::state::{ConnectionState, StatisticsCollectorState};
use crate::internal::state::{CoreState, PeerState};
use crate::internal::utils::{JoinHandle, spawn_task};
#[cfg(target_os = "ios")]
use crate::internal::utils::{configure_audio_session, deactivate_audio_session};
use crate::internal::utils::{
    loopback, read_message, select_best_connection, statistics_collector,
    stream_to_audio_transport, write_message,
};
use crate::internal::{
    CHAT_PROTOCOL, DCUTR_TIMEOUT, EarlyCallState, HELLO_TIMEOUT, KEEP_ALIVE, RoomState,
    SESSION_MAX_FRAME_LENGTH, SessionState,
};
use crate::overlay::CONNECTED;
use crate::overlay::overlay::Overlay;
use crate::types::{
    CallState, ChatMessage, CodecConfig, Contact, NetworkConfig, ScreenshareConfig, SessionStatus,
};
use chrono::Local;
use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
use libp2p::{PeerId, Stream, dcutr::Event as DcutrEvent, identify::Event as IdentifyEvent};
use libp2p_stream::Control;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
#[cfg(target_family = "wasm")]
use telepathy_audio::WebAudioWrapper;
use telepathy_audio::devices::AudioHost;
use tokio::select;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::{Notify, RwLock};
#[cfg(not(target_family = "wasm"))]
use tokio::time::{Instant, Interval, interval, sleep_until, timeout};
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span, debug, error, field, info, instrument, trace, warn};
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::std::Instant;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::{Interval, interval, sleep_until, timeout};

pub(crate) struct TelepathyCore<C, S>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
{
    /// The audio host
    pub(crate) host: AudioHost,

    /// Core state for telepathy
    pub(crate) core_state: CoreState,

    /// Tracks state for the current room
    pub(crate) room_state: Arc<RwLock<Option<RoomState>>>,

    /// Keeps track of and controls the sessions
    pub(crate) session_states: Arc<RwLock<HashMap<PeerId, Arc<SessionState>>>>,

    /// Signals the session manager to start a new session
    pub(crate) start_session: Option<Sender<PeerId>>,

    /// Signals the session manager to start a screenshare
    pub(crate) start_screenshare: Option<Sender<StartScreenshare>>,

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
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
{
    pub(crate) fn new(
        host: AudioHost,
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
    #[instrument(name = "manager.spawn", skip_all)]
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
        Some(spawn_task(
            async move {
                let mut retries = 0;
                // break when stop_manager==true
                while !manager_clone.core_state.stop_manager.load(Relaxed) {
                    let last_launch = Instant::now();
                    // run the session manager to completion
                    let result = manager_clone
                        .session_manager(&mut receive_session, &mut receive_screenshare)
                        .await;

                    if let Err(error) = result {
                        Span::current().record("restart_count", retries);
                        error!(
                            event = "session_manager_failed",
                            retries,
                            error = %error
                        );
                        retries += 1;
                        let next_launch = last_launch + Duration::from_millis((retries ^ 2) * 500);
                        if next_launch > Instant::now() {
                            // wait for the next launch or restart
                            select! {
                                _ = manager_clone.restart_manager.notified() => (),
                                _ = sleep_until(next_launch) => (),
                            };
                        }
                    } else {
                        Span::current().record("restart_count", retries);
                        info!(event = "session_manager_exited");
                        retries = 0;
                    }
                }
            }
            .in_current_span(),
        ))
    }

    /// Ends all sessions & restores session_states to default
    pub(crate) async fn reset_sessions(&self) {
        for (_, session) in self.session_states.write().await.drain() {
            session.teardown().await;
        }
    }

    /// Builds the libp2p swarm, handles session start requests, screenshare messages, and libp2p events.
    /// spawns outgoing sessions & screenshare threads
    #[instrument(
        name = "manager.run",
        skip_all,
        fields(manager.id = %Uuid::new_v4(), restart_count = field::Empty)
    )]
    async fn session_manager(
        &self,
        start: &mut Receiver<PeerId>,
        screenshare: &mut Receiver<StartScreenshare>,
    ) -> Result<()> {
        let setup_started = Instant::now();
        // build the swarm & connect to relay
        let (mut swarm, relay_address) = self.setup_swarm().await?;
        info!(
            event = "manager_swarm_setup",
            elapsed_ms = setup_started.elapsed().as_millis() as u64
        );
        // contains the state needed for negotiating sessions
        let mut peer_states: HashMap<PeerId, PeerState> = HashMap::new();
        // handles to threads spawned by the session manager
        let mut handles: Vec<SessionTask> = Vec::new();
        // preload public identity
        let public_identity = self.peer_id().await;
        // preload the relay identity
        let relay_identity = *self.core_state.network_config.relay_id.read().await;

        // handle incoming streams
        let control = swarm.behaviour().stream.new_control();
        let stop_handler = Arc::new(Notify::new());
        let stop_handler_clone = stop_handler.clone();
        let self_clone = self.clone();
        let stream_handler_handle = spawn_task(
            async move {
                self_clone
                    .incoming_stream_handler(control, stop_handler_clone)
                    .await
            }
            .in_current_span(),
        );

        // during session initialization, the dialer rechecks state on this interval
        let mut dialer_control_interval = interval(Duration::from_secs(1));

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
                        .map(|(_, c)| (*p, s.selected_connection, c.clone()))
                })
                .collect();

            for (peer, selected, details) in single_connections {
                if selected {
                    debug!(event = "session_opening", peer.id = %peer, ?details);
                    // open a session control stream and start the session controller
                    self.open_session(
                        peer,
                        swarm.behaviour().stream.new_control(),
                        &mut peer_states,
                        &mut handles,
                        details,
                    )
                    .await;
                } else if self.session_states.read().await.get(&peer).is_some() {
                    debug!(event = "listener_connection_selected", peer.id = %peer, ?details);
                    // only the non-dialing peer will reach this branch
                    // this peer state is no longer needed
                    peer_states.remove(&peer);
                    // update the connection details in the frontend
                    self.callbacks
                        .session_status(SessionStatus::from(details), peer)
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
                        debug!(event = "dial_ignored_self", peer.id = %peer_id);
                        continue;
                    } else if swarm.is_connected(&peer_id) {
                        // TODO is it possible that this check can result in invalid states where two peers cannot get into a session?
                        // prevents dialing a peer who is already connected
                        warn!(
                            event = "edge_case",
                            case = "dial_to_connected_peer",
                            peer.id = %peer_id
                        );
                        continue;
                    }

                    debug!(event = "dial_initial", peer.id = %peer_id);

                    // dial the peer through the relay
                    let status = if let Err(error) = swarm.dial(relay_address.clone().with(Protocol::P2p(peer_id))) {
                        error!(event = "dial_error", peer.id = %peer_id, error = %error);
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
                    info!(event = "screenshare_starting", ?message);

                    #[cfg(not(target_family = "wasm"))]
                    {
                        // when the header is some, a control is required to open the stream
                        let control_option = message.header.is_some()
                            .then(|| swarm.behaviour().stream.new_control());
                        let self_clone = self.clone();
                        spawn_task(async move {
                            let result = self_clone.start_screenshare(message, control_option).await;
                            if let Err(error) = result {
                                error!(event = "screenshare_start_failed", error = ?error);
                            }
                        }.in_current_span());
                    }

                    continue;
                }
                _ = dialer_control_interval.tick(), if dialer_control_needed(&peer_states) => {
                    for (peer, peer_state) in peer_states.iter_mut() {
                        if !peer_state.dialer || peer_state.selected_connection {
                            continue;
                        }

                        if peer_state.created.elapsed() > DCUTR_TIMEOUT {
                            // give up on direct connection upgrade
                            // fall through to connection selection
                            debug!(
                                event = "dcutr_timeout_reached",
                                peer.id = %peer,
                                dcutr.elapsed_ms = peer_state.created.elapsed().as_millis() as u64,
                                dcutr.timeout_ms = DCUTR_TIMEOUT.as_millis() as u64
                            );
                        } else if peer_state.latencies_missing() {
                            // only start a session if all connections have latency
                            debug!(event = "connection_selection_waiting_latencies", peer.id = %peer);
                            continue;
                        } else if peer_state.relayed_only() {
                            // only start a session if there is a non-relayed connection
                            // if dcutr times out, fallback
                            debug!(event = "connection_selection_all_relayed", peer.id = %peer);
                            continue;
                        }

                        // select the best connection
                        let Some((id, state)) = select_best_connection(&peer_state.connections) else {
                            warn!(event = "connection_selection_none_available", peer.id = %peer);
                            continue;
                        };
                        info!(
                            event = "connection_selected",
                            peer.id = %peer,
                            connection.id = %id,
                            ?state
                        );
                        peer_state.selected_connection = true;
                        // close the other connections
                        for other_id in peer_state.connections.keys() {
                            if &id != other_id {
                                swarm.close_connection(*other_id);
                            }
                        }
                    }

                    continue;
                }
                else => {
                    warn!(event = "edge_case", case = "session_manager_else_branch");
                    break;
                },
            };

            match event {
                SwarmEvent::ConnectionEstablished {
                    peer_id,
                    endpoint,
                    connection_id,
                    established_in,
                    num_established,
                    ..
                } if peer_id != relay_identity => {
                    debug!(
                        event = "connection_established",
                        peer.id = %peer_id,
                        connection.id = %connection_id,
                        ?endpoint,
                        established_in_ms = established_in.as_millis() as u64,
                        num_established
                    );

                    if self.session_states.read().await.contains_key(&peer_id) {
                        // ignore connections with peers who have a session
                        // in normal operation, extra connections may be created
                        // when the session is initialized
                        warn!(event = "connection_ignored_existing_session", peer.id = %peer_id);
                        continue;
                    }

                    let contact = self.callbacks.get_contact(peer_id.to_bytes()).await;
                    let listener = endpoint.is_listener();

                    if contact.is_none() && !self.is_in_room(&peer_id).await {
                        warn!(event = "unknown_peer_connected", peer.id = %peer_id);
                        if swarm.disconnect_peer_id(peer_id).is_err() {
                            warn!(event = "unknown_peer_disconnect_race", peer.id = %peer_id);
                        }
                    } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        // if two clients dial each other at the same time, one switches to non-dialer
                        // non p2p connections are ignored to prevent accidental switches
                        if listener
                            && peer_state.dialer
                            && endpoint
                                .get_remote_address()
                                .ends_with(&Protocol::P2p(peer_id).into())
                        {
                            debug!(event = "dialer_received_listener_connection", peer.id = %peer_id);
                            if peer_id < public_identity {
                                info!(event = "dialer_switched_to_listener", peer.id = %peer_id);
                                peer_state.dialer = false;
                            }
                        }

                        // track the new connection
                        peer_state
                            .connections
                            .insert(connection_id, endpoint.into());
                    } else if listener {
                        info!(event = "listener_connection_established_first", peer.id = %peer_id);
                        // insert initial non-dialer state
                        peer_states.insert(peer_id, PeerState::non_dialer(endpoint, connection_id));
                        // alert the frontend that the session is connecting
                        self.callbacks
                            .session_status(SessionStatus::Connecting, peer_id)
                            .await;
                    } else {
                        warn!(
                            event = "edge_case",
                            case = "simultaneous_dial_unreachable",
                            peer.id = %peer_id
                        );
                    }
                }
                SwarmEvent::OutgoingConnectionError {
                    peer_id: Some(peer_id),
                    error,
                    connection_id,
                } => {
                    let peer_state_option = peer_states.remove(&peer_id);
                    if let Some(mut peer_state) = peer_state_option {
                        // untrack the failed connection
                        peer_state.connections.remove(&connection_id);
                        if peer_state.connections.is_empty() {
                            // session initialization has failed, clean up state
                            warn!(
                                event = "outgoing_connections_failed_all",
                                peer.id = %peer_id,
                                error = %error
                            );
                            self.callbacks
                                .session_status(SessionStatus::Inactive, peer_id)
                                .await;
                        } else {
                            // session initialization is still possible
                            info!(
                                event = "outgoing_connection_failed_partial",
                                peer.id = %peer_id,
                                error = %error,
                                ?peer_state
                            );
                            peer_states.insert(peer_id, peer_state);
                        }
                    } else if self.session_states.read().await.contains_key(&peer_id) {
                        // this case occurs when a connection was slow to close for the non-dialer
                        info!(
                            event = "outgoing_connection_failed_existing_session",
                            peer.id = %peer_id,
                            error = %error
                        );
                    } else {
                        warn!(
                            event = "outgoing_connection_failed_no_state",
                            peer.id = %peer_id,
                            error = %error
                        );
                    }
                }
                SwarmEvent::OutgoingConnectionError {
                    peer_id: None,
                    error,
                    ..
                } => {
                    warn!(event = "outgoing_connection_error_without_peer", error = %error);
                }
                SwarmEvent::ConnectionClosed {
                    peer_id,
                    cause,
                    connection_id,
                    ..
                } => {
                    let remove_state = if !swarm.is_connected(&peer_id) {
                        // if there is no connection to the peer, the session initialization failed
                        debug!(
                            event = "session_initialization_failed",
                            peer.id = %peer_id,
                            ?cause
                        );
                        self.callbacks
                            .session_status(SessionStatus::Inactive, peer_id)
                            .await;
                        true
                    } else if let Some(peer_state) = peer_states.get_mut(&peer_id) {
                        // untrack the connection
                        debug!(
                            event = "connection_untracked",
                            peer.id = %peer_id,
                            connection.id = %connection_id
                        );
                        peer_state.connections.remove(&connection_id);
                        peer_state.connections.is_empty()
                    } else {
                        warn!(
                            event = "edge_case",
                            case = "unexpected_connection_closed",
                            connection.id = %connection_id,
                            ?cause
                        );
                        continue;
                    };

                    if remove_state {
                        info!(event = "peer_state_removed", peer.id = %peer_id);
                        peer_states.remove(&peer_id);
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Ping(event))
                    if event.peer != relay_identity =>
                {
                    let Ok(latency) = event.result else {
                        warn!(event = "ping_result_unexpected", ?event);
                        continue;
                    };

                    // update the latency for the peer's session
                    if let Some(state) = self.session_states.read().await.get(&event.peer) {
                        let latency_ms = latency.as_millis() as usize;
                        debug!(
                            event = "ping_latency_session_updated",
                            peer.id = %event.peer,
                            latency_ms
                        );
                        state.latency.store(latency_ms, Relaxed);
                        continue; // the remaining logic is not needed while a session is active
                    }

                    // if the session is still connecting, update the latency and try to choose a connection
                    let Some(peer_state) = peer_states.get_mut(&event.peer) else {
                        info!(event = "ping_without_state", peer.id = %event.peer, ?event);
                        continue;
                    };

                    if !peer_state.dialer {
                        continue; // the dialer chooses the connection
                    } else if let Some(state) = peer_state.connections.get_mut(&event.connection) {
                        // update the latency for the peer's connections
                        state.latency = Some(latency);
                        info!(
                            event = "connection_latency_updated",
                            peer.id = %event.peer,
                            connection.id = %event.connection,
                            latency_ms = latency.as_millis() as u64
                        );
                    } else {
                        warn!(
                            event = "ping_untracked_connection",
                            peer.id = %event.peer,
                            connection.id = %event.connection
                        );
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(IdentifyEvent::Received {
                    peer_id,
                    info,
                    ..
                })) if peer_id != relay_identity => {
                    let Some(peer_state) = peer_states.get_mut(&peer_id) else {
                        // peers with sessions may land here
                        debug!(event = "identify_without_peer_state", peer.id = %peer_id);
                        continue;
                    };
                    // skip if the peer is not the dialer or has already dialed
                    if !peer_state.dialer || peer_state.dialed {
                        debug!(event = "identify_skipped", peer.id = %peer_id);
                        continue;
                    }
                    debug!(event = "identify_received_first", peer.id = %peer_id, ?info);
                    peer_state.dialed = true;
                    // in order to find the best connection between peers (i.e. LAN or localhost)
                    // it is important to dial every non-relayed addresses they discover
                    for mut address in info.listen_addrs {
                        // ignore relayed addresses here
                        if address.ends_with(&Protocol::P2p(peer_id).into()) {
                            continue;
                        }
                        // add the peer ID
                        address.push(Protocol::P2p(peer_id));
                        // dials the non-relayed addresses to attempt direct connections
                        debug!(
                            event = "identify_dialing_address",
                            peer.id = %peer_id,
                            address = %address
                        );
                        if let Err(error) = swarm.dial(address) {
                            error!(event = "identify_dial_error", peer.id = %peer_id, error = %error);
                        }
                    }
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(IdentifyEvent::Error {
                    peer_id,
                    error,
                    ..
                })) => {
                    warn!(event = "identify_error", peer.id = %peer_id, error = %error);
                }
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(DcutrEvent {
                    remote_peer_id,
                    result: Err(error),
                })) => {
                    let has_peer_state = peer_states.get_mut(&remote_peer_id).is_some();
                    let has_session_state = self
                        .session_states
                        .read()
                        .await
                        .get(&remote_peer_id)
                        .is_some();
                    warn!(
                        event = "dcutr_failed",
                        peer.id = %remote_peer_id,
                        has_peer_state,
                        has_session_state,
                        ?error
                    );
                }
                SwarmEvent::Behaviour(BehaviourEvent::Dcutr(DcutrEvent {
                    remote_peer_id,
                    result: Ok(connection),
                })) => {
                    debug!(
                        event = "dcutr_succeeded",
                        peer.id = %remote_peer_id,
                        ?connection
                    );
                }

                event => {
                    trace!(event = "swarm_event_other", ?event);
                }
            }
        }

        debug!(event = "manager_teardown_start");
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
        debug!(event = "manager_stream_handler_joined");
        // join all sessions created in manager
        for handle in handles {
            handle.join().await?;
        }
        debug!(event = "manager_session_handles_joined");
        Ok(())
    }

    /// Handles incoming streams for the libp2p swarm. spawns incoming sessions
    #[instrument(name = "streams.accept_loop", skip_all)]
    async fn incoming_stream_handler(&self, mut control: Control, stop: Arc<Notify>) -> Result<()> {
        let mut incoming_streams = control.accept(CHAT_PROTOCOL)?;
        let mut handles: Vec<SessionTask> = Vec::new();

        let result = loop {
            select! {
                _ = stop.notified() => break Ok(()),
                Some((peer, stream)) = incoming_streams.next() => {
                    let state_option = self.session_states.read().await.get(&peer).cloned();

                    if let Some(state) = state_option {
                        if state.wants_stream.load(Relaxed) {
                            info!(event = "substream_accepted", peer.id = %peer);

                            if let Err(error) = state.stream_sender.send(stream).await {
                                error!(
                                    event = "substream_forward_failed",
                                    peer.id = %peer,
                                    error = %error
                                );
                            }

                            continue;
                        } else {
                            warn!(
                                event = "substream_unexpected_starting_session",
                                peer.id = %peer
                            );
                        }
                    } else {
                        info!(event = "stream_accepted_new_session", peer.id = %peer);
                    }

                    handles.push(self.initialize_session(peer, None, stream, None).await);
                }
                else => {
                    error!(event = "incoming_streams_closed_unexpectedly");
                    break Err(ErrorKind::StreamsEnded.into())
                }
            }
        };

        for handle in handles {
            handle.join().await?;
        }

        result
    }

    /// Called by the dialer to open a stream and session
    #[instrument(
        name = "session.open",
        skip_all,
        fields(peer.id = %peer, relayed = state.relayed)
    )]
    async fn open_session(
        &self,
        peer: PeerId,
        mut control: Control,
        peer_states: &mut HashMap<PeerId, PeerState>,
        handles: &mut Vec<SessionTask>,
        state: ConnectionState,
    ) {
        match control.open_stream(peer, CHAT_PROTOCOL).await {
            Ok(stream) => {
                info!(event = "session_stream_opened", peer.id = %peer);
                handles.push(
                    self.initialize_session(peer, Some(control), stream, Some(state))
                        .await,
                );
                // the peer state is no longer needed
                peer_states.remove(&peer);
            }
            Err(error) => {
                let retries = state.retries.fetch_add(1, Relaxed);
                if retries > 3 {
                    warn!(event = "session_open_give_up", peer.id = %peer, retries);
                    peer_states.remove(&peer);
                    self.callbacks
                        .session_status(SessionStatus::Inactive, peer)
                        .await;
                } else {
                    warn!(
                        event = "session_open_stream_error",
                        peer.id = %peer,
                        retries,
                        error = %error
                    );
                }
            }
        }
    }

    /// Entry point to a session that sets up state and spawns session outer
    #[instrument(name = "session.init", skip_all, fields(peer.id = %peer, session.id = field::Empty))]
    async fn initialize_session(
        &self,
        peer: PeerId,
        control: Option<Control>,
        stream: Stream,
        connection: Option<ConnectionState>,
    ) -> SessionTask {
        let contact_option = self.callbacks.get_contact(peer.to_bytes()).await;
        // sends messages to the session from elsewhere in the program
        let message_channel = channel::<ProtocolMessage>(8);
        // create the state and a clone of it for the session
        let state = Arc::new(SessionState::new(&message_channel.0));
        Span::current().record("session.id", state.id.to_string());
        // insert the new state
        let old_state_option = self
            .session_states
            .write()
            .await
            .insert(peer, state.clone());

        if let Some(old_state) = old_state_option {
            warn!(event = "session_replaced_existing_state", peer.id = %peer);
            old_state.teardown().await;
        }

        let contact = if let Some(contact) = contact_option {
            // if we have details now, let the frontend know
            if let Some(details) = connection {
                self.callbacks
                    .session_status(SessionStatus::from(details), peer)
                    .await;
            }
            contact
        } else {
            // there may be no contact for members of a group
            debug!(event = "group_contact_created", peer.id = %peer);
            Contact {
                id: Uuid::new_v4().to_string(),
                nickname: String::from("GroupContact"),
                peer_id: peer,
                is_room_only: true,
            }
        };

        let self_clone = self.clone();
        SessionTask(spawn_task(
            async move {
                self_clone
                    .session_outer(peer, control, stream, state, contact, message_channel)
                    .await;

                Ok(())
            }
            .in_current_span(),
        ))
    }

    /// Runs session_inner as many times as needed, performs cleanup if needed
    #[instrument(
        name = "session.run",
        skip_all,
        fields(
            peer.id = %peer,
            session.id = %state.id,
            peer.nickname = %contact.nickname,
            session.role = field::Empty
        )
    )]
    async fn session_outer(
        &self,
        peer: PeerId,
        mut control: Option<Control>,
        stream: Stream,
        state: Arc<SessionState>,
        contact: Contact,
        mut message_channel: (Sender<ProtocolMessage>, Receiver<ProtocolMessage>),
    ) {
        let session_role = if control.is_some() {
            "dialer"
        } else {
            "listener"
        };
        Span::current().record("session.role", session_role);
        // controls keep alive messages
        let mut keep_alive = interval(KEEP_ALIVE);
        // the length delimited transport used for the session
        let mut transport = LengthDelimitedCodec::builder()
            .max_frame_length(SESSION_MAX_FRAME_LENGTH)
            .length_field_type::<u64>()
            .new_framed(stream.compat());

        // the dialer for room sessions always starts a call
        if self.is_in_room(&peer).await && control.is_some() {
            state.start_call.notify_one();
        }

        loop {
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
                (Ok(false), _) => break,
                // room only sessions never continue
                (Ok(true), true) => {
                    info!(event = "session_room_only_completed", peer.id = %contact.peer_id);
                    break;
                }
                // normal session continue
                (Ok(true), false) => {
                    // the session is not in a call
                    debug!(event = "session_continuing_after_call");
                    state.in_call.store(false, Relaxed);
                }
                (Err(error), room_only) => {
                    // if an error occurred during a non-room call, it is ended now
                    if state.in_call.swap(false, Relaxed)
                        && !room_only
                        && !self.is_in_room(&contact.peer_id).await
                    {
                        warn!(event = "session_error_while_call_active", ?error);
                        self.callbacks
                            .call_state(CallState::CallEnded(error.to_string(), false))
                            .await;
                    }

                    if room_only || error.is_session_critical() {
                        // session cannot recover from these errors
                        error!(event = "session_error_critical", ?error);
                        break;
                    } else {
                        warn!(event = "session_error_recoverable", ?error);
                    }
                }
            }
        }

        // if the state exists and has the same id, clean it up
        // if a new session state already exists with a new ID, we don't want to clean it up
        let mut states = self.session_states.write().await;
        if states.get(&peer).map(|s| s.id == state.id).unwrap_or(false) {
            states.remove(&peer);
        }
        drop(states);

        // avoid sending session statuses for dummy contacts
        if !contact.is_room_only {
            self.callbacks
                .session_status(SessionStatus::Inactive, peer)
                .await;
        }

        info!(event = "session_cleaned_up", session.id = %state.id);
    }

    /// The inner logic of a session that may execute many times
    /// Returns true if the session should continue
    #[instrument(
        name = "session.iter",
        skip_all,
        fields(peer.id = %contact.peer_id, room.hash = field::Empty)
    )]
    async fn session_inner(
        &self,
        contact: &Contact,
        control: Option<&mut Control>,
        transport: &mut Transport<TransportStream>,
        state: &Arc<SessionState>,
        message_channel: &mut (Sender<ProtocolMessage>, Receiver<ProtocolMessage>),
        keep_alive: &mut Interval,
    ) -> Result<bool> {
        let room_hash = self.room_hash().await;
        Span::current().record("room.hash", field::debug(room_hash));
        info!(event = "session_waiting_for_event");

        select! {
            _ = state.stop_session.cancelled() => {
                info!(event = "session_stopped");
                Ok(false)
            },
            result = read_message(transport) => {
                let mut other_ringtone = None;
                let remote_audio_header;
                let room_hash_option;

                info!(event = "session_message_received", ?result);

                match result? {
                    ProtocolMessage::Hello { ringtone, audio_header, room_hash } => {
                        if !audio_header.is_valid() {
                            warn!(event = "invalid_audio_header_rejected");
                            write_message(transport, &ProtocolMessage::Reject).await?;
                            return Ok(false);
                        }

                        remote_audio_header = audio_header;
                        room_hash_option = room_hash;
                        if self.core_state.play_custom_ringtones.load(Relaxed) {
                            other_ringtone = ringtone;
                        }
                    },
                    ProtocolMessage::KeepAlive => return Ok(true),
                    message => {
                        warn!(event = "session_message_unexpected", ?message);
                        return Ok(true);
                    }
                }

                let is_in_room = self.is_in_room(&contact.peer_id).await;
                let mut cancel_prompt = None;
                let mut accept_handle = None;

                if is_in_room && room_hash_option == room_hash {
                    // automatically accept calls from member of current room
                } else if room_hash_option.is_some() {
                    // the call is part of a room, but the client is not in the room
                    info!(event = "room_call_rejected_not_in_room");
                    write_message(transport, &ProtocolMessage::Reject).await?;
                    return Ok(true);
                } else if self.is_call_active().await {
                    // do not accept another call if already active
                    info!(event = "call_busy_sent_call_already_active");
                    write_message(transport, &ProtocolMessage::Busy).await?;
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
                        info!(event = "session_stopped_during_accept_prompt");
                        if let Some(cancel) = cancel_prompt {
                            cancel.notify_one();
                        }
                        return Ok(false);
                    }
                    accepted = accept_future => {
                        if !accepted? {
                            // reject the call if not accepted
                            write_message(transport, &ProtocolMessage::Reject).await?;
                            return Ok(true);
                        }

                        match self.setup_call(contact.peer_id).await {
                            Ok(mut call_state) => {
                                // respond with hello ack containing audio header
                                call_state.remote_configuration = remote_audio_header;
                                write_message(transport, &ProtocolMessage::HelloAck { audio_header: call_state.local_configuration.clone() }).await?;

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
                                error!(event = "setup_call_failed", ?error);
                                write_message(transport, &ProtocolMessage::Goodbye {
                                    reason: Some("audio device error".to_string())
                                }).await?;
                                // still propagate the error
                                return Err(error);
                            }
                        }
                    }
                    result = read_message(transport) => {
                        // always cancel prompt because there is no chance of the call succeeding now
                        if let Some(cancel) = cancel_prompt {
                            cancel.notify_one();
                        }
                        // propagate errors for handling
                        let message = result?;
                        // log message
                        warn!(event = "accept_prompt_interrupted_by_message", ?message);
                    }
                }

                Ok(true)
            }
            _ = state.start_call.notified() => {
                // limits session restarts
                state.in_call.store(true, Relaxed);

                let is_in_room = room_hash.is_some();
                // load custom ringtone if enabled
                let other_ringtone = self.load_ringtone().await;
                // initialize call state
                let mut call_state = self.setup_call(contact.peer_id).await?;
                // when custom ringtone is used wait longer for a response to account for extra data being sent in Hello
                let hello_timeout = HELLO_TIMEOUT + if other_ringtone.is_some() { Duration::from_secs(10) } else { Default::default() };
                // queries the other client for a call
                write_message(transport, &ProtocolMessage::Hello { ringtone: other_ringtone, audio_header: call_state.local_configuration.clone(), room_hash }).await?;

                loop {
                    select! {
                        _ = state.stop_session.cancelled() => {
                            info!(event = "session_stopped_waiting_hello_ack");
                            return Ok(false);
                        }
                        _ = state.end_call.notified() => {
                            // gracefully end the call & continue the session
                            info!(event = "end_call_notified_waiting_hello_ack");
                            write_message(transport, &ProtocolMessage::Goodbye { reason: None }).await?;
                            break;
                        }
                        result = timeout(hello_timeout, read_message(transport)) => {
                            if result.is_err() {
                                warn!(
                                    event = "hello_ack_timeout",
                                    hello_timeout_ms = hello_timeout.as_millis() as u64,
                                    peer.id = %contact.peer_id
                                );
                            }
                            // handles a variety of outcomes in response to Hello
                            let message_option = match result?? {
                                ProtocolMessage::HelloAck { audio_header } => {
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
                                ProtocolMessage::Goodbye { reason: Some(m) } => {
                                    Some(format!("{} did not accept the call because of {m}", contact.nickname))
                                }
                                ProtocolMessage::Reject | ProtocolMessage::Busy if is_in_room => {
                                    info!(event = "room_peer_rejected_or_busy_ignored");
                                    None
                                },
                                ProtocolMessage::Goodbye { .. } | ProtocolMessage::Reject => {
                                    info!(event = "call_not_accepted");
                                    Some(format!("{} did not accept the call", contact.nickname))
                                },
                                ProtocolMessage::Busy => {
                                    info!(event = "call_peer_busy");
                                    Some(format!("{} is busy", contact.nickname))
                                },
                                // keep alive messages are sometimes received here
                                ProtocolMessage::KeepAlive => continue,
                                message => {
                                    // the front end needs to know that the call ended here
                                    warn!(event = "hello_ack_flow_unexpected_message", ?message);
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
                debug!(event = "session_keep_alive_sent");
                write_message(transport, &ProtocolMessage::KeepAlive).await?;
                Ok(true)
            },
        }
    }

    /// Gets everything ready for the call
    #[instrument(
        name = "call.handshake",
        skip_all,
        fields(peer.id = %call_state.peer, call.kind = "direct")
    )]
    async fn call_handshake(
        &self,
        transport: &mut Transport<TransportStream>,
        control: Option<&mut Control>,
        message_receiver: &mut Receiver<ProtocolMessage>,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
        let stream = state.open_stream(control, &call_state).await?;
        // stop_io must always cancel, even when the call fails
        let stop_io = CancellationToken::new();
        // change the app call state
        self.core_state.in_call.store(true, Relaxed);
        // show the overlay
        self.overlay.show();

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

        info!(event = "call_handshake_ended");
        // ensure that all background i/o threads are stopped
        stop_io.cancel();
        // the call has ended
        self.core_state.in_call.store(false, Relaxed);
        // hide the overlay
        self.overlay.hide();
        // send a goodbye message on errors
        if let Err(error) = result.as_ref() {
            warn!(event = "call_handshake_sending_error_goodbye", ?error);
            let message = ProtocolMessage::error_goodbye(error);
            write_message(transport, &message).await?;
        }

        result
    }

    /// Normal call & self-test logic
    #[instrument(
        name = "call.run",
        skip_all,
        fields(
            peer.id = %call_state.peer,
            codec.enabled = call_state.codec_config().0,
            sample_rate = call_state.remote_configuration.sample_rate
        )
    )]
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

        // Setup input (stream is managed internally)
        let mut input_helper = self
            .setup_input(codec_config, &statistics_state, end_call)
            .await?;

        // Setup output (stream is managed internally)
        let mut output_helper = self
            .setup_output(
                call_state.remote_configuration.sample_rate as f64,
                codec_config.0,
                &statistics_state,
                end_call.clone(),
            )
            .await?;

        let statistics_handle = spawn_task(statistics_collector(
            statistics_state,
            self.callbacks.statistics_callback(),
            stop_io.clone(),
            self.core_state.efficiency_mode.load(Relaxed),
            self.core_state.statistics_paused.clone(),
        ));

        if let Some(o) = optional {
            let (write, read) = o.audio_transport.split();

            let input_handle = spawn_task(audio_input(
                input_helper.receiver(),
                ConstSocket::new(write),
                stop_io.clone(),
                upload_bandwidth,
            ));

            let output_handle = spawn_task(audio_output(
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

            info!(event = "call_controller_starting");

            let message_option = match controller_future.await {
                Ok((message, notify)) if notify => {
                    info!(event = "call_controller_result", notify, ?message);
                    Some(message.unwrap_or_default())
                }
                Err(error) => {
                    error!(event = "call_controller_error", error = %error);
                    Some(error.to_string())
                }
                _ => None,
            };

            if let Some(message) = message_option {
                self.callbacks
                    .call_state(CallState::CallEnded(message, true))
                    .await;
            }

            info!(event = "call_controller_done_notifying_stop_io");
            stop_io.cancel();

            match input_handle.await {
                Ok(Ok(())) => info!(event = "audio_input_joined"),
                Ok(Err(error)) => {
                    error!(event = "audio_input_failed", error = %error);
                }
                Err(error) => {
                    error!(event = "audio_input_join_failed", error = %error);
                }
            }

            match output_handle.await {
                Ok(Ok(())) => info!(event = "audio_output_joined"),
                Ok(Err(error)) => {
                    error!(event = "audio_output_failed", error = %error);
                }
                Err(error) => {
                    error!(event = "audio_output_join_failed", error = %error);
                }
            }

            info!(event = "call_controller_returned");
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

        debug!(event = "call_teardown_start");
        // on ios the audio session must be deactivated
        #[cfg(target_os = "ios")]
        deactivate_audio_session();
        // cleanup web input on WASM
        #[cfg(target_family = "wasm")]
        {
            *self.web_input.lock().await = None;
        }
        // join background tasks
        statistics_handle.await?;
        // dropping input and output handles cleans up resources
        debug!(event = "call_teardown_done");
        Ok(())
    }

    /// Controller for normal calls
    #[instrument(name = "call.controller", skip_all)]
    async fn call_controller(
        &self,
        transport: &mut Transport<TransportStream>,
        receiver: &mut Receiver<ProtocolMessage>,
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
                    let message: ProtocolMessage = result?;

                    match message {
                        ProtocolMessage::Goodbye { reason } => {
                            debug!(event = "call_goodbye_received", ?reason);
                            break Ok((reason, true));
                        },
                        ProtocolMessage::Chat { text, attachments } => {
                            self.callbacks.message_received(ChatMessage {
                                text,
                                receiver: identity,
                                timestamp: Local::now(),
                                attachments,
                            }).await;
                        }
                        ProtocolMessage::ScreenshareHeader { .. } => {
                            info!(event = "screenshare_header_received", ?message);
                            self.send_start_screenshare(peer, Some(message)).await;
                        }
                        _ => error!(event = "call_controller_unexpected_message", ?message),
                    }
                },
                // sends messages to the callee
                result = receiver.recv() => {
                    if let Some(message) = result {
                        write_message(transport, &message).await?;
                    } else {
                        // if the channel closes, the call has ended
                        info!(event = "call_message_channel_closed");
                        break Ok((None, true));
                    }
                },
                // ends the call
                _ = end_call.notified() => {
                    write_message(transport, &ProtocolMessage::Goodbye { reason: None }).await?;
                    break Ok((None, false));
                },
            }
        }
    }

    /// Manages connection with one room peer
    #[instrument(
        name = "room.handshake",
        skip_all,
        fields(peer.id = %call_state.peer, call.kind = "room")
    )]
    async fn room_handshake(
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
                    info!(event = "room_cancelled_sending_goodbye", peer.id = %peer_id);
                    _ = write_message(transport, &ProtocolMessage::Goodbye { reason: None }).await;
                    break
                }
                result = read_message(transport) => {
                    match result {
                        Ok(ProtocolMessage::Goodbye { .. }) => {
                            info!(event = "room_goodbye_received", peer.id = %peer_id);
                            break;
                        }
                        Ok(ProtocolMessage::Chat { .. }) => {
                            // TODO handle chat messages
                        }
                        Err(error) => {
                            warn!(event = "room_transport_error", peer.id = %peer_id, ?error);
                            break;
                        }
                        Ok(other) => {
                            warn!(event = "room_handshake_unexpected_message", peer.id = %peer_id, ?other);
                        }
                    }
                }
            }
        }

        // sender may already be closed at this point
        _ = sender.send(RoomMessage::Leave(peer_id)).await;
        Ok(())
    }

    /// The controller for rooms
    #[instrument(
        name = "room.controller",
        skip_all,
        fields(room.hash = field::Empty)
    )]
    pub(crate) async fn room_controller(
        &self,
        mut receiver: Receiver<RoomMessage>,
        end_sessions: CancellationToken,
        stop_io: &CancellationToken,
        end_call: Arc<Notify>,
    ) -> Result<()> {
        let room_hash = self.room_hash().await;
        Span::current().record("room.hash", field::debug(room_hash));
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // moves sockets to audio_input
        let new_sockets = SharedSockets::default();
        // shared statistics
        let statistics_state = StatisticsCollectorState::new(None);
        // tracks connection state for peers
        let mut connections = HashMap::new();

        // Setup input (stream is managed internally)
        let mut input_helper = self
            .setup_input(
                (true, true, 5_f32), // hard coded room codec options
                &statistics_state,
                &end_call,
            )
            .await?;

        let input_handle = spawn_task(audio_input(
            input_helper.receiver(),
            SendingSockets::new(new_sockets.clone()),
            stop_io.clone(),
            statistics_state.upload_bandwidth.clone(),
        ));

        let statistics_handle = spawn_task(statistics_collector(
            statistics_state.clone(),
            self.callbacks.statistics_callback(),
            stop_io.clone(),
            self.core_state.efficiency_mode.load(Relaxed),
            self.core_state.statistics_paused.clone(),
        ));

        // kick the UI out of connecting mode
        self.callbacks.call_state(CallState::Waiting).await;

        loop {
            select! {
                message = receiver.recv() => {
                    match message {
                        Some(RoomMessage::Join { audio_transport, state }) => {
                            info!(event = "room_join_received", peer.id = %state.peer);

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
                                    end_call.clone(),
                                )
                                .await?;
                            // begin sending
                            let handle = spawn_task(audio_output(
                                helper.sender(),
                                read,
                                stop_io.clone(),
                                statistics_state.download_bandwidth.clone(),
                                statistics_state.loss.clone(),
                            ));

                            connections.insert(state.peer, RoomConnection {
                                _output: helper,
                                handle,
                            });
                            self.callbacks.call_state(CallState::RoomJoin(state.peer.to_string())).await;
                        }
                        Some(RoomMessage::Leave(peer)) => {
                            self.callbacks.call_state(CallState::RoomLeave(peer.to_string())).await;

                            if let Some(connection) = connections.remove(&peer) {
                                connection.handle.await??;
                                info!(event = "room_connection_cleaned_up", peer.id = %peer);
                            } else {
                                warn!(event = "room_leave_without_connection", peer.id = %peer);
                            }
                        }
                        None => {
                            warn!(event = "room_controller_channel_closed_unexpectedly");
                            break;
                        }
                    }
                }
                _ = end_call.notified() => {
                    info!(event = "room_call_ended_signal");
                    break;
                }
            }
        }

        // tear down processing stack
        debug!(event = "room_processing_teardown_start");
        // on ios the audio session must be deactivated
        #[cfg(target_os = "ios")]
        deactivate_audio_session();
        // cleanup web input on WASM
        #[cfg(target_family = "wasm")]
        {
            *self.web_input.lock().await = None;
        }
        stop_io.cancel();
        // join input IO task
        input_handle.await??;
        // join output tasks, dropping output helpers to close
        for connection in connections.into_values() {
            connection.handle.await??;
        }
        debug!(event = "room_processing_teardown_done");
        // cleanup room state
        self.room_state.write().await.take();
        // cleanup sessions blocked by room
        end_sessions.cancel();
        // join statistics collector
        statistics_handle.await?;
        Ok(())
    }
}

impl<C, S> Clone for TelepathyCore<C, S>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
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

struct SessionTask(JoinHandle<Result<()>>);

impl SessionTask {
    async fn join(self) -> Result<()> {
        self.0.await??;
        Ok(())
    }
}

struct RoomConnection {
    _output: OutputHelper,
    handle: JoinHandle<Result<()>>,
}

pub(crate) struct OptionalCallArgs<'a> {
    audio_transport: Transport<TransportStream>,
    control_transport: &'a mut Transport<TransportStream>,
    message_receiver: &'a mut Receiver<ProtocolMessage>,
    state: &'a Arc<SessionState>,
}

fn dialer_control_needed(state: &HashMap<PeerId, PeerState>) -> bool {
    state
        .values()
        .any(|state| state.dialer && !state.selected_connection)
}
