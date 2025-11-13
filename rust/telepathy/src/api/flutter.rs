use crate::api::error::{DartError, Error, ErrorKind};
use crate::api::screenshare;
use crate::api::screenshare::{Decoder, Encoder};
use crate::api::utils::{
    atomic_u32_deserialize, atomic_u32_serialize, rwlock_option_recording_config,
};
use crate::frb_generated::StreamSink;
use atomic_float::AtomicF32;
use chrono::{DateTime, Local};
#[cfg(not(target_family = "wasm"))]
use fast_log::Config;
#[cfg(not(target_family = "wasm"))]
use fast_log::appender::{FastLogRecord, LogAppender};
use flutter_rust_bridge::frb;
use lazy_static::lazy_static;
use libp2p::PeerId;
use libp2p::identity::Keypair;
use log::{LevelFilter, info, warn};
use messages::Attachment;
use serde::{Deserialize, Serialize};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Once};
#[cfg(not(target_family = "wasm"))]
use tokio::net::lookup_host;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tokio::spawn;
use tokio::sync::{Notify, RwLock};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tokio::time::Instant;
use uuid::Uuid;

static INIT_LOGGER_ONCE: Once = Once::new();

lazy_static! {
    static ref SEND_TO_DART_LOGGER_STREAM_SINK: parking_lot::RwLock<Option<StreamSink<String>>> =
        parking_lot::RwLock::new(None);
}

#[derive(Clone, Debug)]
#[frb(opaque)]
pub struct Contact {
    /// A random ID to identify the contact
    pub(crate) id: String,

    /// The nickname of the contact
    pub(crate) nickname: String,

    /// The public/verifying key for the contact
    pub(crate) peer_id: PeerId,
}

impl Contact {
    #[frb(sync)]
    pub fn new(nickname: String, peer_id: String) -> Result<Contact, DartError> {
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            nickname,
            peer_id: PeerId::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
        })
    }

    #[frb(sync)]
    pub fn from_parts(id: String, nickname: String, peer_id: String) -> Result<Contact, DartError> {
        Ok(Self {
            id,
            nickname,
            peer_id: PeerId::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
        })
    }

    #[frb(sync)]
    pub fn peer_id(&self) -> String {
        self.peer_id.to_string()
    }

    #[frb(sync)]
    pub fn nickname(&self) -> String {
        self.nickname.clone()
    }

    #[frb(sync)]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    #[frb(sync)]
    pub fn set_nickname(&mut self, nickname: String) {
        self.nickname = nickname;
    }

    #[frb(sync)]
    pub fn pub_clone(&self) -> Contact {
        self.clone()
    }

    #[frb(sync)]
    pub fn id_eq(&self, id: Vec<u8>) -> bool {
        self.peer_id.to_bytes() == id
    }
}

#[frb(opaque)]
#[derive(Clone)]
pub struct NetworkConfig {
    /// the relay server's address
    pub(crate) relay_address: Arc<RwLock<SocketAddr>>,

    /// the relay server's peer id
    pub(crate) relay_id: Arc<RwLock<PeerId>>,

    /// the libp2p port for the swarm
    pub(crate) listen_port: Arc<RwLock<u16>>,
}

impl NetworkConfig {
    #[frb(sync)]
    pub fn new(relay_address: String, relay_id: String) -> Result<Self, DartError> {
        Ok(Self {
            relay_address: Arc::new(RwLock::new(relay_address.parse().map_err(Error::from)?)),
            relay_id: Arc::new(RwLock::new(
                PeerId::from_str(&relay_id).map_err(Error::from)?,
            )),
            listen_port: Arc::new(RwLock::new(0)),
        })
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn set_relay_address(&self, relay_address: String) -> Result<(), DartError> {
        if let Some(address) = lookup_host(&relay_address)
            .await
            .map_err(Error::from)?
            .next()
        {
            *self.relay_address.write().await = address;
            Ok(())
        } else {
            Err("Failed to resolve address".to_string().into())
        }
    }

    #[cfg(target_family = "wasm")]
    pub async fn set_relay_address(&self, relay_address: String) -> Result<(), DartError> {
        *self.relay_address.write().await = SocketAddr::from_str(&relay_address)
            .map_err(|error| DartError::from(error.to_string()))?;
        Ok(())
    }

    pub async fn get_relay_address(&self) -> String {
        self.relay_address.read().await.to_string()
    }

    pub async fn set_relay_id(&self, relay_id: String) -> Result<(), DartError> {
        *self.relay_id.write().await = PeerId::from_str(&relay_id).map_err(Error::from)?;
        Ok(())
    }

    pub async fn get_relay_id(&self) -> String {
        self.relay_id.read().await.to_string()
    }
}

#[frb(opaque)]
#[derive(Clone, Serialize, Deserialize)]
pub struct ScreenshareConfig {
    /// the screenshare capabilities. default until loaded
    #[serde(skip)]
    capabilities: Arc<RwLock<Capabilities>>,

    /// a validated recording configuration
    #[serde(with = "rwlock_option_recording_config")]
    pub(crate) recording_config: Arc<RwLock<Option<RecordingConfig>>>,

    /// the default width of the playback window
    #[serde(
        serialize_with = "atomic_u32_serialize",
        deserialize_with = "atomic_u32_deserialize"
    )]
    pub(crate) width: Arc<AtomicU32>,

    /// the default height of the playback window
    #[serde(
        serialize_with = "atomic_u32_serialize",
        deserialize_with = "atomic_u32_deserialize"
    )]
    pub(crate) height: Arc<AtomicU32>,
}

impl Default for ScreenshareConfig {
    fn default() -> Self {
        Self {
            capabilities: Default::default(),
            recording_config: Default::default(),
            width: Arc::new(AtomicU32::new(1280)),
            height: Arc::new(AtomicU32::new(720)),
        }
    }
}

impl ScreenshareConfig {
    // this function must be async to use spawn
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub async fn new(config_str: String) -> Self {
        let config: ScreenshareConfig = serde_json::from_str(&config_str).unwrap_or_default();

        let capabilities_clone = Arc::clone(&config.capabilities);
        spawn(async move {
            let now = Instant::now();
            let c = Capabilities::new().await;
            *capabilities_clone.write().await = c;
            info!("Capabilities loaded in {:?}", now.elapsed());
        });

        config
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub async fn new(_config_str: String) -> Self {
        Self::default()
    }

    pub async fn capabilities(&self) -> Capabilities {
        self.capabilities.read().await.clone()
    }

    pub async fn recording_config(&self) -> Option<RecordingConfig> {
        self.recording_config.read().await.clone()
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub async fn update_recording_config(
        &self,
        encoder: String,
        device: String,
        bitrate: u32,
        framerate: u32,
        height: Option<u32>,
    ) -> std::result::Result<(), DartError> {
        let encoder = Encoder::from_str(&encoder).map_err(|_| ErrorKind::InvalidEncoder)?;

        let recording_config = RecordingConfig {
            encoder,
            device: screenshare::Device::from_str(&device)
                .map_err(|_| "Invalid device".to_string())?,
            bitrate,
            framerate,
            height,
        };

        if let Ok(status) = recording_config.test_config().await
            && status.success()
        {
            *self.recording_config.write().await = Some(recording_config);
            return Ok(());
        }

        Err("Invalid configuration".to_string().into())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub async fn update_recording_config(
        &self,
        _encoder: String,
        _device: String,
        _bitrate: u32,
        _framerate: u32,
        _height: Option<u32>,
    ) -> std::result::Result<(), DartError> {
        Ok(())
    }

    #[frb(sync)]
    pub fn to_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

/// capabilities for ffmpeg and ffplay supported by this client
#[derive(Default, Debug, Clone)]
#[frb(opaque)]
pub struct Capabilities {
    pub(crate) _available: bool,

    pub(crate) encoders: Vec<Encoder>,

    pub(crate) _decoders: Vec<Decoder>,

    pub(crate) devices: Vec<screenshare::Device>,
}

impl Capabilities {
    #[frb(sync)]
    pub fn encoders(&self) -> Vec<String> {
        self.encoders.iter().map(|e| e.to_string()).collect()
    }

    #[frb(sync)]
    pub fn devices(&self) -> Vec<String> {
        self.devices.iter().map(|d| d.to_string()).collect()
    }
}

/// recording config for screenshare
#[derive(Debug, Clone, Serialize, Deserialize)]
#[frb(opaque)]
pub struct RecordingConfig {
    pub(crate) encoder: Encoder,

    pub(crate) device: screenshare::Device,

    pub(crate) bitrate: u32,

    pub(crate) framerate: u32,

    /// the height for the video output
    pub(crate) height: Option<u32>,
}

impl RecordingConfig {
    #[frb(sync)]
    pub fn encoder(&self) -> String {
        let encoder_str: &str = self.encoder.into();
        encoder_str.to_string()
    }

    #[frb(sync)]
    pub fn device(&self) -> String {
        self.device.to_string()
    }

    #[frb(sync)]
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    #[frb(sync)]
    pub fn framerate(&self) -> u32 {
        self.framerate
    }

    #[frb(sync)]
    pub fn height(&self) -> Option<u32> {
        self.height
    }
}

#[frb(opaque)]
#[derive(Clone)]
pub struct CodecConfig {
    /// whether to use the codec
    pub(crate) enabled: Arc<AtomicBool>,

    /// whether to use variable bitrate
    pub(crate) vbr: Arc<AtomicBool>,

    /// the compression level
    pub(crate) residual_bits: Arc<AtomicF32>,
}

impl CodecConfig {
    #[frb(sync)]
    pub fn new(enabled: bool, vbr: bool, residual_bits: f32) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            vbr: Arc::new(AtomicBool::new(vbr)),
            residual_bits: Arc::new(AtomicF32::new(residual_bits)),
        }
    }

    #[frb(sync)]
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Relaxed);
    }

    #[frb(sync)]
    pub fn set_vbr(&self, vbr: bool) {
        self.vbr.store(vbr, Relaxed);
    }

    #[frb(sync)]
    pub fn set_residual_bits(&self, residual_bits: f32) {
        self.residual_bits.store(residual_bits, Relaxed);
    }

    #[frb(sync)]
    pub fn to_values(&self) -> (bool, bool, f32) {
        (
            self.enabled.load(Relaxed),
            self.vbr.load(Relaxed),
            self.residual_bits.load(Relaxed),
        )
    }
}

/// a shared notifier that can be passed to dart code
#[frb(opaque)]
pub struct DartNotify {
    pub(crate) inner: Arc<Notify>,
}

impl DartNotify {
    /// public notified function for dart
    pub async fn notified(&self) {
        self.inner.notified().await;
    }

    /// notifies one waiter
    #[frb(sync)]
    pub fn notify(&self) {
        self.inner.notify_waiters();
    }
}

pub struct ChatMessage {
    pub text: String,

    pub(crate) receiver: PeerId,

    pub(crate) timestamp: DateTime<Local>,

    pub(crate) attachments: Vec<Attachment>,
}

impl ChatMessage {
    #[frb(sync)]
    pub fn is_sender(&self, identity: String) -> bool {
        self.receiver.to_string() != identity
    }

    #[frb(sync)]
    pub fn time(&self) -> String {
        self.timestamp.format("%l:%M %p").to_string()
    }

    #[frb(sync)]
    pub fn attachments(&self) -> Vec<(String, Vec<u8>)> {
        self.attachments
            .iter()
            .map(|a| (a.name.clone(), a.data.clone()))
            .collect()
    }

    #[frb(sync)]
    pub fn clear_attachments(&mut self) {
        for attachment in self.attachments.iter_mut() {
            attachment.data.truncate(0);
        }
    }
}

/// processed statistics for the frontend
#[derive(Default)]
pub struct Statistics {
    /// a percentage of the max input volume in the window
    pub input_level: f32,

    /// a percentage of the max output volume in the window
    pub output_level: f32,

    /// the current call latency
    pub latency: usize,

    /// the approximate upload bandwidth used by the current call
    pub upload_bandwidth: usize,

    /// the approximate download bandwidth used by the current call
    pub download_bandwidth: usize,

    /// a value between 0 and 1 representing the percent of audio lost in a sliding window
    pub loss: f64,
}

// The following is a modified version of the code found at
// https://github.com/fzyzcjy/flutter_rust_bridge/issues/486

pub struct SendToDartLogger {}

impl SendToDartLogger {
    pub fn set_stream_sink(stream_sink: StreamSink<String>) {
        let mut guard = SEND_TO_DART_LOGGER_STREAM_SINK.write();
        let overriding = guard.is_some();

        *guard = Some(stream_sink);

        drop(guard);

        if overriding {
            warn!(
                "SendToDartLogger::set_stream_sink but already exist a sink, thus overriding. \
                (This may or may not be a problem. It will happen normally if hot-reload Flutter app.)"
            );
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl LogAppender for SendToDartLogger {
    fn do_logs(&mut self, records: &[FastLogRecord]) {
        if let Some(stream) = SEND_TO_DART_LOGGER_STREAM_SINK.read().as_ref() {
            for record in records {
                _ = stream.add(record.formated.clone());
            }
        }
    }
}

#[frb(sync)]
pub fn create_log_stream(s: StreamSink<String>) {
    SendToDartLogger::set_stream_sink(s);
}

#[frb(sync)]
pub fn rust_set_up() {
    // https://stackoverflow.com/questions/30177845/how-to-initialize-the-logger-for-integration-tests
    INIT_LOGGER_ONCE.call_once(|| {
        // let level = if cfg!(debug_assertions) {
        //     LevelFilter::Debug
        // } else {
        //     LevelFilter::Warn
        // };

        let level = LevelFilter::Debug;

        assert!(
            level <= log::STATIC_MAX_LEVEL,
            "Should respect log::STATIC_MAX_LEVEL={:?}, which is done in compile time. level{:?}",
            log::STATIC_MAX_LEVEL,
            level
        );

        #[cfg(not(target_family = "wasm"))]
        fast_log::init(
            Config::new()
                .file("telepathy.log")
                .level(level)
                .add_appender(SendToDartLogger {}),
        )
        .unwrap();

        #[cfg(target_family = "wasm")]
        wasm_logger::init(wasm_logger::Config::default());

        log_panics::init();

        info!("init_logger (inside 'once') finished");

        warn!(
            "init_logger finished, chosen level={:?} (deliberately output by warn level)",
            level
        );
    });
}

#[frb(sync)]
pub fn generate_keys() -> Result<(String, Vec<u8>), DartError> {
    let pair = Keypair::generate_ed25519();

    let peer_id = pair.public().to_peer_id();

    Ok((
        peer_id.to_string(),
        pair.to_protobuf_encoding()
            .map_err(|e| DartError::from(e.to_string()))?,
    ))
}

#[frb(sync)]
pub fn room_hash(peers: Vec<String>) -> String {
    let peer_hash = peers
        .into_iter()
        .filter_map(|p| PeerId::from_str(&p).ok())
        .fold(0u64, |acc, peer| {
            let mut hasher = DefaultHasher::new();
            peer.hash(&mut hasher);
            acc ^ hasher.finish()
        });

    format!("room-{}", peer_hash)
}
