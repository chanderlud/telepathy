use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::{AudioHeader, ProtocolMessage, RoomMessage};
use crate::internal::{HELLO_TIMEOUT, Result, STREAM_PROTOCOL};
use crate::types::{CodecConfig, NetworkConfig, ScreenshareConfig, SessionStatus};
use atomic_float::AtomicF32;
use kanal::{AsyncReceiver, AsyncSender, unbounded_async};
use libp2p::core::ConnectedPoint;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::ConnectionId;
use libp2p::{PeerId, Stream};
use libp2p_stream::Control;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Duration;
use telepathy_audio::RnnModel;
use tokio::select;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type SharedDeviceId = Arc<Mutex<Option<String>>>;

#[derive(Clone, Default)]
pub(crate) struct CoreState {
    /// Controls the threshold for silence detection
    pub(crate) rms_threshold: Arc<AtomicF32>,

    /// The factor to adjust the input volume by
    pub(crate) input_volume: Arc<AtomicF32>,

    /// The factor to adjust the output volume by
    pub(crate) output_volume: Arc<AtomicF32>,

    /// Enables rnnoise denoising
    pub(crate) denoise: Arc<AtomicBool>,

    /// The rnnoise model
    pub(crate) denoise_model: Arc<RwLock<RnnModel>>,

    /// Manually set the input device
    pub(crate) input_device: SharedDeviceId,

    /// Manually set the output device
    pub(crate) output_device: SharedDeviceId,

    /// The current libp2p private key
    pub(crate) identity: Arc<RwLock<Option<Keypair>>>,

    /// Keeps track of whether the user is in a call
    pub(crate) in_call: Arc<AtomicBool>,

    /// used to end an audio test, if there is one
    pub(crate) end_audio_test: Arc<Mutex<Option<Arc<Notify>>>>,

    /// Disables the output stream
    pub(crate) deafened: Arc<AtomicBool>,

    /// Disables the input stream
    pub(crate) muted: Arc<AtomicBool>,

    /// Disables the playback of custom ringtones
    pub(crate) play_custom_ringtones: Arc<AtomicBool>,

    /// Enables sending your custom ringtone
    pub(crate) send_custom_ringtone: Arc<AtomicBool>,

    /// Decreases the statistics update rate
    pub(crate) efficiency_mode: Arc<AtomicBool>,

    /// Pauses statistics callbacks when window is minimized
    pub(crate) statistics_paused: Arc<AtomicBool>,

    /// set to true at shutdown to break manager loop
    pub(crate) stop_manager: Arc<AtomicBool>,

    /// notifies when a manager starts
    pub(crate) manager_active: Arc<Notify>,

    /// Network configuration for p2p connections
    pub(crate) network_config: NetworkConfig,

    /// Configuration for the screenshare functionality
    #[allow(dead_code)]
    pub(crate) screenshare_config: ScreenshareConfig,

    /// configuration for audio codec, or lack thereof
    pub(crate) codec_config: CodecConfig,
}

/// a state used for session negotiation
#[derive(Debug)]
pub(crate) struct PeerState {
    /// set to true after dialing peer's identity addresses
    pub(crate) dialed: bool,

    /// when true the peer is the dialer
    pub(crate) dialer: bool,

    /// a map of connections and their latencies
    pub(crate) connections: HashMap<ConnectionId, ConnectionState>,

    /// a single connection has been selected to become a session
    pub(crate) selected_connection: bool,

    /// the instant the state was created
    pub(crate) created: Instant,
}

impl PeerState {
    pub(crate) fn dialer() -> Self {
        Self {
            dialed: false,
            dialer: true,
            connections: Default::default(),
            selected_connection: false,
            created: Instant::now(),
        }
    }

    pub(crate) fn non_dialer(endpoint: ConnectedPoint, connection_id: ConnectionId) -> Self {
        Self {
            dialed: false,
            dialer: false,
            connections: HashMap::from([(connection_id, endpoint.into())]),
            selected_connection: false,
            created: Instant::now(),
        }
    }

    pub(crate) fn relayed_only(&self) -> bool {
        self.connections.iter().all(|(_, state)| state.relayed)
    }

    pub(crate) fn latencies_missing(&self) -> bool {
        self.connections
            .iter()
            .any(|(_, state)| state.latency.is_none())
    }
}

pub(crate) struct RoomState {
    pub(crate) peers: Vec<PeerId>,

    pub(crate) sender: Sender<RoomMessage>,

    pub(crate) cancel: CancellationToken,

    pub(crate) end_call: Arc<Notify>,

    pub(crate) early_state: EarlyCallState,
}

#[derive(Clone)]
pub(crate) struct StatisticsCollectorState {
    pub(crate) input_rms: Arc<AtomicF32>,
    pub(crate) output_rms: Arc<AtomicF32>,
    pub(crate) latency: Arc<AtomicUsize>,
    pub(crate) upload_bandwidth: Arc<AtomicUsize>,
    pub(crate) download_bandwidth: Arc<AtomicUsize>,
    pub(crate) loss: Arc<AtomicUsize>,
}

impl StatisticsCollectorState {
    pub(crate) fn new(state: Option<&Arc<SessionState>>) -> Self {
        Self {
            input_rms: Arc::new(Default::default()),
            output_rms: Arc::new(Default::default()),
            latency: state.map(|s| s.latency.clone()).unwrap_or_default(),
            upload_bandwidth: state
                .map(|s| s.upload_bandwidth.clone())
                .unwrap_or_default(),
            download_bandwidth: state
                .map(|s| s.download_bandwidth.clone())
                .unwrap_or_default(),
            loss: Arc::new(Default::default()),
        }
    }
}

/// the state of a single connection during session negotiation
#[derive(Debug, Clone)]
pub(crate) struct ConnectionState {
    /// the latest latency, when available
    pub(crate) latency: Option<Duration>,

    /// whether the connection is relayed
    pub(crate) relayed: bool,

    /// an IP address for the underlying connection, if known
    pub(crate) remote_address: Option<IpAddr>,

    /// tracks failed open stream attempts
    pub(crate) retries: Arc<AtomicUsize>,
}

impl From<ConnectedPoint> for ConnectionState {
    fn from(endpoint: ConnectedPoint) -> Self {
        Self {
            latency: None,
            relayed: endpoint.is_relayed(),
            remote_address: Self::remote_address(&endpoint),
            retries: Default::default(),
        }
    }
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

impl ConnectionState {
    /// extract an IP address from the endpoint if possible
    pub(crate) fn remote_address(endpoint: &ConnectedPoint) -> Option<IpAddr> {
        let remote_address = endpoint.get_remote_address();
        remote_address.iter().find_map(|p| match p {
            Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
            Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
            _ => None,
        })
    }
}

/// state used early in the call before it starts
#[derive(Clone)]
pub(crate) struct EarlyCallState {
    pub(crate) peer: PeerId,
    pub(crate) local_configuration: AudioHeader,
    pub(crate) remote_configuration: AudioHeader,
}

impl EarlyCallState {
    pub(crate) fn codec_config(&self) -> (bool, bool, f32) {
        let codec_enabled =
            self.remote_configuration.codec_enabled || self.local_configuration.codec_enabled;
        let vbr = self.remote_configuration.vbr || self.local_configuration.vbr;
        let residual_bits = (self.remote_configuration.residual_bits as f32)
            .min(self.local_configuration.residual_bits as f32);
        (codec_enabled, vbr, residual_bits)
    }
}

/// shared values for a single session
#[derive(Debug)]
pub(crate) struct SessionState {
    /// identifies a unique session state
    pub(crate) id: Uuid,

    /// signals the session to initiate a call
    pub(crate) start_call: Notify,

    /// notifies during shutdown & manager restarts
    pub(crate) stop_session: CancellationToken,

    /// if the session is in a call
    pub(crate) in_call: AtomicBool,

    /// a reusable sender for messages while a call is active
    pub(crate) message_sender: Sender<ProtocolMessage>,

    /// forwards sub-streams to the session
    pub(crate) stream_sender: AsyncSender<Stream>,

    /// receives sub-streams for the session
    stream_receiver: AsyncReceiver<Stream>,

    /// a shared latency value for the session from libp2p ping
    pub(crate) latency: Arc<AtomicUsize>,

    /// a shared upload bandwidth value for the session
    pub(crate) upload_bandwidth: Arc<AtomicUsize>,

    /// a shared download bandwidth value for the session
    pub(crate) download_bandwidth: Arc<AtomicUsize>,

    pub(crate) end_call: Arc<Notify>,

    pub(crate) stop_screenshare: Arc<Mutex<Option<Arc<Notify>>>>,

    /// Set when `call_handshake` emits `CallFailedSessionAlive` so `session_outer` does not duplicate it.
    pub(crate) call_failed_watchdog_emitted: AtomicBool,
}

impl SessionState {
    pub(crate) fn new(message_sender: &Sender<ProtocolMessage>) -> Self {
        let stream_channel = unbounded_async();

        Self {
            id: Uuid::new_v4(),
            start_call: Notify::new(),
            stop_session: Default::default(),
            in_call: AtomicBool::new(false),
            message_sender: message_sender.clone(),
            stream_sender: stream_channel.0,
            stream_receiver: stream_channel.1,
            latency: Default::default(),
            upload_bandwidth: Default::default(),
            download_bandwidth: Default::default(),
            end_call: Default::default(),
            stop_screenshare: Default::default(),
            call_failed_watchdog_emitted: AtomicBool::new(false),
        }
    }

    pub(crate) async fn open_stream(
        &self,
        mut control: Option<&mut Control>,
        call_state: &EarlyCallState,
    ) -> Result<Stream> {
        let stream_future = async {
            if let Some(control) = control.as_mut() {
                // if dialer, open stream
                control
                    .open_stream(call_state.peer, STREAM_PROTOCOL)
                    .await
                    .map_err(Error::from)
            } else {
                // if listener, receive stream
                self.stream_receiver.recv().await.map_err(Error::from)
            }
        };

        let stream_result = select! {
            _ = self.end_call.notified() => Ok(Err(ErrorKind::NoStream.into())),
            _ = self.stop_session.cancelled() => Ok(Err(ErrorKind::NoStream.into())),
            result = timeout(HELLO_TIMEOUT, stream_future) => result,
        };

        stream_result?
    }

    pub(crate) async fn receive_stream(&self) -> Result<Stream> {
        self.stream_receiver.recv().await.map_err(Into::into)
    }

    pub(crate) async fn teardown(&self) {
        // stops any call
        self.end_call.notify_one();
        // stops the session loop
        self.stop_session.cancel();
        // stops any active screenshare threads
        if let Some(notify) = self.stop_screenshare.lock().await.take() {
            notify.notify_waiters();
        }
    }
}
