#[cfg(target_family = "wasm")]
use crate::audio::WebOutput;
use crate::audio::codec::{decoder, encoder};
#[cfg(target_family = "wasm")]
use crate::audio::web_audio::{WebAudioInput, WebAudioWrapper};
#[cfg(not(target_family = "wasm"))]
use crate::audio::{ChannelInput, ChannelOutput};
use crate::audio::{InputProcessorState, OutputProcessorState, input_processor, output_processor};
use crate::error::ErrorKind;
use crate::flutter::DartNotify;
use crate::flutter::callbacks::{FrbCallbacks, FrbStatisticsCallback};
use crate::frb_generated::FLUTTER_RUST_BRIDGE_HANDLER;
use crate::telepathy::core::TelepathyCore;
use crate::telepathy::messages::{AudioHeader, Message};
#[cfg(not(target_family = "wasm"))]
use crate::telepathy::screenshare;
use crate::telepathy::utils::{SendStream, get_output_device};
use crate::telepathy::{
    CHANNEL_SIZE, CHAT_PROTOCOL, EarlyCallState, Result, StartScreenshare, StatisticsCollectorState,
};
use crate::{Behaviour, BehaviourEvent};
#[cfg(not(target_family = "wasm"))]
use cpal::Device;
use cpal::SampleFormat;
#[cfg(not(target_family = "wasm"))]
use cpal::traits::HostTrait;
use cpal::traits::{DeviceTrait, StreamTrait};
use flutter_rust_bridge::{JoinHandle, spawn_blocking_with};
use kanal::{AsyncReceiver, AsyncSender, Sender, bounded, unbounded_async};
use libp2p::futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::SwarmEvent;
#[cfg(not(target_family = "wasm"))]
use libp2p::tcp;
use libp2p::{Multiaddr, PeerId, Swarm, dcutr, identify, noise, ping, yamux};
use libp2p_stream::Control;
use log::{error, info, warn};
use nnnoiseless::DenoiseState;
use sea_codec::ProcessorMessage;
use sea_codec::codec::file::SeaFileHeader;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
use tokio::fs::File;
#[cfg(not(target_family = "wasm"))]
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

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

    /// helper method to set up audio input stack between the network and device layers
    pub(crate) async fn setup_input(
        &self,
        sample_rate: f64,
        codec_options: (bool, bool, f32),
        statistics_state: &StatisticsCollectorState,
        is_room: bool,
    ) -> Result<InputHelper> {
        // input stream -> input processor
        let (input_sender, input_receiver) = bounded::<f32>(CHANNEL_SIZE);

        #[cfg(not(target_family = "wasm"))]
        let processor_input = ChannelInput::from(input_receiver);
        #[cfg(target_family = "wasm")]
        let processor_input = {
            // normal channel is unused on the web
            drop(input_receiver);

            if let Some(web_input) = self.web_input.lock().await.as_ref() {
                WebAudioInput::from(web_input)
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
        let denoise = self.core_state.denoise.load(Relaxed);
        // the rnnoise denoiser
        let denoiser = denoise.then_some(DenoiseState::from_model(
            self.core_state.denoise_model.read().await.clone(),
        ));
        let state = InputProcessorState::new(
            &self.core_state.input_volume,
            &self.core_state.rms_threshold,
            &self.core_state.muted,
            statistics_state.input_rms.clone(),
        );

        // spawn the input processor thread
        let processor_handle = spawn_blocking_with(
            move || {
                input_processor(
                    processor_input,
                    processed_input_sender.to_sync(),
                    sample_rate,
                    denoiser,
                    codec_enabled,
                    state,
                )
            },
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        if codec_enabled {
            // if using codec, spawn extra encoder thread
            let encoder_handle = spawn_blocking_with(
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

            Ok(InputHelper {
                receiver: Some(encoded_input_receiver),
                sender: Some(input_sender),
                processor_handle,
                encoder_handle: Some(encoder_handle),
            })
        } else {
            Ok(InputHelper {
                receiver: Some(processed_input_receiver),
                sender: Some(input_sender),
                processor_handle,
                encoder_handle: None,
            })
        }
    }

    /// helper method to set up audio output stack above network layer
    pub(crate) async fn setup_output(
        &self,
        remote_sample_rate: f64,
        codec_enabled: bool,
        statistics_state: &StatisticsCollectorState,
        is_room: bool,
        end_call: Arc<Notify>,
    ) -> Result<OutputHelper> {
        // receiving socket -> output processor or decoder
        let (network_output_sender, network_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // decoder -> output processor
        let (decoded_output_sender, decoded_output_receiver) =
            unbounded_async::<ProcessorMessage>();

        // output processor -> output stream
        #[cfg(not(target_family = "wasm"))]
        let (output_sender, output_receiver) = bounded::<f32>(CHANNEL_SIZE * 4);
        #[cfg(not(target_family = "wasm"))]
        let processor_input = ChannelOutput::from(output_sender);

        // output processor -> output stream
        #[cfg(target_family = "wasm")]
        let processor_input = WebOutput::default();
        #[cfg(target_family = "wasm")]
        let web_output = processor_input.buf.clone();

        // get the output device and its default configuration
        let output_device = get_output_device(&self.core_state.output_device, &self.host).await?;
        let output_config = output_device.default_output_config()?;
        if output_config.sample_format() != SampleFormat::F32 {
            return Err(ErrorKind::UnsupportedSampleFormat.into());
        }
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
        let state = OutputProcessorState::new(
            &self.core_state.output_volume,
            statistics_state.output_rms.clone(),
            &self.core_state.deafened,
            statistics_state.loss.clone(),
        );

        let mut decoder_handle = None;
        let output_processor_receiver = if codec_enabled {
            // if codec enabled, spawn extra decoder thread
            decoder_handle = Some(spawn_blocking_with(
                move || {
                    decoder(
                        network_output_receiver.to_sync(),
                        decoded_output_sender.to_sync(),
                        header,
                    );
                },
                FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
            ));

            decoded_output_receiver.to_sync()
        } else {
            network_output_receiver.to_sync()
        };

        // spawn the output processor thread
        let processor_handle = spawn_blocking_with(
            move || output_processor(output_processor_receiver, processor_input, ratio, state),
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        // get the output channels for chunking the output
        let output_channels = output_config.channels() as usize;

        let stream = SendStream {
            stream: output_device.build_output_stream(
                &output_config.into(),
                move |output: &mut [f32], _: &_| {
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
                        frame.fill(sample);
                    }
                },
                move |err| {
                    error!("Error in output stream: {}", err);
                    end_call.notify_one();
                },
                None,
            )?,
        };
        stream.stream.play()?;

        Ok(OutputHelper {
            sender: Some(network_output_sender),
            stream,
            processor_handle,
            decoder_handle,
        })
    }

    /// helper method to set up non-web audio input stream
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn setup_input_stream(
        &self,
        call_state: &EarlyCallState,
        input_sender: Sender<f32>,
        end_call: Arc<Notify>,
    ) -> Result<SendStream> {
        let input_channels = call_state.input_channels;
        Ok(SendStream {
            stream: call_state.input_device.build_input_stream(
                &call_state.input_config.clone().into(),
                move |input, _| {
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

        #[cfg(not(target_family = "wasm"))]
        let input_device;
        #[cfg(not(target_family = "wasm"))]
        let input_config;

        let input_sample_rate;
        let input_channels;

        #[cfg(not(target_family = "wasm"))]
        {
            // get the input device and its default configuration
            input_device = self.get_input_device().await?;
            input_config = input_device.default_input_config()?;
            if input_config.sample_format() != SampleFormat::F32 {
                return Err(ErrorKind::UnsupportedSampleFormat.into());
            }
            info!("input_device: {:?}", input_device.name());
            input_sample_rate = input_config.sample_rate().0;
            input_channels = input_config.channels() as usize;
        }

        #[cfg(target_family = "wasm")]
        {
            if let Some(web_input) = self.web_input.lock().await.as_ref() {
                input_sample_rate = web_input.sample_rate as u32;
            } else {
                return Err(ErrorKind::NoInputDevice.into());
            }

            input_channels = 1; // only ever 1 channel on web
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
            #[cfg(not(target_family = "wasm"))]
            input_config,
            #[cfg(not(target_family = "wasm"))]
            input_device,
            input_channels,
        })
    }

    /// helper method to get the user specified device or default as fallback
    #[cfg(not(target_family = "wasm"))]
    pub(crate) async fn get_input_device(&self) -> Result<Device> {
        match *self.core_state.input_device.lock().await {
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
    pub(crate) async fn load_ringtone(&self) -> Option<Vec<u8>> {
        #[cfg(not(target_family = "wasm"))]
        if self.core_state.send_custom_ringtone.load(Relaxed) {
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
        } else {
            None
        }

        #[cfg(target_family = "wasm")]
        None
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
    sender: Option<AsyncSender<ProcessorMessage>>,
    stream: SendStream,
    processor_handle: JoinHandle<Result<()>>,
    decoder_handle: Option<JoinHandle<()>>,
}

impl OutputHelper {
    pub(crate) fn sender(&mut self) -> AsyncSender<ProcessorMessage> {
        self.sender.take().unwrap()
    }

    pub(crate) async fn join(self) -> Result<()> {
        self.processor_handle.await??;
        if let Some(handle) = self.decoder_handle {
            handle.await?;
        }
        drop(self.stream);
        Ok(())
    }
}

pub(crate) struct InputHelper {
    receiver: Option<AsyncReceiver<ProcessorMessage>>,
    sender: Option<Sender<f32>>,
    processor_handle: JoinHandle<Result<()>>,
    encoder_handle: Option<JoinHandle<()>>,
}

impl InputHelper {
    pub(crate) fn receiver(&mut self) -> AsyncReceiver<ProcessorMessage> {
        self.receiver.take().unwrap()
    }

    pub(crate) fn sender(&mut self) -> Sender<f32> {
        self.sender.take().unwrap()
    }

    pub(crate) async fn join(self) -> Result<()> {
        match self.processor_handle.await? {
            Ok(()) => (),
            Err(error) => match error.kind {
                // input processor may end when channels close
                ErrorKind::KanalSend(_) => (),
                // propagate other errors
                _ => return Err(error),
            },
        }

        if let Some(handle) = self.encoder_handle {
            handle.await?;
        }
        Ok(())
    }
}
