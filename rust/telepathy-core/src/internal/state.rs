use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::messages::{AudioHeader, ProtocolMessage, RoomMessage};
use crate::types::{CodecConfig, Contact, NetworkConfig, ScreenshareConfig, SessionStatus};
use atomic_float::AtomicF32;
use iroh::endpoint::{Connection, Path};
use iroh::{PublicKey, SecretKey, TransportAddr};
use std::collections::HashMap;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use telepathy_audio::RnnModel;
use telepathy_audio::internal::utils::db_to_multiplier;
use tokio::select;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, Notify, RwLock};
#[cfg(not(target_family = "wasm"))]
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::interval;

type SharedDeviceId = Arc<Mutex<Option<String>>>;

/// Per-state lifecycle for the global call slot. Only one non-idle state may be held at a time.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CallSlotState {
    Idle = 0,
    PendingIncoming = 1,
    PendingOutgoing = 2,
    ActiveDirect = 3,
    RoomCall = 4,
    AudioTest = 5,
}

impl CallSlotState {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Idle),
            1 => Some(Self::PendingIncoming),
            2 => Some(Self::PendingOutgoing),
            3 => Some(Self::ActiveDirect),
            4 => Some(Self::RoomCall),
            5 => Some(Self::AudioTest),
            _ => None,
        }
    }
}

/// Result of [`CallSlot::try_acquire_or_match`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CallSlotAcquireResult {
    /// The slot was idle and is now `state` for `peer`.
    Acquired,
    /// The slot was already in a compatible state for `peer`.
    Matched,
    /// The slot is held by another call or peer.
    Failed,
}

/// Authoritative global call-slot lifecycle. All transitions must use the atomic helpers on
/// [`CallSlot`]; do not read [`CallSlot::current`] and then mutate the slot in separate steps.
#[derive(Clone)]
pub(crate) struct CallSlot {
    state: Arc<AtomicU8>,
    /// Set for direct-call states so `end_call` can route to the active session.
    direct_peer: Arc<StdMutex<Option<PublicKey>>>,
}

impl Default for CallSlot {
    fn default() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(CallSlotState::Idle as u8)),
            direct_peer: Arc::new(StdMutex::new(None)),
        }
    }
}

impl CallSlot {
    pub(crate) fn occupied(&self) -> bool {
        self.state.load(Acquire) != CallSlotState::Idle as u8
    }

    pub(crate) fn current(&self) -> CallSlotState {
        CallSlotState::from_u8(self.state.load(Acquire)).unwrap_or(CallSlotState::Idle)
    }

    pub(crate) fn direct_peer(&self) -> Option<PublicKey> {
        *self
            .direct_peer
            .lock()
            .expect("call slot direct peer mutex poisoned")
    }

    /// Atomically claims the call slot from idle.
    pub(crate) fn try_acquire(&self, state: CallSlotState, peer: Option<PublicKey>) -> bool {
        debug_assert_ne!(state, CallSlotState::Idle);
        if self
            .state
            .compare_exchange(CallSlotState::Idle as u8, state as u8, AcqRel, Acquire)
            .is_ok()
        {
            if let Some(peer) = peer {
                *self
                    .direct_peer
                    .lock()
                    .expect("call slot direct peer mutex poisoned") = Some(peer);
            }
            true
        } else {
            false
        }
    }

    /// Atomically claims the slot from idle, or confirms it is already compatible for `peer`.
    ///
    /// Succeeds when:
    /// - the slot is idle and becomes `state` for `peer`, or
    /// - the slot is already `state` for `peer`, or
    /// - `state` is [`CallSlotState::PendingIncoming`] and the slot is
    ///   [`CallSlotState::PendingOutgoing`] for the same `peer` (simultaneous dial).
    pub(crate) fn try_acquire_or_match(
        &self,
        state: CallSlotState,
        peer: PublicKey,
    ) -> CallSlotAcquireResult {
        debug_assert_ne!(state, CallSlotState::Idle);
        loop {
            let direct_peer_snapshot = *self
                .direct_peer
                .lock()
                .expect("call slot direct peer mutex poisoned");
            let current = self.current();
            if Self::slot_matches_for_peer(state, current, peer, direct_peer_snapshot) {
                return CallSlotAcquireResult::Matched;
            }

            if current == CallSlotState::Idle
                && self
                    .state
                    .compare_exchange(CallSlotState::Idle as u8, state as u8, AcqRel, Acquire)
                    .is_ok()
            {
                *self
                    .direct_peer
                    .lock()
                    .expect("call slot direct peer mutex poisoned") = Some(peer);
                return CallSlotAcquireResult::Acquired;
            }

            if self.current() == CallSlotState::Idle {
                continue;
            }

            return CallSlotAcquireResult::Failed;
        }
    }

    fn slot_matches_for_peer(
        state: CallSlotState,
        current: CallSlotState,
        peer: PublicKey,
        direct_peer: Option<PublicKey>,
    ) -> bool {
        if direct_peer != Some(peer) {
            return false;
        }

        if current == state {
            return true;
        }

        state == CallSlotState::PendingIncoming && current == CallSlotState::PendingOutgoing
    }

    pub(crate) fn transition_pending_to_active_for_peer(&self, peer: PublicKey) -> bool {
        loop {
            let current = self.current();
            match current {
                CallSlotState::PendingIncoming | CallSlotState::PendingOutgoing
                    if self.direct_peer() == Some(peer) =>
                {
                    if self.try_transition(current, CallSlotState::ActiveDirect) {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    }

    pub(crate) fn try_transition(&self, from: CallSlotState, to: CallSlotState) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, AcqRel, Acquire)
            .is_ok()
    }

    pub(crate) fn release(&self) {
        self.state.store(CallSlotState::Idle as u8, Release);
        *self
            .direct_peer
            .lock()
            .expect("call slot direct peer mutex poisoned") = None;
    }

    pub(crate) fn release_if_state(&self, expected: CallSlotState) -> bool {
        if self.current() == expected {
            self.release();
            true
        } else {
            warn!(
                event = "call_slot_release_skipped",
                ?expected,
                actual = ?self.current()
            );
            false
        }
    }

    pub(crate) fn release_if_pending_for_peer(&self, peer: PublicKey) {
        let current = self.current();
        if matches!(
            current,
            CallSlotState::PendingIncoming | CallSlotState::PendingOutgoing
        ) {
            if self.direct_peer() == Some(peer) {
                self.release();
            } else {
                warn!(
                    event = "call_slot_release_skipped_peer_mismatch",
                    ?current,
                    expected_peer.id = %peer
                );
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct CoreState {
    /// Enables rnnoise denoising
    pub(crate) denoise: Arc<AtomicBool>,

    /// The rnnoise model
    pub(crate) denoise_model: Arc<RwLock<RnnModel>>,

    /// Manually set the input device
    pub(crate) input_device: SharedDeviceId,

    /// Manually set the output device
    pub(crate) output_device: SharedDeviceId,

    /// The current iroh secret key
    pub identity: Arc<RwLock<Option<SecretKey>>>,

    /// Authoritative global call-slot guard covering negotiation and active calls.
    pub(crate) call_slot: CallSlot,

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
    pub manager_active: Arc<Notify>,

    /// Network configuration for p2p connections
    pub(crate) network_config: NetworkConfig,

    /// Configuration for the screenshare functionality
    #[allow(dead_code)]
    pub(crate) screenshare_config: ScreenshareConfig,

    /// configuration for audio codec, or lack thereof
    pub(crate) codec_config: CodecConfig,

    /// Controls the threshold for silence detection
    rms_threshold: Arc<AtomicF32>,

    /// Every input sample is multiplied by this number
    input_multiplier: Arc<AtomicF32>,

    /// The output volume in decibels
    output_volume: Arc<AtomicF32>,

    /// Output samples are multiplied by this number, per-peer
    peer_output_volumes: Arc<StdMutex<HashMap<PublicKey, PeerVolume>>>,

    /// serializes access to shared volume state
    output_lock: Arc<StdMutex<()>>,
}

impl CoreState {
    pub(crate) fn new(
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        codec_config: &CodecConfig,
    ) -> Self {
        Self {
            network_config: network_config.clone(),
            screenshare_config: screenshare_config.clone(),
            codec_config: codec_config.clone(),
            ..Self::default()
        }
    }

    pub fn set_input_volume(&self, decibel: f32) {
        self.input_multiplier
            .store(db_to_multiplier(decibel), Relaxed);
    }

    pub(crate) fn get_input_volume(&self) -> &Arc<AtomicF32> {
        &self.input_multiplier
    }

    pub(crate) fn set_rms_threshold(&self, decibel: f32) {
        self.rms_threshold.store(db_to_multiplier(decibel), Relaxed);
    }

    pub(crate) fn get_rms_threshold(&self) -> &Arc<AtomicF32> {
        &self.rms_threshold
    }

    /// returns the volume multiplier to share with the output processor
    pub(crate) fn output_volume_for_peer(&self, peer: PublicKey) -> Arc<AtomicF32> {
        self.get_peer_volume(peer).multiplier
    }

    /// updates the base output volume in decibels
    /// all peer output volumes are updated with the new base
    pub(crate) fn set_output_volume(&self, decibel: f32) {
        let lock = self.output_lock.lock();
        let peer_volume_lock = self
            .peer_output_volumes
            .lock()
            .expect("peer output volume mutex poisoned");
        let old_decibel = self.output_volume.swap(decibel, Relaxed);
        let offset = decibel - old_decibel;
        for peer in peer_volume_lock.values() {
            let new_volume = peer.volume.fetch_add(offset, Relaxed) + offset;
            peer.multiplier.store(db_to_multiplier(new_volume), Relaxed);
        }
        drop(lock);
    }

    /// updates the peer output volume for a contact
    pub(crate) fn set_peer_output_volume(&self, contact: &Contact) {
        let lock = self.output_lock.lock();
        let global_volume = self.output_volume.load(Relaxed);
        let peer_volume = self.get_peer_volume(contact.peer_id);
        let new_volume = global_volume + contact.output_volume;
        peer_volume.volume.store(new_volume, Relaxed);
        peer_volume
            .multiplier
            .store(db_to_multiplier(new_volume), Relaxed);
        drop(lock);
    }

    pub(crate) fn reset_peer_output_volumes(&self) {
        self.peer_output_volumes
            .lock()
            .expect("peer output volume mutex poisoned")
            .clear();
    }

    pub(crate) fn reset_peer_output_volume(&self, peer: &PublicKey) {
        self.peer_output_volumes
            .lock()
            .expect("peer output volume mutex poisoned")
            .remove(peer);
    }

    fn get_peer_volume(&self, peer: PublicKey) -> PeerVolume {
        self.peer_output_volumes
            .lock()
            .expect("peer output volume mutex poisoned")
            .entry(peer)
            // peers from rooms will not have a cached output volume
            .or_insert_with(|| PeerVolume::new(self.output_volume.load(Relaxed)))
            .clone()
    }
}

pub(crate) struct RoomState {
    pub(crate) peers: Vec<PublicKey>,

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

/// state used early in the call before it starts
#[derive(Clone)]
pub(crate) struct EarlyCallState {
    pub(crate) peer: PublicKey,
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
pub struct SessionState {
    /// identifies a unique session state
    pub(crate) id: Uuid,

    /// signals the session to initiate a call
    pub start_call: Notify,

    /// notifies during shutdown & manager restarts
    pub(crate) stop_session: CancellationToken,

    /// a reusable sender for messages while a call is active
    pub(crate) message_sender: Sender<ProtocolMessage>,

    /// a shared latency value for the session from iroh rtt
    pub(crate) latency: Arc<AtomicUsize>,

    /// a shared upload bandwidth value for the session
    pub(crate) upload_bandwidth: Arc<AtomicUsize>,

    /// a shared download bandwidth value for the session
    pub(crate) download_bandwidth: Arc<AtomicUsize>,

    pub(crate) end_call: Arc<Notify>,

    pub(crate) start_screenshare: Notify,

    pub(crate) stop_screenshare: Arc<Mutex<Option<Arc<Notify>>>>,
}

impl SessionState {
    pub(crate) fn new(message_sender: &Sender<ProtocolMessage>) -> Self {
        Self {
            id: Uuid::new_v4(),
            start_call: Notify::new(),
            stop_session: Default::default(),
            message_sender: message_sender.clone(),
            latency: Default::default(),
            upload_bandwidth: Default::default(),
            download_bandwidth: Default::default(),
            end_call: Default::default(),
            start_screenshare: Default::default(),
            stop_screenshare: Default::default(),
        }
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

    /// monitors the session connection to update bandwidth, latency, and push session statuses
    pub(crate) async fn connection_monitor<S, C>(
        &self,
        connection: Connection,
        callbacks: Arc<C>,
        peer: PublicKey,
    ) where
        S: CoreStatisticsCallback + Send + Sync + 'static,
        C: CoreCallbacks<S> + Send + Sync + 'static,
    {
        let mut interval = interval(Duration::from_secs(1));
        interval.tick().await;

        loop {
            select! {
                _ = self.stop_session.cancelled() => break,
                _ = interval.tick() => {
                    if connection.close_reason().is_some() {
                        break;
                    }

                    // track overall bandwidth across all connections
                    self.upload_bandwidth.store(connection.stats().udp_tx.bytes as usize, Relaxed);
                    self.download_bandwidth.store(connection.stats().udp_rx.bytes as usize, Relaxed);

                    let paths = connection.paths();
                    let mut max_data = u64::MIN;
                    let mut primary_connection: Option<Path> = None;

                    for path in paths.iter().filter(|p| p.is_selected()) {
                        info!(event = "connection_path", path = ?path);
                        let stats = path.stats();

                        // the connection with the most bandwidth should be considered primary
                        let bandwidth = stats.udp_rx.bytes + stats.udp_tx.bytes;
                        if bandwidth > max_data {
                            max_data = bandwidth;
                            primary_connection = Some(path);
                        }
                    }

                    if let Some(primary_connection) = primary_connection {
                        self.latency.store(primary_connection.rtt().as_millis() as usize, Relaxed);

                        callbacks
                            .session_status(
                                SessionStatus::Connected {
                                    relayed: primary_connection.is_relay(),
                                    remote_address: match *primary_connection.remote_addr() {
                                        TransportAddr::Ip(socket) => socket.ip().to_string(),
                                        TransportAddr::Relay(_) => "relay".to_string(),
                                        TransportAddr::Custom(_) => "custom".to_string(),
                                        _ => "unknown".to_string(),
                                    },
                                },
                                peer,
                            )
                            .await;
                    } else {
                        info!(event = "no_primary_connection", peer.id = %peer)
                    }
                }
            }
        }
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        self.stop_session.cancel();
    }
}

#[derive(Clone, Default)]
struct PeerVolume {
    /// the volume is stored for updating the multiplier
    volume: Arc<AtomicF32>,

    /// multiplier is shared with the output processor thread
    multiplier: Arc<AtomicF32>,
}

impl PeerVolume {
    fn new(decibel: f32) -> Self {
        Self {
            volume: Arc::new(AtomicF32::new(decibel)),
            multiplier: Arc::new(AtomicF32::new(db_to_multiplier(decibel))),
        }
    }
}

#[cfg(test)]
mod call_slot_tests {
    use super::{CallSlot, CallSlotAcquireResult, CallSlotState};
    use iroh::SecretKey;

    #[test]
    fn call_slot_acquire_and_release() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer)));
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
        assert_eq!(slot.direct_peer(), Some(peer));
        assert!(!slot.try_acquire(CallSlotState::PendingIncoming, Some(peer)));

        slot.release();
        assert_eq!(slot.current(), CallSlotState::Idle);
        assert_eq!(slot.direct_peer(), None);
    }

    #[test]
    fn call_slot_transition_pending_to_active() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingIncoming, Some(peer)));
        assert!(slot.try_transition(CallSlotState::PendingIncoming, CallSlotState::ActiveDirect));
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.direct_peer(), Some(peer));
    }

    #[test]
    fn call_slot_release_if_pending_for_peer() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        let other = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer)));
        slot.release_if_pending_for_peer(other);
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);

        slot.release_if_pending_for_peer(peer);
        assert_eq!(slot.current(), CallSlotState::Idle);
    }

    #[test]
    fn call_slot_try_acquire_or_match_from_idle() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer),
            CallSlotAcquireResult::Acquired
        );
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
    }

    #[test]
    fn call_slot_try_acquire_or_match_already_held() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer)));
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer),
            CallSlotAcquireResult::Matched
        );
    }

    #[test]
    fn call_slot_try_acquire_or_match_simultaneous_dial() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer)));
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingIncoming, peer),
            CallSlotAcquireResult::Matched
        );
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
    }

    #[test]
    fn call_slot_try_acquire_or_match_fails_when_busy() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        let other = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer)));
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, other),
            CallSlotAcquireResult::Failed
        );
    }

    #[test]
    fn call_slot_transition_pending_to_active_for_peer() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer)));
        assert!(slot.transition_pending_to_active_for_peer(peer));
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
    }

    #[test]
    fn call_slot_try_acquire_or_match_never_matches_after_ownership_lost() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let slot = Arc::new(CallSlot::default());
        let peer_a = SecretKey::generate().public();
        let peer_b = SecretKey::generate().public();

        assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer_a)));

        let start = Arc::new(Barrier::new(2));
        let done = Arc::new(Barrier::new(2));
        let releaser = {
            let slot = Arc::clone(&slot);
            let start = Arc::clone(&start);
            let done = Arc::clone(&done);
            thread::spawn(move || {
                for _ in 0..5_000 {
                    start.wait();
                    slot.release();
                    assert!(slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer_b)));
                    done.wait();
                }
            })
        };

        for _ in 0..5_000 {
            start.wait();
            let result = slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer_a);
            assert_ne!(
                result,
                CallSlotAcquireResult::Matched,
                "must not match for a peer that lost ownership mid-call"
            );
            done.wait();
        }

        releaser.join().unwrap();
    }
}
