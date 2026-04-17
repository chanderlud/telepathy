use crate::error::{DartError, Error, ErrorKind};
use crate::internal::ConnectionState;
use crate::internal::messages::Attachment;
use crate::internal::runtime::spawn_task;
pub use crate::internal::screenshare::{Capabilities, RecordingConfig};
use crate::internal::screenshare::{Device, ScreenshareConfigDisk, encoder_from_str};
use atomic_float::AtomicF32;
use chrono::{DateTime, Local};
use libp2p::PeerId;
use speedy::{Readable, Writable};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
#[cfg(not(target_family = "wasm"))]
use tokio::net::lookup_host;
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
    pub(crate) peer_id: PeerId,

    /// In rooms, some contacts are dummy representing unknown peers
    pub(crate) is_room_only: bool,
}

impl Contact {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn new(nickname: String, peer_id: String) -> Result<Contact, DartError> {
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            nickname,
            peer_id: PeerId::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
            is_room_only: false,
        })
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn from_parts(id: String, nickname: String, peer_id: String) -> Result<Contact, DartError> {
        Ok(Self {
            id,
            nickname,
            peer_id: PeerId::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
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
        self.peer_id.to_bytes() == id
    }
}

#[derive(Debug)]
pub enum CallState {
    Connected,
    Waiting,
    RoomJoin(String),
    RoomLeave(String),
    CallEnded(String, bool),
}

#[derive(Debug)]
pub enum SessionStatus {
    Connecting,
    Connected {
        relayed: bool,
        remote_address: String,
    },
    Inactive,
    Unknown,
}

impl From<ConnectionState> for SessionStatus {
    fn from(value: ConnectionState) -> Self {
        Self::Connected {
            relayed: value.relayed,
            remote_address: value
                .remote_address
                .map(|a| a.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        }
    }
}

pub struct ChatMessage {
    pub text: String,

    pub(crate) receiver: PeerId,

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

#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
#[derive(Clone)]
pub struct NetworkConfig {
    /// the relay server's address
    pub(crate) relay_address: Arc<RwLock<SocketAddr>>,

    /// the relay server's peer id
    pub(crate) relay_id: Arc<RwLock<PeerId>>,

    /// the libp2p port for the swarm
    pub(crate) listen_port: Arc<AtomicU16>,

    /// the addresses libp2p will listen on
    pub(crate) bind_addresses: Vec<IpAddr>,
}

impl NetworkConfig {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn new(relay_address: String, relay_id: String) -> Result<Self, DartError> {
        Ok(Self {
            relay_address: Arc::new(RwLock::new(relay_address.parse().map_err(Error::from)?)),
            relay_id: Arc::new(RwLock::new(
                PeerId::from_str(&relay_id).map_err(Error::from)?,
            )),
            listen_port: Arc::new(AtomicU16::new(0)),
            bind_addresses: vec![
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            ],
        })
    }

    #[cfg(test)]
    pub(crate) fn mock(
        relay_address: SocketAddr,
        relay_id: PeerId,
        port: u16,
        bind_addresses: Vec<IpAddr>,
    ) -> Self {
        Self {
            relay_address: Arc::new(RwLock::new(relay_address)),
            relay_id: Arc::new(RwLock::new(relay_id)),
            listen_port: Arc::new(AtomicU16::new(port)),
            bind_addresses,
        }
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

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            relay_address: Arc::new(RwLock::new(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                0,
            )))),
            relay_id: Arc::new(RwLock::new(PeerId::random())),
            listen_port: Arc::new(Default::default()),
            bind_addresses: vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)],
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
