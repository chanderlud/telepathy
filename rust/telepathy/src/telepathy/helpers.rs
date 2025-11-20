use crate::audio::codec::{decoder, encoder};
use crate::audio::{InputProcessorState, OutputProcessorState, input_processor, output_processor};
use crate::error::ErrorKind;
use crate::frb_generated::FLUTTER_RUST_BRIDGE_HANDLER;
use crate::telepathy::utils::{SendStream, get_output_device};
use crate::telepathy::{
    CHANNEL_SIZE, EarlyCallState, StartScreenshare, StatisticsCollectorState, Telepathy,
};
use cpal::Device;
use cpal::traits::{DeviceTrait, HostTrait};
use flutter_rust_bridge::spawn_blocking_with;
use kanal::{AsyncReceiver, AsyncSender, Sender, bounded, unbounded_async};
use libp2p::PeerId;
use log::{error, info, warn};
use messages::{AudioHeader, Message};
use nnnoiseless::DenoiseState;
use sea_codec::ProcessorMessage;
use sea_codec::codec::file::SeaFileHeader;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

use crate::telepathy::Result;

impl Telepathy {
    /// helper method to set up audio input stack between the network and device layers
    pub(crate) async fn setup_input(
        &self,
        sample_rate: f64,
        codec_options: (bool, bool, f32),
        statistics_state: &StatisticsCollectorState,
        is_room: bool,
    ) -> Result<(AsyncReceiver<ProcessorMessage>, Sender<f32>)> {
        // input stream -> input processor
        let (input_sender, input_receiver) = bounded::<f32>(CHANNEL_SIZE);

        #[cfg(target_family = "wasm")]
        let input_receiver = {
            // normal channel is unused on the web
            drop(input_receiver);

            if let Some(web_input) = self.web_input.lock().await.as_ref() {
                crate::audio::web_audio::WebInput::from(web_input)
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
        // the rnnoise denoiser
        let denoiser = denoise.then_some(DenoiseState::from_model(
            self.denoise_model.read().await.clone(),
        ));
        let state = InputProcessorState::new(
            &self.input_volume,
            &self.rms_threshold,
            &self.muted,
            statistics_state.input_rms.clone(),
        );

        // spawn the input processor thread
        spawn_blocking_with(
            move || {
                input_processor(
                    input_receiver,
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
    pub(crate) async fn setup_output(
        &self,
        remote_sample_rate: f64,
        codec_enabled: bool,
        statistics_state: &StatisticsCollectorState,
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
        let state = OutputProcessorState::new(
            &self.output_volume,
            statistics_state.output_rms.clone(),
            &self.deafened,
            statistics_state.loss.clone(),
        );

        let output_processor_receiver = if codec_enabled {
            // if codec enabled, spawn extra decoder thread
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
            move || output_processor(output_processor_receiver, output_sender, ratio, state),
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        // get the output channels for chunking the output
        let output_channels = output_config.channels() as usize;

        let output_stream = SendStream {
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

        Ok((network_output_sender, output_stream))
    }

    /// helper method to set up non-web audio input stream
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn setup_input_stream(
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
    pub(crate) async fn get_input_device(&self) -> Result<Device> {
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
    pub(crate) async fn load_ringtone(&self) -> Option<Vec<u8>> {
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
        self.in_call.load(Relaxed)
            || self.room_state.read().await.is_some()
            || self.end_audio_test.lock().await.is_some()
    }

    pub(crate) async fn send_start_screenshare(&self, peer: PeerId, header: Option<Message>) {
        _ = self
            .start_screenshare
            .send(StartScreenshare { peer, header })
            .await;
    }

    #[cfg(target_family = "wasm")]
    pub(crate) async fn init_web_audio(&self) -> Result<()> {
        let wrapper = crate::audio::web_audio::WebAudioWrapper::new().await?;
        *self.web_input.lock().await = Some(wrapper);
        Ok(())
    }
}
