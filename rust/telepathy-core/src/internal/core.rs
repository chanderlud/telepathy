//! `TelepathyCore` lifecycle: session manager spawns per-peer sessions, each session
//! negotiates incoming or outgoing calls, then transitions into direct [`call_handshake`]
//! or room [`room_handshake`] handling.

use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::connections::{
    ConstConnection, DynamicConnection, SharedConnections, audio_input, audio_output,
};
use crate::internal::error::ErrorKind;
use crate::internal::helpers::{InputHelper, OutputHelper};
use crate::internal::messages::{
    AudioHeader, ProtocolMessage, RoomMessage, SESSION_STOPPED_REASON, StartScreenshare,
};
use crate::internal::state::{
    CallSlot, CallSlotAcquireResult, CallSlotSnapshot, CallSlotState, CoreState,
    StatisticsCollectorState,
};
use crate::internal::utils::{JoinHandle, spawn_task};
#[cfg(target_os = "ios")]
use crate::internal::utils::{configure_audio_session, deactivate_audio_session};
use crate::internal::utils::{loopback, read_message, statistics_collector, write_message};
use crate::internal::{
    ALPN, EarlyCallState, HELLO_TIMEOUT, KEEP_ALIVE, Result, RoomState, SESSION_MAX_FRAME_LENGTH,
    SessionState,
};
use crate::overlay::CONNECTED;
use crate::overlay::Overlay;
use crate::types::{
    CallState, ChatMessage, CodecConfig, Contact, ManagerState, NetworkConfig, ScreenshareConfig,
    SessionStatus,
};
use chrono::Local;
use iroh::endpoint::{
    ConnectError, ConnectingError, Connection, ConnectionError, RecvStream, SendStream, VarInt,
};
use iroh::{Endpoint, PublicKey};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
#[cfg(target_family = "wasm")]
use telepathy_audio::WebAudioWrapper;
use telepathy_audio::devices::AudioHost;
use tokio::select;
#[cfg(target_family = "wasm")]
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::{Notify, RwLock};
#[cfg(not(target_family = "wasm"))]
use tokio::time::{Instant, Interval, interval, sleep_until, timeout};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span, debug, error, field, info, instrument, warn};
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::std::Instant;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::{Interval, interval, sleep_until, timeout};

const MANAGER_RETRY_BASE_MS: u64 = 500;
const MANAGER_RETRY_MAX_MS: u64 = 30_000;

pub struct TelepathyCore<C, S, H, I, O>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    /// The audio host
    pub(crate) host: H,

    /// Core state for telepathy
    pub core_state: CoreState,

    /// Tracks state for the current room
    pub(crate) room_state: Arc<RwLock<Option<RoomState>>>,

    /// Keeps track of and controls the sessions
    pub session_states: Arc<RwLock<HashMap<PublicKey, Arc<SessionState>>>>,

    /// Signals the session manager to start a new session
    pub start_session: Option<Sender<PublicKey>>,

    /// Restarts the session manager when needed
    pub(crate) restart_manager: Arc<Notify>,

    pub(crate) cancel_outbound_connections: Arc<Notify>,

    /// Monotonic outbound dial generation per peer; stale attempts must not emit UI status.
    pub(crate) outbound_attempts: Arc<RwLock<HashMap<PublicKey, u64>>>,

    /// A reference to the object that controls the call overlay
    pub(crate) overlay: Overlay,

    /// A wrapper to provide audio input on the web
    #[cfg(target_family = "wasm")]
    pub(crate) web_input: Arc<Mutex<Option<WebAudioWrapper>>>,

    /// callback methods provided by the flutter frontend
    pub(crate) callbacks: Arc<C>,

    phantom_statistics: PhantomData<Arc<S>>,
    phantom_input: PhantomData<Arc<I>>,
    phantom_output: PhantomData<Arc<O>>,
}

impl<C, S, H, I, O> TelepathyCore<C, S, H, I, O>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    pub fn new(
        host: H,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: C,
    ) -> TelepathyCore<C, S, H, I, O> {
        Self {
            host,
            core_state: CoreState::new(network_config, screenshare_config, codec_config),
            room_state: Default::default(),
            session_states: Default::default(),
            start_session: None,
            restart_manager: Default::default(),
            cancel_outbound_connections: Default::default(),
            outbound_attempts: Default::default(),
            overlay: overlay.clone(),
            #[cfg(target_family = "wasm")]
            web_input: Default::default(),
            callbacks: Arc::new(callbacks),
            phantom_statistics: Default::default(),
            phantom_input: Default::default(),
            phantom_output: Default::default(),
        }
    }

    /// Spawns the manager & returns the handle if no manager exists yet
    #[instrument(name = "manager.spawn", skip_all)]
    pub async fn start_manager(&mut self) -> Option<JoinHandle<()>> {
        // only allow one manager
        if self.start_session.is_some() {
            return None;
        }

        let (start_session, mut receive_session) = channel(8);

        self.start_session = Some(start_session);

        // start the session manager
        let manager_clone = self.clone();
        Some(spawn_task(
            async move {
                let mut retries = 0;
                // break when stop_manager==true
                while !manager_clone.core_state.stop_manager.load(Relaxed) {
                    let last_launch = Instant::now();
                    // run the session manager to completion
                    let result = manager_clone.session_manager(&mut receive_session).await;

                    if let Err(error) = result {
                        manager_clone
                            .callbacks
                            .manager_state(ManagerState::Failed)
                            .await;
                        Span::current().record("restart_count", retries);
                        error!(
                            event = "session_manager_failed",
                            retries,
                            error = %error
                        );
                        retries += 1;
                        let next_launch =
                            last_launch + Duration::from_millis(manager_retry_delay_ms(retries));
                        if next_launch > Instant::now() {
                            // wait for the next launch or restart
                            select! {
                                _ = manager_clone.restart_manager.notified() => (),
                                _ = sleep_until(next_launch) => (),
                            }
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

    /// Builds the iroh endpoint, handles session start requests and incoming connections
    #[instrument(
        name = "manager.run",
        skip_all,
        fields(manager.id = %Uuid::new_v4(), restart_count = field::Empty)
    )]
    async fn session_manager(&self, start: &mut Receiver<PublicKey>) -> Result<()> {
        let setup_started = Instant::now();
        // build the endpoint & bring online
        let Some(endpoint) = self.setup_endpoint().await? else {
            info!(event = "mananger_restart_setup_endpoint");
            return Ok(());
        };
        info!(
            event = "manager_endpoint_setup",
            elapsed_ms = setup_started.elapsed().as_millis() as u64
        );

        // handles to threads spawned by the session manager
        let mut handles: Vec<JoinHandle<()>> = Vec::new();
        // preload public identity
        let public_identity = self.peer_id().await;

        // the manager is about to start processing events
        self.core_state.manager_active.notify_waiters();

        loop {
            select! {
                // restart the manager
                _ = self.restart_manager.notified() => {
                    break;
                }
                Some(incoming) = endpoint.accept() => {
                    info!(event = "incoming_connection", remote_addr = ?incoming.remote_addr());

                    let accepting = match incoming.accept() {
                        Ok(accepting) => accepting,
                        Err(error) => {
                            warn!(event = "accept_incoming_failed", error = %error);
                            continue;
                        }
                    };

                    match accepting.await {
                        Ok(connection) => {
                            let peer_id = connection.remote_id();
                            let contact = self.callbacks.get_contact(peer_id.to_vec()).await;

                            if contact.is_none() && !self.is_in_room(&peer_id).await {
                                warn!(event = "unknown_peer_connected", peer.id = %peer_id);
                                connection.close(VarInt::from_u32(1), b"unknown peer");
                                continue;
                            }

                            let self_clone = self.clone();
                            handles.push(spawn_task(async move {
                                if let Err(error) = self_clone
                                    .initialize_session(connection.remote_id(), connection, None)
                                    .await
                                {
                                    error!(event = "session_init_failed", error = %error);
                                }
                            }))
                        }
                        Err(error) => {
                            warn!(event = "incoming_connection_failed", error = %error);
                            continue;
                        }
                    }
                }
                // start a new session
                Some(peer_id) = start.recv() => {
                    if peer_id == public_identity {
                        // prevents dialing yourself
                        debug!(event = "dial_ignored_self", peer.id = %peer_id);
                    } else if self.session_states.read().await.get(&peer_id).is_some() {
                        warn!(event = "ignored_redundant_outgoing", peer.id = %peer_id);
                    } else {
                        debug!(event = "dial_initial", peer.id = %peer_id);
                        let self_clone = self.clone();
                        let endpoint_clone = endpoint.clone();
                        handles.push(spawn_task(async move {
                            self_clone
                                .open_session(peer_id, endpoint_clone)
                                .await;
                        }));
                    }
                }
                else => {
                    warn!(event = "edge_case", case = "session_manager_else_branch");
                    break;
                },
            }
        }

        debug!(event = "manager_teardown_start");
        self.callbacks.manager_state(ManagerState::Stopped).await;
        self.cancel_outbound_connections.notify_waiters();
        self.outbound_attempts.write().await.clear();
        // reset room state
        if let Some(state) = self.room_state.write().await.take() {
            state.end_call.notify_one();
            state.cancel.cancel();
        }
        // join all sessions created in manager
        for handle in handles {
            handle.await?;
        }
        debug!(event = "manager_session_handles_joined");
        // TODO this currently takes 3 seconds when there are outgoing connections, i think we need to decouple the UI disappearing from the shutdown
        endpoint.close().await;
        debug!(event = "endpoint_closed");
        Ok(())
    }

    /// Called by the dialer to open a connection and initialize a session
    #[instrument(
        name = "session.open",
        skip_all,
        fields(peer.id = %peer)
    )]
    async fn open_session(&self, peer: PublicKey, endpoint: Endpoint) {
        let generation = self.begin_outbound_attempt(peer).await;
        self.emit_outbound_status(peer, generation, SessionStatus::Connecting)
            .await;

        let connect_future = async {
            let mut retries = 0;
            loop {
                match endpoint.connect(peer, ALPN).await {
                    Ok(connection) => {
                        break Some(connection);
                    }
                    Err(error) => {
                        if let ConnectError::Connecting {
                            source:
                                ConnectingError::ConnectionError {
                                    source: ConnectionError::TimedOut,
                                    ..
                                },
                            ..
                        } = error
                        {
                            break None;
                        }

                        if retries > 3 {
                            break None;
                        } else {
                            retries += 1;
                            warn!(event = "connect_failed", peer.id = %peer, error = %error, retries = retries);
                        }
                    }
                }
            }
        };

        select! {
            _ = self.cancel_outbound_connections.notified() => {
                warn!(event = "outbound_connection_canceled", peer = %peer);
            }
            result = connect_future => {
                if let Some(connection) = result {
                    info!(event = "connect_succeeded", peer.id = %peer);
                    if let Err(error) = self
                        .initialize_session(peer, connection, Some(generation))
                        .await
                    {
                        error!(event = "session_init_failed", error = %error);
                    }
                } else {
                    warn!(event = "connect_abandoned", peer.id = %peer);
                    self.emit_outbound_status(
                        peer,
                        generation,
                        SessionStatus::Inactive,
                    )
                    .await;
                }
            }
        }
    }

    /// Entry point to a session that sets up state and spawns session outer
    #[instrument(name = "session.init", skip_all, fields(peer.id = %peer, session.id = field::Empty))]
    async fn initialize_session(
        &self,
        peer: PublicKey,
        connection: Connection,
        outbound_generation: Option<u64>,
    ) -> Result<()> {
        let session_generation = match outbound_generation {
            Some(generation) => generation,
            None => self.get_outbound_generation(peer).await,
        };

        let contact_option = self.callbacks.get_contact(peer.to_vec()).await;
        // sends messages to the session from elsewhere in the program
        let message_channel = channel::<ProtocolMessage>(8);
        // create the state and a clone of it for the session
        let state = Arc::new(SessionState::new(&message_channel.0));
        Span::current().record("session.id", state.id.to_string());
        let local_peer = self.peer_id().await;
        let keep_new_session =
            should_keep_new_session(&local_peer, &peer, connection.side().is_client());
        let mut states = self.session_states.write().await;
        let old_state_option = if let Some(old_state) = states.get(&peer).cloned() {
            if keep_new_session {
                states.insert(peer, state.clone());
            }

            Some(old_state)
        } else {
            states.insert(peer, state.clone());
            None
        };
        drop(states);

        if let Some(old_state) = old_state_option {
            if keep_new_session {
                warn!(
                    event = "session_collision_kept_new",
                    peer.id = %peer,
                    peer.local = %local_peer,
                    session.id = %state.id,
                    old_session.id = %old_state.id,
                    connection.side.client = connection.side().is_client()
                );
                old_state.teardown().await;
            } else {
                warn!(
                    event = "session_collision_kept_existing",
                    peer.id = %peer,
                    peer.local = %local_peer,
                    session.id = %state.id,
                    old_session.id = %old_state.id,
                    connection.side.client = connection.side().is_client()
                );
                state.teardown().await;
                connection.close(VarInt::from_u32(0), &[]);
                return Ok(());
            }
        }

        // connection monitor sends SessionStatus::Connected to the frontend
        let state_clone = state.clone();
        let callbacks_clone = self.callbacks.clone();
        let connection_clone = connection.clone();
        spawn_task(async move {
            state_clone
                .connection_monitor(connection_clone, callbacks_clone, peer)
                .await;
        });

        let contact = contact_option.unwrap_or_else(|| {
            // there may be no contact for members of a group
            debug!(event = "group_contact_created", peer.id = %peer);
            Contact {
                id: Uuid::new_v4().to_string(),
                nickname: String::from("GroupContact"),
                peer_id: peer,
                output_volume: 0_f32,
                is_room_only: true,
            }
        });

        // seed the initial per-contact output volume
        self.core_state.set_peer_output_volume(&contact)?;

        let _ = self
            .session_outer(peer, &connection, &state, &contact, message_channel)
            .await;

        // Determine whether this session is still the current map entry for `peer`. If a newer
        // session has already replaced us (collision-loser cleanup, reset_sessions drain, or
        // test-driven map removal that produces a "stale" session), the connection is owned by
        // the active replacement session, so we MUST NOT close it. We also MUST NOT touch
        // call-slot state, output volume, or emit Inactive — all of those are owned by the
        // replacement session.
        let mut states = self.session_states.write().await;
        let still_current = states.get(&peer).map(|s| s.id == state.id).unwrap_or(false);
        if still_current {
            // this session still owns the connection — close it before tearing down our
            // per-session state
            connection.close(VarInt::from_u32(0), &[]);
            // release any pending negotiation owned by this session
            self.core_state
                .call_slot
                .release_if_pending_for_peer(peer)?;
            states.remove(&peer);
            // clean up output volume state
            self.core_state.reset_peer_output_volume(&contact.peer_id)?;
        } else {
            debug!(
                event = "session_cleanup_skipped_replaced",
                peer.id = %peer,
                session.id = %state.id
            );
        }
        drop(states);

        // avoid sending session statuses for dummy contacts
        if still_current && !contact.is_room_only {
            self.emit_inactive(peer, session_generation).await;
        }

        info!(event = "session_cleaned_up", session.id = %state.id);
        Ok(())
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
        peer: PublicKey,
        connection: &Connection,
        state: &Arc<SessionState>,
        contact: &Contact,
        mut message_channel: (Sender<ProtocolMessage>, Receiver<ProtocolMessage>),
    ) -> Result<()> {
        let stream_result = if connection.side().is_client() {
            Span::current().record("session.role", "dialer");
            connection.open_bi().await
        } else {
            Span::current().record("session.role", "listener");
            connection.accept_bi().await
        };

        let stream = match stream_result {
            Ok(streams) => streams,
            Err(error) => {
                error!(event = "session_stream_failure", error = ?error, peer.id = %peer);
                return Ok(());
            }
        };

        // controls keep alive messages
        let mut keep_alive = interval(KEEP_ALIVE);
        // the length delimited transport used for the session
        let mut send = LengthDelimitedCodec::builder()
            .max_frame_length(SESSION_MAX_FRAME_LENGTH)
            .length_field_type::<u64>()
            .new_write(stream.0);
        let mut recv = LengthDelimitedCodec::builder()
            .max_frame_length(SESSION_MAX_FRAME_LENGTH)
            .length_field_type::<u64>()
            .new_read(stream.1);

        // the dialer for room sessions always starts a call
        if self.is_in_room(&peer).await && connection.side().is_client() {
            state.start_call.notify_one();
        } else {
            // Re-arm a pending outgoing call intent that may have been notified to a stale
            // session for this peer. Without this, a session-collision replacement loses the
            // user's start_call intent
            match self.core_state.call_slot.snapshot()? {
                snapshot
                    if snapshot.state == CallSlotState::PendingOutgoing
                        && snapshot.direct_peer == Some(peer) =>
                {
                    info!(
                        event = "session_rearmed_pending_outgoing",
                        peer.id = %peer,
                        session.id = %state.id
                    );
                    if is_session_still_current(&self.session_states, peer, state.id).await {
                        state.start_call.notify_one();
                    }
                }
                _ => {}
            }
        }

        let mut io = SessionIo {
            send: &mut send,
            recv: &mut recv,
            connection,
            state,
            message_channel: &mut message_channel,
            keep_alive: &mut keep_alive,
        };

        loop {
            let result = self.session_inner(contact, &mut io).await;

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
                    debug!(event = "session_continuing_after_call");
                }
                (Err(error), room_only) => {
                    let peer = contact.peer_id;
                    let call_slot = &self.core_state.call_slot;
                    // Snapshot state + owning peer in one lock acquisition; a split read could
                    // observe a newer call's slot between the two checks and incorrectly release it.
                    let snapshot = call_slot.snapshot()?;
                    if !room_only
                        && !self.is_in_room(&peer).await
                        && snapshot.state == CallSlotState::ActiveDirect
                        && snapshot.direct_peer == Some(peer)
                    {
                        warn!(event = "session_error_while_call_active", ?error);
                        if call_slot.release_if_match(snapshot)? {
                            self.callbacks
                                .call_state(CallState::CallEnded(error.to_string(), false))
                                .await;
                        }
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
        Ok(())
    }

    /// Routes a negotiated call into [`room_handshake`] or [`call_handshake`] depending on call kind.
    ///
    /// On [`HandshakeDispatch::Completed`] the caller must call [`finalize_handshake_success`].
    async fn perform_call_handshake_dispatch(
        &self,
        io: &mut SessionIo<'_>,
        pending_slot: &mut Option<PendingDirectCallSlot<'_>>,
        call_state: EarlyCallState,
        is_in_room: bool,
    ) -> Result<HandshakeDispatch> {
        if is_in_room {
            self.room_handshake(
                io.send,
                io.recv,
                io.connection,
                &io.state.stop_session,
                call_state,
                io.state.id,
            )
            .await?;
            Ok(HandshakeDispatch::Completed)
        } else if let Some(_slot) = pending_slot.take() {
            match self
                .call_handshake(
                    io.send,
                    io.recv,
                    io.connection,
                    &mut io.message_channel.1,
                    io.state,
                    call_state,
                )
                .await
            {
                Ok(()) => Ok(HandshakeDispatch::Completed),
                Err(error) if error.is_session_stopped() => Ok(HandshakeDispatch::SessionStopped),
                Err(error) => Err(error),
            }
        } else {
            Ok(HandshakeDispatch::Completed)
        }
    }

    /// Decides how to claim the call slot for an incoming `Hello` before accept/reject negotiation.
    async fn acquire_incoming_call_slot<'a>(
        &self,
        send: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
        connection: &Connection,
        call_slot: &'a CallSlot,
        peer: PublicKey,
        session_id: Uuid,
        room: IncomingRoomDecision,
    ) -> Result<IncomingSlotDecision<'a>> {
        // Three cases: matching room call (peer already in our room) -> room handshake;
        // mismatched room call -> reject; direct call -> try to acquire the direct call slot.
        if room.is_in_room && room.peer_room_hash == room.local_room_hash {
            Ok(IncomingSlotDecision::RoomMatch)
        } else if room.peer_room_hash.is_some() {
            info!(event = "room_call_rejected_not_in_room");
            write_message(send, &ProtocolMessage::Reject).await?;
            Ok(IncomingSlotDecision::RejectedNotInRoom)
        } else {
            // A direct-call pending slot may only be acquired by a session that is still
            // the current map entry for `peer` and whose `stop_session` token has not
            // been canceled.
            if !is_session_still_current(&self.session_states, peer, session_id).await {
                // see IncomingSlotDecision::StaleSession
                info!(event = "incoming_call_skipped_stale_session", peer.id = %peer);
                let states = self.session_states.read().await;
                if states.get(&peer).is_none() {
                    connection.close(VarInt::from_u32(0), &[]);
                }
                drop(states);
                return Ok(IncomingSlotDecision::StaleSession);
            }
            match PendingDirectCallSlot::try_acquire_incoming(call_slot, peer)? {
                Some(slot) => Ok(IncomingSlotDecision::Acquired(slot)),
                None => {
                    info!(event = "call_busy_sent_call_already_active");
                    write_message(send, &ProtocolMessage::Busy).await?;
                    Ok(IncomingSlotDecision::Busy)
                }
            }
        }
    }

    /// Handles one protocol message received while awaiting `HelloAck` on an outgoing call.
    async fn handle_outgoing_hello_response(
        &self,
        io: &mut SessionIo<'_>,
        args: &OutgoingCallArgs<'_>,
        call_state: &mut EarlyCallState,
        pending_slot: &mut Option<PendingDirectCallSlot<'_>>,
        message: ProtocolMessage,
    ) -> Result<HelloResponse> {
        let is_in_room = args.room_hash.is_some();
        match message {
            ProtocolMessage::HelloAck { audio_header } => {
                if !audio_header.is_valid() {
                    warn!(event = "invalid_audio_header_rejected");
                    write_message(io.send, &ProtocolMessage::Reject).await?;
                    return Ok(HelloResponse::EndedSilently);
                };

                call_state.remote_configuration = audio_header;
                match self
                    .perform_call_handshake_dispatch(
                        io,
                        pending_slot,
                        call_state.clone(),
                        is_in_room,
                    )
                    .await?
                {
                    HandshakeDispatch::Completed => {
                        io.keep_alive.reset();
                        Ok(HelloResponse::Completed)
                    }
                    HandshakeDispatch::SessionStopped => Ok(HelloResponse::SessionStopped),
                }
            }
            ProtocolMessage::Goodbye { reason: Some(m) } => Ok(HelloResponse::EndedWith(format!(
                "{} did not accept the call because of {m}",
                args.contact.nickname
            ))),
            // In a room, peer-level reject/busy is non-fatal: other peers may still join, so keep waiting.
            ProtocolMessage::Reject | ProtocolMessage::Busy if is_in_room => {
                info!(event = "room_peer_rejected_or_busy_ignored");
                Ok(HelloResponse::Continue)
            }
            ProtocolMessage::Goodbye { .. } | ProtocolMessage::Reject => {
                info!(event = "call_not_accepted");
                Ok(HelloResponse::EndedWith(format!(
                    "{} did not accept the call",
                    args.contact.nickname
                )))
            }
            ProtocolMessage::Busy => {
                info!(event = "call_peer_busy");
                Ok(HelloResponse::EndedWith(format!(
                    "{} is busy",
                    args.contact.nickname
                )))
            }
            ProtocolMessage::KeepAlive => Ok(HelloResponse::Continue),
            // Simultaneous dial: both sides sent Hello before receiving the other's. The lower peer-id
            // yields and accepts the incoming Hello as if it were the callee.
            ProtocolMessage::Hello { audio_header, .. } => {
                // We are the lower peer -> we lose the tiebreaker -> accept their Hello here
                // (mirrors negotiate_incoming_call's HelloAck path). Otherwise, we win and keep
                // waiting for their HelloAck.
                if self.peer_id().await < args.contact.peer_id {
                    info!(event = "simultaneous_dial_detected_yielding");
                    if !audio_header.is_valid() {
                        warn!(event = "invalid_audio_header_rejected");
                        write_message(io.send, &ProtocolMessage::Reject).await?;
                        Ok(HelloResponse::EndedSilently)
                    } else {
                        call_state.remote_configuration = audio_header;
                        write_message(
                            io.send,
                            &ProtocolMessage::HelloAck {
                                audio_header: call_state.local_configuration.clone(),
                            },
                        )
                        .await?;

                        match self
                            .perform_call_handshake_dispatch(
                                io,
                                pending_slot,
                                call_state.clone(),
                                is_in_room,
                            )
                            .await?
                        {
                            HandshakeDispatch::Completed => {
                                io.keep_alive.reset();
                                Ok(HelloResponse::Completed)
                            }
                            HandshakeDispatch::SessionStopped => Ok(HelloResponse::SessionStopped),
                        }
                    }
                } else {
                    info!(event = "simultaneous_dial_detected_winning");
                    Ok(HelloResponse::Continue)
                }
            }
            message => {
                warn!(event = "hello_ack_flow_unexpected_message", ?message);
                Ok(HelloResponse::EndedWith(format!(
                    "Received an unexpected message from {}",
                    args.contact.nickname
                )))
            }
        }
    }

    /// Negotiates an incoming call after `session_inner` has parsed and validated a peer `Hello`.
    ///
    /// Handles room vs direct routing, the accept prompt for direct calls, accept/reject/busy
    /// responses, and on success transitions into [`room_handshake`] or [`call_handshake`].
    ///
    /// Pre-condition: the peer `Hello` is already validated by the caller.
    /// Post-condition: on all returns except [`IncomingNegotiationOutcome::HandshakeComplete`],
    /// the global call slot is idle or in [`CallSlotState::RoomCall`].
    async fn negotiate_incoming_call(
        &self,
        io: &mut SessionIo<'_>,
        args: IncomingCallArgs<'_>,
    ) -> Result<IncomingNegotiationOutcome> {
        let peer = args.contact.peer_id;
        let call_slot = &self.core_state.call_slot;
        let mut pending_slot = None;
        let mut cancel_prompt = None;
        let mut accept_handle = None;

        // Honor cancellation before acquiring any pending direct-call slot
        if io.state.stop_session.is_cancelled() {
            info!(event = "incoming_call_cancelled_before_acquire");
            return Ok(IncomingNegotiationOutcome::SessionStopped);
        }

        match self
            .acquire_incoming_call_slot(
                io.send,
                io.connection,
                call_slot,
                peer,
                io.state.id,
                IncomingRoomDecision {
                    is_in_room: args.is_in_room,
                    peer_room_hash: args.peer_room_hash,
                    local_room_hash: args.local_room_hash,
                },
            )
            .await?
        {
            IncomingSlotDecision::RoomMatch => {}
            IncomingSlotDecision::RejectedNotInRoom | IncomingSlotDecision::Busy => {
                return Ok(IncomingNegotiationOutcome::ContinueSession);
            }
            // `StaleSession` is terminal for the session task: this session is no longer the
            // current map entry for the peer (replaced by a collision winner or drained by
            // `reset_sessions`), so processing further messages on it is pointless and risks
            // acting on stale state. Unlike `Busy` — a transient state for a still-valid
            // session that may legitimately receive more traffic — a stale session exits
            // the session loop without writing a wire response. The dialer is informed via
            // connection teardown: if a fresh session exists for the peer it owns/closes
            // the relevant connection and serves the dialer on its own connection; if no
            // fresh session exists, `acquire_incoming_call_slot` already closed this
            // connection so the dialer sees a transport close instead of waiting the
            // full `HELLO_TIMEOUT` for a `HelloAck` that will never come.
            IncomingSlotDecision::StaleSession => {
                return Ok(IncomingNegotiationOutcome::SessionStopped);
            }
            IncomingSlotDecision::Acquired(slot) => {
                pending_slot = Some(slot);
                // Only direct calls show an accept prompt; room calls auto-accept.
                let cancel = Arc::new(Notify::new());
                accept_handle = Some(self.callbacks.get_accept_handle(
                    &args.contact.id,
                    args.other_ringtone,
                    &cancel,
                ));
                cancel_prompt = Some(cancel);
            }
        }

        let cancel_prompt_clone = cancel_prompt.clone();
        let accept_future = async {
            if let Some(accept_handle) = accept_handle {
                select! {
                    accept_result = accept_handle => accept_result,
                    _ = io.state.start_call.notified() => {
                        // Local user pressed "accept" via start_call before the platform prompt resolved;
                        // cancel the prompt and proceed.
                        info!(event = "call_started_while_prompting");
                        if let Some(cancel) = cancel_prompt_clone {
                            cancel.notify_one();
                        }
                        Ok(true)
                    },
                }
            } else {
                Ok(true)
            }
        };

        select! {
            _ = io.state.stop_session.cancelled() => {
                info!(event = "session_stopped_during_accept_prompt");
                if let Some(cancel) = cancel_prompt {
                    cancel.notify_one();
                }
                abort_negotiation_session_stopped(
                    &self.session_states,
                    peer,
                    io.state.id,
                    io.send,
                    &mut pending_slot,
                )
                .await?;
                Ok(IncomingNegotiationOutcome::SessionStopped)
            }
            accept_result = accept_future => {
                if !accept_result? {
                    if io.state.stop_session.is_cancelled() {
                        abort_negotiation_session_stopped(
                            &self.session_states,
                            peer,
                            io.state.id,
                            io.send,
                            &mut pending_slot,
                        )
                        .await?;
                        return Ok(IncomingNegotiationOutcome::SessionStopped);
                    }
                    write_message(io.send, &ProtocolMessage::Reject).await?;
                    release_pending(
                        &self.session_states,
                        peer,
                        io.state.id,
                        &mut pending_slot,
                    )
                    .await?;
                    return Ok(IncomingNegotiationOutcome::ContinueSession);
                }

                match self.setup_call(peer).await {
                    Ok(mut call_state) => {
                        call_state.remote_configuration = args.remote_audio_header;
                        write_message(
                            io.send,
                            &ProtocolMessage::HelloAck {
                                audio_header: call_state.local_configuration.clone(),
                            },
                        )
                        .await?;

                        match self
                            .perform_call_handshake_dispatch(
                                io,
                                &mut pending_slot,
                                call_state,
                                args.is_in_room,
                            )
                            .await?
                        {
                            HandshakeDispatch::Completed => {
                                io.keep_alive.reset();
                                Ok(IncomingNegotiationOutcome::HandshakeComplete)
                            }
                            HandshakeDispatch::SessionStopped => {
                                Ok(IncomingNegotiationOutcome::SessionStopped)
                            }
                        }
                    }
                    Err(error) => {
                        error!(event = "setup_call_failed", ?error);
                        write_message(
                            io.send,
                            &ProtocolMessage::Goodbye {
                                reason: Some("audio device error".to_string()),
                            },
                        )
                        .await?;
                        release_pending(
                            &self.session_states,
                            peer,
                            io.state.id,
                            &mut pending_slot,
                        )
                        .await?;
                        Err(error)
                    }
                }
            }
            result = read_message(io.recv) => {
                // Receiving any message during the accept prompt means the caller hung up (Goodbye)
                // or sent something out-of-protocol; abort negotiation but keep the session alive.
                if let Some(cancel) = cancel_prompt {
                    cancel.notify_one();
                }
                release_pending(
                    &self.session_states,
                    peer,
                    io.state.id,
                    &mut pending_slot,
                )
                .await?;
                let message = result?;
                warn!(event = "accept_prompt_interrupted_by_message", ?message);
                Ok(IncomingNegotiationOutcome::ContinueSession)
            }
        }
    }

    /// Negotiates an outgoing call when the local user starts a call via [`SessionState::start_call`].
    ///
    /// Sends `Hello`, awaits `HelloAck` under a (possibly ringtone-extended) timeout, and resolves
    /// simultaneous-dial collisions using the peer-id tiebreaker in [`should_keep_new_session`].
    async fn negotiate_outgoing_call(
        &self,
        io: &mut SessionIo<'_>,
        args: OutgoingCallArgs<'_>,
    ) -> Result<OutgoingNegotiationOutcome> {
        let peer = args.contact.peer_id;
        let call_slot = &self.core_state.call_slot;
        let is_in_room = args.room_hash.is_some();
        let mut pending_slot = None;

        // Honor cancellation before acquiring any pending direct-call slot
        if io.state.stop_session.is_cancelled() {
            info!(event = "outgoing_call_cancelled_before_acquire");
            return Ok(OutgoingNegotiationOutcome::SessionStopped);
        } else if is_in_room {
            if call_slot.current() != CallSlotState::RoomCall {
                warn!(event = "outgoing_room_call_without_room_slot");
                return Ok(OutgoingNegotiationOutcome::CallEnded);
            }
        } else {
            // Per the per-session ownership invariant, a direct-call pending slot may
            // only be acquired (or matched) by a session that is still the current map
            // entry for `peer`. A session that has been replaced by a collision
            // replacement (or drained by `reset_sessions`) must not be allowed to
            // re-pend a slot — its replacement session will take over via the
            // `session_rearmed_pending_outgoing` path in `session_outer`.
            if !is_session_still_current(&self.session_states, peer, io.state.id).await {
                info!(event = "outgoing_call_skipped_stale_session", peer.id = %peer);
                self.callbacks
                    .call_state(CallState::CallEnded(
                        "A call is already active".to_string(),
                        true,
                    ))
                    .await;
                return Ok(OutgoingNegotiationOutcome::CallEnded);
            }
            match PendingDirectCallSlot::try_acquire_outgoing(call_slot, peer)? {
                Some(slot) => pending_slot = Some(slot),
                None => {
                    warn!(event = "call_slot_busy_outgoing", peer.id = %peer);
                    self.callbacks
                        .call_state(CallState::CallEnded(
                            "A call is already active".to_string(),
                            true,
                        ))
                        .await;
                    return Ok(OutgoingNegotiationOutcome::CallEnded);
                }
            }
        }

        let other_ringtone = self.load_ringtone().await;
        let mut call_state = match self.setup_call(peer).await {
            Ok(state) => state,
            Err(error) => {
                self.callbacks
                    .call_state(CallState::CallEnded(error.to_string(), false))
                    .await;
                release_pending(&self.session_states, peer, io.state.id, &mut pending_slot).await?;
                return Err(error);
            }
        };
        // Extend the timeout when we're sending a custom ringtone so the callee's device has time
        // to download and play it before deciding.
        let hello_timeout = HELLO_TIMEOUT
            + if other_ringtone.is_some() {
                Duration::from_secs(10)
            } else {
                Default::default()
            };
        write_message(
            io.send,
            &ProtocolMessage::Hello {
                ringtone: other_ringtone,
                audio_header: call_state.local_configuration.clone(),
                room_hash: args.room_hash,
            },
        )
        .await?;

        loop {
            select! {
                _ = io.state.stop_session.cancelled() => {
                    info!(event = "session_stopped_waiting_hello_ack");
                    release_pending(
                        &self.session_states,
                        peer,
                        io.state.id,
                        &mut pending_slot,
                    )
                    .await?;
                    return Ok(OutgoingNegotiationOutcome::SessionStopped);
                }
                _ = io.state.end_call.notified() => {
                    info!(event = "end_call_notified_waiting_hello_ack");
                    write_message(io.send, &ProtocolMessage::Goodbye { reason: None }).await?;
                    release_pending(
                        &self.session_states,
                        peer,
                        io.state.id,
                        &mut pending_slot,
                    )
                    .await?;
                    return Ok(OutgoingNegotiationOutcome::CallEnded);
                }
                result = timeout(hello_timeout, read_message(io.recv)) => {
                    match result {
                        Err(_elapsed) => {
                            warn!(
                                event = "hello_ack_timeout",
                                hello_timeout_ms = hello_timeout.as_millis() as u64,
                                peer.id = %args.contact.peer_id
                            );
                            self.callbacks
                                .call_state(CallState::CallEnded(
                                    format!(
                                        "{} did not respond to the call",
                                        args.contact.nickname
                                    ),
                                    true,
                                ))
                                .await;
                            release_pending(
                                &self.session_states,
                                peer,
                                io.state.id,
                                &mut pending_slot,
                            )
                            .await?;
                            return Ok(OutgoingNegotiationOutcome::CallEnded);
                        }
                        Ok(Err(error)) => return Err(error),
                        Ok(Ok(message)) => {
                            match self
                                .handle_outgoing_hello_response(
                                    io,
                                    &args,
                                    &mut call_state,
                                    &mut pending_slot,
                                    message,
                                )
                                .await?
                            {
                                HelloResponse::Completed => {
                                    return Ok(OutgoingNegotiationOutcome::HandshakeComplete);
                                }
                                HelloResponse::SessionStopped => {
                                    return Ok(OutgoingNegotiationOutcome::SessionStopped);
                                }
                                HelloResponse::EndedWith(message) => {
                                    self.callbacks
                                        .call_state(CallState::CallEnded(message, true))
                                        .await;
                                    release_pending(
                                        &self.session_states,
                                        peer,
                                        io.state.id,
                                        &mut pending_slot,
                                    )
                                    .await?;
                                    return Ok(OutgoingNegotiationOutcome::CallEnded);
                                }
                                HelloResponse::EndedSilently => {
                                    release_pending(
                                        &self.session_states,
                                        peer,
                                        io.state.id,
                                        &mut pending_slot,
                                    )
                                    .await?;
                                    return Ok(OutgoingNegotiationOutcome::CallEnded);
                                }
                                HelloResponse::Continue => continue,
                            }
                        }
                    }
                }
            }
        }
    }

    /// The inner logic of a session that may execute many times
    /// Returns true if the session should continue
    #[instrument(
        name = "session.iter",
        skip_all,
        fields(peer.id = %contact.peer_id, room.hash = field::Empty, room.generation = field::Empty)
    )]
    async fn session_inner(&self, contact: &Contact, io: &mut SessionIo<'_>) -> Result<bool> {
        info!(event = "session_waiting_for_event");

        select! {
            _ = io.state.stop_session.cancelled() => {
                info!(event = "session_stopped");
                Ok(false)
            },
            result = read_message(io.recv) => {
                info!(event = "session_message_received", ?result);
                let mut other_ringtone = None;
                let remote_audio_header;
                let peer_room_hash;

                match result? {
                    ProtocolMessage::Hello { ringtone, audio_header, room_hash } => {
                        if !audio_header.is_valid() {
                            warn!(event = "invalid_audio_header_rejected");
                            write_message(io.send, &ProtocolMessage::Reject).await?;
                            return Ok(false);
                        }

                        remote_audio_header = audio_header;
                        peer_room_hash = room_hash;
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

                let room_snapshot = self.room_snapshot_for_peer(&contact.peer_id).await;
                Span::current().record("room.hash", field::debug(room_snapshot.local_room_hash));
                Span::current().record("room.generation", room_snapshot.room_generation);
                let outcome = self
                    .negotiate_incoming_call(
                        io,
                        IncomingCallArgs {
                            contact,
                            remote_audio_header,
                            peer_room_hash,
                            other_ringtone,
                            is_in_room: room_snapshot.is_in_room,
                            local_room_hash: room_snapshot.local_room_hash,
                        },
                    )
                    .await?;
                Ok(outcome.to_outcome())
            }
            _ = io.state.start_call.notified() => {
                let room_snapshot = self.room_snapshot_for_peer(&contact.peer_id).await;
                Span::current().record("room.hash", field::debug(room_snapshot.local_room_hash));
                Span::current().record("room.generation", room_snapshot.room_generation);
                let outcome = self
                    .negotiate_outgoing_call(
                        io,
                        OutgoingCallArgs {
                            contact,
                            room_hash: room_snapshot.local_room_hash,
                        },
                    )
                    .await?;
                Ok(outcome.to_outcome())
            }
            _ = io.keep_alive.tick() => {
                debug!(event = "session_keep_alive_sent");
                write_message(io.send, &ProtocolMessage::KeepAlive).await?;
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
        send: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
        recv: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
        connection: &Connection,
        message_receiver: &mut Receiver<ProtocolMessage>,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
        // stop_io must always cancel, even when the call fails
        let stop_io = CancellationToken::new();
        let call_slot = &self.core_state.call_slot;
        if !call_slot.transition_pending_to_active_for_peer(call_state.peer)? {
            if state.stop_session.is_cancelled() {
                info!(
                    event = "call_handshake_slot_released_session_stopped",
                    peer.id = %call_state.peer
                );
                write_message(
                    send,
                    &ProtocolMessage::Goodbye {
                        reason: Some(SESSION_STOPPED_REASON.to_string()),
                    },
                )
                .await?;
                return Err(ErrorKind::SessionStopped.into());
            }
            error!(
                event = "call_handshake_slot_transition_failed",
                peer.id = %call_state.peer
            );
            // Release only if this session is still the current map entry for the peer;
            // a collision-loser cleanup must not clobber the replacement session's slot.
            if is_session_still_current(&self.session_states, call_state.peer, state.id).await {
                call_slot.release_if_pending_for_peer(call_state.peer)?;
            }
            return Err(ErrorKind::CallAlreadyActive.into());
        }
        // capture the expected active-direct slot ownership immediately after the transition
        let expected_active = call_slot.snapshot()?;
        // show the overlay
        self.overlay.show();

        let result = self
            .call(
                &stop_io,
                call_state,
                &state.end_call,
                Some(OptionalCallArgs {
                    connection,
                    control_send: send,
                    control_recv: recv,
                    message_receiver,
                    state,
                }),
            )
            .await;

        info!(event = "call_handshake_ended");
        // ensure that all background i/o threads are stopped
        stop_io.cancel();
        // the call has ended; release only against the snapshot captured before `call()` ran.
        if call_slot.release_if_match(expected_active)? {
            // hide the overlay
            self.overlay.hide();
        }
        // send a goodbye message on errors
        if let Err(error) = result.as_ref() {
            warn!(event = "call_handshake_sending_error_goodbye", ?error);
            let message = ProtocolMessage::error_goodbye(error);
            write_message(send, &message).await?;
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
        // reference for use in networking tasks
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
                call_state.peer,
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
            let input_handle = spawn_task(audio_input(
                input_helper.receiver(),
                ConstConnection::new(o.connection.clone(), self.core_state.audio_sequence.clone()),
                stop_io.clone(),
            ));

            let output_handle = spawn_task(audio_output(
                output_helper.sender(),
                o.connection.clone(),
                stop_io.clone(),
                loss,
                call_state.remote_configuration.sample_rate,
            ));

            let controller_future = self.call_controller(o, call_state.peer, end_call);

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
        o: OptionalCallArgs<'_>,
        peer: PublicKey,
        end_call: &Arc<Notify>,
    ) -> Result<(Option<String>, bool)> {
        let identity = self.peer_id().await;

        CONNECTED.store(true, Relaxed);
        self.callbacks.call_state(CallState::Connected).await;

        loop {
            select! {
                // ends the call
                _ = end_call.notified() => {
                    write_message(o.control_send, &ProtocolMessage::Goodbye { reason: None }).await?;
                    break Ok((None, false));
                },
                _ = o.state.start_screenshare.notified() => {
                    info!(event = "starting_screenshare", peer.id = ?peer);

                    #[cfg(not(target_family = "wasm"))]
                    {
                        let message = StartScreenshare::new_sender(peer, o.connection.clone());
                        let self_clone = self.clone();
                        spawn_task(async move {
                            let result = self_clone.start_screenshare(message).await;
                            if let Err(error) = result {
                                error!(event = "screenshare_start_failed", error = ?error);
                            }
                        }.in_current_span());
                    }
                }
                // receives and handles messages from the callee
                result = read_message(o.control_recv) => {
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
                            info!(event = "screenshare_header_received", ?message, peer.id = ?peer);

                            #[cfg(not(target_family = "wasm"))]
                            {
                                let message = StartScreenshare::new_receiver(peer, message, o.connection.clone());
                                let self_clone = self.clone();
                                spawn_task(async move {
                                    let result = self_clone.start_screenshare(message).await;
                                    if let Err(error) = result {
                                        error!(event = "screenshare_start_failed", error = ?error);
                                    }
                                }.in_current_span());
                            }

                        }
                        _ => error!(event = "call_controller_unexpected_message", ?message),
                    }
                },
                // sends messages to the callee
                result = o.message_receiver.recv() => {
                    if let Some(message) = result {
                        write_message(o.control_send, &message).await?;
                    } else {
                        // if the channel closes, the call has ended
                        info!(event = "call_message_channel_closed");
                        break Ok((None, true));
                    }
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
        send: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
        recv: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
        connection: &Connection,
        stop_session: &CancellationToken,
        call_state: EarlyCallState,
        session_id: Uuid,
    ) -> Result<()> {
        let peer_id = call_state.peer;
        let connection_id = connection.stable_id();
        let (sender, cancel) = self
            .room_handshake_snapshot()
            .await
            .ok_or(ErrorKind::RoomStateMissing)?;

        sender
            .send(RoomMessage::Join {
                connection: connection.clone(),
                state: call_state,
                session_id,
            })
            .await
            .map_err(|_| ErrorKind::RoomStateMissing)?;

        loop {
            select! {
                _ = stop_session.cancelled() => {
                    info!(event = "room_session_stopped_sending_goodbye", peer.id = %peer_id);
                    _ = write_message(send, &ProtocolMessage::Goodbye { reason: None }).await;
                    break
                }
                _ = cancel.cancelled() => {
                    // try to say goodbye
                    info!(event = "room_cancelled_sending_goodbye", peer.id = %peer_id);
                    _ = write_message(send, &ProtocolMessage::Goodbye { reason: None }).await;
                    break
                }
                result = read_message(recv) => {
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
        _ = sender
            .send(RoomMessage::Leave {
                peer: peer_id,
                connection_id,
            })
            .await;
        Ok(())
    }

    /// The controller for rooms
    #[instrument(
        name = "room.controller",
        skip_all,
        fields(room.hash = field::Empty, room.generation = room_generation)
    )]
    pub(crate) async fn room_controller(
        &self,
        mut receiver: Receiver<RoomMessage>,
        end_sessions: CancellationToken,
        stop_io: &CancellationToken,
        end_call: Arc<Notify>,
        room_owner: CallSlotSnapshot,
        room_generation: u64,
    ) -> Result<()> {
        let room_hash = self.room_hash().await;
        Span::current().record("room.hash", field::debug(room_hash));
        // on ios the audio session must be configured
        #[cfg(target_os = "ios")]
        configure_audio_session();

        // moves sockets to audio_input
        let connection_sender = SharedConnections::default();
        // shared statistics
        let statistics_state = StatisticsCollectorState::new(None);
        // tracks connection state for peers keyed by transport stable id
        let mut connections: HashMap<usize, RoomConnection<O>> = HashMap::new();
        let mut peer_connections: HashMap<PublicKey, usize> = HashMap::new();

        // Setup input (stream is managed internally)
        let mut input_helper: InputHelper<I> = self
            .setup_input(
                (true, true, 5_f32), // hard coded room codec options
                &statistics_state,
                &end_call,
            )
            .await?;

        let input_handle = spawn_task(audio_input(
            input_helper.receiver(),
            DynamicConnection::new(
                connection_sender.clone(),
                self.core_state.audio_sequence.clone(),
            ),
            stop_io.clone(),
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
                        Some(RoomMessage::Join { connection, state, session_id }) => {
                            if let Some(session) = self.session_states.read().await.get(&state.peer) {
                                if session.id != session_id {
                                    warn!(event = "room_join_stale_session", peer.id = %state.peer);
                                    continue;
                                }
                            } else {
                                warn!(event = "room_join_missing_session", peer.id = %state.peer);
                                continue;
                            }

                            let connection_id = connection.stable_id();
                            info!(
                                event = "room_join_received",
                                peer.id = %state.peer,
                                connection.id = connection_id,
                            );

                            if connections.contains_key(&connection_id) {
                                warn!(
                                    event = "room_duplicate_join_same_connection",
                                    peer.id = %state.peer,
                                    connection.id = connection_id
                                );
                                continue;
                            }

                            if let Some(old_connection_id) = peer_connections.get(&state.peer).copied()
                                && old_connection_id != connection_id
                            {
                                if let Some(old_connection) = connections.remove(&old_connection_id) {
                                    info!(
                                        event = "room_duplicate_join_replacing_connection",
                                        peer.id = %state.peer,
                                        old.connection.id = old_connection_id,
                                        new.connection.id = connection_id
                                    );
                                    connection_sender.remove(&old_connection.connection);
                                    old_connection
                                        .connection
                                        .close(VarInt::from_u32(0), b"replaced");
                                    match old_connection.handle.await {
                                        Ok(Ok(())) => {}
                                        Ok(Err(error)) => {
                                            warn!(event = "room_output_closed_on_replacement", ?error);
                                        }
                                        Err(error) => return Err(error.into()),
                                    }
                                }
                                peer_connections.remove(&state.peer);
                            }

                            // first connection
                            if connections.is_empty() {
                                CONNECTED.store(true, Relaxed);
                                self.callbacks.call_state(CallState::Connected).await;
                            }

                            connection_sender.push(connection.clone());
                            // setup output stack
                            let mut helper = self
                                .setup_output(
                                    state.peer,
                                    state.remote_configuration.sample_rate as f64,
                                    true,
                                    &statistics_state,
                                    end_call.clone(),
                                )
                                .await?;
                            // begin sending
                            let handle = spawn_task(audio_output(
                                helper.sender(),
                                connection.clone(),
                                stop_io.clone(),
                                statistics_state.loss.clone(),
                                state.remote_configuration.sample_rate,
                            ));

                            peer_connections.insert(state.peer, connection_id);
                            connections.insert(
                                connection_id,
                                RoomConnection {
                                    connection,
                                    _output: helper,
                                    handle,
                                },
                            );
                            self.callbacks
                                .call_state(CallState::RoomJoin(state.peer.to_string()))
                                .await;
                        }
                        Some(RoomMessage::Leave {
                            peer,
                            connection_id,
                        }) => {
                            match peer_connections.get(&peer).copied() {
                                Some(active_connection_id)
                                    if active_connection_id == connection_id =>
                                {
                                    self.callbacks
                                        .call_state(CallState::RoomLeave(peer.to_string()))
                                        .await;
                                    peer_connections.remove(&peer);
                                    if let Some(connection) = connections.remove(&connection_id) {
                                        connection_sender.remove(&connection.connection);
                                        match connection.handle.await {
                                            Ok(Ok(())) => (),
                                            Ok(Err(error)) => {
                                                warn!(event = "room_output_closed_on_leave", ?error);
                                            }
                                            Err(error) => return Err(error.into()),
                                        }
                                        info!(
                                            event = "room_connection_cleaned_up",
                                            peer.id = %peer,
                                            connection.id = connection_id
                                        );
                                    }
                                }
                                Some(active_connection_id) => {
                                    warn!(
                                        event = "room_leave_stale_connection",
                                        peer.id = %peer,
                                        leave.connection.id = connection_id,
                                        active.connection.id = active_connection_id
                                    );
                                }
                                None => {
                                    warn!(
                                        event = "room_leave_without_connection",
                                        peer.id = %peer,
                                        connection.id = connection_id
                                    );
                                }
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
            match connection.handle.await {
                Ok(Ok(())) => (),
                Ok(Err(error)) => {
                    warn!(event = "room_output_closed_on_teardown", ?error);
                }
                Err(error) => return Err(error.into()),
            }
        }
        debug!(event = "room_processing_teardown_done");
        // cleanup room state ONLY if still the currently installed `room_state`
        {
            let mut room_guard = self.room_state.write().await;
            if room_guard
                .as_ref()
                .is_some_and(|state| state.generation == room_generation)
            {
                let _ = room_guard.take();
            } else {
                info!(
                    event = "room_state_take_skipped_stale_generation",
                    room.generation = room_generation
                );
            }
        }
        // release only against the exact `room_owner` snapshot
        self.core_state.call_slot.release_if_match(room_owner)?;
        // cleanup sessions blocked by room
        end_sessions.cancel();
        // join statistics collector
        statistics_handle.await?;
        Ok(())
    }
}

impl<C, S, H, I, O> Clone for TelepathyCore<C, S, H, I, O>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            core_state: self.core_state.clone(),
            room_state: Arc::clone(&self.room_state),
            session_states: Arc::clone(&self.session_states),
            start_session: self.start_session.clone(),
            restart_manager: Arc::clone(&self.restart_manager),
            cancel_outbound_connections: Arc::clone(&self.cancel_outbound_connections),
            outbound_attempts: Arc::clone(&self.outbound_attempts),
            overlay: self.overlay.clone(),
            #[cfg(target_family = "wasm")]
            web_input: Arc::clone(&self.web_input),
            callbacks: Arc::clone(&self.callbacks),
            phantom_statistics: self.phantom_statistics,
            phantom_input: self.phantom_input,
            phantom_output: self.phantom_output,
        }
    }
}

struct RoomConnection<O> {
    connection: Connection,
    _output: OutputHelper<O>,
    handle: JoinHandle<Result<()>>,
}

pub(crate) struct OptionalCallArgs<'a> {
    connection: &'a Connection,
    control_send: &'a mut FramedWrite<SendStream, LengthDelimitedCodec>,
    control_recv: &'a mut FramedRead<RecvStream, LengthDelimitedCodec>,
    message_receiver: &'a mut Receiver<ProtocolMessage>,
    state: &'a Arc<SessionState>,
}

/// Shared session transport and control handles passed through negotiation and handshake.
pub(crate) struct SessionIo<'a> {
    pub(crate) send: &'a mut FramedWrite<SendStream, LengthDelimitedCodec>,
    pub(crate) recv: &'a mut FramedRead<RecvStream, LengthDelimitedCodec>,
    pub(crate) connection: &'a Connection,
    pub(crate) state: &'a Arc<SessionState>,
    pub(crate) message_channel: &'a mut (Sender<ProtocolMessage>, Receiver<ProtocolMessage>),
    pub(crate) keep_alive: &'a mut Interval,
}

/// Per-call inputs for [`TelepathyCore::negotiate_incoming_call`].
struct IncomingCallArgs<'a> {
    contact: &'a Contact,
    remote_audio_header: AudioHeader,
    /// Room hash advertised by the peer in their `Hello`; `Some` means they intend a room call.
    peer_room_hash: Option<u64>,
    other_ringtone: Option<Vec<u8>>,
    is_in_room: bool,
    /// Our current room hash from local state; compared against [`IncomingCallArgs::peer_room_hash`].
    local_room_hash: Option<u64>,
}

/// Room-decision inputs to [`TelepathyCore::acquire_incoming_call_slot`].
///
/// Bundles the three values that decide whether the incoming `Hello` matches our
/// current room (room handshake) or a direct call. Kept separate from
/// [`IncomingCallArgs`] so the slot-acquisition signature stays compact.
struct IncomingRoomDecision {
    is_in_room: bool,
    peer_room_hash: Option<u64>,
    local_room_hash: Option<u64>,
}

/// Per-call inputs for [`TelepathyCore::negotiate_outgoing_call`].
struct OutgoingCallArgs<'a> {
    contact: &'a Contact,
    /// Our current room hash, sent to the peer in `Hello`; `Some` means a room call.
    room_hash: Option<u64>,
}

/// Result of routing a negotiated call into room or direct handshake.
enum HandshakeDispatch {
    Completed,
    SessionStopped,
}

/// Early slot-acquisition decision for an incoming `Hello`.
enum IncomingSlotDecision<'a> {
    /// Peer room hash matches ours; proceed without acquiring a direct-call slot.
    RoomMatch,
    /// Peer wants a room call but we are not in that room; `Reject` already sent.
    RejectedNotInRoom,
    /// Direct-call slot is busy; `Busy` already sent.
    Busy,
    /// Session is no longer the current map entry for the peer (collision-replacement
    /// loser or drained by `reset_sessions`); NO wire response is sent. The dialer is
    /// informed via connection teardown: a fresh replacement session (if any) owns
    /// its own connection and serves the dialer there; if no fresh session exists,
    /// the connection was closed in `acquire_incoming_call_slot` so the dialer
    /// observes a transport close rather than waiting for a `HelloAck` that will
    /// never come.
    StaleSession,
    Acquired(PendingDirectCallSlot<'a>),
}

/// Result of handling one message while awaiting `HelloAck` on an outgoing call.
enum HelloResponse {
    Completed,
    SessionStopped,
    EndedWith(String),
    /// End the outgoing negotiation without notifying call ended (slot cleanup only).
    EndedSilently,
    /// Keep waiting (e.g. `KeepAlive`, ignored room reject/busy, simultaneous-dial winner).
    Continue,
}

/// Owns a direct-call pending slot from acquisition until handshake entry or explicit release.
///
/// `release_on_failure` is `false` for an incoming `Matched*` slot — the peer already holds
/// the matching pending slot (outgoing in the simultaneous-dial case) and is responsible
/// for its lifecycle. Outgoing acquisition sets it for both `Acquired` and `Matched*`.
/// The handshake path does not release the slot; it transitions the slot to active.
struct PendingDirectCallSlot<'a> {
    call_slot: &'a CallSlot,
    peer: PublicKey,
    release_on_failure: bool,
}

impl<'a> PendingDirectCallSlot<'a> {
    /// Acquires or matches an incoming direct-call pending slot for `peer`.
    fn try_acquire_incoming(call_slot: &'a CallSlot, peer: PublicKey) -> Result<Option<Self>> {
        match call_slot.try_acquire_or_match(CallSlotState::PendingIncoming, peer)? {
            CallSlotAcquireResult::Acquired => Ok(Some(Self {
                call_slot,
                peer,
                release_on_failure: true,
            })),
            // The peer already holds the matching pending slot; do not release on failure.
            CallSlotAcquireResult::MatchedPendingIncoming
            | CallSlotAcquireResult::MatchedPendingOutgoing => Ok(Some(Self {
                call_slot,
                peer,
                release_on_failure: false,
            })),
            CallSlotAcquireResult::Failed => Ok(None),
        }
    }

    /// Acquires or matches an outgoing direct-call pending slot for `peer`.
    fn try_acquire_outgoing(call_slot: &'a CallSlot, peer: PublicKey) -> Result<Option<Self>> {
        match call_slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)? {
            CallSlotAcquireResult::Acquired
            | CallSlotAcquireResult::MatchedPendingIncoming
            | CallSlotAcquireResult::MatchedPendingOutgoing => Ok(Some(Self {
                call_slot,
                peer,
                release_on_failure: true,
            })),
            CallSlotAcquireResult::Failed => Ok(None),
        }
    }

    /// Releases the pending slot when negotiation fails before handshake.
    fn release(self) -> Result<()> {
        // no-op when release_on_failure is false; peer owns the slot in the Matched-incoming case
        if self.release_on_failure {
            self.call_slot.release_if_pending_for_peer(self.peer)?;
        }
        Ok(())
    }
}

/// Outcome of [`TelepathyCore::negotiate_incoming_call`], mapped by [`TelepathyCore::session_inner`].
enum IncomingNegotiationOutcome {
    /// Handshake finished; session loop continues (`Ok(true)`).
    HandshakeComplete,
    /// Negotiation ended without handshake; session loop continues (`Ok(true)`).
    ContinueSession,
    /// Session was stopped during negotiation; exit session loop (`Ok(false)`).
    SessionStopped,
}

impl IncomingNegotiationOutcome {
    fn to_outcome(&self) -> bool {
        !matches!(self, Self::SessionStopped)
    }
}

/// Outcome of [`TelepathyCore::negotiate_outgoing_call`], mapped by [`TelepathyCore::session_inner`].
enum OutgoingNegotiationOutcome {
    /// Handshake finished; session loop continues (`Ok(true)`).
    HandshakeComplete,
    /// Call ended before handshake; session loop continues (`Ok(true)`).
    CallEnded,
    /// Session was stopped during negotiation; exit session loop (`Ok(false)`).
    SessionStopped,
}

impl OutgoingNegotiationOutcome {
    fn to_outcome(&self) -> bool {
        !matches!(self, Self::SessionStopped)
    }
}

/// Bounded exponential backoff before restarting the session manager.
/// Retry `n` waits `BASE * 2^(n-1)` milliseconds, capped at [`MANAGER_RETRY_MAX_MS`].
fn manager_retry_delay_ms(retries: u32) -> u64 {
    if retries == 0 {
        return 0;
    }
    let exponent = retries.saturating_sub(1).min(63);
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    MANAGER_RETRY_BASE_MS
        .saturating_mul(multiplier)
        .min(MANAGER_RETRY_MAX_MS)
}

/// Tiebreaker for simultaneous-dial collision resolution.
///
/// When both peers send `Hello` before receiving the other's, the lower [`PublicKey`] yields
/// and accepts the incoming `Hello` (see `simultaneous_dial_detected_yielding` in
/// [`TelepathyCore::negotiate_outgoing_call`]); the higher peer keeps waiting for `HelloAck`.
fn should_keep_new_session(local_peer: &PublicKey, peer: &PublicKey, new_is_client: bool) -> bool {
    new_is_client == (local_peer < peer)
}

/// Returns `true` if `session_id` is still the current map entry for `peer` in
/// `session_states`. Used to gate call-slot releases caused by session tasks so that a
/// collision-loser cleanup cannot tear down a slot owned by a replacement session.
async fn is_session_still_current(
    session_states: &Arc<RwLock<HashMap<PublicKey, Arc<SessionState>>>>,
    peer: PublicKey,
    session_id: Uuid,
) -> bool {
    session_states
        .read()
        .await
        .get(&peer)
        .map(|s| s.id == session_id)
        .unwrap_or(false)
}

/// Releases a pending direct-call slot only if the owning session is still the current
/// map entry for `peer` in `session_states`.
///
/// A collision-loser session that is being torn down MUST NOT release a slot now owned by
/// the replacement session: the replacement session will re-arm and take ownership via
/// the `session_rearmed_pending_outgoing` path in `session_outer` and run its own
/// terminal cleanup. Calling `release_if_pending_for_peer` here would clobber that intent
/// and leave the replacement session waiting forever for a notify that never arrives.
///
/// Explicit terminal operations (e.g. `stop_session`, manager reset, shutdown) clear
/// `session_states` before invoking release, so `is_session_still_current` is `false` for
/// those paths and this function becomes a no-op as expected.
async fn release_pending(
    session_states: &Arc<RwLock<HashMap<PublicKey, Arc<SessionState>>>>,
    peer: PublicKey,
    session_id: Uuid,
    pending_slot: &mut Option<PendingDirectCallSlot<'_>>,
) -> Result<()> {
    // Two independent silent no-op conditions below MUST be preserved together:
    //
    // 1. `is_session_still_current` returns `false` -> we exit early to protect a
    //    replacement session's slot. A collision-loser / drained session must not
    //    clobber the slot now owned by its replacement.
    //
    // 2. `PendingDirectCallSlot::release_on_failure` is `false`
    //    (matched-incoming / simultaneous-dial) -> `slot.release()` is itself a
    //    no-op, because the outgoing peer owns the slot and is responsible for
    //    its lifecycle. Removing this guard (e.g. by "simplifying"
    //    `release_on_failure` to always release) would let an incoming
    //    `Matched*` session tear down the outgoing peer's slot — a silent
    //    slot-clobbering bug.
    if !is_session_still_current(session_states, peer, session_id).await {
        return Ok(());
    }

    if let Some(slot) = pending_slot.take() {
        slot.release()?;
    }
    Ok(())
}

/// Sends a session-stopped `Goodbye`, releases any pending slot only if the owning
/// session is still the current map entry, and asserts the post-condition.
async fn abort_negotiation_session_stopped(
    session_states: &Arc<RwLock<HashMap<PublicKey, Arc<SessionState>>>>,
    peer: PublicKey,
    session_id: Uuid,
    send: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    pending_slot: &mut Option<PendingDirectCallSlot<'_>>,
) -> Result<()> {
    _ = write_message(
        send,
        &ProtocolMessage::Goodbye {
            reason: Some(SESSION_STOPPED_REASON.to_string()),
        },
    )
    .await;
    release_pending(session_states, peer, session_id, pending_slot).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        MANAGER_RETRY_BASE_MS, MANAGER_RETRY_MAX_MS, manager_retry_delay_ms,
        should_keep_new_session,
    };
    use iroh::SecretKey;

    #[test]
    fn manager_retry_delay_schedule_is_bounded_exponential_backoff() {
        assert_eq!(manager_retry_delay_ms(0), 0);
        assert_eq!(manager_retry_delay_ms(1), MANAGER_RETRY_BASE_MS);
        assert_eq!(manager_retry_delay_ms(2), MANAGER_RETRY_BASE_MS * 2);
        assert_eq!(manager_retry_delay_ms(3), MANAGER_RETRY_BASE_MS * 4);
        assert_eq!(manager_retry_delay_ms(4), MANAGER_RETRY_BASE_MS * 8);
        assert_eq!(manager_retry_delay_ms(6), MANAGER_RETRY_BASE_MS * 32);
        assert_eq!(manager_retry_delay_ms(7), MANAGER_RETRY_MAX_MS);
        assert_eq!(manager_retry_delay_ms(8), MANAGER_RETRY_MAX_MS);
        assert_eq!(manager_retry_delay_ms(u32::MAX), MANAGER_RETRY_MAX_MS);
    }

    #[test]
    fn session_collision_lower_peer_keeps_client_connection() {
        let first = SecretKey::generate().public();
        let second = SecretKey::generate().public();
        let (lower, higher) = if first < second {
            (first, second)
        } else {
            (second, first)
        };

        assert!(should_keep_new_session(&lower, &higher, true));
        assert!(!should_keep_new_session(&lower, &higher, false));
    }

    #[test]
    fn session_collision_higher_peer_keeps_server_connection() {
        let first = SecretKey::generate().public();
        let second = SecretKey::generate().public();
        let (lower, higher) = if first < second {
            (first, second)
        } else {
            (second, first)
        };

        assert!(!should_keep_new_session(&higher, &lower, true));
        assert!(should_keep_new_session(&higher, &lower, false));
    }
}
