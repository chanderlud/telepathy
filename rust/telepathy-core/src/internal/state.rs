use crate::internal::Result;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::{AudioHeader, ProtocolMessage, RoomMessage};
use crate::types::{CodecConfig, Contact, NetworkConfig, ScreenshareConfig, SessionStatus};
use atomic_float::AtomicF32;
use iroh::endpoint::{Connection, Path};
use iroh::{PublicKey, SecretKey, TransportAddr};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
use std::time::Duration;
use telepathy_audio::RnnModel;
use telepathy_audio::internal::utils::db_to_multiplier;
use tokio::select;
use tokio::sync::mpsc::Sender;
use tokio::sync::{Mutex, Notify, RwLock, oneshot};
#[cfg(not(target_family = "wasm"))]
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::interval;

type SharedDeviceId = Arc<Mutex<Option<String>>>;

/// Per-state lifecycle for the global call slot. Only one non-idle state may be held at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallSlotState {
    Idle,
    PendingIncoming,
    PendingOutgoing,
    ActiveDirect,
    RoomCall,
    AudioTest,
    /// Held while the signing identity is being swapped and the session
    /// manager restarted.
    /// [`crate::internal::TelepathyHandle::begin_identity_switch`] acquires
    /// this state atomically with its idle check so a call cannot start
    /// before [`crate::internal::TelepathyHandle::commit_identity_switch`]
    /// (or
    /// [`crate::internal::TelepathyHandle::cancel_identity_switch`]) releases
    /// the gate.
    IdentitySwitch,
}

/// Result of [`CallSlot::try_acquire_or_match`].
///
/// `Matched*` variants report which pending state the held slot was in. The caller asked for
/// `state` (the first argument to `try_acquire_or_match`) and the held slot was already in a
/// compatible pending state; the variant identifies that held state so the caller can decide
/// whether the match is the same direction (idempotent retry) or the opposite direction
/// (e.g. accepting a peer's incoming prompt while asking for an outgoing slot).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallSlotAcquireResult {
    /// The slot was idle and is now `state` for `peer`.
    Acquired,
    /// The slot was already pending incoming for `peer`.
    MatchedPendingIncoming,
    /// The slot was already pending outgoing for `peer`.
    MatchedPendingOutgoing,
    /// The slot is held by another call or peer.
    Failed,
}

/// Atomic snapshot of [`CallSlot`] state and ownership captured under a single lock acquisition.
///
/// Callers must use this type when they need to reason about both `state` and `direct_peer`
/// together; split `current()` + `direct_peer()` reads can observe ownership that has already
/// transitioned to a newer call by the time the second read is taken.
///
/// `generation` is a monotonically increasing ownership token: it is bumped every time a new
/// non-idle owner acquires the slot. It is preserved across a matched simultaneous-dial path so
/// that the matched peer observes the same generation it would have observed as the original
/// acquirer. This guarantees that release/reacquire cycles for the same `(state, peer)` pair
/// produce snapshots with different generations, so a stale snapshot cannot accidentally match
/// a newer owner that happens to share the same state and peer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CallSlotSnapshot {
    pub state: CallSlotState,
    pub direct_peer: Option<PublicKey>,
    pub generation: u64,
}

#[derive(Clone, Copy)]
struct CallSlotInner {
    state: CallSlotState,
    direct_peer: Option<PublicKey>,
    /// Monotonic ownership token; bumped on every transition from idle to a non-idle state
    /// and preserved across simultaneous-dial match.
    generation: u64,
}

#[derive(Clone)]
pub struct CallSlot {
    inner: Arc<StdMutex<CallSlotInner>>,
    released: Arc<Notify>,
}

impl Default for CallSlot {
    fn default() -> Self {
        Self {
            inner: Arc::new(StdMutex::new(CallSlotInner {
                state: CallSlotState::Idle,
                direct_peer: None,
                generation: 0,
            })),
            released: Arc::new(Notify::new()),
        }
    }
}

impl CallSlot {
    pub fn current(&self) -> CallSlotState {
        self.inner
            .lock()
            .map(|inner| inner.state)
            .unwrap_or_else(|poisoned| poisoned.into_inner().state)
    }

    /// Returns a consistent snapshot of the slot's state, owning peer, and ownership generation
    /// from one lock acquisition.
    ///
    /// Prefer this over separate `current()` + `direct_peer()` reads whenever both fields are
    /// needed together: a snapshot cannot observe a peer mismatch where the state has been
    /// released and the slot re-acquired by a different call between the two reads. The
    /// `generation` token additionally distinguishes release/reacquire cycles that would
    /// otherwise appear identical (same state, same peer).
    pub fn snapshot(&self) -> Result<CallSlotSnapshot> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        Ok(CallSlotSnapshot {
            state: inner.state,
            direct_peer: inner.direct_peer,
            generation: inner.generation,
        })
    }

    /// Atomically claims the call slot from idle, bumping the ownership generation.
    pub fn try_acquire(&self, state: CallSlotState, peer: Option<PublicKey>) -> Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        if inner.state == CallSlotState::Idle {
            inner.state = state;
            inner.direct_peer = peer;
            // Bump the generation so callers that snapshot the slot before this acquisition
            // cannot accidentally match a future reacquire of the same state/peer.
            inner.generation = inner.generation.wrapping_add(1);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Atomically claims the slot from idle, or confirms it is already compatible for `peer`.
    ///
    /// Succeeds when:
    /// - the slot is idle and becomes `state` for `peer`, or
    /// - `state` is a pending direct-call state and the slot is already pending for
    ///   the same `peer` (including simultaneous dial).
    ///
    /// On `Acquired` the ownership generation is bumped so a stale snapshot from a prior
    /// owner can never match this new acquisition. On `Matched*` the existing generation is
    /// preserved so the matched peer observes the same ownership token the original acquirer
    /// would have used. The `Matched*` variant reports the held pending state, so callers
    /// that asked for `PendingOutgoing` can distinguish a same-peer retry
    /// (`MatchedPendingOutgoing`, idempotent) from accepting a peer's incoming prompt
    /// (`MatchedPendingIncoming`).
    pub fn try_acquire_or_match(
        &self,
        state: CallSlotState,
        peer: PublicKey,
    ) -> Result<CallSlotAcquireResult> {
        Ok(self.try_acquire_or_match_with_owner(state, peer)?.0)
    }

    /// Atomic variant of [`try_acquire_or_match`] that also returns the exact
    /// [`CallSlotSnapshot`] captured under the same mutex acquisition.
    ///
    /// The returned snapshot is `Some(_)` only on [`CallSlotAcquireResult::Acquired`]:
    /// it reflects precisely the ownership this call established (the new pending
    /// state, the requesting peer, and the freshly bumped generation). A concurrent
    /// caller (for example a handshake thread transitioning the slot from pending
    /// to active) cannot leak its transition into this snapshot because both the
    /// acquisition and the snapshot read happen under a single lock hold.
    ///
    /// [`CallSlotAcquireResult::MatchedPendingIncoming`] and
    /// [`CallSlotAcquireResult::MatchedPendingOutgoing`] deliberately return `None`:
    /// the matcher does not own the slot (the original acquirer does) and must not
    /// be able to release it through the snapshot-based path. [`Failed`] likewise
    /// returns `None`.
    ///
    /// Callers that observe the operation they used to acquire the slot (e.g.
    /// cancellation of a `start_call` operation) must release through
    /// [`release_if_match`] using the snapshot returned here. Deriving the snapshot
    /// from a later [`snapshot`] call re-acquires the mutex and can observe a state
    /// that was transitioned or replaced after acquisition, which would let the
    /// cancellation release a call this operation never owned.
    ///
    /// [`Failed`]: CallSlotAcquireResult::Failed
    pub fn try_acquire_or_match_with_owner(
        &self,
        state: CallSlotState,
        peer: PublicKey,
    ) -> Result<(CallSlotAcquireResult, Option<CallSlotSnapshot>)> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        if let Some(matched) =
            Self::matched_pending_for_peer(state, inner.state, peer, inner.direct_peer)
        {
            // Matched callers do not own the slot; the original acquirer does. Return
            // no snapshot so the matcher cannot release ownership it never held.
            return Ok((matched, None));
        }

        if inner.state == CallSlotState::Idle {
            inner.state = state;
            inner.direct_peer = Some(peer);
            inner.generation = inner.generation.wrapping_add(1);
            // Capture the ownership snapshot under the same lock that performed the
            // acquisition. A release keyed on this snapshot can never match a state
            // that a concurrent caller transitioned to after this method returned.
            let snapshot = CallSlotSnapshot {
                state: inner.state,
                direct_peer: inner.direct_peer,
                generation: inner.generation,
            };
            return Ok((CallSlotAcquireResult::Acquired, Some(snapshot)));
        }

        Ok((CallSlotAcquireResult::Failed, None))
    }

    /// Returns the [`CallSlotAcquireResult::Matched*`] variant for the held pending state when
    /// `state` is a pending direct-call state and the slot is already compatible for `peer`,
    /// otherwise `None`. Reports the held state so callers can distinguish a same-direction
    /// retry from a cross-direction (e.g. simultaneous-dial) match.
    fn matched_pending_for_peer(
        state: CallSlotState,
        current: CallSlotState,
        peer: PublicKey,
        direct_peer: Option<PublicKey>,
    ) -> Option<CallSlotAcquireResult> {
        if direct_peer != Some(peer) {
            return None;
        }

        match (state, current) {
            (CallSlotState::PendingOutgoing, CallSlotState::PendingOutgoing) => {
                Some(CallSlotAcquireResult::MatchedPendingOutgoing)
            }
            (CallSlotState::PendingIncoming, CallSlotState::PendingIncoming) => {
                Some(CallSlotAcquireResult::MatchedPendingIncoming)
            }
            (CallSlotState::PendingIncoming, CallSlotState::PendingOutgoing)
            | (CallSlotState::PendingOutgoing, CallSlotState::PendingIncoming) => {
                // Cross-direction match: report the held state (the slot the call will run
                // against) so the caller can decide whether to notify.
                match current {
                    CallSlotState::PendingIncoming => {
                        Some(CallSlotAcquireResult::MatchedPendingIncoming)
                    }
                    CallSlotState::PendingOutgoing => {
                        Some(CallSlotAcquireResult::MatchedPendingOutgoing)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub fn transition_pending_to_active_for_peer(&self, peer: PublicKey) -> Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        if matches!(
            inner.state,
            CallSlotState::PendingIncoming | CallSlotState::PendingOutgoing
        ) && inner.direct_peer == Some(peer)
        {
            inner.state = CallSlotState::ActiveDirect;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn release(&self) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        inner.state = CallSlotState::Idle;
        inner.direct_peer = None;
        drop(inner);
        self.released.notify_waiters();
        Ok(())
    }

    /// Waits until the owner represented by [expected] has released the slot.
    /// A newer generation also proves that this owner released before replacement.
    pub async fn wait_for_release(&self, expected: CallSlotSnapshot) -> Result<()> {
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            let current = self.snapshot()?;
            if current.state == CallSlotState::Idle || current.generation != expected.generation {
                return Ok(());
            }
            released.await;
        }
    }

    /// Releases the slot only if the current state, peer, and generation still match `expected`.
    ///
    /// Use this after observing a [`CallSlotSnapshot`] for `expected` to avoid the classic
    /// "release a newer call's slot" race: between snapshotting and releasing, another path
    /// may have already released and re-acquired the slot for a different call. The
    /// generation check additionally guards against release/reacquire cycles that reuse the
    /// same `(state, peer)` pair — a stale snapshot from the prior owner will not match the
    /// post-reacquire slot.
    pub fn release_if_match(&self, expected: CallSlotSnapshot) -> Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        if inner.state == expected.state
            && inner.direct_peer == expected.direct_peer
            && inner.generation == expected.generation
        {
            inner.state = CallSlotState::Idle;
            inner.direct_peer = None;
            drop(inner);
            self.released.notify_waiters();
            Ok(true)
        } else {
            warn!(
                event = "call_slot_release_skipped_snapshot_mismatch",
                ?expected,
                actual.state = ?inner.state,
                actual.direct_peer = ?inner.direct_peer,
                actual.generation = inner.generation
            );
            Ok(false)
        }
    }

    /// Atomically claims the call slot from idle, capturing the ownership
    /// snapshot under the same lock so a release keyed on the snapshot cannot
    /// race with a state transition that happens between a separate
    /// `try_acquire` + `snapshot()` pair.
    ///
    /// Used by identity-switch begin to acquire the `IdentitySwitch` gate and
    /// capture the snapshot in one operation. Returns `Ok(Some(snapshot))`
    /// when the slot moved from idle to `state`, `Ok(None)` when the slot was
    /// not idle so begin can surface `ManagerRestartDuringCall`.
    pub fn try_acquire_with_snapshot(
        &self,
        state: CallSlotState,
    ) -> Result<Option<CallSlotSnapshot>> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        if inner.state == CallSlotState::Idle {
            inner.state = state;
            inner.direct_peer = None;
            inner.generation = inner.generation.wrapping_add(1);
            Ok(Some(CallSlotSnapshot {
                state: inner.state,
                direct_peer: None,
                generation: inner.generation,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn release_if_pending_for_peer(&self, peer: PublicKey) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        let current = inner.state;
        if matches!(
            current,
            CallSlotState::PendingIncoming | CallSlotState::PendingOutgoing
        ) {
            if inner.direct_peer == Some(peer) {
                inner.state = CallSlotState::Idle;
                inner.direct_peer = None;
                drop(inner);
                self.released.notify_waiters();
            } else {
                warn!(
                    event = "call_slot_release_skipped_peer_mismatch",
                    ?current,
                    expected_peer.id = %peer
                );
            }
        }
        Ok(())
    }

    /// Clears a `PendingIncoming` or `PendingOutgoing` slot, regardless of which peer
    /// (if any) currently owns it, in a single lock acquisition.
    ///
    /// This is the terminal-clear path used by [`TelepathyCore::reset_sessions`] and is
    /// only safe to call when no `SessionState` in `session_states` is allowed to own
    /// the pending direct-call slot anymore (the per-session ownership invariant in
    /// [`crate::internal::core::TelepathyCore`] guarantees that a drained session cannot
    /// re-acquire a pending slot). Active non-pending states (`Idle`, `ActiveDirect`,
    /// `RoomCall`, `AudioTest`) are left untouched so terminal teardown can never
    /// clobber a live call.
    pub fn clear_pending_direct(&self) -> Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ErrorKind::Poison("call slot mutex poisoned"))?;
        if matches!(
            inner.state,
            CallSlotState::PendingIncoming | CallSlotState::PendingOutgoing
        ) {
            inner.state = CallSlotState::Idle;
            inner.direct_peer = None;
            inner.generation = inner.generation.wrapping_add(1);
            drop(inner);
            self.released.notify_waiters();
            Ok(true)
        } else {
            Ok(false)
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
    pub call_slot: CallSlot,

    /// Monotonic room-generation counter. Bumped on every `join_room`
    /// acquisition, before the new `RoomState` is published. This is the room
    /// analog of `CallSlot::generation` and is captured into `RoomState` so
    /// `room_handshake` and `room_controller` can validate against the
    /// currently installed room.
    pub(crate) next_room_generation: Arc<AtomicU64>,

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

    pub(crate) stop_manager: CancellationToken,

    /// notifies when a manager starts
    pub manager_active: Arc<Notify>,

    /// Serialized state for the two-phase identity-switch transaction.
    /// Replaces the bare `identity_switch_snapshot` Mutex: the transaction
    /// object enforces begin/commit/cancel mutual exclusion so a stray
    /// cancel can no longer invalidate an unrelated active call.
    pub(crate) identity_transaction: Arc<IdentityTransaction>,

    /// Serializes public session starts with identity-switch installation.
    pub(crate) identity_session_gate: Arc<Mutex<()>>,

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

    /// global audio sequence number
    pub(crate) audio_sequence: Arc<AtomicU32>,
}

/// Pre-mutation state captured at [`crate::internal::TelepathyHandle::begin_identity_switch`]
/// so the transaction can roll back the signing identity and the session set if
/// [`crate::internal::TelepathyHandle::commit_identity_switch`] fails after the
/// identity has already been mutated.
#[derive(Clone)]
pub(crate) struct IdentitySwitchSnapshot {
    /// Identity in effect before the transaction mutated it.
    pub previous_identity: SecretKey,
    /// Contact snapshot in effect before the transaction mutated the session set.
    /// Used to rehydrate the previous sessions on rollback so the backend does
    /// not advertise sessions for the rejected profile's contacts.
    pub previous_contacts: Vec<Contact>,
}

/// Serialized pending state for the two-phase identity-switch transaction.
///
/// Held under the inner Mutex so begin, commit, and cancel are mutually
/// exclusive: only one terminal action can consume a pending transaction.
/// Cancel without begin is a no-op (returns `None`) and never touches the
/// call slot — this is the invariant that lets a stray or racing cancel
/// invalidate an unrelated active call.
pub(crate) struct IdentityTransaction {
    state: StdMutex<Option<IdentityTransactionState>>,
}

enum IdentityTransactionState {
    Pending(IdentityTransactionPending),
    Committing(IdentityTransactionPending),
    RollingBack(IdentitySwitchRecovery),
    RecoveryRequired(IdentitySwitchRecovery),
    Recovering(IdentitySwitchRecovery),
}

impl IdentityTransactionState {
    fn slot_snapshot(&self) -> CallSlotSnapshot {
        match self {
            Self::Pending(pending) | Self::Committing(pending) => pending.slot_snapshot(),
            Self::RollingBack(recovery)
            | Self::RecoveryRequired(recovery)
            | Self::Recovering(recovery) => recovery.slot_snapshot(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct IdentityTransactionPending {
    /// Snapshot the backend captured at begin so commit can roll back to it
    /// if the target identity or its contacts fail to apply.
    previous: IdentitySwitchSnapshot,
    /// Target payload validated and stored at begin. Stashing it here means
    /// a malformed target key (wrong length, etc.) fails BEFORE the slot is
    /// reserved — so a commit-time validation failure can never wedge the
    /// slot, and a parameterless commit cannot fail validation at all.
    target_key: [u8; 32],
    target_contacts: Vec<Contact>,
    /// Exact slot ownership captured by begin. Commit/cancel release through
    /// [`CallSlot::release_if_match`] keyed on this snapshot so a stray
    /// cancel cannot invalidate an unrelated active call that re-acquired
    /// the slot after a timeout or race.
    slot_snapshot: CallSlotSnapshot,
}

/// Previous identity snapshot retained after rollback setup failed.
///
/// The `IdentitySwitch` slot remains held until an explicit recovery request
/// receives a successful manager setup acknowledgement for this snapshot.
#[derive(Clone)]
pub(crate) struct IdentitySwitchRecovery {
    previous: IdentitySwitchSnapshot,
    slot_snapshot: CallSlotSnapshot,
}

impl IdentityTransaction {
    pub(crate) fn new() -> Self {
        Self {
            state: StdMutex::new(None),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, Option<IdentityTransactionState>>> {
        self.state
            .lock()
            .map_err(|_| Error::from(ErrorKind::Poison("identity transaction mutex poisoned")))
    }

    /// Install a pending transaction. The caller MUST have already acquired
    /// the slot atomically (capturing `slot_snapshot`) and validated the
    /// target payload BEFORE calling this; otherwise the install would race
    /// a concurrent begin or commit a slot we do not own.
    ///
    /// Returns `Poison` if a transaction is already pending: a well-behaved
    /// caller never reaches this branch because the slot gate serializes
    /// begins, but the check defends against a bug elsewhere producing two
    /// consecutive installs without an intervening terminal action.
    pub(crate) fn install(
        &self,
        previous: IdentitySwitchSnapshot,
        target_key: [u8; 32],
        target_contacts: Vec<Contact>,
        slot_snapshot: CallSlotSnapshot,
    ) -> Result<()> {
        let mut guard = self.lock_state()?;
        if guard.is_some() {
            return Err(Error::from(ErrorKind::Poison(
                "identity_switch_begin_overwrote_pending_transaction",
            )));
        }
        *guard = Some(IdentityTransactionState::Pending(
            IdentityTransactionPending {
                previous,
                target_key,
                target_contacts,
                slot_snapshot,
            },
        ));
        Ok(())
    }

    pub(crate) fn begin_commit(&self) -> Result<IdentityTransactionPending> {
        let mut guard = self.lock_state()?;
        match guard.as_ref() {
            Some(IdentityTransactionState::Pending(pending)) => {
                let pending = pending.clone();
                *guard = Some(IdentityTransactionState::Committing(pending.clone()));
                Ok(pending)
            }
            _ => Err(Error::from(ErrorKind::Poison(
                "commit_identity_switch called without a pending transaction",
            ))),
        }
    }

    /// Consume the pending transaction for cancel. Returns `None` when no
    /// transaction is pending so cancel without begin is a safe no-op that
    /// never touches the call slot.
    pub(crate) fn take_for_cancel(&self) -> Result<Option<IdentityTransactionPending>> {
        let mut guard = self.lock_state()?;
        match guard.take() {
            Some(IdentityTransactionState::Pending(pending)) => Ok(Some(pending)),
            state => {
                *guard = state;
                Ok(None)
            }
        }
    }

    pub(crate) fn begin_rollback(
        &self,
        pending: &IdentityTransactionPending,
    ) -> Result<IdentitySwitchRecovery> {
        let mut guard = self.lock_state()?;
        match guard.as_ref() {
            Some(IdentityTransactionState::Committing(current))
                if current.slot_snapshot() == pending.slot_snapshot() =>
            {
                let recovery = pending.clone().into_recovery();
                *guard = Some(IdentityTransactionState::RollingBack(recovery.clone()));
                Ok(recovery)
            }
            _ => Err(Error::from(ErrorKind::Poison(
                "identity switch commit state changed before rollback",
            ))),
        }
    }

    pub(crate) fn rollback_failed(&self, recovery: &IdentitySwitchRecovery) -> Result<()> {
        let mut guard = self.lock_state()?;
        match guard.as_ref() {
            Some(IdentityTransactionState::RollingBack(current))
                if current.slot_snapshot() == recovery.slot_snapshot() =>
            {
                *guard = Some(IdentityTransactionState::RecoveryRequired(recovery.clone()));
                Ok(())
            }
            _ => Err(Error::from(ErrorKind::Poison(
                "identity switch rollback state changed before recovery",
            ))),
        }
    }

    pub(crate) fn finish_commit(&self, pending: &IdentityTransactionPending) -> Result<()> {
        self.finish_matching(pending.slot_snapshot(), |state| {
            matches!(state, IdentityTransactionState::Committing(_))
        })
    }

    pub(crate) fn finish_rollback(&self, recovery: &IdentitySwitchRecovery) -> Result<()> {
        self.finish_matching(recovery.slot_snapshot(), |state| {
            matches!(state, IdentityTransactionState::RollingBack(_))
        })
    }

    fn finish_matching(
        &self,
        slot_snapshot: CallSlotSnapshot,
        expected: impl FnOnce(&IdentityTransactionState) -> bool,
    ) -> Result<()> {
        let mut guard = self.lock_state()?;
        match guard.as_ref() {
            Some(state) if expected(state) && state.slot_snapshot() == slot_snapshot => {
                *guard = None;
                Ok(())
            }
            _ => Err(Error::from(ErrorKind::Poison(
                "identity switch state changed before completion",
            ))),
        }
    }

    pub(crate) fn begin_recovery(&self) -> Result<Option<IdentitySwitchRecovery>> {
        let mut guard = self.lock_state()?;
        match guard.as_ref() {
            Some(IdentityTransactionState::RecoveryRequired(recovery)) => {
                let recovery = recovery.clone();
                *guard = Some(IdentityTransactionState::Recovering(recovery.clone()));
                Ok(Some(recovery))
            }
            Some(IdentityTransactionState::Pending(_))
            | Some(IdentityTransactionState::Committing(_))
            | Some(IdentityTransactionState::RollingBack(_))
            | Some(IdentityTransactionState::Recovering(_))
            | None => Ok(None),
        }
    }

    pub(crate) fn restore_recovery_required(&self, recovery: IdentitySwitchRecovery) -> Result<()> {
        let mut guard = self.lock_state()?;
        match guard.as_ref() {
            Some(IdentityTransactionState::Recovering(current))
                if current.slot_snapshot() == recovery.slot_snapshot() =>
            {
                *guard = Some(IdentityTransactionState::RecoveryRequired(recovery));
                Ok(())
            }
            _ => Err(Error::from(ErrorKind::Poison(
                "identity switch recovery state changed before retry",
            ))),
        }
    }

    pub(crate) fn finish_recovery(&self, recovery: &IdentitySwitchRecovery) -> Result<()> {
        self.finish_matching(recovery.slot_snapshot(), |state| {
            matches!(state, IdentityTransactionState::Recovering(_))
        })
    }

    pub(crate) fn abandon_to_recovery(&self, recovery: &IdentitySwitchRecovery) {
        let Ok(mut guard) = self.state.lock() else {
            return;
        };
        let matches_recovery = guard.as_ref().is_some_and(|state| {
            matches!(
                state,
                IdentityTransactionState::Committing(_)
                    | IdentityTransactionState::RollingBack(_)
                    | IdentityTransactionState::Recovering(_)
            ) && state.slot_snapshot() == recovery.slot_snapshot()
        });
        if matches_recovery {
            *guard = Some(IdentityTransactionState::RecoveryRequired(recovery.clone()));
        }
    }

    pub(crate) fn switch_in_progress(&self) -> Result<bool> {
        Ok(self.lock_state()?.is_some())
    }
}

impl Default for IdentityTransaction {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityTransactionPending {
    pub(crate) fn previous_identity(&self) -> &SecretKey {
        &self.previous.previous_identity
    }

    pub(crate) fn target_key(&self) -> &[u8; 32] {
        &self.target_key
    }

    pub(crate) fn target_contacts(&self) -> &[Contact] {
        &self.target_contacts
    }

    pub(crate) fn slot_snapshot(&self) -> CallSlotSnapshot {
        self.slot_snapshot
    }

    pub(crate) fn into_recovery(self) -> IdentitySwitchRecovery {
        IdentitySwitchRecovery {
            previous: self.previous,
            slot_snapshot: self.slot_snapshot,
        }
    }
}

impl IdentitySwitchRecovery {
    pub(crate) fn previous_identity(&self) -> &SecretKey {
        &self.previous.previous_identity
    }

    pub(crate) fn previous_contacts(&self) -> &[Contact] {
        &self.previous.previous_contacts
    }

    pub(crate) fn slot_snapshot(&self) -> CallSlotSnapshot {
        self.slot_snapshot
    }
}

pub(crate) struct RestartRequest {
    ack: oneshot::Sender<RestartOutcome>,
    cancellation: CancellationToken,
}

impl RestartRequest {
    pub(crate) fn new() -> (Self, oneshot::Receiver<RestartOutcome>, CancellationToken) {
        let (ack, receiver) = oneshot::channel();
        let cancellation = CancellationToken::new();
        (
            Self {
                ack,
                cancellation: cancellation.clone(),
            },
            receiver,
            cancellation,
        )
    }

    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn complete(self, outcome: RestartOutcome) {
        let _ = self.ack.send(outcome);
    }
}

/// Outcome published by the iteration that owns a given [`RestartRequest`].
pub(crate) enum RestartOutcome {
    /// The replacement iteration reached its active milestone.
    SetupSucceeded,
    /// `setup_endpoint` failed before the replacement iteration came
    /// online. Carries a human-readable diagnostic so the requester can
    /// surface a useful error.
    SetupFailed(String),
    Cancelled,
    ManagerStopped,
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
    pub(crate) fn output_volume_for_peer(&self, peer: PublicKey) -> Result<Arc<AtomicF32>> {
        Ok(self.get_peer_volume(peer)?.multiplier)
    }

    /// updates the base output volume in decibels
    /// all peer output volumes are updated with the new base
    pub(crate) fn set_output_volume(&self, decibel: f32) -> Result<()> {
        let lock = self
            .output_lock
            .lock()
            .map_err(|_| ErrorKind::Poison("output lock mutex poisoned"))?;
        let peer_volume_lock = self
            .peer_output_volumes
            .lock()
            .map_err(|_| ErrorKind::Poison("peer output volume mutex poisoned"))?;
        let old_decibel = self.output_volume.swap(decibel, Relaxed);
        let offset = decibel - old_decibel;
        for peer in peer_volume_lock.values() {
            let new_volume = peer.volume.fetch_add(offset, Relaxed) + offset;
            peer.multiplier.store(db_to_multiplier(new_volume), Relaxed);
        }
        drop(lock);
        Ok(())
    }

    /// updates the peer output volume for a contact
    pub(crate) fn set_peer_output_volume(&self, contact: &Contact) -> Result<()> {
        let lock = self
            .output_lock
            .lock()
            .map_err(|_| ErrorKind::Poison("output lock mutex poisoned"))?;
        let global_volume = self.output_volume.load(Relaxed);
        let peer_volume = self.get_peer_volume(contact.peer_id)?;
        let new_volume = global_volume + contact.output_volume;
        peer_volume.volume.store(new_volume, Relaxed);
        peer_volume
            .multiplier
            .store(db_to_multiplier(new_volume), Relaxed);
        drop(lock);
        Ok(())
    }

    pub(crate) fn reset_peer_output_volumes(&self) -> Result<()> {
        self.peer_output_volumes
            .lock()
            .map_err(|_| ErrorKind::Poison("peer output volume mutex poisoned"))?
            .clear();
        Ok(())
    }

    pub(crate) fn reset_peer_output_volume(&self, peer: &PublicKey) -> Result<()> {
        self.peer_output_volumes
            .lock()
            .map_err(|_| ErrorKind::Poison("peer output volume mutex poisoned"))?
            .remove(peer);
        Ok(())
    }

    fn get_peer_volume(&self, peer: PublicKey) -> Result<PeerVolume> {
        Ok(self
            .peer_output_volumes
            .lock()
            .map_err(|_| ErrorKind::Poison("peer output volume mutex poisoned"))?
            .entry(peer)
            // peers from rooms will not have a cached output volume
            .or_insert_with(|| PeerVolume::new(self.output_volume.load(Relaxed)))
            .clone())
    }
}

pub(crate) struct RoomState {
    pub(crate) peers: Vec<PublicKey>,

    pub(crate) sender: Sender<RoomMessage>,

    pub(crate) cancel: CancellationToken,

    pub(crate) end_call: Arc<Notify>,

    pub(crate) early_state: EarlyCallState,

    /// Monotonic ownership token bumped every time a new room is established.
    pub(crate) generation: u64,
}

impl RoomState {
    /// Computes the room hash from the current member list
    pub(crate) fn room_hash(&self) -> u64 {
        self.peers.iter().fold(0u64, |acc, peer| {
            let mut hasher = DefaultHasher::new();
            peer.hash(&mut hasher);
            acc ^ hasher.finish()
        })
    }
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

    /// Test-only constructor that creates a fresh `SessionState` with a
    /// unique `id` and a dummy `message_sender`. Used by integration tests
    /// to simulate a "fresh replacement session" entry in `session_states`
    /// without actually dialing a new connection. The dummy sender has no
    /// attached receiver and is never written to in test scenarios, so the
    /// dropped-on-drop sender is sufficient.
    #[cfg(feature = "integration-testing")]
    pub fn new_for_test() -> Self {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ProtocolMessage>(1);
        Self::new(&tx)
    }

    /// Returns the unique identifier for this session state.
    pub fn id(&self) -> Uuid {
        self.id
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

                    for path in paths.iter() {
                        debug!(event = "connection_path", path = ?path);
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
                                    remote_address: match primary_connection.remote_addr() {
                                        TransportAddr::Ip(socket) => socket.ip().to_string(),
                                        TransportAddr::Relay(relay_url) => relay_identifier(relay_url),
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

fn relay_identifier(relay_url: &iroh::RelayUrl) -> String {
    let port = relay_url.port().map(|port| format!(":{port}"));

    match relay_url.host() {
        Some(url::Host::Domain(domain)) => {
            let mut labels = domain.trim_end_matches('.').split('.');
            let first = labels.next().unwrap_or("unknown");
            let identifier = if first.eq_ignore_ascii_case("relay") {
                labels
                    .next()
                    .map(|second| format!("{first}.{second}"))
                    .unwrap_or_else(|| first.to_string())
            } else {
                first.to_string()
            };
            format!("{identifier}{}", port.as_deref().unwrap_or_default())
        }
        Some(url::Host::Ipv4(address)) => {
            format!("{address}{}", port.as_deref().unwrap_or_default())
        }
        Some(url::Host::Ipv6(address)) => match port {
            Some(port) => format!("[{address}]{port}"),
            None => address.to_string(),
        },
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod call_slot_tests {
    use super::{CallSlot, CallSlotAcquireResult, CallSlotState, relay_identifier};
    use iroh::{RelayUrl, SecretKey};

    #[test]
    fn relay_identifier_uses_short_distinct_part_of_domain() {
        let production: RelayUrl = "https://use1-1.relay.n0.iroh.link."
            .parse()
            .expect("valid production relay URL");
        let custom: RelayUrl = "https://relay.example.com"
            .parse()
            .expect("valid custom relay URL");

        assert_eq!(relay_identifier(&production), "use1-1");
        assert_eq!(relay_identifier(&custom), "relay.example");
    }

    #[test]
    fn relay_identifier_keeps_ip_and_port_for_local_relays() {
        let local: RelayUrl = "https://127.0.0.1:3340"
            .parse()
            .expect("valid local relay URL");

        assert_eq!(relay_identifier(&local), "127.0.0.1:3340");
    }

    impl CallSlot {
        fn try_transition(&self, from: CallSlotState, to: CallSlotState) -> bool {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if inner.state == from {
                inner.state = to;
                true
            } else {
                false
            }
        }
    }

    #[test]
    fn call_slot_acquire_and_release() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer));
        assert!(
            !slot
                .try_acquire(CallSlotState::PendingIncoming, Some(peer))
                .unwrap()
        );

        slot.release().unwrap();
        assert_eq!(slot.current(), CallSlotState::Idle);
        assert_eq!(slot.snapshot().unwrap().direct_peer, None);
    }

    #[test]
    fn call_slot_transition_pending_to_active() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingIncoming, Some(peer))
                .unwrap()
        );
        assert!(slot.try_transition(CallSlotState::PendingIncoming, CallSlotState::ActiveDirect));
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer));
    }

    #[test]
    fn call_slot_release_if_pending_for_peer() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        let other = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        slot.release_if_pending_for_peer(other).unwrap();
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);

        slot.release_if_pending_for_peer(peer).unwrap();
        assert_eq!(slot.current(), CallSlotState::Idle);
    }

    #[tokio::test]
    async fn call_slot_wait_for_release_confirms_owner_release() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        let owner = slot.snapshot().unwrap();
        let waiter = {
            let slot = slot.clone();
            tokio::spawn(async move { slot.wait_for_release(owner).await })
        };

        tokio::task::yield_now().await;
        slot.release().unwrap();

        waiter.await.unwrap().unwrap();
    }

    #[test]
    fn call_slot_clear_pending_direct_clears_pending_incoming() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingIncoming, Some(peer))
                .unwrap()
        );
        assert!(slot.clear_pending_direct().unwrap());
        assert_eq!(slot.current(), CallSlotState::Idle);
        assert_eq!(slot.snapshot().unwrap().direct_peer, None);
    }

    #[test]
    fn call_slot_clear_pending_direct_clears_pending_outgoing() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        assert!(slot.clear_pending_direct().unwrap());
        assert_eq!(slot.current(), CallSlotState::Idle);
        assert_eq!(slot.snapshot().unwrap().direct_peer, None);
    }

    #[test]
    fn call_slot_clear_pending_direct_leaves_active_direct_untouched() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::ActiveDirect, Some(peer))
                .unwrap()
        );
        assert!(!slot.clear_pending_direct().unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer));
    }

    #[test]
    fn call_slot_clear_pending_direct_leaves_room_call_untouched() {
        let slot = CallSlot::default();

        assert!(slot.try_acquire(CallSlotState::RoomCall, None).unwrap());
        assert!(!slot.clear_pending_direct().unwrap());
        assert_eq!(slot.current(), CallSlotState::RoomCall);
        assert_eq!(slot.snapshot().unwrap().direct_peer, None);
    }

    #[test]
    fn call_slot_clear_pending_direct_leaves_audio_test_untouched() {
        let slot = CallSlot::default();

        assert!(slot.try_acquire(CallSlotState::AudioTest, None).unwrap());
        assert!(!slot.clear_pending_direct().unwrap());
        assert_eq!(slot.current(), CallSlotState::AudioTest);
        assert_eq!(slot.snapshot().unwrap().direct_peer, None);
    }

    #[test]
    fn call_slot_clear_pending_direct_on_idle_is_noop() {
        let slot = CallSlot::default();
        assert!(!slot.clear_pending_direct().unwrap());
        assert_eq!(slot.current(), CallSlotState::Idle);
    }

    #[test]
    fn call_slot_try_acquire_or_match_from_idle() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)
                .unwrap(),
            CallSlotAcquireResult::Acquired
        );
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
    }

    #[test]
    fn call_slot_try_acquire_or_match_same_pending_state_matches() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)
                .unwrap(),
            CallSlotAcquireResult::MatchedPendingOutgoing
        );

        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        assert!(
            slot.try_acquire(CallSlotState::PendingIncoming, Some(peer))
                .unwrap()
        );
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingIncoming, peer)
                .unwrap(),
            CallSlotAcquireResult::MatchedPendingIncoming
        );
    }

    #[test]
    fn call_slot_try_acquire_or_match_incoming_matches_existing_outgoing() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        let before = slot.snapshot().unwrap();
        // held state is reported by the matched variant so the caller can decide whether to
        // notify on top of an already-pending outgoing request.
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingIncoming, peer)
                .unwrap(),
            CallSlotAcquireResult::MatchedPendingOutgoing
        );
        let after = slot.snapshot().unwrap();
        assert_eq!(after, before);
        assert_eq!(after.state, CallSlotState::PendingOutgoing);
        assert_eq!(after.direct_peer, Some(peer));
    }

    #[test]
    fn call_slot_try_acquire_or_match_outgoing_matches_existing_incoming() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingIncoming, Some(peer))
                .unwrap()
        );
        let before = slot.snapshot().unwrap();
        // held state is reported by the matched variant so the caller can distinguish
        // "accept the incoming prompt" (MatchedPendingIncoming) from a same-direction retry.
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)
                .unwrap(),
            CallSlotAcquireResult::MatchedPendingIncoming
        );
        let after = slot.snapshot().unwrap();
        assert_eq!(after, before);
        assert_eq!(after.state, CallSlotState::PendingIncoming);
        assert_eq!(after.direct_peer, Some(peer));
    }

    #[test]
    fn call_slot_try_acquire_or_match_fails_when_busy() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        let other = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, other)
                .unwrap(),
            CallSlotAcquireResult::Failed
        );
    }

    #[test]
    fn call_slot_try_acquire_or_match_different_peer_pending_direct_fails() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        let other = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingIncoming, Some(peer))
                .unwrap()
        );
        let before = slot.snapshot().unwrap();
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, other)
                .unwrap(),
            CallSlotAcquireResult::Failed
        );
        assert_eq!(slot.snapshot().unwrap(), before);
    }

    #[test]
    fn call_slot_try_acquire_or_match_does_not_match_active_room_or_audio_test() {
        let peer = SecretKey::generate().public();

        for state in [
            CallSlotState::ActiveDirect,
            CallSlotState::RoomCall,
            CallSlotState::AudioTest,
        ] {
            let slot = CallSlot::default();
            assert!(slot.try_acquire(state, Some(peer)).unwrap());
            let before = slot.snapshot().unwrap();

            assert_eq!(
                slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)
                    .unwrap(),
                CallSlotAcquireResult::Failed,
                "{state:?} must not match a pending outgoing request"
            );
            assert_eq!(slot.snapshot().unwrap(), before);
        }
    }

    #[test]
    fn call_slot_transition_pending_to_active_for_peer() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        assert!(slot.transition_pending_to_active_for_peer(peer).unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
    }

    #[test]
    fn call_slot_try_acquire_or_match_never_matches_after_ownership_lost() {
        use std::sync::Arc;
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        // The refactor this test guards against must keep `try_acquire_or_match` from
        // returning `Matched*` for a peer that has already released the slot, even when
        // a competing thread is racing to release and re-acquire it for a different
        // peer.
        //
        // The observer thread waits until the releaser has *committed* peer_b as
        // the new owner — meaning the releaser's `try_acquire(peer_b)` has
        // already returned `true` — and only then evaluates
        // `try_acquire_or_match` for peer_a. A `Matched*` here would mean the
        // ownership transition from peer_a to peer_b was lost — the exact
        // regression this test guards against.
        //
        // To make this a real concurrent race rather than a serial correctness
        // check, the releaser and observer both start at a barrier and the
        // releaser's start-up is jittered per iteration so the scheduler
        // exercises different timings across the 1024 iterations. The
        // observer's single post-signal call is the one that asserts the
        // invariant: a `Matched*` after peer_b has taken the slot is a bug.
        const ITERATIONS: usize = 1024;

        let slot = Arc::new(CallSlot::default());
        let peer_a = SecretKey::generate().public();
        let peer_b = SecretKey::generate().public();

        for iteration in 0..ITERATIONS {
            // Reset the slot: each iteration must start from a clean state where peer_a
            // is the only owner, otherwise we cannot attribute a `Matched*` to a lost
            // ownership transition.
            assert_eq!(slot.current(), CallSlotState::Idle);
            assert_eq!(slot.snapshot().unwrap().direct_peer, None);

            assert!(
                slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer_a))
                    .unwrap()
            );

            let start_barrier = Arc::new(Barrier::new(2));
            let observer_matched = Arc::new(AtomicBool::new(false));
            let (peer_b_tx, peer_b_rx) = mpsc::channel();

            // Releaser thread: release the slot and re-acquire it for peer_b.
            // The jitter varies the timing so different iterations exercise
            // different interleavings with the observer's barrier release.
            let releaser_slot = Arc::clone(&slot);
            let releaser_barrier = Arc::clone(&start_barrier);
            let releaser = thread::spawn(move || {
                releaser_barrier.wait();
                // Vary the delay so the interleaving changes between iterations.
                // Even a tiny jitter is enough to break any fixed ordering the
                // scheduler might otherwise settle into.
                let jitter_nanos = (iteration as u64 * 37) % 200;
                if jitter_nanos > 0 {
                    thread::sleep(Duration::from_nanos(jitter_nanos));
                }
                releaser_slot.release().unwrap();
                let reacquired = releaser_slot
                    .try_acquire(CallSlotState::PendingOutgoing, Some(peer_b))
                    .unwrap();
                assert!(
                    reacquired,
                    "iteration {iteration}: releaser failed to reclaim the slot for peer_b \
                     after peer_a released; another caller must have stolen it"
                );
                // Signal the observer that peer_b now owns the slot. Any
                // subsequent `try_acquire_or_match(peer_a)` call must
                // observe peer_b's ownership and return `Failed`.
                peer_b_tx.send(()).unwrap();
            });

            // Observer thread: wait until peer_b has committed, then call
            // `try_acquire_or_match` for the old owner peer_a. The result must
            // be `Failed` because the slot is now owned by peer_b. A `Matched*`
            // here would mean the ownership transition from peer_a to peer_b
            // was lost — the exact regression this test guards against.
            let observer_slot = Arc::clone(&slot);
            let observer_out = Arc::clone(&observer_matched);
            let observer_barrier = Arc::clone(&start_barrier);
            let observer = thread::spawn(move || {
                observer_barrier.wait();
                // Block until the releaser has committed peer_b as the owner.
                // This is the synchronization that turns the test into a real
                // check of the named invariant: peer_a's call is evaluated
                // after peer_b has definitely taken the slot.
                peer_b_rx.recv().unwrap();
                let result = observer_slot
                    .try_acquire_or_match(CallSlotState::PendingOutgoing, peer_a)
                    .unwrap();
                if matches!(
                    result,
                    CallSlotAcquireResult::MatchedPendingIncoming
                        | CallSlotAcquireResult::MatchedPendingOutgoing
                ) {
                    observer_out.store(true, Ordering::SeqCst);
                }
            });

            releaser.join().unwrap();
            observer.join().unwrap();

            assert!(
                !observer_matched.load(Ordering::SeqCst),
                "iteration {iteration}: try_acquire_or_match returned Matched* for peer_a \
                 after peer_a had already released and peer_b had re-acquired the slot"
            );

            // The slot must end up owned by peer_b at the end of the race:
            // the releaser's reacquire succeeded and the observer's
            // `try_acquire_or_match` did not change ownership.
            assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
            assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer_b));

            // Clean up so the next iteration starts from a known idle state.
            slot.release().unwrap();
        }
    }

    #[test]
    fn call_slot_try_acquire_or_match_with_owner_acquired_carries_snapshot() {
        use super::CallSlotSnapshot;

        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        let (result, owner) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::Acquired);
        let owner = owner.expect("Acquired must carry an ownership snapshot");
        assert_eq!(
            owner,
            CallSlotSnapshot {
                state: CallSlotState::PendingOutgoing,
                direct_peer: Some(peer),
                generation: 1,
            }
        );
    }

    #[test]
    fn call_slot_try_acquire_or_match_with_owner_matched_is_non_owning() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );

        let (result, owner) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingIncoming, peer)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::MatchedPendingOutgoing);
        assert!(
            owner.is_none(),
            "Matched* must be explicitly non-owning: the original acquirer owns the slot"
        );

        let (result, owner) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::MatchedPendingOutgoing);
        assert!(owner.is_none());
    }

    #[test]
    fn call_slot_try_acquire_or_match_with_owner_failed_is_non_owning() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();
        let other = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );

        let (result, owner) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, other)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::Failed);
        assert!(owner.is_none());
    }

    #[test]
    fn call_slot_acquisition_snapshot_does_not_release_post_acquisition_transition() {
        // Regression guard for the atomic ownership snapshot: a snapshot captured
        // atomically with acquisition must reflect the acquisition-time state, so
        // releasing against it after a concurrent handshake transitioned the slot
        // to ActiveDirect must NOT release the active call.
        //
        // transition_pending_to_active_for_peer preserves the generation, so a
        // non-atomic snapshot taken after the transition would share the
        // generation but report ActiveDirect; release_if_match against it would
        // succeed and release the active call. The atomic snapshot reports the
        // acquisition-time PendingOutgoing state, so the state mismatch prevents
        // the release.
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        let (result, owner) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::Acquired);
        let acquisition_snapshot = owner.expect("Acquired must carry an ownership snapshot");
        let acquisition_generation = acquisition_snapshot.generation;

        assert!(
            slot.transition_pending_to_active_for_peer(peer).unwrap(),
            "pending slot must transition to ActiveDirect for the acquiring peer"
        );
        let after_transition = slot.snapshot().unwrap();
        assert_eq!(after_transition.state, CallSlotState::ActiveDirect);
        assert_eq!(after_transition.direct_peer, Some(peer));
        assert_eq!(
            after_transition.generation, acquisition_generation,
            "transition_pending_to_active_for_peer must preserve the acquisition generation"
        );

        let released = slot.release_if_match(acquisition_snapshot).unwrap();
        assert!(
            !released,
            "cancelling the original operation must not release the active slot a \
             concurrent handshake transitioned to after acquisition"
        );

        let final_snapshot = slot.snapshot().unwrap();
        assert_eq!(final_snapshot.state, CallSlotState::ActiveDirect);
        assert_eq!(final_snapshot.direct_peer, Some(peer));
        assert_eq!(final_snapshot.generation, acquisition_generation);

        slot.release().unwrap();
    }

    #[test]
    fn call_slot_acquisition_snapshot_does_not_release_replacement_generation() {
        // Companion to the transition test above: if the slot is released and
        // re-acquired (rather than transitioned) after acquisition, the new owner
        // has a different generation. The acquisition-time snapshot must not match
        // the replacement, so the cancellation release is skipped.
        let slot = CallSlot::default();
        let peer_a = SecretKey::generate().public();
        let peer_b = SecretKey::generate().public();

        let (result, owner_a) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer_a)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::Acquired);
        let acquisition_snapshot = owner_a.expect("Acquired must carry an ownership snapshot");

        slot.release().unwrap();
        let (result, _owner_b) = slot
            .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer_b)
            .unwrap();
        assert_eq!(result, CallSlotAcquireResult::Acquired);

        let released = slot.release_if_match(acquisition_snapshot).unwrap();
        assert!(
            !released,
            "cancelling the original operation must not release a replacement \
             acquisition's slot (different generation)"
        );

        let final_snapshot = slot.snapshot().unwrap();
        assert_eq!(final_snapshot.state, CallSlotState::PendingOutgoing);
        assert_eq!(final_snapshot.direct_peer, Some(peer_b));

        slot.release().unwrap();
    }

    #[test]
    fn call_slot_snapshot_captures_state_and_peer_atomically() {
        use super::CallSlotSnapshot;
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        let idle = slot.snapshot().unwrap();
        assert_eq!(idle.state, CallSlotState::Idle);
        assert_eq!(idle.direct_peer, None);

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer))
                .unwrap()
        );
        let acquired = slot.snapshot().unwrap();
        assert_eq!(
            acquired,
            CallSlotSnapshot {
                state: CallSlotState::PendingOutgoing,
                direct_peer: Some(peer),
                generation: 1,
            }
        );
    }

    #[test]
    fn call_slot_release_if_match_releases_only_matching_snapshot() {
        use super::CallSlotSnapshot;
        let slot = CallSlot::default();
        let peer_a = SecretKey::generate().public();
        let peer_b = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::PendingOutgoing, Some(peer_a))
                .unwrap()
        );
        let snapshot = slot.snapshot().unwrap();
        assert_eq!(snapshot.state, CallSlotState::PendingOutgoing);
        assert_eq!(snapshot.direct_peer, Some(peer_a));

        assert!(slot.release_if_match(snapshot).unwrap());
        assert_eq!(slot.current(), CallSlotState::Idle);

        assert!(
            slot.try_acquire(CallSlotState::ActiveDirect, Some(peer_b))
                .unwrap()
        );
        let stale = CallSlotSnapshot {
            state: CallSlotState::PendingOutgoing,
            direct_peer: Some(peer_a),
            generation: 1,
        };
        assert!(!slot.release_if_match(stale).unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer_b));
    }

    #[test]
    fn call_slot_release_if_match_never_releases_newer_call() {
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::thread;

        let slot = Arc::new(CallSlot::default());
        let peer_a = SecretKey::generate().public();
        let peer_b = SecretKey::generate().public();

        assert!(
            slot.try_acquire(CallSlotState::ActiveDirect, Some(peer_a))
                .unwrap()
        );
        let failing_snapshot = slot.snapshot().unwrap();

        let (ready, wait) = mpsc::channel();
        let releaser = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                slot.release().unwrap();
                assert!(
                    slot.try_acquire(CallSlotState::ActiveDirect, Some(peer_b))
                        .unwrap()
                );
                ready.send(()).unwrap();
            })
        };
        wait.recv().unwrap();

        assert!(!slot.release_if_match(failing_snapshot).unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer_b));
        releaser.join().unwrap();
    }

    /// Regression test for the direct-call teardown race.
    ///
    /// `call_handshake` now captures an `ActiveDirect` snapshot of the slot *before* the
    /// long-running `call()` and uses that fixed expectation at teardown. This test
    /// reproduces the same shape: a stale snapshot is taken when peer_a owns the slot,
    /// the slot is then released and re-acquired by peer_b, and the teardown path must
    /// observe the mismatch and skip the release. With a freshly-read snapshot, peer_b's
    /// slot would be released incorrectly.
    #[test]
    fn call_slot_stale_teardown_does_not_release_newer_direct_call() {
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::thread;

        let slot = Arc::new(CallSlot::default());
        let peer_a = SecretKey::generate().public();
        let peer_b = SecretKey::generate().public();

        // Simulate the post-handshake state: peer_a is in an active direct call.
        assert!(
            slot.try_acquire(CallSlotState::ActiveDirect, Some(peer_a))
                .unwrap()
        );

        // Snapshot the expected owner *before* the long-running call path, mirroring
        // the fixed expectation now captured in `call_handshake` immediately after
        // `transition_pending_to_active_for_peer` succeeds.
        let expected_active = slot.snapshot().unwrap();
        assert_eq!(expected_active.state, CallSlotState::ActiveDirect);
        assert_eq!(expected_active.direct_peer, Some(peer_a));

        // While the call is running, another path releases the slot and re-acquires
        // it for a different call. This must NOT happen in the real call path because
        // a direct call holds the slot exclusively, but the regression we are guarding
        // against is exactly the case where it could happen and a stale teardown would
        // clobber the new owner.
        let (ready, wait) = mpsc::channel();
        let releaser = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                slot.release().unwrap();
                assert!(
                    slot.try_acquire(CallSlotState::ActiveDirect, Some(peer_b))
                        .unwrap()
                );
                ready.send(()).unwrap();
            })
        };
        wait.recv().unwrap();

        // The fixed expectation from before must NOT match the new owner, so the
        // teardown's `release_if_match` returns false and the slot is preserved.
        assert!(!slot.release_if_match(expected_active).unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer_b));
        releaser.join().unwrap();
    }

    /// Regression test for the room teardown race.
    ///
    /// `room_controller` now captures a fixed `RoomCall` expectation at startup (the slot
    /// is acquired as `RoomCall` with no direct peer in `join_room`) and uses that
    /// expectation at teardown. This test reproduces the same shape: a stale fresh
    /// snapshot at teardown time would observe a different (newer) call's slot and
    /// release it. With the fixed `RoomCall` expectation, the release is skipped when
    /// the slot is no longer the room's.
    #[test]
    fn call_slot_stale_teardown_does_not_release_newer_room_call() {
        use super::CallSlotSnapshot;
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::thread;

        let slot = Arc::new(CallSlot::default());
        let peer = SecretKey::generate().public();

        // Simulate the post-`join_room` state: the room owns the slot as `RoomCall`.
        assert!(slot.try_acquire(CallSlotState::RoomCall, None).unwrap());

        // Capture the fixed expectation up front, mirroring the snapshot now built in
        // `room_controller` before the long-running loop.
        let expected_room = CallSlotSnapshot {
            state: CallSlotState::RoomCall,
            direct_peer: None,
            generation: 1,
        };

        // While the room controller is running, another path (e.g. an audio test or
        // a newer direct call) releases the slot and acquires a new state. The
        // teardown must observe that the slot no longer matches `RoomCall`/`None`
        // and skip the release.
        let (ready, wait) = mpsc::channel();
        let releaser = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                slot.release().unwrap();
                assert!(
                    slot.try_acquire(CallSlotState::ActiveDirect, Some(peer))
                        .unwrap()
                );
                ready.send(()).unwrap();
            })
        };
        wait.recv().unwrap();

        assert!(!slot.release_if_match(expected_room).unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer));
        releaser.join().unwrap();
    }

    /// Regression test for the slot-generation token.
    ///
    /// Two acquisitions of the slot by the *same* peer in the *same* state would, without
    /// a generation token, produce indistinguishable snapshots. A teardown holding the
    /// earlier snapshot could then release a slot it no longer owns. This test reproduces
    /// the failure mode the generation token guards against: a fresh acquisition bumps the
    /// generation, so the stale snapshot from the prior owner does not match and the slot
    /// is preserved for the newer owner.
    #[test]
    fn call_slot_generation_token_distinguishes_same_peer_reacquire() {
        use super::CallSlotSnapshot;
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        // First acquisition for peer in `ActiveDirect`.
        assert!(
            slot.try_acquire(CallSlotState::ActiveDirect, Some(peer))
                .unwrap()
        );
        let first = slot.snapshot().unwrap();
        assert_eq!(first.state, CallSlotState::ActiveDirect);
        assert_eq!(first.direct_peer, Some(peer));
        let first_generation = first.generation;
        assert!(first_generation > 0);

        // Simulate the teardown path: release the slot, then a *newer* call from the same
        // peer acquires it again in the same state. The newer acquisition MUST bump the
        // generation so a stale snapshot from the first call cannot release the new one.
        slot.release().unwrap();
        assert_eq!(slot.snapshot().unwrap().state, CallSlotState::Idle);

        assert!(
            slot.try_acquire(CallSlotState::ActiveDirect, Some(peer))
                .unwrap()
        );
        let second = slot.snapshot().unwrap();
        assert_eq!(
            second,
            CallSlotSnapshot {
                state: CallSlotState::ActiveDirect,
                direct_peer: Some(peer),
                generation: first_generation + 1,
            }
        );
        assert_ne!(first, second);

        // The stale teardown snapshot from the first call MUST NOT release the slot now
        // owned by the second call.
        assert!(!slot.release_if_match(first).unwrap());
        assert_eq!(slot.current(), CallSlotState::ActiveDirect);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer));
        assert_eq!(slot.snapshot().unwrap().generation, first_generation + 1);

        // The matching snapshot from the second call MUST still release correctly.
        assert!(slot.release_if_match(second).unwrap());
        assert_eq!(slot.current(), CallSlotState::Idle);
    }

    /// Regression test for the slot-generation token across `try_acquire_or_match`.
    ///
    /// A matched simultaneous-dial path must preserve the existing generation so both peers
    /// observe the same ownership token. A later reacquire of the slot (e.g. by the same
    /// outgoing call after a release) must bump the generation so a stale matched
    /// snapshot cannot release the new acquisition.
    #[test]
    fn call_slot_generation_token_distinguishes_matched_then_reacquired_direct() {
        let slot = CallSlot::default();
        let peer = SecretKey::generate().public();

        // Outgoing call acquires the slot first.
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)
                .unwrap(),
            CallSlotAcquireResult::Acquired
        );
        let outgoing_snapshot = slot.snapshot().unwrap();
        let outgoing_generation = outgoing_snapshot.generation;
        assert!(outgoing_generation > 0);

        // Incoming call for the same peer matches (simultaneous dial); the generation MUST
        // be preserved so both sides observe the same ownership token. The variant reports
        // the held pending state so the caller can tell this is matching a peer's outgoing
        // request rather than a same-direction retry.
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingIncoming, peer)
                .unwrap(),
            CallSlotAcquireResult::MatchedPendingOutgoing
        );
        let matched = slot.snapshot().unwrap();
        assert_eq!(matched.generation, outgoing_generation);
        assert_eq!(matched.state, CallSlotState::PendingOutgoing);
        assert_eq!(matched.direct_peer, Some(peer));

        // Release the slot, then re-acquire via `try_acquire_or_match` for the same peer.
        // The generation MUST bump so a stale matched snapshot from the prior owner does
        // not match the new acquisition.
        slot.release().unwrap();
        assert_eq!(
            slot.try_acquire_or_match(CallSlotState::PendingOutgoing, peer)
                .unwrap(),
            CallSlotAcquireResult::Acquired
        );
        let reacquired = slot.snapshot().unwrap();
        assert_eq!(reacquired.generation, outgoing_generation + 1);
        assert_ne!(matched, reacquired);

        assert!(!slot.release_if_match(matched).unwrap());
        assert_eq!(slot.current(), CallSlotState::PendingOutgoing);
        assert_eq!(slot.snapshot().unwrap().direct_peer, Some(peer));
    }

    /// Regression test for the slot-generation token on `RoomCall`.
    ///
    /// Two consecutive room acquisitions would, without a generation token, produce
    /// indistinguishable `(RoomCall, None)` snapshots. A teardown of the older room
    /// holding the earlier snapshot could then release a slot now owned by the newer
    /// room. This test reproduces the failure mode the generation token guards against:
    /// after a release + reacquire, the stale snapshot must not match the new owner.
    #[test]
    fn call_slot_generation_token_distinguishes_room_reacquire() {
        use super::CallSlotSnapshot;
        let slot = CallSlot::default();

        // First room acquires the slot.
        assert!(slot.try_acquire(CallSlotState::RoomCall, None).unwrap());
        let first_room = slot.snapshot().unwrap();
        assert_eq!(first_room.state, CallSlotState::RoomCall);
        assert_eq!(first_room.direct_peer, None);
        let first_generation = first_room.generation;
        assert!(first_generation > 0);

        // Simulate the older room's teardown: it releases the slot, then a newer room
        // acquires the same state with the same (None) peer. The newer acquisition MUST
        // bump the generation.
        slot.release().unwrap();
        assert!(slot.try_acquire(CallSlotState::RoomCall, None).unwrap());
        let second_room = slot.snapshot().unwrap();
        assert_eq!(
            second_room,
            CallSlotSnapshot {
                state: CallSlotState::RoomCall,
                direct_peer: None,
                generation: first_generation + 1,
            }
        );
        assert_ne!(first_room, second_room);

        // The older room's teardown snapshot MUST NOT release the slot now owned by the
        // newer room.
        assert!(!slot.release_if_match(first_room).unwrap());
        assert_eq!(slot.current(), CallSlotState::RoomCall);
        assert_eq!(slot.snapshot().unwrap().generation, first_generation + 1);

        // The newer room's teardown snapshot MUST still release correctly.
        assert!(slot.release_if_match(second_room).unwrap());
        assert_eq!(slot.current(), CallSlotState::Idle);
    }
}
