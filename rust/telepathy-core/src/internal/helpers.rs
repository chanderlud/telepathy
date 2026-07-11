use crate::types::CallState;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::core::{RoomControllerCleanup, TelepathyCore};
use crate::internal::error::{AudioStreamError, CallEndMessage, Error, ErrorKind};
use crate::internal::messages::{AudioHeader, RoomMessage};
#[cfg(not(target_family = "wasm"))]
use crate::internal::messages::{ProtocolMessage, StartScreenshare};
#[cfg(not(target_family = "wasm"))]
use crate::internal::screenshare;
use crate::internal::state::{EarlyCallState, StatisticsCollectorState};
use crate::internal::utils::{KanalSink, KanalSource};
use crate::internal::{ALPN, Result};
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
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, trace, warn};
use url::Url;

impl<C, S, H, I, O> TelepathyCore<C, S, H, I, O>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    /// builds an iroh endpoint and waits for it to come online
    #[instrument(name = "manager.setup_endpoint", skip_all)]
    pub(crate) async fn setup_endpoint(&self) -> Result<Option<Endpoint>> {
        let identity = if let Some(keypair) = self.core_state.identity.read().await.as_ref() {
            keypair.clone()
        } else {
            return Err(ErrorKind::NoIdentityAvailable.into());
        };

        trace!(event = "endpoint_launch", config = ?self.core_state.network_config);
        self.callbacks.manager_state(ManagerState::Starting).await;

        let mut endpoint_builder = Endpoint::builder(presets::Empty)
            .secret_key(identity)
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

        let endpoint = endpoint_builder.bind().await?;

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
            _ = self.restart_manager.notified() => {
                self.callbacks.manager_state(ManagerState::Stopped).await;
                Ok(None)
            },
            _ = endpoint.online() => {
                self.callbacks.manager_state(ManagerState::Active).await;
                Ok(Some(endpoint))
            }
        }
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

    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn prune_stale_input_device(&self) -> Option<String> {
        let mut device_id_guard = self.core_state.input_device.lock().await;
        prune_stale_device_id(&mut device_id_guard, "input", || {
            self.host.list_input_devices()
        })
        .await
    }

    #[cfg(target_family = "wasm")]
    pub(crate) async fn prune_stale_input_device(&self) -> Option<String> {
        self.core_state.input_device.lock().await.clone()
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn prune_stale_output_device(&self) -> Option<String> {
        let mut device_id_guard = self.core_state.output_device.lock().await;
        prune_stale_device_id(&mut device_id_guard, "output", || {
            self.host.list_output_devices()
        })
        .await
    }

    #[cfg(target_family = "wasm")]
    pub(crate) async fn prune_stale_output_device(&self) -> Option<String> {
        self.core_state.output_device.lock().await.clone()
    }

    /// helper method to set up audio input stack using the telepathy-audio library
    pub(crate) async fn setup_input(
        &self,
        codec_options: (bool, bool, f32),
        statistics_state: &StatisticsCollectorState,
        end_call: &Arc<Notify>,
        stream_error: UnboundedSender<AudioStreamError>,
    ) -> Result<InputHelper<I>> {
        let (codec_enabled, vbr, residual_bits) = codec_options;
        // Channel for receiving processed audio data
        let (sender, receiver) = kanal::unbounded_async();
        let input_end_call = end_call.clone();

        let input_device_id = self.prune_stale_input_device().await;

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
    ) -> Result<OutputHelper<O>> {
        let device_id = self.prune_stale_output_device().await;
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
                    let device_id = self.prune_stale_input_device().await;
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
                        Ok(mut file) => {
                            let mut buffer = Vec::new();

                            if let Err(error) = file.read_to_end(&mut buffer).await {
                                error!("failed to read ringtone: {:?}", error);
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
    pub(crate) async fn room_handshake_snapshot(
        &self,
    ) -> Option<(Sender<RoomMessage>, CancellationToken)> {
        self.room_state
            .read()
            .await
            .as_ref()
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
        self.reset_sessions().await;
        self.core_state.stop_manager.store(true, Relaxed);
        self.restart_manager.notify_one();
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
    }

    pub(crate) async fn cleanup_room_controller(
        &self,
        stop_io: &CancellationToken,
        cleanup: RoomControllerCleanup<O>,
    ) -> Result<()> {
        let RoomControllerCleanup {
            end_sessions,
            room_owner,
            room_generation,
            input_handle,
            connections,
            statistics_handle,
            setup_error,
        } = cleanup;

        debug!(event = "room_processing_teardown_start");
        #[cfg(target_os = "ios")]
        deactivate_audio_session();
        #[cfg(target_family = "wasm")]
        {
            *self.web_input.lock().await = None;
        }
        stop_io.cancel();
        let setup_failed = setup_error.is_some();
        if let Some(input_handle) = input_handle {
            match input_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if setup_failed => {
                    warn!(event = "room_input_closed_on_setup_failure", ?error);
                }
                Ok(Err(error)) => return Err(error),
                Err(error) if setup_failed => {
                    warn!(event = "room_input_join_failed_on_setup_failure", ?error);
                }
                Err(error) => return Err(error.into()),
            }
        }
        for connection in connections.into_values() {
            match connection.handle.await {
                Ok(Ok(())) => (),
                Ok(Err(error)) => {
                    warn!(event = "room_output_closed_on_teardown", ?error);
                }
                Err(error) if setup_failed => {
                    warn!(event = "room_output_join_failed_on_setup_failure", ?error);
                }
                Err(error) => return Err(error.into()),
            }
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
        // Release the slot only against the exact `room_owner` snapshot.
        match self.core_state.call_slot.release_if_match(room_owner) {
            Ok(_) => {}
            Err(error) if setup_failed => {
                warn!(
                    event = "room_call_slot_release_failed_on_setup_failure",
                    ?error
                );
            }
            Err(error) => return Err(error),
        }
        end_sessions.cancel();
        if let Some(statistics_handle) = statistics_handle {
            match statistics_handle.await {
                Ok(()) => {}
                Err(error) if setup_failed => {
                    warn!(
                        event = "room_statistics_join_failed_on_setup_failure",
                        ?error
                    );
                }
                Err(error) => return Err(error.into()),
            }
        }
        match setup_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub(crate) async fn notify_setup_failure(&self, error: &Error) {
        self.callbacks
            .call_state(CallState::CallEnded(
                CallEndMessage::from_error(error).into_string(),
                false,
            ))
            .await;
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

#[cfg(not(target_family = "wasm"))]
async fn prune_stale_device_id<F>(
    device_id_guard: &mut tokio::sync::MutexGuard<'_, Option<String>>,
    kind: &str,
    list_devices: F,
) -> Option<String>
where
    F: FnOnce() -> std::result::Result<
        Vec<telepathy_audio::devices::AudioDeviceInfo>,
        telepathy_audio::devices::DeviceError,
    >,
{
    let id = device_id_guard.clone()?;
    let still_present = match list_devices() {
        Ok(devices) => devices.iter().any(|device| device.id == id),
        Err(error) => {
            warn!(
                event = format!("{kind}_device_enumeration_skipped"),
                error = %error,
                "treating saved device as present because enumeration failed"
            );
            true
        }
    };

    if !still_present {
        warn!(event = format!("stale_{kind}_device_cleared"), id = %id);
        **device_id_guard = None;
        return None;
    }

    Some(id)
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

#[cfg(test)]
#[cfg(not(target_family = "wasm"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use telepathy_audio::devices::DeviceError;
    use tokio::sync::Mutex;

    fn device_info(id: &str) -> telepathy_audio::devices::AudioDeviceInfo {
        telepathy_audio::devices::AudioDeviceInfo {
            name: id.to_string(),
            id: id.to_string(),
        }
    }

    #[tokio::test]
    async fn prune_clears_unknown_id() {
        let mutex = Mutex::new(Some("missing".to_string()));
        let mut guard = mutex.lock().await;
        let list = vec![device_info("present")];

        let pruned = prune_stale_device_id(&mut guard, "input", || Ok(list.clone())).await;

        assert!(pruned.is_none(), "stale ID should be cleared");
        assert!(
            guard.is_none(),
            "in-memory state should reflect the cleared selection"
        );
    }

    #[tokio::test]
    async fn prune_keeps_known_id() {
        let mutex = Mutex::new(Some("present".to_string()));
        let mut guard = mutex.lock().await;
        let list = vec![device_info("present")];

        let pruned = prune_stale_device_id(&mut guard, "input", || Ok(list.clone())).await;

        assert_eq!(pruned.as_deref(), Some("present"));
        assert_eq!(guard.as_deref(), Some("present"));
    }

    #[tokio::test]
    async fn prune_with_no_saved_id_is_noop() {
        let mutex = Mutex::new(None);
        let mut guard = mutex.lock().await;
        let list: Vec<telepathy_audio::devices::AudioDeviceInfo> = vec![];

        let pruned = prune_stale_device_id(&mut guard, "input", || Ok(list.clone())).await;

        assert!(pruned.is_none());
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn prune_treats_enumeration_failure_as_present() {
        let mutex = Mutex::new(Some("saved".to_string()));
        let mut guard = mutex.lock().await;

        let pruned = prune_stale_device_id(&mut guard, "input", || {
            Err(DeviceError::NoDefaultDevice {
                direction: telepathy_audio::devices::DeviceDirection::Input,
            })
        })
        .await;

        assert_eq!(
            pruned.as_deref(),
            Some("saved"),
            "enumeration failure must not clear the saved ID"
        );
        assert_eq!(guard.as_deref(), Some("saved"));
    }

    #[tokio::test]
    async fn prune_clears_shared_arc_mutex_state() {
        let shared: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Some("missing".to_string())));
        let mut guard = shared.lock().await;

        prune_stale_device_id(&mut guard, "input", || Ok(vec![])).await;
        drop(guard);

        assert!(
            shared.lock().await.is_none(),
            "the shared state must reflect the cleared ID"
        );
    }
}
