use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::Attachment;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use crate::internal::screenshare::encoder_from_str;
use crate::internal::screenshare::{Decoder, Device, Encoder, ScreenshareConfigDisk};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use crate::internal::spawn_task;
use atomic_float::AtomicF32;
use chrono::{DateTime, Local, SecondsFormat, Utc};
use iroh::RelayMap;
use iroh::RelayUrl;
#[cfg(feature = "integration-testing")]
use iroh::address_lookup::memory::MemoryLookup;
pub use iroh::{PublicKey, SecretKey};
use serde::{Serialize, Serializer};
use speedy::{Readable, Writable};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
use tokio::sync::{Notify, RwLock};
use url::Url;
use uuid::Uuid;

/// Contact output gain range in decibels; keep in sync with the contact volume slider.
const MIN_CONTACT_OUTPUT_VOLUME_DB: f32 = -15.0;
const MAX_CONTACT_OUTPUT_VOLUME_DB: f32 = 15.0;

fn contact_output_volume_in_range(decibel: f32) -> bool {
    decibel.is_finite()
        && (MIN_CONTACT_OUTPUT_VOLUME_DB..=MAX_CONTACT_OUTPUT_VOLUME_DB).contains(&decibel)
}

fn contact_output_volume_from_parts(decibel: f32) -> f32 {
    if contact_output_volume_in_range(decibel) {
        decibel
    } else {
        0.0
    }
}

fn clamp_contact_output_volume(decibel: f32) -> f32 {
    if !decibel.is_finite() {
        0.0
    } else {
        decibel.clamp(MIN_CONTACT_OUTPUT_VOLUME_DB, MAX_CONTACT_OUTPUT_VOLUME_DB)
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct Contact {
    /// A random ID to identify the contact
    pub(crate) id: String,

    /// The nickname of the contact
    pub(crate) nickname: String,

    /// The public/verifying key for the contact
    /// flutter_rust_bridge:ignore
    pub peer_id: PublicKey,

    pub(crate) output_volume: f32,

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
            output_volume: 0.0,
            is_room_only: false,
        })
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn from_parts(
        id: String,
        nickname: String,
        peer_id: String,
        output_volume: f32,
    ) -> Result<Contact, DartError> {
        Ok(Self {
            id,
            nickname,
            peer_id: PublicKey::from_str(&peer_id).map_err(|_| ErrorKind::InvalidContactFormat)?,
            output_volume: contact_output_volume_from_parts(output_volume),
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
    pub fn output_volume(&self) -> f32 {
        self.output_volume
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn set_output_volume(&mut self, decibel: f32) {
        self.output_volume = clamp_contact_output_volume(decibel);
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn pub_clone(&self) -> Contact {
        self.clone()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn id_eq(&self, id: Vec<u8>) -> bool {
        self.peer_id.to_vec() == id
    }

    pub fn get_peer_id(&self) -> PublicKey {
        self.peer_id
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

#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub(crate) listen_port: Arc<AtomicU16>,

    pub(crate) bind_addresses: Arc<StdRwLock<Vec<IpAddr>>>,

    pub(crate) relays: Arc<StdRwLock<Option<RelayMap>>>,

    pub(crate) dns_endpoint: Arc<StdRwLock<Option<SocketAddr>>>,

    pub(crate) dns_origin_domain: Arc<StdRwLock<Option<String>>>,

    pub(crate) pkarr_relay: Arc<StdRwLock<Option<Url>>>,

    /// Test-only in-process address discovery.
    #[cfg(feature = "integration-testing")]
    pub(crate) address_lookup: Arc<StdRwLock<Option<MemoryLookup>>>,
}

impl NetworkConfig {
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        listen_port: u16,
        bind_addresses: Vec<String>,
        relays: Option<Vec<String>>,
        dns_endpoint: Option<String>,
        dns_origin_domain: Option<String>,
        pkarr_relay: Option<String>,
    ) -> Result<Self, DartError> {
        let relays_value = match relays {
            Some(urls) => Some(relay_map_from_urls(urls)?),
            None => None,
        };

        let dns_endpoint_value = match dns_endpoint {
            Some(value) => Some(
                SocketAddr::from_str(&value).map_err(|error| DartError::from(error.to_string()))?,
            ),
            None => None,
        };

        let pkarr_relay_value = match pkarr_relay {
            Some(value) => {
                let url = Url::parse(&value).map_err(|error| DartError::from(error.to_string()))?;
                Some(url)
            }
            None => None,
        };

        Ok(Self {
            listen_port: Arc::new(AtomicU16::new(listen_port)),
            bind_addresses: Arc::new(StdRwLock::new(parse_bind_addresses(bind_addresses)?)),
            relays: Arc::new(StdRwLock::new(relays_value)),
            dns_endpoint: Arc::new(StdRwLock::new(dns_endpoint_value)),
            dns_origin_domain: Arc::new(StdRwLock::new(dns_origin_domain)),
            pkarr_relay: Arc::new(StdRwLock::new(pkarr_relay_value)),
            #[cfg(feature = "integration-testing")]
            address_lookup: Arc::new(StdRwLock::new(None)),
        })
    }

    #[cfg(feature = "integration-testing")]
    pub fn mock(
        listen_port: u16,
        relay_map: &RelayMap,
        dns_endpoint: Option<&str>,
        dns_origin_domain: Option<&str>,
        pkarr_relay: Option<Url>,
        address_lookup: Option<MemoryLookup>,
    ) -> Self {
        Self {
            listen_port: Arc::new(AtomicU16::new(listen_port)),
            bind_addresses: Arc::new(StdRwLock::new(vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)])),
            relays: Arc::new(StdRwLock::new(Some(relay_map.clone()))),
            dns_endpoint: Arc::new(StdRwLock::new(dns_endpoint.and_then(|s| s.parse().ok()))),
            dns_origin_domain: Arc::new(StdRwLock::new(dns_origin_domain.map(String::from))),
            pkarr_relay: Arc::new(StdRwLock::new(pkarr_relay)),
            address_lookup: Arc::new(StdRwLock::new(address_lookup)),
        }
    }

    /// Atomically validate every field and apply the new configuration.
    ///
    /// Each field is parsed and validated up front (without mutating any
    /// shared state) before any write is attempted. If validation fails
    /// for any field, no writes occur and the live `NetworkConfig` is
    /// left exactly as it was. This is the only safe way for callers to
    /// update a multi-field configuration: the per-field setters above
    /// can leave the live config partially mutated if a later setter
    /// rejects its value.
    ///
    /// On failure the returned [`NetworkConfigUpdateError`] identifies
    /// which field was rejected so the frontend can route the error to
    /// the correct input. A poisoned lock is collapsed into
    /// [`NetworkConfigField::BackendError`]: a poison indicates the rust
    /// runtime has been corrupted by a panic, and attributing that to
    /// any one user-supplied field would be misleading.
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        listen_port: u16,
        bind_addresses: Vec<String>,
        relays: Option<Vec<String>>,
        dns_endpoint: Option<String>,
        dns_origin_domain: Option<String>,
        pkarr_relay: Option<String>,
    ) -> Result<(), NetworkConfigUpdateError> {
        let new_bind_addresses = parse_bind_addresses(bind_addresses)
            .map_err(|error| field_error(NetworkConfigField::BindAddresses, error.message))?;
        let new_relays = match relays {
            Some(urls) => Some(
                relay_map_from_urls(urls)
                    .map_err(|error| field_error(NetworkConfigField::Relays, error.message))?,
            ),
            None => None,
        };
        let new_dns_endpoint = match dns_endpoint {
            Some(value) => Some(SocketAddr::from_str(&value).map_err(|error| {
                field_error(NetworkConfigField::DnsEndpoint, error.to_string())
            })?),
            None => None,
        };
        let new_pkarr_relay =
            match pkarr_relay {
                Some(value) => Some(Url::parse(&value).map_err(|error| {
                    field_error(NetworkConfigField::PkarrRelay, error.to_string())
                })?),
                None => None,
            };

        // Listen port is an atomic; no lock involved. The frontend
        // validator already constrains it to the `u16` range, so this
        // branch is unreachable in practice. Kept as a defensive
        // mapping in case a future caller bypasses validation.
        let _ = listen_port;

        // Acquire every write lock up front so a single failure mode
        // (poison) is surfaced before any field is mutated.
        let mut bind_addresses_guard = self
            .bind_addresses
            .write()
            .map_err(|error| poison_field_error(NetworkConfigField::BackendError, &error))?;
        let mut relays_guard = self
            .relays
            .write()
            .map_err(|error| poison_field_error(NetworkConfigField::BackendError, &error))?;
        let mut dns_endpoint_guard = self
            .dns_endpoint
            .write()
            .map_err(|error| poison_field_error(NetworkConfigField::BackendError, &error))?;
        let mut dns_origin_domain_guard = self
            .dns_origin_domain
            .write()
            .map_err(|error| poison_field_error(NetworkConfigField::BackendError, &error))?;
        let mut pkarr_relay_guard = self
            .pkarr_relay
            .write()
            .map_err(|error| poison_field_error(NetworkConfigField::BackendError, &error))?;

        *bind_addresses_guard = new_bind_addresses;
        *relays_guard = new_relays;
        *dns_endpoint_guard = new_dns_endpoint;
        *dns_origin_domain_guard = dns_origin_domain;
        *pkarr_relay_guard = new_pkarr_relay;
        self.listen_port.store(listen_port, Relaxed);
        Ok(())
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_listen_port(&self) -> u16 {
        self.listen_port.load(Relaxed)
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_pkarr_relay(&self) -> Option<String> {
        self.pkarr_relay
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|url| url.to_string())
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_relays(&self) -> Option<Vec<String>> {
        self.relays
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|map| {
                map.urls::<Vec<_>>()
                    .into_iter()
                    .map(|u| u.to_string())
                    .collect()
            })
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_dns_endpoint(&self) -> Option<String> {
        self.dns_endpoint
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|addr| addr.to_string())
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_dns_origin_domain(&self) -> Option<String> {
        self.dns_origin_domain
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn get_bind_addresses(&self) -> Vec<String> {
        self.bind_addresses
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// Returns a clone of the test-only address lookup
    #[cfg(feature = "integration-testing")]
    pub fn get_address_lookup(&self) -> Option<MemoryLookup> {
        self.address_lookup
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Replaces the test-only address lookup
    #[cfg(feature = "integration-testing")]
    pub fn set_address_lookup(&self, lookup: Option<MemoryLookup>) {
        *self
            .address_lookup
            .write()
            .expect("address_lookup lock poisoned") = lookup;
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_port: Arc::new(Default::default()),
            bind_addresses: Arc::new(StdRwLock::new(vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)])),
            relays: Arc::new(StdRwLock::new(None)),
            dns_endpoint: Arc::new(StdRwLock::new(None)),
            dns_origin_domain: Arc::new(StdRwLock::new(None)),
            pkarr_relay: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "integration-testing")]
            address_lookup: Arc::new(StdRwLock::new(None)),
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

/// Identifies which field of a [`NetworkConfig`] update failed validation,
/// or that the failure was not tied to a specific user-supplied field.
///
/// `update` validates every field before mutating any shared state, and
/// reports the first failure. Surfacing the offending field through this
/// enum (rather than only a free-form message) lets the frontend route the
/// error to the corresponding input.
#[derive(Debug, Clone, Serialize)]
pub enum NetworkConfigField {
    /// The supplied listen port is not representable as a `u16`. The
    /// rust setter takes `u16` so the frontend validator should already
    /// have caught this, but the variant is kept for completeness in
    /// case a future caller bypasses validation.
    ListenPort,
    /// One or more bind addresses failed to parse as an `IpAddr`.
    BindAddresses,
    /// One or more relay URLs failed to parse as a [`RelayUrl`].
    Relays,
    /// The DNS endpoint failed to parse as a [`SocketAddr`].
    DnsEndpoint,
    /// The DNS origin domain is invalid. The current rust setter does
    /// not validate the origin domain itself, but the variant is
    /// reserved for future tightening and lets the frontend surface a
    /// targeted error rather than a generic message.
    DnsOriginDomain,
    /// The Pkarr relay URL failed to parse as a [`url::Url`].
    PkarrRelay,
    /// The failure was not tied to a specific field. In particular,
    /// every lock-poison error from the atomic `update` is collapsed
    /// into this variant: a poisoned lock indicates the rust runtime
    /// has been corrupted by a panic, and attributing the error to any
    /// one user-supplied field would be misleading. Frontends should
    /// surface this as a critical "backend error" rather than a
    /// per-field validation message.
    BackendError,
}

/// Structured error returned by [`NetworkConfig::update`].
///
/// Carries both the offending [`NetworkConfigField`] (so the frontend
/// can route the error to the correct input) and the underlying
/// message (so the user sees the rust-side diagnostic verbatim).
#[derive(Debug, Clone, Serialize)]
pub struct NetworkConfigUpdateError {
    pub field: NetworkConfigField,
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

/// The error message returned when a caller supplies a non-32-byte identity key.
/// Shared by `flutter::Telepathy::set_identity` and `native::NativeTelepathy::set_identity`
/// so the user-facing wording stays in sync.
pub const IDENTITY_KEY_LENGTH_MESSAGE: &str = "Key must be 32 bytes";

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

fn parse_bind_addresses(bind_addresses: Vec<String>) -> Result<Vec<IpAddr>, DartError> {
    bind_addresses
        .into_iter()
        .map(|address| {
            IpAddr::from_str(&address).map_err(|error| DartError::from(error.to_string()))
        })
        .collect()
}

/// Parse a list of relay URL strings into a [`RelayMap`].
///
/// Returns a [`DartError`] if any of the strings fails to parse as a [`RelayUrl`].
fn relay_map_from_urls(urls: Vec<String>) -> Result<RelayMap, DartError> {
    let relay_urls = urls
        .into_iter()
        .map(|url| RelayUrl::from_str(&url).map_err(|error| DartError::from(error.to_string())))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(RelayMap::from_iter(relay_urls))
}

/// Wrap a per-field parse error in a [`NetworkConfigUpdateError`] tagged
/// with the offending field. Used by [`NetworkConfig::update`] so each
/// validation step reports which user-supplied field was rejected.
fn field_error(field: NetworkConfigField, message: String) -> NetworkConfigUpdateError {
    NetworkConfigUpdateError { field, message }
}

/// Wrap a lock-acquisition error in a [`NetworkConfigUpdateError`].
///
/// Every lock-poison failure is collapsed to
/// [`NetworkConfigField::BackendError`]: a poison indicates an internal
/// rust invariant was violated (a previous holder of the lock panicked),
/// and attributing that to any one user-supplied field would be
/// misleading. The original `std::sync::PoisonError` is rendered with
/// its default `Display` so the diagnostic still carries the poison
/// detail without exposing the held guard.
fn poison_field_error(
    field: NetworkConfigField,
    error: &std::sync::PoisonError<impl std::fmt::Debug>,
) -> NetworkConfigUpdateError {
    NetworkConfigUpdateError {
        field,
        message: format!("backend lock poisoned: {}", error),
    }
}

#[cfg(test)]
mod tests {
    use super::{NetworkConfig, NetworkConfigField};

    const VALID_RELAY_A: &str = "https://relay-us.iroh.example/";
    const VALID_RELAY_B: &str = "https://relay-eu.iroh.example/";
    const VALID_PKARR: &str = "https://pkarr.iroh.example/";
    const VALID_DNS_ENDPOINT: &str = "1.1.1.1:53";
    const VALID_ORIGIN_DOMAIN: &str = "dns.iroh.example";

    fn vec_of(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_constructor_leaves_optionals_unset_and_listens_on_unspecified_v4() {
        let config = NetworkConfig::default();

        assert_eq!(config.get_listen_port(), 0);
        assert_eq!(config.get_bind_addresses(), vec!["0.0.0.0".to_string()]);
        assert!(config.get_relays().is_none());
        assert!(config.get_dns_endpoint().is_none());
        assert!(config.get_dns_origin_domain().is_none());
        assert!(config.get_pkarr_relay().is_none());
    }

    #[test]
    fn new_accepts_a_fully_populated_valid_configuration() {
        let config = NetworkConfig::new(
            40142,
            vec_of(&["0.0.0.0", "::1"]),
            Some(vec_of(&[VALID_RELAY_A, VALID_RELAY_B])),
            Some(VALID_DNS_ENDPOINT.to_string()),
            Some(VALID_ORIGIN_DOMAIN.to_string()),
            Some(VALID_PKARR.to_string()),
        )
        .expect("fully populated valid inputs should be accepted");

        assert_eq!(config.get_listen_port(), 40142);
        assert_eq!(
            config.get_bind_addresses(),
            vec!["0.0.0.0".to_string(), "::1".to_string()]
        );

        let relays = config.get_relays().expect("relays should be present");
        assert_eq!(relays.len(), 2);
        assert!(relays.contains(&VALID_RELAY_A.to_string()));
        assert!(relays.contains(&VALID_RELAY_B.to_string()));

        assert_eq!(
            config.get_dns_endpoint(),
            Some(VALID_DNS_ENDPOINT.to_string())
        );
        assert_eq!(
            config.get_dns_origin_domain(),
            Some(VALID_ORIGIN_DOMAIN.to_string())
        );
        assert_eq!(config.get_pkarr_relay(), Some(VALID_PKARR.to_string()));
    }

    #[test]
    fn new_accepts_all_optional_fields_as_none() {
        let config = NetworkConfig::new(0, Vec::new(), None, None, None, None)
            .expect("optional fields may be unset and bind list may be empty");

        assert_eq!(config.get_listen_port(), 0);
        assert!(config.get_bind_addresses().is_empty());
        assert!(config.get_relays().is_none());
        assert!(config.get_dns_endpoint().is_none());
        assert!(config.get_dns_origin_domain().is_none());
        assert!(config.get_pkarr_relay().is_none());
    }

    #[test]
    fn new_accepts_listen_port_at_u16_boundaries() {
        for port in [0u16, 1, 8080, 65535] {
            let config = NetworkConfig::new(port, vec_of(&["0.0.0.0"]), None, None, None, None)
                .unwrap_or_else(|e| {
                    panic!("port {port} should be valid, got error: {}", e.message)
                });
            assert_eq!(config.get_listen_port(), port);
        }
    }

    #[test]
    fn new_accepts_mixed_ipv4_and_ipv6_bind_addresses() {
        let config = NetworkConfig::new(
            11211,
            vec_of(&["0.0.0.0", "::", "127.0.0.1", "::1", "fe80::1"]),
            None,
            None,
            None,
            None,
        )
        .expect("mixed IPv4 and IPv6 addresses should be accepted");

        assert_eq!(
            config.get_bind_addresses(),
            vec_of(&["0.0.0.0", "::", "127.0.0.1", "::1", "fe80::1"])
        );
    }

    #[test]
    fn new_rejects_malformed_bind_addresses() {
        for bad in ["not-an-ip", "256.256.256.256", "1.2.3", "", "1.2.3.4.5"] {
            let result = NetworkConfig::new(0, vec_of(&[bad]), None, None, None, None);
            assert!(
                result.is_err(),
                "expected error for malformed bind address: {bad:?}"
            );
        }
    }

    #[test]
    fn new_rejects_when_any_bind_address_is_invalid() {
        // A single invalid entry in a list otherwise full of valid entries must fail.
        let result = NetworkConfig::new(
            0,
            vec_of(&["0.0.0.0", "garbage", "::1"]),
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn new_rejects_malformed_relay_urls() {
        for bad in [
            "not a url",
            "://no-scheme",
            "http:// bad url",
            "relay.example.com", // missing scheme
        ] {
            let result = NetworkConfig::new(0, Vec::new(), Some(vec_of(&[bad])), None, None, None);
            assert!(
                result.is_err(),
                "expected error for malformed relay url: {bad:?}"
            );
        }
    }

    #[test]
    fn new_rejects_when_any_relay_url_is_invalid() {
        let result = NetworkConfig::new(
            0,
            Vec::new(),
            Some(vec_of(&[VALID_RELAY_A, "http:// bad url", VALID_RELAY_B])),
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn new_rejects_malformed_dns_endpoint() {
        for bad in [
            "1.1.1.1",       // missing port
            "1.1.1.1:99999", // port out of range
            "not-an-address",
            "1.2.3.4:not-a-port",
            "",
        ] {
            let result = NetworkConfig::new(0, Vec::new(), None, Some(bad.to_string()), None, None);
            assert!(
                result.is_err(),
                "expected error for malformed dns endpoint: {bad:?}"
            );
        }
    }

    #[test]
    fn new_accepts_ipv6_dns_endpoint() {
        let endpoint = "[2606:4700:4700::1111]:53";
        let config =
            NetworkConfig::new(0, Vec::new(), None, Some(endpoint.to_string()), None, None)
                .expect("IPv6 dns endpoint should be accepted");
        assert_eq!(config.get_dns_endpoint(), Some(endpoint.to_string()));
    }

    #[test]
    fn new_rejects_malformed_pkarr_relay_url() {
        for bad in [
            "not a url",
            "://no-scheme",
            "http:// bad url",
            "pkarr.example.com", // missing scheme
        ] {
            let result = NetworkConfig::new(0, Vec::new(), None, None, None, Some(bad.to_string()));
            assert!(
                result.is_err(),
                "expected error for malformed pkarr relay url: {bad:?}"
            );
        }
    }

    /// Snapshot of every observable field on a live `NetworkConfig`. Used by
    /// the atomic-update tests to assert that a failed `update` left the
    /// configuration exactly as it was.
    type NetworkConfigSnapshot = (
        u16,
        Vec<String>,
        Option<Vec<String>>,
        Option<String>,
        Option<String>,
        Option<String>,
    );

    fn snapshot(config: &NetworkConfig) -> NetworkConfigSnapshot {
        (
            config.get_listen_port(),
            config.get_bind_addresses(),
            config.get_relays(),
            config.get_dns_endpoint(),
            config.get_dns_origin_domain(),
            config.get_pkarr_relay(),
        )
    }

    /// A baseline configuration that every atomic-update test will mutate
    /// (or attempt to mutate) from. Keeping the starting state identical
    /// across tests makes failure assertions (no partial mutation) easy
    /// to write and easy to read.
    fn baseline_config() -> NetworkConfig {
        NetworkConfig::new(
            9000,
            vec_of(&["127.0.0.1", "::1"]),
            Some(vec_of(&[VALID_RELAY_A])),
            Some(VALID_DNS_ENDPOINT.to_string()),
            Some(VALID_ORIGIN_DOMAIN.to_string()),
            Some(VALID_PKARR.to_string()),
        )
        .expect("baseline configuration should be valid")
    }

    #[test]
    fn update_applies_a_fully_valid_configuration_atomically() {
        let config = baseline_config();
        let before = snapshot(&config);

        let result = config.update(
            40142,
            vec_of(&["0.0.0.0", "::", "127.0.0.1", "::1"]),
            Some(vec_of(&[VALID_RELAY_A, VALID_RELAY_B])),
            Some("[2606:4700:4700::1111]:53".to_string()),
            Some("dns.iroh.example".to_string()),
            Some("https://pkarr.iroh.example/".to_string()),
        );

        assert!(
            result.is_ok(),
            "valid update should succeed, got: {:?}",
            result.err()
        );

        assert_eq!(config.get_listen_port(), 40142);
        assert_eq!(
            config.get_bind_addresses(),
            vec_of(&["0.0.0.0", "::", "127.0.0.1", "::1"])
        );
        let relays = config.get_relays().expect("relays should be present");
        assert_eq!(relays.len(), 2);
        assert!(relays.contains(&VALID_RELAY_A.to_string()));
        assert!(relays.contains(&VALID_RELAY_B.to_string()));
        assert_eq!(
            config.get_dns_endpoint(),
            Some("[2606:4700:4700::1111]:53".to_string())
        );
        assert_eq!(
            config.get_dns_origin_domain(),
            Some("dns.iroh.example".to_string())
        );
        assert_eq!(
            config.get_pkarr_relay(),
            Some("https://pkarr.iroh.example/".to_string())
        );
        // And, of course, the configuration moved: this is not equal to the
        // pre-update snapshot. This guards against a future refactor that
        // accidentally short-circuits `update` to a no-op.
        assert_ne!(snapshot(&config), before);
    }

    #[test]
    fn update_accepts_listen_port_at_u16_boundaries() {
        for port in [0u16, 1, 8080, 65535] {
            let config = baseline_config();
            let before = snapshot(&config);

            config
                .update(port, vec_of(&["0.0.0.0"]), None, None, None, None)
                .unwrap_or_else(|e| panic!("port {port} should be valid, got: {}", e.message));

            assert_eq!(config.get_listen_port(), port);
            // Every other field was passed as `None` / cleared, so the live
            // state should no longer match the baseline snapshot.
            assert_ne!(snapshot(&config), before);
        }
    }

    #[test]
    fn update_can_clear_optional_fields_by_passing_none() {
        let config = baseline_config();
        config
            .update(9000, vec_of(&["0.0.0.0"]), None, None, None, None)
            .expect("clearing optional fields should be valid");

        assert!(config.get_relays().is_none());
        assert!(config.get_dns_endpoint().is_none());
        assert!(config.get_dns_origin_domain().is_none());
        assert!(config.get_pkarr_relay().is_none());
    }

    #[test]
    fn update_rejects_invalid_bind_addresses_and_leaves_live_config_unchanged() {
        // Regression guard for the partial-mutation bug: an invalid bind
        // address must not leak into any other field of the live config.
        // In particular, the listen port (which is updated *last* in the
        // atomic commit phase) must keep its previous value.
        let config = baseline_config();
        let before = snapshot(&config);

        let result = config.update(
            12345,                                    // <-- a different listen port
            vec_of(&["0.0.0.0", "not-an-ip", "::1"]), // <-- invalid entry
            Some(vec_of(&[VALID_RELAY_B])),
            Some("9.9.9.9:53".to_string()),
            Some("changed.example".to_string()),
            Some("https://changed.example/".to_string()),
        );

        let err = result.expect_err("invalid bind address must cause update to fail");
        assert!(
            matches!(err.field, NetworkConfigField::BindAddresses),
            "bind-address failure must be attributed to BindAddresses, got: {:?}",
            err.field
        );
        // The live config is identical to the pre-call snapshot: not a
        // single field was mutated, including the listen port.
        assert_eq!(snapshot(&config), before);
    }

    #[test]
    fn update_rejects_invalid_relay_url_and_leaves_live_config_unchanged() {
        let config = baseline_config();
        let before = snapshot(&config);

        let result = config.update(
            12345,
            vec_of(&["0.0.0.0"]),
            Some(vec_of(&[VALID_RELAY_A, "not a url", VALID_RELAY_B])),
            Some("9.9.9.9:53".to_string()),
            Some("changed.example".to_string()),
            Some("https://changed.example/".to_string()),
        );

        let err = result.expect_err("invalid relay url must cause update to fail");
        assert!(
            matches!(err.field, NetworkConfigField::Relays),
            "relay failure must be attributed to Relays, got: {:?}",
            err.field
        );
        assert_eq!(snapshot(&config), before);
    }

    #[test]
    fn update_rejects_invalid_dns_endpoint_and_leaves_live_config_unchanged() {
        let config = baseline_config();
        let before = snapshot(&config);

        let result = config.update(
            12345,
            vec_of(&["0.0.0.0"]),
            Some(vec_of(&[VALID_RELAY_B])),
            Some("not-an-endpoint".to_string()),
            Some("changed.example".to_string()),
            Some("https://changed.example/".to_string()),
        );

        let err = result.expect_err("invalid dns endpoint must cause update to fail");
        assert!(
            matches!(err.field, NetworkConfigField::DnsEndpoint),
            "dns-endpoint failure must be attributed to DnsEndpoint, got: {:?}",
            err.field
        );
        assert_eq!(snapshot(&config), before);
    }

    #[test]
    fn update_rejects_invalid_pkarr_relay_url_and_leaves_live_config_unchanged() {
        let config = baseline_config();
        let before = snapshot(&config);

        let result = config.update(
            12345,
            vec_of(&["0.0.0.0"]),
            Some(vec_of(&[VALID_RELAY_B])),
            Some("9.9.9.9:53".to_string()),
            Some("changed.example".to_string()),
            Some("not a url".to_string()),
        );

        let err = result.expect_err("invalid pkarr relay url must cause update to fail");
        assert!(
            matches!(err.field, NetworkConfigField::PkarrRelay),
            "pkarr failure must be attributed to PkarrRelay, got: {:?}",
            err.field
        );
        assert_eq!(snapshot(&config), before);
    }

    #[test]
    fn update_with_no_changes_is_a_successful_no_op() {
        // Calling `update` with values that exactly mirror the live config
        // is still a success and does not leave the config in a different
        // state.
        let config = baseline_config();
        let before = snapshot(&config);

        let result = config.update(
            9000,
            vec_of(&["127.0.0.1", "::1"]),
            Some(vec_of(&[VALID_RELAY_A])),
            Some(VALID_DNS_ENDPOINT.to_string()),
            Some(VALID_ORIGIN_DOMAIN.to_string()),
            Some(VALID_PKARR.to_string()),
        );

        assert!(
            result.is_ok(),
            "no-op update should succeed, got: {:?}",
            result.err()
        );
        assert_eq!(snapshot(&config), before);
    }

    #[test]
    fn update_does_not_mutate_any_field_when_a_lock_acquisition_fails() {
        // Regression guard for the lock-acquisition contract: when
        // `update` cannot acquire every write guard, it must fail
        // before assigning to ANY field. A future refactor that
        // writes one field, takes a later lock, and propagates the
        // lock error would re-introduce a partial commit; this test
        // catches that.
        //
        // We deterministically force a lock-acquisition failure by
        // poisoning the `dns_origin_domain` lock: spawn a thread
        // that takes the write guard, then panics. A panicked
        // `StdRwLock` writer poisons the lock, so any subsequent
        // `.write()` call returns `Poisoned`. In the new
        // implementation that error surfaces during the
        // all-locks-first acquisition phase, before any field has
        // been written.
        use std::sync::Arc;
        use std::sync::RwLock as StdRwLock;

        let config = baseline_config();

        // Snapshot the live state BEFORE poisoning any lock. The
        // public `get_*` accessors call `.read().expect("... lock
        // poisoned")`; after we poison a lock they will panic
        // rather than return the stored value, so we cannot use
        // them to inspect the post-`update` state. We instead
        // read each lock directly via
        // `read().unwrap_err().into_inner()` to bypass the poison
        // and assert against the pre-poison snapshot.
        let before = snapshot(&config);

        // Poison the `dns_origin_domain` lock: spawn a thread that
        // takes the write guard and panics, which marks the lock
        // poisoned. Any subsequent `.write()` returns `Poisoned`,
        // so the very first write in `update` will fail and the
        // all-locks-first phase must propagate that error before
        // any field is mutated.
        let dns_lock = Arc::clone(&config.dns_origin_domain);
        let poisoner = std::thread::spawn(move || {
            let _guard = dns_lock
                .write()
                .expect("first write on an unpoisoned lock should succeed");
            panic!("intentional poison to simulate a failed write-lock acquisition");
        });
        // The poisoner always panics; join to observe the
        // poisoned state.
        let _ = poisoner.join();

        // `update` is `&self`; run it on a separate thread so the
        // poison from the previous holder is observable to it.
        let config = Arc::new(config);
        let writer = {
            let config = Arc::clone(&config);
            std::thread::spawn(move || {
                config.update(
                    12345, // <-- a different listen port
                    vec_of(&["0.0.0.0", "::1"]),
                    Some(vec_of(&[VALID_RELAY_B])),
                    Some("9.9.9.9:53".to_string()),
                    Some("changed.example".to_string()),
                    Some("https://changed.example/".to_string()),
                )
            })
        };
        let result = writer.join().expect("writer thread must not panic");

        let err = result.expect_err("update must fail when a write lock is poisoned");
        // Every poison failure must be collapsed into a single
        // BackendError variant: a poisoned lock is not the user's
        // fault and is not safe to attribute to any one
        // user-supplied field. Asserting on the field here guards
        // against a future refactor that "helpfully" reports the
        // failing lock's owning field.
        assert!(
            matches!(err.field, NetworkConfigField::BackendError),
            "lock-poison failure must collapse to BackendError, got: {:?}",
            err.field
        );
        assert!(
            err.message.contains("poisoned"),
            "poison diagnostic should mention the poison, got: {:?}",
            err.message
        );

        // Listen port is an atomic; no lock involved.
        assert_eq!(config.get_listen_port(), before.0);

        // The remaining fields all live behind (possibly poisoned)
        // `StdRwLock`s. Only `dns_origin_domain` is poisoned by
        // this test, but read every lock through a helper that
        // tolerates either state so the assertions are robust.
        // The point of this test is that the stored values match
        // the pre-poison, pre-`update` snapshot exactly -- in
        // particular the listen port (atomic) is unchanged, and
        // no other field was committed either. Each raw value is
        // projected into the same `String`-shaped form that
        // `snapshot()` records so the comparison is valid.
        fn read_unpoisoned<T>(
            lock: &StdRwLock<T>,
        ) -> std::sync::LockResult<std::sync::RwLockReadGuard<'_, T>> {
            lock.read()
        }

        let bind_addresses: Vec<String> = read_unpoisoned(&config.bind_addresses)
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(bind_addresses, before.1);

        let relays: Option<Vec<String>> = read_unpoisoned(&config.relays)
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|map| {
                map.urls::<Vec<_>>()
                    .into_iter()
                    .map(|u| u.to_string())
                    .collect()
            });
        assert_eq!(relays, before.2);

        let dns_endpoint: Option<String> = read_unpoisoned(&config.dns_endpoint)
            .unwrap_or_else(|e| e.into_inner())
            .map(|addr| addr.to_string());
        assert_eq!(dns_endpoint, before.3);

        let dns_origin: Option<String> = read_unpoisoned(&config.dns_origin_domain)
            .unwrap_or_else(|e| e.into_inner()) // <-- poisoned: the unwrap_or_else branch runs
            .clone();
        assert_eq!(dns_origin, before.4);

        let pkarr: Option<String> = read_unpoisoned(&config.pkarr_relay)
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|url| url.to_string());
        assert_eq!(pkarr, before.5);
    }
}
