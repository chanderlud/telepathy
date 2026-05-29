use crate::flutter::ManagerState;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::core::TelepathyCore;
use crate::internal::error::ErrorKind;
use crate::internal::messages::{AudioHeader, ProtocolMessage, StartScreenshare};
#[cfg(not(target_family = "wasm"))]
use crate::internal::screenshare;
use crate::internal::state::{EarlyCallState, StatisticsCollectorState};
use crate::internal::utils::{KanalSink, KanalSource};
use crate::internal::{ALPN, Result};
use crate::types::FrontendNotify;
use bytes::Bytes;
use iroh::endpoint::presets;
use iroh::{Endpoint, PublicKey, SecretKey};
use rustls::crypto::aws_lc_rs::{self, kx_group};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::SocketAddr;
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
use tracing::{error, info, instrument, warn};

impl<C, S, H> TelepathyCore<C, S, H>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    /// builds an iroh endpoint and waits for it to come online
    #[instrument(name = "manager.setup_endpoint", skip_all)]
    pub(crate) async fn setup_endpoint(&self) -> Result<Option<Endpoint>> {
        let identity = if let Some(keypair) = self.core_state.identity.read().await.as_ref() {
            keypair.clone()
        } else {
            return Err(ErrorKind::NoIdentityAvailable.into());
        };

        self.callbacks.manager_state(ManagerState::Starting).await;

        let mut provider = aws_lc_rs::default_provider();
        provider.kx_groups = vec![
            kx_group::X25519MLKEM768,
            kx_group::X25519,
            kx_group::SECP256R1,
            kx_group::SECP384R1,
        ];

        let listen_port = self.core_state.network_config.listen_port.load(Relaxed);
        let bind_addresses = self
            .core_state
            .network_config
            .bind_addresses
            .read()
            .expect("bind_addresses lock poisoned")
            .clone();

        let mut endpoint_builder = Endpoint::builder(presets::N0)
            .crypto_provider(Arc::new(provider))
            .secret_key(identity)
            .alpns(vec![ALPN.to_vec()])
            .clear_ip_transports();

        for ip in bind_addresses {
            endpoint_builder = endpoint_builder
                .bind_addr(SocketAddr::new(ip, listen_port))
                .expect("validated bind address must produce a valid socket address");
        }

        let endpoint = endpoint_builder.bind().await?;

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
            // start playing back the screenshare
            screenshare::playback(
                message.connection,
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
                .send(ProtocolMessage::ScreenshareHeader {
                    encoder_name: config.encoder.to_string(),
                })
                .await;

            if result.is_ok() {
                // alert the frontend & provide the stop object
                self.callbacks.screenshare_started(dart_stop, true).await;
                // start recording the screenshare
                screenshare::record(
                    message.connection,
                    stop,
                    state.upload_bandwidth.clone(),
                    config,
                )
                .await?;
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
    ) -> Result<InputHelper> {
        let (codec_enabled, vbr, residual_bits) = codec_options;
        // Channel for receiving processed audio data
        let (sender, receiver) = kanal::unbounded_async();

        let mut builder = AudioInputBuilder::new()
            .device(self.core_state.input_device.lock().await.clone())
            .input_volume_shared(&self.core_state.input_volume)
            .rms_threshold_shared(&self.core_state.rms_threshold)
            .muted_shared(&self.core_state.muted)
            .rms_shared(&statistics_state.input_rms)
            .on_error(end_call)
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
        remote_sample_rate: f64,
        codec_enabled: bool,
        statistics_state: &StatisticsCollectorState,
        end_call: Arc<Notify>,
    ) -> Result<OutputHelper> {
        // Get device ID
        let device_id = self.core_state.output_device.lock().await.clone();
        // Create the input channel
        let (sender, receiver) = kanal::unbounded();
        // Create the audio output using the builder
        let handle = AudioOutputBuilder::new()
            .source(KanalSource::new(receiver))
            .device(device_id)
            .sample_rate(remote_sample_rate as u32)
            .output_volume_shared(&self.core_state.output_volume)
            .deafened_shared(&self.core_state.deafened)
            .rms_shared(&statistics_state.output_rms)
            .loss_shared(&statistics_state.loss)
            .codec(codec_enabled)
            .on_error(end_call)
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
                    let device_handle = self.host.get_input_device(device_id.as_deref())?;
                    info!("input_device: {:?}", device_handle.name());
                    device_handle.sample_rate()?
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
        self.room_state.read().await.as_ref().map(|state| {
            state.peers.iter().fold(0u64, |acc, peer| {
                let mut hasher = DefaultHasher::new();
                peer.hash(&mut hasher);
                acc ^ hasher.finish()
            })
        })
    }

    pub(crate) async fn is_call_active(&self) -> bool {
        self.core_state.in_call.load(Relaxed)
            || self.room_state.read().await.is_some()
            || self.core_state.end_audio_test.lock().await.is_some()
    }

    pub(crate) async fn peer_id(&self) -> PublicKey {
        if let Some(keypair) = self.core_state.identity.read().await.as_ref() {
            keypair.public()
        } else {
            SecretKey::generate().public()
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
    _handle: AudioOutputHandle,
    sender: Option<kanal::Sender<Bytes>>,
}

impl OutputHelper {
    /// Creates a new OutputHelper and stores the handle in the shared storage
    pub(crate) fn new(handle: AudioOutputHandle, sender: kanal::Sender<Bytes>) -> Self {
        Self {
            _handle: handle,
            sender: Some(sender),
        }
    }

    pub(crate) fn sender(&mut self) -> kanal::Sender<Bytes> {
        self.sender.take().expect("sender already taken")
    }
}

pub(crate) struct InputHelper {
    _handle: AudioInputHandle,
    receiver: Option<kanal::AsyncReceiver<PooledBuffer>>,
}

impl InputHelper {
    /// Creates a new InputHelper and stores the handle in the shared storage
    pub(crate) fn new(
        handle: AudioInputHandle,
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
