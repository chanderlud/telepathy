#[cfg(target_family = "wasm")]
use crate::audio::web_audio::WebAudioWrapper;
use crate::error::ErrorKind;
use crate::flutter::DartNotify;
use crate::flutter::callbacks::{FrbCallbacks, FrbStatisticsCallback};
use crate::telepathy::core::{ActiveInputHandle, ActiveOutputHandles, TelepathyCore};
use crate::telepathy::messages::{AudioHeader, Message};
#[cfg(not(target_family = "wasm"))]
use crate::telepathy::screenshare;
use crate::telepathy::{
    CHAT_PROTOCOL, EarlyCallState, Result, StartScreenshare, StatisticsCollectorState,
};
use crate::{Behaviour, BehaviourEvent};
#[cfg(not(target_family = "wasm"))]
use cpal::traits::DeviceTrait;
use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
#[cfg(not(target_family = "wasm"))]
use libp2p::tcp;
use libp2p::{Multiaddr, PeerId, Swarm, autonat, dcutr, identify, noise, ping, yamux};
use libp2p_stream::Control;
use log::{debug, error, info, warn};
use sea_codec::ProcessorMessage;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use telepathy_audio::{AudioInputBuilder, AudioInputHandle, AudioOutputBuilder, AudioOutputHandle};
#[cfg(not(target_family = "wasm"))]
use tokio::fs::File;
#[cfg(not(target_family = "wasm"))]
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;
use uuid::Uuid;

impl<C, S> TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
{
    /// builds a p2p swarm & connects to the relay server
    pub(crate) async fn setup_swarm(&self) -> Result<(Swarm<Behaviour>, Multiaddr)> {
        let identity = if let Some(keypair) = self.core_state.identity.read().await.as_ref() {
            keypair.clone()
        } else {
            return Err(ErrorKind::NoIdentityAvailable.into());
        };

        let builder = libp2p::SwarmBuilder::with_existing_identity(identity);

        #[cfg(not(target_family = "wasm"))]
        let provider_phase = builder
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )
            .map_err(|_| ErrorKind::SwarmBuild)?
            .with_quic();

        #[cfg(target_family = "wasm")]
        let provider_phase = builder
            .with_wasm_bindgen()
            .with_other_transport(|id_keys| {
                Ok(libp2p_webtransport_websys::Transport::new(
                    libp2p_webtransport_websys::Config::new(id_keys),
                ))
            })?;

        let mut swarm = provider_phase
            .with_relay_client(noise::Config::new, yamux::Config::default)
            .map_err(|_| ErrorKind::SwarmBuild)?
            .with_behaviour(|keypair, relay_behaviour| Behaviour {
                relay_client: relay_behaviour,
                ping: ping::Behaviour::new(ping::Config::new()),
                identify: identify::Behaviour::new(identify::Config::new(
                    CHAT_PROTOCOL.to_string(),
                    keypair.public(),
                )),
                dcutr: dcutr::Behaviour::new(keypair.public().to_peer_id()),
                stream: libp2p_stream::Behaviour::new(),
                auto_nat: autonat::Behaviour::new(
                    keypair.public().to_peer_id(),
                    autonat::Config::default(),
                ),
            })
            .map_err(|_| ErrorKind::SwarmBuild)?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();

        let listen_port = self.core_state.network_config.listen_port.load(Relaxed);
        for bind_address in &self.core_state.network_config.bind_addresses {
            #[cfg(not(target_family = "wasm"))]
            {
                let listen_addr_quic = Multiaddr::empty()
                    .with(Protocol::from(*bind_address))
                    .with(Protocol::Udp(listen_port))
                    .with(Protocol::QuicV1);

                swarm.listen_on(listen_addr_quic)?;

                let listen_addr_tcp = Multiaddr::empty()
                    .with(Protocol::from(*bind_address))
                    .with(Protocol::Tcp(listen_port));

                swarm.listen_on(listen_addr_tcp)?;
            }

            #[cfg(target_family = "wasm")]
            {
                let listen_addr = Multiaddr::empty()
                    .with(Protocol::from(*bind_address))
                    .with(Protocol::Udp(listen_port))
                    .with(Protocol::QuicV1)
                    .with(Protocol::WebTransport);

                swarm.listen_on(listen_addr)?;
            }
        }

        let socket_address = *self.core_state.network_config.relay_address.read().await;
        let relay_identity = *self.core_state.network_config.relay_id.read().await;

        #[cfg(not(target_family = "wasm"))]
        let relay_address = {
            let relay_address_udp = Multiaddr::from(socket_address.ip())
                .with(Protocol::Udp(socket_address.port()))
                .with(Protocol::QuicV1)
                .with_p2p(relay_identity)
                .map_err(|_| ErrorKind::SwarmBuild)?;

            let relay_address_tcp = Multiaddr::from(socket_address.ip())
                .with(Protocol::Tcp(socket_address.port()))
                .with_p2p(relay_identity)
                .map_err(|_| ErrorKind::SwarmBuild)?;

            if swarm.dial(relay_address_udp.clone()).is_err() {
                swarm.dial(relay_address_tcp.clone())?;
                info!("connected to relay with tcp");
                relay_address_tcp.with(Protocol::P2pCircuit)
            } else {
                info!("connected to relay with udp");
                relay_address_udp.with(Protocol::P2pCircuit)
            }
        };

        #[cfg(target_family = "wasm")]
        let relay_address = {
            // TODO the relay currently does not support WebTransport
            let address = Multiaddr::from(socket_address.ip())
                .with(Protocol::Udp(socket_address.port()))
                .with(Protocol::QuicV1)
                .with(Protocol::WebTransport)
                .with_p2p(relay_identity)
                .map_err(|_| ErrorKind::SwarmBuild)?;

            swarm.dial(address.clone())?;
            info!("connected to relay with webtransport");
            address.with(Protocol::P2pCircuit)
        };

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
        Ok((swarm, relay_address))
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn start_screenshare(
        &self,
        message: StartScreenshare,
        control_option: Option<Control>,
    ) -> Result<()> {
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
        let dart_stop = DartNotify {
            inner: stop.clone(),
        };

        if let Some(Message::ScreenshareHeader { encoder_name }) = message.header
            && let Some(mut control) = control_option
        {
            // the other peer is waiting for a stream
            let stream = control.open_stream(message.peer, CHAT_PROTOCOL).await?;
            // alert the frontend
            self.callbacks.screenshare_started(dart_stop, false).await;
            // start playing back the screenshare
            screenshare::playback(
                stream,
                stop,
                state.download_bandwidth.clone(),
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
                .send(Message::ScreenshareHeader {
                    encoder_name: config.encoder.to_string(),
                })
                .await;

            if result.is_ok() {
                // wait for the other peer to open a stream
                let stream = state.receive_stream().await?;
                // alert the frontend & provide the stop object
                self.callbacks.screenshare_started(dart_stop, true).await;
                // start recording the screenshare
                screenshare::record(stream, stop, state.upload_bandwidth.clone(), config).await?;
            } else {
                warn!("giving up on screenshare start, state closed");
            }
        }

        Ok(())
    }

    /// helper method to set up audio input stack using the telepathy_audio library
    pub(crate) async fn setup_input(
        &self,
        codec_options: (bool, bool, f32),
        statistics_state: &StatisticsCollectorState,
        is_room: bool,
        end_call: Arc<Notify>,
    ) -> Result<InputHelper> {
        let (codec_enabled, vbr, residual_bits) = codec_options;
        let denoise = self.core_state.denoise.load(Relaxed);

        // Get denoise model bytes if using custom model
        let denoise_model = if denoise {
            Some(self.core_state.denoise_model.read().await.clone())
        } else {
            None
        };

        // Get device ID
        let device_id = self.core_state.input_device.lock().await.clone();

        // Create a channel for receiving processed audio data
        let (sender, receiver) = kanal::unbounded_async::<ProcessorMessage>();
        let sender_sync = sender.to_sync();

        // Store statistics RMS sender (currently unused, but reserved for future use)
        let _input_rms = statistics_state.input_rms.clone();

        let builder = AudioInputBuilder::new()
            .device(device_id)
            .denoise(denoise, denoise_model)
            .input_volume_shared(&self.core_state.input_volume)
            .rms_threshold_shared(&self.core_state.rms_threshold)
            .muted_shared(&self.core_state.muted)
            .codec(codec_enabled, vbr, residual_bits)
            .room(is_room)
            .on_error(end_call)
            .callback(move |data| {
                // Convert Bytes to ProcessorMessage
                let message = ProcessorMessage::Data(data);
                let _ = sender_sync.send(message);
            });

        #[cfg(not(target_family = "wasm"))]
        let handle = builder.build(&self.host)?;
        #[cfg(target_family = "wasm")]
        let handle = builder.build_async(&self.host).await?;

        // Create InputHelper which stores the handle in CoreState for live control
        Ok(InputHelper::new(
            handle,
            self.core_state.active_input_handle.clone(),
            receiver,
        ))
    }

    /// helper method to set up audio output stack using the telepathy_audio library
    pub(crate) async fn setup_output(
        &self,
        remote_sample_rate: f64,
        codec_enabled: bool,
        statistics_state: &StatisticsCollectorState,
        is_room: bool,
        end_call: Arc<Notify>,
    ) -> Result<OutputHelper> {
        // Get device ID
        let device_id = self.core_state.output_device.lock().await.clone();

        // In rooms, the SEA header is hard coded
        let header = is_room.then_some(telepathy_audio::SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 960,
            frames_per_chunk: 480,
            sample_rate: remote_sample_rate as u32,
        });

        // Create the audio output using the builder
        let handle = AudioOutputBuilder::new()
            .device(device_id)
            .sample_rate(remote_sample_rate as u32)
            .output_volume_shared(&self.core_state.output_volume)
            .deafened_shared(&self.core_state.deafened)
            .codec(codec_enabled, header)
            .on_error(end_call)
            .build(&self.host)?;

        // Store the loss receiver for statistics
        let loss = handle.loss_receiver();
        // Update the statistics state with the loss receiver
        statistics_state.loss.store(loss.load(Relaxed), Relaxed);

        // Create OutputHelper which stores the handle in CoreState for live control
        Ok(OutputHelper::new(
            handle,
            self.core_state.active_output_handles.clone(),
        ))
    }

    /// helper method to set up EarlyCallState
    pub(crate) async fn setup_call(&self, peer: PeerId) -> Result<EarlyCallState> {
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

        let input_sample_rate;

        #[cfg(not(target_family = "wasm"))]
        {
            // Query sample rate from input device using library
            let device_id = self.core_state.input_device.lock().await.clone();
            let device_handle =
                telepathy_audio::get_input_device(&self.host, device_id.as_deref())?;
            let device = device_handle.device();
            let config = device.default_input_config()?;
            input_sample_rate = config.sample_rate();
            info!("input_device: {:?}", device_handle.name());
        }

        #[cfg(target_family = "wasm")]
        {
            if let Some(web_input) = self.web_input.lock().await.as_ref() {
                input_sample_rate = web_input.sample_rate as u32;
            } else {
                return Err(ErrorKind::NoInputDevice.into());
            }
        }

        // load the shared codec config values
        let config_codec_enabled = self.core_state.codec_config.enabled.load(Relaxed);
        let config_vbr = self.core_state.codec_config.vbr.load(Relaxed);
        let config_residual_bits = self.core_state.codec_config.residual_bits.load(Relaxed);

        let mut local_configuration = AudioHeader {
            sample_rate: input_sample_rate,
            codec_enabled: config_codec_enabled,
            vbr: config_vbr,
            residual_bits: config_residual_bits as f64,
        };

        // rnnoise requires a 48kHz sample rate
        if self.core_state.denoise.load(Relaxed) {
            local_configuration.sample_rate = 48_000;
        }

        Ok(EarlyCallState {
            peer,
            local_configuration,
            remote_configuration: AudioHeader::default(),
        })
    }

    /// helper method to load pre-encoded ringtone bytes
    pub(crate) async fn load_ringtone(&self) -> Option<Vec<u8>> {
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
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

    /// helper method to check if a peer is in the current room
    pub(crate) async fn is_in_room(&self, peer_id: &PeerId) -> bool {
        self.room_state
            .read()
            .await
            .as_ref()
            .map(|m| m.peers.contains(peer_id))
            .unwrap_or(false)
    }

    pub(crate) async fn room_hash(&self) -> Option<Vec<u8>> {
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

    pub(crate) async fn is_call_active(&self) -> bool {
        self.core_state.in_call.load(Relaxed)
            || self.room_state.read().await.is_some()
            || self.core_state.end_audio_test.lock().await.is_some()
    }

    pub(crate) async fn send_start_screenshare(&self, peer: PeerId, header: Option<Message>) {
        if let Some(ref sender) = self.start_screenshare {
            _ = sender.send(StartScreenshare { peer, header }).await;
        }
    }

    pub(crate) async fn peer_id(&self) -> PeerId {
        if let Some(keypair) = self.core_state.identity.read().await.as_ref() {
            keypair.public().to_peer_id()
        } else {
            PeerId::random()
        }
    }

    pub(crate) async fn shutdown(&self) {
        // guaranteed to end all sessions
        self.reset_sessions().await;
        // the manager will now stop & not run again
        self.core_state.stop_manager.store(true, Relaxed);
        // end the current manager
        self.restart_manager.notify_one();
    }

    #[cfg(target_family = "wasm")]
    pub(crate) async fn init_web_audio(&self) -> Result<()> {
        let wrapper = WebAudioWrapper::new().await?;
        *self.web_input.lock().await = Some(wrapper);
        Ok(())
    }
}

pub(crate) struct OutputHelper {
    /// Reference to the shared output handles storage in CoreState
    output_handles: ActiveOutputHandles,
    /// Unique ID for this output handle in the HashMap
    id: Uuid,
    /// Tracks whether the handle has been consumed by join()
    consumed: bool,
}

impl OutputHelper {
    /// Creates a new OutputHelper and stores the handle in the shared storage
    pub(crate) fn new(handle: AudioOutputHandle, output_handles: ActiveOutputHandles) -> Self {
        let id = Uuid::new_v4();
        output_handles
            .lock()
            .expect("output handles mutex poisoned")
            .insert(id, handle);
        Self {
            output_handles,
            id,
            consumed: false,
        }
    }

    pub(crate) fn sender(&self) -> kanal::Sender<ProcessorMessage> {
        self.output_handles
            .lock()
            .expect("output handles mutex poisoned")
            .get(&self.id)
            .expect("output handle should exist")
            .sender()
    }

    pub(crate) fn join(mut self) -> Result<()> {
        debug!("stopping output handle via join");
        self.consumed = true;
        if let Some(handle) = self
            .output_handles
            .lock()
            .expect("output handles mutex poisoned")
            .remove(&self.id)
        {
            handle.stop();
        }
        Ok(())
    }
}

impl Drop for OutputHelper {
    fn drop(&mut self) {
        // Only clean up if join() wasn't called
        if !self.consumed {
            debug!("cleaning up output handle via drop");
            if let Ok(mut guard) = self.output_handles.lock()
                && let Some(handle) = guard.remove(&self.id)
            {
                handle.stop();
            }
        }
    }
}

pub(crate) struct InputHelper {
    /// Reference to the shared input handle storage in CoreState
    input_handle: ActiveInputHandle,
    receiver: Option<kanal::AsyncReceiver<ProcessorMessage>>,
    /// Tracks whether the handle has been consumed by join()
    consumed: bool,
}

impl InputHelper {
    /// Creates a new InputHelper and stores the handle in the shared storage
    pub(crate) fn new(
        handle: AudioInputHandle,
        input_handle: ActiveInputHandle,
        receiver: kanal::AsyncReceiver<ProcessorMessage>,
    ) -> Self {
        *input_handle.lock().expect("input handle mutex poisoned") = Some(handle);
        Self {
            input_handle,
            receiver: Some(receiver),
            consumed: false,
        }
    }

    pub(crate) fn receiver(&mut self) -> kanal::AsyncReceiver<ProcessorMessage> {
        self.receiver.take().unwrap()
    }

    pub(crate) fn join(mut self) -> Result<()> {
        debug!("stopping input handle via join");
        self.consumed = true;
        if let Some(handle) = self
            .input_handle
            .lock()
            .expect("input handle mutex poisoned")
            .take()
        {
            handle.stop();
        }
        Ok(())
    }
}

impl Drop for InputHelper {
    fn drop(&mut self) {
        // Only clean up if join() wasn't called
        if !self.consumed {
            debug!("cleaning up input handle via drop");
            if let Ok(mut guard) = self.input_handle.lock()
                && let Some(handle) = guard.take()
            {
                handle.stop();
            }
        }
    }
}
