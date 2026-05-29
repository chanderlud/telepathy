use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::Attachment;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use crate::internal::screenshare::encoder_from_str;
use crate::internal::screenshare::{Decoder, Device, Encoder, ScreenshareConfigDisk};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use crate::internal::spawn_task;
use atomic_float::AtomicF32;
use chrono::{DateTime, Local, SecondsFormat, Utc};
pub use iroh::{PublicKey, SecretKey};
use serde::{Serialize, Serializer};
use speedy::{Readable, Writable};
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
use tokio::sync::{Notify, RwLock};
use uuid::Uuid;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct Contact {
    /// A random ID to identify the contact
    pub(crate) id: String,

    /// The nickname of the contact
    pub(crate) nickname: String,

    /// The public/verifying key for the contact
    pub(crate) peer_id: PublicKey,

    /// In rooms, some contacts are dummy representing unknown peers
    pub(crate) is_room_only: bool,
}

impl Contact {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn new(nickname: String, peer_id: String) -> Result<Contact, DartError> {
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            nickname,
            peer_id: PublicKey::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
            is_room_only: false,
        })
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn from_parts(id: String, nickname: String, peer_id: String) -> Result<Contact, DartError> {
        Ok(Self {
            id,
            nickname,
            peer_id: PublicKey::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
            is_room_only: false,
        })
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn peer_id(&self) -> String {
        self.peer_id.to_string()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn nickname(&self) -> String {
        self.nickname.clone()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_nickname(&mut self, nickname: String) {
        self.nickname = nickname;
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn pub_clone(&self) -> Contact {
        self.clone()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn id_eq(&self, id: Vec<u8>) -> bool {
        self.peer_id.to_vec() == id
    }
}

#[derive(Debug, Serialize, Clone)]
pub enum CallState {
    Connected,
    Waiting,
    RoomJoin(String),
    RoomLeave(String),
    CallEnded(String, bool),
}

#[derive(Debug, Serialize, Clone)]
pub enum SessionStatus {
    Connecting,
    Connected {
        relayed: bool,
        remote_address: String,
    },
    Inactive,
    Unknown,
}

#[derive(Serialize, Clone, Debug)]
pub struct ChatMessage {
    pub text: String,

    pub receiver: PublicKey,

    #[serde(rename = "time", serialize_with = "serialize_timestamp_rfc3339_utc")]
    pub(crate) timestamp: DateTime<Local>,

    pub(crate) attachments: Vec<Attachment>,
}

impl ChatMessage {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn is_sender(&self, identity: String) -> bool {
        self.receiver.to_string() != identity
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn time(&self) -> String {
        self.timestamp.format("%l:%M %p").to_string()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn attachments(&self) -> Vec<(String, Vec<u8>)> {
        self.attachments
            .iter()
            .map(|a| (a.name.clone(), a.data.clone()))
            .collect()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn clear_attachments(&mut self) {
        for attachment in self.attachments.iter_mut() {
            attachment.data.truncate(0);
        }
    }
}

/// processed statistics for the frontend
// TODO add packet loss & other connection stats
#[derive(Default, Serialize)]
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

    /// the number of output samples that were lost in the interval
    pub loss: usize,
}

/// a shared notifier that can be passed to frontend code
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct FrontendNotify {
    pub(crate) inner: Arc<Notify>,
}

impl FrontendNotify {
    pub(crate) fn new(inner: &Arc<Notify>) -> Self {
        Self {
            inner: inner.clone(),
        }
    }
}

impl FrontendNotify {
    /// public notified function for dart
    pub async fn notified(&self) {
        self.inner.notified().await;
    }

    /// notifies one waiter
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn notify(&self) {
        self.inner.notify_waiters();
    }
}

// TODO extend NetworkConfig with relay, address discovery, nat traversal, timeouts
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
#[derive(Clone)]
pub struct NetworkConfig {
    pub(crate) listen_port: Arc<AtomicU16>,

    pub(crate) bind_addresses: Arc<StdRwLock<Vec<IpAddr>>>,
}

impl NetworkConfig {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn new(listen_port: u16, bind_addresses: Vec<String>) -> Result<Self, DartError> {
        Ok(Self {
            listen_port: Arc::new(AtomicU16::new(listen_port)),
            bind_addresses: Arc::new(StdRwLock::new(Self::parse_bind_addresses(bind_addresses)?)),
        })
    }

    #[cfg(test)]
    pub(crate) fn mock(port: u16, bind_addresses: Vec<IpAddr>) -> Self {
        Self {
            listen_port: Arc::new(AtomicU16::new(port)),
            bind_addresses: Arc::new(StdRwLock::new(bind_addresses)),
        }
    }

    fn parse_bind_addresses(bind_addresses: Vec<String>) -> Result<Vec<IpAddr>, DartError> {
        bind_addresses
            .into_iter()
            .map(|address| {
                IpAddr::from_str(&address).map_err(|error| DartError::from(error.to_string()))
            })
            .collect()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_listen_port(&self) -> u16 {
        self.listen_port.load(Relaxed)
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_listen_port(&self, listen_port: u16) {
        self.listen_port.store(listen_port, Relaxed);
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_bind_addresses(&self) -> Vec<String> {
        self.bind_addresses
            .read()
            .expect("bind_addresses lock poisoned")
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_bind_addresses(&self, bind_addresses: Vec<String>) -> Result<(), DartError> {
        *self
            .bind_addresses
            .write()
            .map_err(|error| DartError::from(error.to_string()))? =
            Self::parse_bind_addresses(bind_addresses)?;
        Ok(())
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_port: Arc::new(Default::default()),
            bind_addresses: Arc::new(StdRwLock::new(vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)])),
        }
    }
}

#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
#[derive(Clone)]
pub struct ScreenshareConfig {
    /// the screenshare capabilities. default until loaded
    capabilities: Arc<RwLock<Capabilities>>,

    /// a validated recording configuration
    pub(crate) recording_config: Arc<RwLock<Option<RecordingConfig>>>,

    /// the default width of the playback window
    pub(crate) width: Arc<AtomicU32>,

    /// the default height of the playback window
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
    pub async fn new(buffer: Vec<u8>) -> Self {
        let disk_config = ScreenshareConfigDisk::read_from_buffer(&buffer);
        let config = disk_config.map(ScreenshareConfig::from).unwrap_or_default();

        let capabilities_clone = Arc::clone(&config.capabilities);
        spawn_task(async move {
            let c = Capabilities::new().await;
            *capabilities_clone.write().await = c;
        });

        config
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    pub async fn new(_buffer: Vec<u8>) -> Self {
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
    ) -> Result<(), DartError> {
        let encoder = encoder_from_str(&encoder).map_err(|_| ErrorKind::InvalidEncoder)?;

        let recording_config = RecordingConfig {
            encoder,
            device: Device::from_str(&device).map_err(|_| "Invalid device".to_string())?,
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
    ) -> Result<(), DartError> {
        Ok(())
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn to_bytes(&self) -> Result<Vec<u8>, DartError> {
        ScreenshareConfigDisk::from(self)
            .write_to_vec()
            .map_err(|error| DartError::from(Error::from(error)))
    }
}

impl From<ScreenshareConfigDisk> for ScreenshareConfig {
    fn from(d: ScreenshareConfigDisk) -> Self {
        Self {
            capabilities: Arc::new(RwLock::new(Capabilities::default())),
            recording_config: Arc::new(RwLock::new(d.recording_config)),
            width: Arc::new(AtomicU32::new(d.width)),
            height: Arc::new(AtomicU32::new(d.height)),
        }
    }
}

#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
#[derive(Clone, Default)]
pub struct CodecConfig {
    /// whether to use the codec
    pub(crate) enabled: Arc<AtomicBool>,

    /// whether to use variable bitrate
    pub(crate) vbr: Arc<AtomicBool>,

    /// the compression level
    pub(crate) residual_bits: Arc<AtomicF32>,
}

impl CodecConfig {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn new(enabled: bool, vbr: bool, residual_bits: f32) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            vbr: Arc::new(AtomicBool::new(vbr)),
            residual_bits: Arc::new(AtomicF32::new(residual_bits)),
        }
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Relaxed);
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_vbr(&self, vbr: bool) {
        self.vbr.store(vbr, Relaxed);
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_residual_bits(&self, residual_bits: f32) {
        self.residual_bits.store(residual_bits, Relaxed);
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn to_values(&self) -> (bool, bool, f32) {
        (
            self.enabled.load(Relaxed),
            self.vbr.load(Relaxed),
            self.residual_bits.load(Relaxed),
        )
    }
}

/// capabilities for ffmpeg and ffplay supported by this client
#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct Capabilities {
    pub(crate) _available: bool,

    pub(crate) encoders: Vec<Encoder>,

    pub(crate) _decoders: Vec<Decoder>,

    pub(crate) devices: Vec<Device>,
}

impl Capabilities {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn encoders(&self) -> Vec<String> {
        self.encoders.iter().map(|e| e.to_string()).collect()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn devices(&self) -> Vec<String> {
        self.devices.iter().map(|d| d.to_string()).collect()
    }
}

/// recording config for screenshare
#[derive(Debug, Clone, Readable, Writable)]
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct RecordingConfig {
    pub(crate) encoder: Encoder,

    pub(crate) device: Device,

    pub(crate) bitrate: u32,

    pub(crate) framerate: u32,

    /// the height for the video output
    pub(crate) height: Option<u32>,
}

impl RecordingConfig {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn encoder(&self) -> String {
        let encoder_str: &str = self.encoder.into();
        encoder_str.to_string()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn device(&self) -> String {
        self.device.to_string()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn framerate(&self) -> u32 {
        self.framerate
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn height(&self) -> Option<u32> {
        self.height
    }
}

#[derive(Debug)]
pub struct DartError {
    pub message: String,
}

impl From<Error> for DartError {
    fn from(err: Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl From<ErrorKind> for DartError {
    fn from(kind: ErrorKind) -> Self {
        Self {
            message: Error { kind }.to_string(),
        }
    }
}

impl From<String> for DartError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum ManagerState {
    Stopped,
    Starting,
    Active,
    Failed,
}

fn serialize_timestamp_rfc3339_utc<S>(
    value: &DateTime<Local>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(
        &value
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

#[cfg(test)]
mod tests {
    use super::NetworkConfig;

    #[tokio::test]
    async fn network_config_defaults_and_mutation_work() {
        let config = NetworkConfig::default();
        assert_eq!(config.get_listen_port(), 0);
        assert_eq!(config.get_bind_addresses(), vec!["0.0.0.0".to_string()]);

        config.set_listen_port(7777);
        assert_eq!(config.get_listen_port(), 7777);

        config
            .set_bind_addresses(vec!["127.0.0.1".to_string(), "::1".to_string()])
            .expect("valid addresses should be accepted");
        assert_eq!(
            config.get_bind_addresses(),
            vec!["127.0.0.1".to_string(), "::1".to_string()]
        );
    }

    #[tokio::test]
    async fn network_config_new_and_validation_work() {
        let config = NetworkConfig::new(40142, vec!["0.0.0.0".to_string(), "::".to_string()])
            .expect("valid constructor input should succeed");
        assert_eq!(config.get_listen_port(), 40142);
        assert_eq!(
            config.get_bind_addresses(),
            vec!["0.0.0.0".to_string(), "::".to_string()]
        );

        let result = config.set_bind_addresses(vec!["not-an-ip".to_string()]);
        assert!(result.is_err());
    }
}
