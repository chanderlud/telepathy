use crate::internal::callbacks::CoreCallbacks;
use crate::internal::core::{
    OutgoingSlotDecision, PendingDirectCallSlot, RoomControllerCleanup, RoomControllerOutcome,
    TelepathyCore,
};
use crate::internal::error::{AudioStreamError, Error, ErrorKind};
use crate::internal::messages::{AudioHeader, RoomMessage};
#[cfg(not(target_family = "wasm"))]
use crate::internal::messages::{ProtocolMessage, StartScreenshare};
#[cfg(not(target_family = "wasm"))]
use crate::internal::screenshare;
use crate::internal::state::{CallSlot, EarlyCallState, StatisticsCollectorState};
#[cfg(target_os = "ios")]
use crate::internal::utils::deactivate_audio_session;
use crate::internal::utils::{JoinHandle, KanalSink, KanalSource};
use crate::internal::{ALPN, MAX_RINGTONE_LENGTH, Result};
#[cfg(not(target_family = "wasm"))]
use crate::types::FrontendNotify;
use crate::types::{ManagerState, SessionStatus};
use bytes::Bytes;
use iroh::address_lookup::PkarrPublisher;
use iroh::endpoint::{default_relay_mode, presets};
use iroh::{Endpoint, PublicKey, RelayMode, SecretKey};
#[cfg(not(target_family = "wasm"))]
use std::net::SocketAddr;
#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
#[cfg(target_family = "wasm")]
use telepathy_audio::WebAudioWrapper;
use telepathy_audio::devices::AudioHost;
use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::io::{
    AudioInputBuilder, AudioInputHandle, AudioOutputBuilder, AudioOutputHandle, CodecBitrateMode,
};
#[cfg(not(target_family = "wasm"))]
use tokio::fs::File;
#[cfg(not(target_family = "wasm"))]
use tokio::io::AsyncReadExt;
use tokio::select;
use tokio::sync::Notify;
use tokio::sync::mpsc::{Sender, UnboundedSender};
#[cfg(not(target_family = "wasm"))]
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, trace, warn};
use url::Url;
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::timeout;

const ROOM_TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

impl<C, H> TelepathyCore<C, H>
where
    C: CoreCallbacks + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    #[instrument(name = "manager.setup_endpoint", skip_all)]
    pub(crate) async fn setup_endpoint(
        &self,
        identity: &SecretKey,
        iteration_cancellation: &CancellationToken,
    ) -> Result<Option<Endpoint>> {
        trace!(event = "endpoint_launch", config = ?self.core_state.network_config);
        select! {
            biased;
            _ = self.core_state.stop_manager.cancelled() => return Ok(None),
            _ = iteration_cancellation.cancelled() => return Ok(None),
            _ = self.callbacks.manager_state(ManagerState::Starting) => (),
        }

        let mut endpoint_builder = Endpoint::builder(presets::Empty)
            .secret_key(identity.clone())
            .alpns(vec![ALPN.to_vec()])
            .relay_mode(default_relay_mode());

        let pkarr_relay_value: Option<Url> = self
            .core_state
            .network_config
            .pkarr_relay
            .read()
            .map_err(|_| ErrorKind::Poison("pkarr_relay"))?
            .clone();

        if let Some(ref relay) = pkarr_relay_value {
            endpoint_builder =
                endpoint_builder.address_lookup(PkarrPublisher::builder(relay.clone()));
        } else {
            endpoint_builder = endpoint_builder.address_lookup(PkarrPublisher::n0_dns());
        }

        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                use rustls::crypto::ring;
                use iroh::address_lookup::PkarrResolver;

                let provider = ring::default_provider();
                endpoint_builder = endpoint_builder.crypto_provider(Arc::new(provider));

                if let Some(relay) = pkarr_relay_value {
                    endpoint_builder = endpoint_builder.address_lookup(PkarrResolver::builder(relay.clone()));
                } else {
                    endpoint_builder = endpoint_builder.address_lookup(PkarrResolver::n0_dns());
                }
            } else {
                use rustls::crypto::aws_lc_rs::{self, kx_group};
                use iroh::address_lookup::DnsAddressLookup;
                use iroh::dns::DnsResolver;

                let mut provider = aws_lc_rs::default_provider();
                provider.kx_groups = vec![
                    kx_group::X25519MLKEM768,
                    kx_group::X25519,
                    kx_group::SECP256R1,
                    kx_group::SECP384R1,
                ];

                endpoint_builder = endpoint_builder
                    .clear_ip_transports()
                    .crypto_provider(Arc::new(provider));

                let listen_port = self.core_state.network_config.listen_port.load(Relaxed);
                let bind_addresses = self
                    .core_state
                    .network_config
                    .bind_addresses
                    .read()
                    .map_err(|_| ErrorKind::Poison("bind_addresses"))?
                    .clone();

                for ip in bind_addresses {
                    endpoint_builder = endpoint_builder
                        .bind_addr(SocketAddr::new(ip, listen_port))?;
                }

                let dns_endpoint = *self
                    .core_state
                    .network_config
                    .dns_endpoint
                    .read()
                    .map_err(|_| ErrorKind::Poison("dns_endpoint"))?;

                let dns_origin_domain = self
                    .core_state
                    .network_config
                    .dns_origin_domain
                    .read()
                    .map_err(|_| ErrorKind::Poison("dns_origin_domain"))?.clone();

                if let (Some(endpoint), Some(origin_domain)) = (dns_endpoint, dns_origin_domain) {
                    let resolver = DnsResolver::with_nameserver(endpoint);
                    endpoint_builder = endpoint_builder.address_lookup(
                        DnsAddressLookup::builder(origin_domain)
                            .dns_resolver(resolver)
                            .build()
                    );
                } else {
                    endpoint_builder = endpoint_builder.address_lookup(DnsAddressLookup::n0_dns());
                }
            }
        }

        if let Some(ref relays) = *self
            .core_state
            .network_config
            .relays
            .read()
            .map_err(|_| ErrorKind::Poison("relays"))?
        {
            // Keep endpoint relay identity exactly aligned with NetworkConfig so PKARR
            // advertisements use one canonical relay URL per physical relay service.
            endpoint_builder = endpoint_builder.relay_mode(RelayMode::Custom(relays.clone()));
        }

        #[cfg(feature = "integration-testing")]
        {
            // Integration tests opt out of the n0 PKARR / DNS address discovery path
            // and register a shared in-process `MemoryLookup` instead.
            let lookup = self
                .core_state
                .network_config
                .address_lookup
                .read()
                .map_err(|_| ErrorKind::Poison("address_lookup"))?
                .clone();

            if let Some(lookup) = lookup {
                endpoint_builder = endpoint_builder
                    .clear_address_lookup()
                    .address_lookup(lookup.clone());
            }

            // Integration tests use a local relay without signed certificates
            endpoint_builder =
                endpoint_builder.ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify());
        }

        let endpoint = select! {
            biased;
            _ = self.core_state.stop_manager.cancelled() => return Ok(None),
            _ = iteration_cancellation.cancelled() => return Ok(None),
            endpoint = endpoint_builder.bind() => endpoint?,
        };

        // Register this endpoint's own `addr()` (relay URL + direct addrs) into
        // the shared `MemoryLookup` so other in-process peers can resolve it.
        #[cfg(feature = "integration-testing")]
        {
            if let Some(shared_lookup) = self
                .core_state
                .network_config
                .address_lookup
                .read()
                .map_err(|_| ErrorKind::Poison("address_lookup"))?
                .clone()
            {
                shared_lookup.add_endpoint_info(endpoint.addr());
            }
        }

        select! {
            biased;
            _ = self.core_state.stop_manager.cancelled() => {
                endpoint.close().await;
                return Ok(None);
            },
            _ = iteration_cancellation.cancelled() => {
                endpoint.close().await;
                return Ok(None);
            },
            _ = endpoint.online() => (),
        }

        Ok(Some(endpoint))
    }

    #[cfg(not(target_family = "wasm"))]
    #[instrument(
        name = "screenshare",
        skip_all,
        fields(
            peer.id = %message.peer,
            role = if message.header.is_some() { "receiver" } else { "sender" }
        )
    )]
    pub(crate) async fn start_screenshare(&self, message: StartScreenshare) -> Result<()> {
        let state = if let Some(s) = self.session_states.read().await.get(&message.peer) {
            s.clone()
        } else {
            warn!(
                "screenshare started for a peer without a session: {}",
                message.peer
            );
            return Ok(());
        };

        let stop = Arc::new(Notify::new());
        *state.stop_screenshare.lock().await = Some(stop.clone());
        let dart_stop = FrontendNotify::new(&stop);

        if let Some(ProtocolMessage::ScreenshareHeader { encoder_name }) = message.header {
            // alert the frontend
            self.callbacks.screenshare_started(dart_stop, false).await;
            let stream = message.connection.accept_uni().await?;
            // start playing back the screenshare
            screenshare::playback(
                stream,
                stop,
                encoder_name,
                self.core_state.screenshare_config.width.load(Relaxed),
                self.core_state.screenshare_config.height.load(Relaxed),
            )
            .await?;
        } else {
            let config = if let Some(c) = self
                .core_state
                .screenshare_config
                .recording_config
                .read()
                .await
                .as_ref()
            {
                c.clone()
            } else {
                // the frontend blocks this case
                warn!("screenshare started without recording configuration");
                return Ok(());
            };

            // send the peer a screenshare header
            // the peer will open a stream after receiving it
            let result = state
                .message_sender
                .send(ProtocolMessage::ScreenshareHeader {
                    encoder_name: config.encoder.to_string(),
                })
                .await;

            if result.is_ok() {
                // alert the frontend & provide the stop object
                self.callbacks.screenshare_started(dart_stop, true).await;
                let stream = message.connection.open_uni().await?;
                // start recording the screenshare
                screenshare::record(stream, stop, config).await?;
            } else {
                warn!("giving up on screenshare start, state closed");
            }
        }

        Ok(())
    }

    /// helper method to set up audio input stack using the telepathy-audio library
    pub(crate) async fn setup_input(
        &self,
        codec_options: (bool, bool, f32),
        statistics_state: &StatisticsCollectorState,
        end_call: &Arc<Notify>,
        stream_error: UnboundedSender<AudioStreamError>,
    ) -> Result<InputHelper<H::InputStream>> {
        let (codec_enabled, vbr, residual_bits) = codec_options;
        // Channel for receiving processed audio data
        let (sender, receiver) = kanal::unbounded_async();
        let input_end_call = end_call.clone();

        let input_device_id = self.core_state.input_device.lock().await.clone();

        let mut builder = AudioInputBuilder::new()
            .device(input_device_id)
            .input_volume_shared(self.core_state.get_input_volume())
            .rms_threshold_shared(self.core_state.get_rms_threshold())
            .muted_shared(&self.core_state.muted)
            .rms_shared(&statistics_state.input_rms)
            .on_error(move |error| {
                error!(error = %error, "input_stream_error");
                report_stream_error(
                    &stream_error,
                    &input_end_call,
                    AudioStreamError::input(error.to_string()),
                );
            })
            .sink(KanalSink::new(sender));

        if codec_enabled {
            builder = builder.codec(
                if vbr {
                    CodecBitrateMode::Vbr
                } else {
                    CodecBitrateMode::Cbr
                },
                residual_bits,
            )
        }

        if self.core_state.denoise.load(Relaxed) {
            builder = builder.denoise(self.core_state.denoise_model.read().await.clone());
        }

        #[cfg(target_family = "wasm")]
        {
            let wrapper = self
                .web_input
                .lock()
                .await
                .take()
                .expect("web audio wrapper was not initialized");

            builder = builder.web_audio_wrapper(wrapper);
        }

        Ok(InputHelper::new(builder.build(&self.host)?, receiver))
    }

    /// helper method to set up audio output stack using the telepathy-audio library
    pub(crate) async fn setup_output(
        &self,
        peer: PublicKey,
        remote_sample_rate: f64,
        codec_enabled: bool,
        statistics_state: &StatisticsCollectorState,
        end_call: Arc<Notify>,
        stream_error: UnboundedSender<AudioStreamError>,
    ) -> Result<OutputHelper<H::OutputStream>> {
        let device_id = self.core_state.output_device.lock().await.clone();
        // Create the input channel
        let (sender, receiver) = kanal::unbounded();
        // Get the shared volume multiplier
        let output_volume = self.core_state.output_volume_for_peer(peer)?;
        // Create the audio output using the builder
        let handle = AudioOutputBuilder::new()
            .source(KanalSource::new(receiver))
            .device(device_id)
            .sample_rate(remote_sample_rate as u32)
            .output_volume_shared(&output_volume)
            .deafened_shared(&self.core_state.deafened)
            .rms_shared(&statistics_state.output_rms)
            .loss_shared(&statistics_state.loss)
            .codec(codec_enabled)
            .on_error(move |error| {
                error!(error = %error, "output_stream_error");
                report_stream_error(
                    &stream_error,
                    &end_call,
                    AudioStreamError::output(error.to_string()),
                );
            })
            .build(&self.host)?;

        Ok(OutputHelper::new(handle, sender))
    }

    /// helper method to set up EarlyCallState
    pub(crate) async fn setup_call(&self, peer: PublicKey) -> Result<EarlyCallState> {
        // if there is an early room state, use it w/ the real peer id
        if let Some(mut state) = self
            .room_state
            .read()
            .await
            .as_ref()
            .map(|s| s.early_state.clone())
        {
            state.peer = peer;
            return Ok(state);
        }

        // rnnoise requires a 48kHz sample rate
        let sample_rate = if self.core_state.denoise.load(Relaxed) {
            48_000
        } else {
            cfg_if::cfg_if! {
                if #[cfg(target_family = "wasm")] {
                     self
                        .web_input
                        .lock()
                        .await
                        .as_ref()
                        .expect("web audio wrapper was not initialized")
                        .sample_rate as u32
                } else {
                    let device_id = self.core_state.input_device.lock().await;
                    self.host.input_sample_rate(device_id.as_deref())?
                }
            }
        };

        Ok(EarlyCallState {
            peer,
            local_configuration: AudioHeader {
                sample_rate,
                codec_enabled: self.core_state.codec_config.enabled.load(Relaxed),
                vbr: self.core_state.codec_config.vbr.load(Relaxed),
                residual_bits: self.core_state.codec_config.residual_bits.load(Relaxed) as f64,
            },
            remote_configuration: AudioHeader::default(),
        })
    }

    /// helper method to load pre-encoded ringtone bytes
    pub(crate) async fn load_ringtone(&self) -> Option<Vec<u8>> {
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                None
            } else {
                if !self.core_state.send_custom_ringtone.load(Relaxed) {
                    return None;
                }
                let path = PathBuf::from("ringtone.sea");
                if !path.exists() {
                    None
                } else {
                    match File::open("ringtone.sea").await {
                        Ok(file) => {
                            let mut buffer = Vec::new();

                            if let Err(error) = file
                                .take((MAX_RINGTONE_LENGTH + 1) as u64)
                                .read_to_end(&mut buffer)
                                .await
                            {
                                error!("failed to read ringtone: {:?}", error);
                                None
                            } else if buffer.len() > MAX_RINGTONE_LENGTH {
                                warn!(
                                    event = "custom_ringtone_too_large",
                                    size = buffer.len(),
                                    limit = MAX_RINGTONE_LENGTH
                                );
                                None
                            } else {
                                Some(buffer)
                            }
                        }
                        Err(error) => {
                            error!("failed to open ringtone: {:?}", error);
                            None
                        }
                    }
                }
            }
        }
    }

    /// Returns the generation of the currently installed `RoomState`, or
    /// `None` if no room is active.
    pub async fn current_room_generation(&self) -> Option<u64> {
        self.room_state.read().await.as_ref().map(|s| s.generation)
    }

    /// helper method to check if a peer is in the current room
    pub(crate) async fn is_in_room(&self, peer_id: &PublicKey) -> bool {
        self.room_state
            .read()
            .await
            .as_ref()
            .map(|m| m.peers.contains(peer_id))
            .unwrap_or(false)
    }

    pub(crate) async fn room_hash(&self) -> Option<u64> {
        self.room_state
            .read()
            .await
            .as_ref()
            .map(|state| state.room_hash())
    }

    /// Atomic snapshot of `(local_room_hash, is_in_room_for_peer, room_generation)`
    pub(crate) async fn room_snapshot_for_peer(
        &self,
        peer_id: &PublicKey,
    ) -> RoomNegotiationSnapshot {
        let room_guard = self.room_state.read().await;
        match room_guard.as_ref() {
            Some(state) => RoomNegotiationSnapshot {
                local_room_hash: Some(state.room_hash()),
                is_in_room: state.peers.contains(peer_id),
                room_generation: state.generation,
            },
            None => RoomNegotiationSnapshot {
                local_room_hash: None,
                is_in_room: false,
                room_generation: 0,
            },
        }
    }

    /// Atomic snapshot of `(sender, cancel)` for the current `room_state`,
    /// or `None` if no room is currently active.
    pub(crate) async fn room_handshake_snapshot_for_peer(
        &self,
        peer: &PublicKey,
        expected_room_hash: u64,
    ) -> Option<(Sender<RoomMessage>, CancellationToken)> {
        self.room_state
            .read()
            .await
            .as_ref()
            .filter(|state| state.peers.contains(peer) && state.room_hash() == expected_room_hash)
            .map(|s| (s.sender.clone(), s.cancel.clone()))
    }

    pub(crate) async fn peer_id(&self) -> PublicKey {
        if let Some(keypair) = self.core_state.identity.read().await.as_ref() {
            keypair.public()
        } else {
            SecretKey::generate().public()
        }
    }

    pub async fn shutdown(&self) {
        self.core_state.stop_manager.cancel();
        self.reset_sessions().await;
    }

    /// Inserts a new outbound attempt
    pub(crate) async fn begin_outbound_attempt(&self, peer: PublicKey) -> u64 {
        let mut attempts = self.outbound_attempts.write().await;
        let generation = attempts.get(&peer).map(|current| current + 1).unwrap_or(1);
        attempts.insert(peer, generation);
        generation
    }

    /// Returns the current outbound generation
    pub(crate) async fn get_outbound_generation(&self, peer: PublicKey) -> u64 {
        self.outbound_attempts
            .read()
            .await
            .get(&peer)
            .copied()
            .unwrap_or(0)
    }

    /// Emits the session status for outbound connections, checks for staleness
    pub(crate) async fn emit_outbound_status(
        &self,
        peer: PublicKey,
        generation: u64,
        status: SessionStatus,
    ) {
        let is_current_outbound_attempt = self
            .outbound_attempts
            .read()
            .await
            .get(&peer)
            .is_some_and(|current| *current == generation);

        if !is_current_outbound_attempt {
            debug!(
                event = "outbound_session_status_stale",
                peer.id = %peer,
                generation,
                ?status
            );
            return;
        }

        if matches!(status, SessionStatus::Inactive)
            && self.session_states.read().await.contains_key(&peer)
        {
            debug!(
                event = "outbound_session_status_suppressed_active_session",
                peer.id = %peer,
                generation
            );
            return;
        }

        self.callbacks.session_status(status, peer).await;
    }

    /// Emits the inactive session status, checking for newer sessions and staleness
    pub(crate) async fn emit_inactive(&self, peer: PublicKey, session_generation: u64) {
        let has_newer_outbound_attempt = self
            .outbound_attempts
            .read()
            .await
            .get(&peer)
            .copied()
            .unwrap_or(0)
            > session_generation;

        if has_newer_outbound_attempt {
            debug!(
                event = "session_inactive_stale_outbound_attempt",
                peer.id = %peer,
                session_generation
            );
            return;
        }

        if self.session_states.read().await.contains_key(&peer) {
            debug!(
                event = "session_inactive_suppressed_active_session",
                peer.id = %peer,
                session_generation
            );
            return;
        }

        self.callbacks
            .session_status(SessionStatus::Inactive, peer)
            .await;
    }

    #[cfg(target_family = "wasm")]
    pub(crate) async fn init_web_audio(&self) -> Result<()> {
        let wrapper = WebAudioWrapper::new().await?;
        *self.web_input.lock().await = Some(wrapper);
        Ok(())
    }

    /// Ends all sessions & restores session_states to default
    pub(crate) async fn reset_sessions(&self) {
        // Drain `session_states` under the write lock so no session task can
        // re-acquire a pending direct-call slot for a peer whose session is no
        // longer the current map entry.
        let sessions: Vec<_> = {
            let mut states = self.session_states.write().await;
            states.drain().map(|(_, session)| session).collect()
        };

        self.cancel_pending_session_candidates();

        for session in &sessions {
            session.teardown().await;
        }

        // Terminal barrier: re-acquire the write lock so any session task that
        // raced past the drain observed an empty map and abandoned its acquisition.
        // Only then can the pending slot be cleared atomically — `Idle`/`ActiveDirect`/
        // `RoomCall`/`AudioTest` are left untouched. The barrier is a secondary
        // defense; the primary one is the `stop_session.is_cancelled()` check at the
        // entry of `negotiate_outgoing_call` and `negotiate_incoming_call`.
        {
            let _states = self.session_states.write().await;
            if let Err(error) = self.core_state.call_slot.clear_pending_direct() {
                warn!(
                    event = "reset_sessions_pending_clear_failed",
                    error = %error
                );
            }
        }

        self.outbound_attempts.write().await.clear();
        self.clear_session_availability();
    }

    pub(crate) async fn cleanup_room_controller(
        &self,
        stop_io: &CancellationToken,
        cleanup: RoomControllerCleanup<H::OutputStream>,
    ) -> RoomControllerOutcome {
        let RoomControllerCleanup {
            end_sessions,
            room_owner,
            room_generation,
            input_handle,
            connections,
            statistics_handle,
            terminal_error,
            outcome,
        } = cleanup;

        debug!(event = "room_processing_teardown_start");
        end_sessions.cancel();
        #[cfg(target_os = "ios")]
        deactivate_audio_session();
        #[cfg(target_family = "wasm")]
        {
            *self.web_input.lock().await = None;
        }
        stop_io.cancel();
        let mut terminal_error = terminal_error;
        if let Some(error) =
            join_room_io_tasks_bounded(input_handle, "room_input", "teardown").await
        {
            record_room_terminal_error(&mut terminal_error, error);
        }
        if let Some(error) = join_room_io_tasks_bounded(
            connections
                .into_values()
                .map(|connection| connection.handle),
            "room_output",
            "teardown",
        )
        .await
        {
            record_room_terminal_error(&mut terminal_error, error);
        }
        debug!(event = "room_processing_teardown_done");
        // Clear `room_state` only if it's still the currently installed generation.
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
        self.request_room_reconcile();
        // Release the slot only against the exact `room_owner` snapshot.
        if let Err(error) = self.core_state.call_slot.release_if_match(room_owner) {
            warn!(event = "room_call_slot_release_failed_on_teardown", ?error);
            record_room_terminal_error(&mut terminal_error, error);
        }
        if let Some(mut statistics_handle) = statistics_handle {
            match timeout(ROOM_TASK_JOIN_TIMEOUT, &mut statistics_handle).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!(event = "room_statistics_join_failed_on_teardown", ?error);
                    record_room_terminal_error(&mut terminal_error, error.into());
                }
                Err(error) => {
                    abort_room_task(&statistics_handle);
                    warn!(event = "room_statistics_join_timed_out", ?error);
                    record_room_terminal_error(&mut terminal_error, error.into());
                }
            }
        }
        if let Some(error) = terminal_error {
            error!(event = "room_controller_terminated_with_error", ?error);
        }
        outcome
    }

    /// Races delivery of a frontend observation callback against any teardown
    /// signal so a stalled frontend delivery cannot block teardown paths that
    /// depend on the controller exiting.
    ///
    /// `stop_signals` carries every cancellation token the controller must
    /// honor in addition to `end_call`: direct calls pass `[&state.stop_session]`,
    /// room calls pass `[&end_sessions, &operation]`.
    ///
    /// Returns `true` when the callback completed normally. Returns `false`
    /// when any teardown signal won; callers must then skip further observation
    /// work and proceed straight to authoritative teardown.
    pub(crate) async fn deliver_callback_against_teardown<F>(
        &self,
        end_call: &Notify,
        stop_signals: &[&CancellationToken],
        callback: F,
    ) -> bool
    where
        F: Future<Output = ()> + Send,
    {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::{Context, Poll};

        // Box so a runtime-sized slice can be raced uniformly. The boxed future
        // borrows each token for the lifetime of this call.
        let mut cancelled_futures: Vec<Pin<Box<dyn Future<Output = ()> + Send + '_>>> =
            stop_signals
                .iter()
                .map(|signal| {
                    Box::pin(signal.cancelled()) as Pin<Box<dyn Future<Output = ()> + Send + '_>>
                })
                .collect();

        // Do not switch back to `std::pin::pin!`: under edition 2024 it
        // expands to `super let`, which `flutter_rust_bridge_codegen`'s
        // bundled `syn` cannot parse, blocking all pub-API codegen cycles.
        let mut end_call_future = Box::pin(end_call.notified());
        let mut callback = Box::pin(callback);

        // Biased: end_call -> each token -> callback. Teardown always wins over
        // a stalled callback; wakers from any branch re-arm this poll_fn.
        std::future::poll_fn(move |cx: &mut Context<'_>| -> Poll<bool> {
            if end_call_future.as_mut().poll(cx).is_ready() {
                return Poll::Ready(false);
            }
            for cancelled in &mut cancelled_futures {
                if cancelled.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(false);
                }
            }
            if callback.as_mut().poll(cx).is_ready() {
                return Poll::Ready(true);
            }
            Poll::Pending
        })
        .await
    }

    /// Atomically validates a direct-call session and acquires its outgoing slot.
    pub(crate) async fn acquire_outgoing_call_slot<'a>(
        &self,
        call_slot: &'a CallSlot,
        peer: PublicKey,
        session_id: Uuid,
        stop_session: &CancellationToken,
    ) -> Result<OutgoingSlotDecision<'a>> {
        // Keep session-map read lock through synchronous slot acquisition.
        // `reset_sessions` needs map write lock before terminal slot clearing, so
        // it drains this session before validation or clears after acquisition.
        // No await occurs while either lock is held.
        let states = self.session_states.read().await;
        if stop_session.is_cancelled() {
            return Ok(OutgoingSlotDecision::SessionStopped);
        }
        if states.get(&peer).is_none_or(|state| state.id != session_id) {
            return Ok(OutgoingSlotDecision::StaleSession);
        }

        match PendingDirectCallSlot::try_acquire_outgoing(call_slot, peer)? {
            Some(slot) => Ok(OutgoingSlotDecision::Acquired(slot)),
            None => Ok(OutgoingSlotDecision::Busy),
        }
    }
}

/// Bounded join classification for room tasks.
///
/// `PeerLocal` covers expected connection closure: the task completed cleanly,
/// returned an `Err` from a peer-side condition (e.g. socket close), or was
/// aborted after exceeding [`ROOM_TASK_JOIN_TIMEOUT`]. The owning connection
/// is already removed by the caller and the task itself is aborted before
/// this verdict is returned, so a slow peer-output shutdown stays scoped to
/// that peer and never tears down the rest of the room.
///
/// `Terminal` covers genuinely unexpected failures: a panic or `JoinError`
/// observed while awaiting the handle. These can indicate corrupted shared
/// state and propagate as terminal room errors with user-visible notification.
pub(crate) enum RoomTaskOutcome {
    PeerLocal,
    Terminal(Error),
}

fn record_room_terminal_error(slot: &mut Option<Error>, error: Error) {
    if slot.is_none() {
        *slot = Some(error);
    }
}

async fn join_room_io_tasks_bounded(
    handles: impl IntoIterator<Item = JoinHandle<Result<()>>>,
    task_kind: &'static str,
    event_kind: &'static str,
) -> Option<Error> {
    let mut terminal_error = None;
    for mut handle in handles {
        if let RoomTaskOutcome::Terminal(error) =
            join_room_task_bounded(&mut handle, task_kind, event_kind).await
        {
            record_room_terminal_error(&mut terminal_error, error);
        }
    }
    terminal_error
}

fn abort_room_task<T>(handle: &JoinHandle<T>) {
    #[cfg(all(feature = "native", not(feature = "flutter")))]
    handle.abort();

    #[cfg(not(all(feature = "native", not(feature = "flutter"))))]
    let _ = handle;
}

/// Joins a room task with a bounded timeout and classifies the outcome.
pub(crate) async fn join_room_task_bounded(
    handle: &mut JoinHandle<Result<()>>,
    task_kind: &'static str,
    event_kind: &'static str,
) -> RoomTaskOutcome {
    match timeout(ROOM_TASK_JOIN_TIMEOUT, &mut *handle).await {
        Ok(Ok(Ok(()))) => RoomTaskOutcome::PeerLocal,
        Ok(Ok(Err(error))) => {
            warn!(event = %format!("{task_kind}_closed_on_{event_kind}"), ?error);
            RoomTaskOutcome::PeerLocal
        }
        Ok(Err(error)) => {
            abort_room_task(handle);
            warn!(event = %format!("{task_kind}_join_failed_on_{event_kind}"), ?error);
            RoomTaskOutcome::Terminal(error.into())
        }
        Err(error) => {
            abort_room_task(handle);
            warn!(event = %format!("{task_kind}_timed_out_on_{event_kind}"), ?error);
            RoomTaskOutcome::PeerLocal
        }
    }
}

fn report_stream_error(
    sender: &UnboundedSender<AudioStreamError>,
    end_call: &Notify,
    error: AudioStreamError,
) {
    let sent = sender.send(error).is_ok();
    if !sent {
        end_call.notify_one();
    }
}

pub(crate) struct OutputHelper<O> {
    _handle: AudioOutputHandle<O>,
    sender: Option<kanal::Sender<Bytes>>,
}

/// Atomic snapshot of room-related values
pub(crate) struct RoomNegotiationSnapshot {
    pub(crate) local_room_hash: Option<u64>,
    pub(crate) is_in_room: bool,
    pub(crate) room_generation: u64,
}

impl<O> OutputHelper<O> {
    /// Creates a new OutputHelper and stores the handle in the shared storage
    pub(crate) fn new(handle: AudioOutputHandle<O>, sender: kanal::Sender<Bytes>) -> Self {
        Self {
            _handle: handle,
            sender: Some(sender),
        }
    }

    pub(crate) fn sender(&mut self) -> kanal::Sender<Bytes> {
        self.sender.take().expect("sender already taken")
    }
}

pub(crate) struct InputHelper<I> {
    _handle: AudioInputHandle<I>,
    receiver: Option<kanal::AsyncReceiver<PooledBuffer>>,
}

impl<I> InputHelper<I> {
    /// Creates a new InputHelper and stores the handle in the shared storage
    pub(crate) fn new(
        handle: AudioInputHandle<I>,
        receiver: kanal::AsyncReceiver<PooledBuffer>,
    ) -> Self {
        Self {
            _handle: handle,
            receiver: Some(receiver),
        }
    }

    pub(crate) fn receiver(&mut self) -> kanal::AsyncReceiver<PooledBuffer> {
        self.receiver.take().expect("receiver already taken")
    }
}
