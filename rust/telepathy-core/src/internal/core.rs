use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::error::ErrorKind;
use crate::internal::helpers::OutputHelper;
use crate::internal::messages::{ProtocolMessage, RoomMessage, StartScreenshare};
use crate::internal::sockets::{SharedConnections, audio_input, audio_output, DynamicConnection, ConstConnection};
use crate::internal::state::{StatisticsCollectorState, CoreState};
use crate::internal::utils::{JoinHandle, spawn_task};
#[cfg(target_os = "ios")]
use crate::internal::utils::{configure_audio_session, deactivate_audio_session};
use crate::internal::utils::{
    loopback, read_message, statistics_collector,
    write_message,
};
use crate::internal::{EarlyCallState, HELLO_TIMEOUT, KEEP_ALIVE, RoomState, SESSION_MAX_FRAME_LENGTH, SessionState, Result, ALPN};
use crate::overlay::CONNECTED;
use crate::overlay::Overlay;
use crate::types::{
    CallState, ChatMessage, CodecConfig, Contact, NetworkConfig, ScreenshareConfig, SessionStatus,
};
use chrono::Local;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use iroh::{Endpoint, PublicKey};
use iroh::endpoint::{Connection, RecvStream, SendStream};
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
use tracing::{Instrument, Span, debug, error, field, info, instrument, trace, warn};
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::std::Instant;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::{Interval, interval, sleep_until, timeout};

pub(crate) struct TelepathyCore<C, S, H>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    /// The audio host
    pub(crate) host: H,

    /// Core state for telepathy
    pub(crate) core_state: CoreState,

    /// Tracks state for the current room
    pub(crate) room_state: Arc<RwLock<Option<RoomState>>>,

    /// Keeps track of and controls the sessions
    pub(crate) session_states: Arc<RwLock<HashMap<PublicKey, Arc<SessionState>>>>,

    /// Signals the session manager to start a new session
    pub(crate) start_session: Option<Sender<PublicKey>>,

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

impl<C, S, H> TelepathyCore<C, S, H>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    pub(crate) fn new(
        host: H,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: C,
    ) -> TelepathyCore<C, S, H> {
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
                    let result = manager_clone
                        .session_manager(&mut receive_session)
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
        start: &mut Receiver<PublicKey>,
    ) -> Result<()> {
        let setup_started = Instant::now();
        // build the endpoint & bring online
        let mut endpoint = self.setup_endpoint().await?;
        info!(
            event = "manager_endpoint_setup",
            elapsed_ms = setup_started.elapsed().as_millis() as u64
        );
        // handles to threads spawned by the session manager
        let mut handles: Vec<SessionTask> = Vec::new();
        // preload public identity
        let public_identity = self.peer_id().await;

        // alerts the UI that the manager is active
        self.callbacks.manager_active(true, true).await;
        // the manager is about to start processing events
        self.core_state.manager_active.notify_waiters();

        loop {
            select! {
                // restart the manager
                _ = self.restart_manager.notified() => {
                    break;
                }
                Some(incoming) = endpoint.accept() => {
                    info!(event = "incoming_connection", incoming = ?incoming);

                    let accepting = match incoming.accept() {
                        Ok(accepting) => accepting,
                        Err(error) => {
                            warn!(event = "accept_incoming_failed", error = %error);
                            continue;
                        }
                    };

                    match accepting.await {
                        Ok(connection) => {
                            self.initialize_session(connection.remote_id(), connection).await;
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
                    } else {
                        debug!(event = "dial_initial", peer.id = %peer_id);
                        self.callbacks.session_status(SessionStatus::Connecting, peer_id).await;
                        self.open_session(peer_id, &mut endpoint, &mut handles).await;
                        debug!(event = "dial_finished", peer.id = %peer_id);
                    }
                }
                else => {
                    warn!(event = "edge_case", case = "session_manager_else_branch");
                    break;
                },
            }
        }

        debug!(event = "manager_teardown_start");
        self.callbacks.manager_active(false, false).await;
        // reset room state
        if let Some(state) = self.room_state.write().await.take() {
            state.end_call.notify_one();
            state.cancel.cancel();
        }
        // join all sessions created in manager
        for handle in handles {
            handle.join().await?;
        }
        debug!(event = "manager_session_handles_joined");
        Ok(())
    }

    /// Called by the dialer to open a connection and session
    #[instrument(
        name = "session.open",
        skip_all,
        fields(peer.id = %peer)
    )]
    async fn open_session(
        &self,
        peer: PublicKey,
        endpoint: &mut Endpoint,
        handles: &mut Vec<SessionTask>,
    ) {
        // TODO this can take a while to timeout, which blocks the manager
        match endpoint.connect(peer, ALPN).await {
            Ok(connection) => {
                info!(event = "session_connection_opened", peer.id = %peer);
                handles.push(
                    self.initialize_session(peer, connection)
                        .await,
                );
            }
            Err(error) => {
                // TODO implement connection retries similar to original implementation
                warn!(event = "session_open_give_up", peer.id = %peer, error = %error);
                self.callbacks
                    .session_status(SessionStatus::Inactive, peer)
                    .await;
            }
        }
    }

    /// Entry point to a session that sets up state and spawns session outer
    #[instrument(name = "session.init", skip_all, fields(peer.id = %peer, session.id = field::Empty))]
    async fn initialize_session(
        &self,
        peer: PublicKey,
        connection: Connection,
    ) -> SessionTask {
        let contact_option = self.callbacks.get_contact(peer.to_vec()).await;
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
            // TODO this isn't a super ideal place to notify the frontend of the relay status and remote address because i think they can still change
            self.callbacks
                .session_status(SessionStatus::Connected { relayed: false, remote_address: "127.0.0.1".to_string() }, peer)
                .await;
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
                    .session_outer(peer, connection, state, contact, message_channel)
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
        peer: PublicKey,
        connection: Connection,
        state: Arc<SessionState>,
        contact: Contact,
        mut message_channel: (Sender<ProtocolMessage>, Receiver<ProtocolMessage>),
    ) {
        // TODO handle errors here
        let (send, recv) = if connection.side().is_client() {
            Span::current().record("session.role", "dialer");
            connection.open_bi().await.unwrap()
        } else {
            Span::current().record("session.role", "listener");
            connection.accept_bi().await.unwrap()
        };

        // controls keep alive messages
        let mut keep_alive = interval(KEEP_ALIVE);
        // the length delimited transport used for the session
        let mut send_transport = LengthDelimitedCodec::builder()
            .max_frame_length(SESSION_MAX_FRAME_LENGTH)
            .length_field_type::<u64>()
            .new_write(send);
        let mut recv_transport = LengthDelimitedCodec::builder()
            .max_frame_length(SESSION_MAX_FRAME_LENGTH)
            .length_field_type::<u64>()
            .new_read(recv);

        // the dialer for room sessions always starts a call
        if self.is_in_room(&peer).await && connection.side().is_client() {
            state.start_call.notify_one();
        }

        loop {
            let result = self
                .session_inner(
                    &contact,
                    &mut send_transport,
                    &mut recv_transport,
                    &connection,
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
        send: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
        recv: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
        connection: &Connection,
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
            result = read_message(recv) => {
                info!(event = "session_message_received", ?result);
                let mut other_ringtone = None;
                let remote_audio_header;
                let room_hash_option;

                match result? {
                    ProtocolMessage::Hello { ringtone, audio_header, room_hash } => {
                        if !audio_header.is_valid() {
                            warn!(event = "invalid_audio_header_rejected");
                            write_message(send, &ProtocolMessage::Reject).await?;
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
                    write_message(send, &ProtocolMessage::Reject).await?;
                    return Ok(true);
                } else if self.is_call_active().await {
                    // do not accept another call if already active
                    info!(event = "call_busy_sent_call_already_active");
                    write_message(send, &ProtocolMessage::Busy).await?;
                    return Ok(true);
                } else {
                    let cancel = Arc::new(Notify::new());
                    accept_handle = Some(self.callbacks.get_accept_handle(&contact.id, other_ringtone, &cancel));
                    cancel_prompt = Some(cancel);
                }

                state.in_call.store(true, Relaxed); // blocks the session from being restarted

                let cancel_prompt_clone = cancel_prompt.clone();
                let accept_future = async {
                    if let Some(accept_handle) = accept_handle {
                        select! {
                            result = accept_handle => result,
                            _ = state.start_call.notified() => {
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
                            write_message(send, &ProtocolMessage::Reject).await?;
                            return Ok(true);
                        }

                        match self.setup_call(contact.peer_id).await {
                            Ok(mut call_state) => {
                                // respond with hello ack containing audio header
                                call_state.remote_configuration = remote_audio_header;
                                write_message(send, &ProtocolMessage::HelloAck { audio_header: call_state.local_configuration.clone() }).await?;

                                if is_in_room {
                                    self.room_handshake(send, recv, connection, state, call_state).await?;
                                } else {
                                    // normal call handshake
                                    self.call_handshake(send, recv, connection, &mut message_channel.1, state, call_state).await?;
                                }

                                keep_alive.reset(); // start sending normal keep alive messages
                            }
                            Err(error) => {
                                // if the audio input setup fails, other client will be left hanging
                                error!(event = "setup_call_failed", ?error);
                                write_message(send, &ProtocolMessage::Goodbye {
                                    reason: Some("audio device error".to_string())
                                }).await?;
                                // still propagate the error
                                return Err(error);
                            }
                        }
                    }
                    result = read_message(recv) => {
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
                write_message(send, &ProtocolMessage::Hello { ringtone: other_ringtone, audio_header: call_state.local_configuration.clone(), room_hash }).await?;

                loop {
                    select! {
                        _ = state.stop_session.cancelled() => {
                            info!(event = "session_stopped_waiting_hello_ack");
                            return Ok(false);
                        }
                        _ = state.end_call.notified() => {
                            // gracefully end the call & continue the session
                            info!(event = "end_call_notified_waiting_hello_ack");
                            write_message(send, &ProtocolMessage::Goodbye { reason: None }).await?;
                            break;
                        }
                        result = timeout(hello_timeout, read_message(recv)) => {
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
                                        self.room_handshake(send, recv, connection, state, call_state).await?;
                                    } else {
                                        // normal call handshake
                                        self.call_handshake(send, recv, connection, &mut message_channel.1, state, call_state).await?;
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
                                ProtocolMessage::Hello { audio_header, .. } => {
                                    if self.peer_id().await < contact.peer_id {
                                        info!(event = "simultaneous_dial_detected_yielding");
                                        if !audio_header.is_valid() {
                                            warn!(event = "invalid_audio_header_rejected");
                                            write_message(send, &ProtocolMessage::Reject).await?;
                                            None
                                        } else {
                                            call_state.remote_configuration = audio_header;
                                            write_message(send, &ProtocolMessage::HelloAck {
                                                audio_header: call_state.local_configuration.clone()
                                            }).await?;

                                            if is_in_room {
                                                self.room_handshake(send, recv, connection, state, call_state).await?;
                                            } else {
                                                // normal call handshake
                                                self.call_handshake(send, recv, connection, &mut message_channel.1, state, call_state).await?;
                                            }

                                            keep_alive.reset(); // start sending normal keep alive messages
                                            None
                                        }
                                    } else {
                                        info!(event = "simultaneous_dial_detected_winning");
                                        continue;
                                    }
                                }
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
                write_message(send, &ProtocolMessage::KeepAlive).await?;
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
        // the call has ended
        self.core_state.in_call.store(false, Relaxed);
        // hide the overlay
        self.overlay.hide();
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
            let input_handle = spawn_task(audio_input(
                input_helper.receiver(),
                ConstConnection::new(o.connection.clone()),
                stop_io.clone(),
                upload_bandwidth,
            ));

            let output_handle = spawn_task(audio_output(
                output_helper.sender(),
                o.connection.clone(),
                stop_io.clone(),
                download_bandwidth,
                loss,
            ));

            let controller_future = self.call_controller(
                o,
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
        o: OptionalCallArgs<'_>,
        peer: PublicKey,
        end_call: &Arc<Notify>,
    ) -> Result<(Option<String>, bool)> {
        let identity = self.peer_id().await;

        CONNECTED.store(true, Relaxed);
        self.callbacks.call_state(CallState::Connected).await;

        loop {
            select! {
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
                            info!(event = "screenshare_header_received", ?message);

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
                // ends the call
                _ = end_call.notified() => {
                    write_message(o.control_send, &ProtocolMessage::Goodbye { reason: None }).await?;
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
        send: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
        recv: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
        connection: &Connection,
        state: &Arc<SessionState>,
        call_state: EarlyCallState,
    ) -> Result<()> {
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
                connection: connection.clone(),
                state: call_state,
            })
            .await
            .map_err(|_| ErrorKind::RoomStateMissing)?;

        loop {
            select! {
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
        let connection_sender = SharedConnections::default();
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
            DynamicConnection::new(connection_sender.clone()),
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
                        Some(RoomMessage::Join { connection, state }) => {
                            info!(event = "room_join_received", peer.id = %state.peer);

                            // first connection
                            if connections.is_empty() {
                                CONNECTED.store(true, Relaxed);
                                self.callbacks.call_state(CallState::Connected).await;
                            }

                            // this unwrap is safe because audio_input never panics
                            connection_sender.lock().unwrap().push((connection.clone(), Instant::now()));
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
                                connection,
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

impl<C, S, H> Clone for TelepathyCore<C, S, H>
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
    connection: &'a Connection,
    control_send: &'a mut FramedWrite<SendStream, LengthDelimitedCodec>,
    control_recv: &'a mut FramedRead<RecvStream, LengthDelimitedCodec>,
    message_receiver: &'a mut Receiver<ProtocolMessage>,
    state: &'a Arc<SessionState>,
}
