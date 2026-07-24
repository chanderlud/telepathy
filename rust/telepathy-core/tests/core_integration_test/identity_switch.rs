use super::common::{
    DEFAULT_SAMPLE_RATE, MockTelepathyHandle, assert_call_slot_idle, init_test_tracing,
    shared_address_lookup, shared_relay_map,
};
use std::future::{Future, poll_fn};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::callbacks::{MockCoreCallbacks, MockCoreStatisticsCallback};
use telepathy_core::internal::state::CallSlotState;
use telepathy_core::overlay::Overlay;
use telepathy_core::types::{
    CallState, CodecConfig, Contact, IdentitySwitchError, ManagerState, NetworkConfig,
    ScreenshareConfig, SessionStatus,
};

use iroh::SecretKey;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, channel, unbounded_channel};
use tokio::time::{interval, timeout};
type MockHandle = MockTelepathyHandle<MockAudioHost<MockAudioInput, MockAudioOutput>, (), ()>;

const MANAGER_EVENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ManagerStateKind {
    Stopped,
    Starting,
    Active,
    Failed,
}

fn manager_state_kind(state: &ManagerState) -> ManagerStateKind {
    match state {
        ManagerState::Stopped => ManagerStateKind::Stopped,
        ManagerState::Starting => ManagerStateKind::Starting,
        ManagerState::Active => ManagerStateKind::Active,
        ManagerState::Failed => ManagerStateKind::Failed,
    }
}

#[derive(Clone)]
struct ManagerStateGate {
    target: ManagerStateKind,
    occurrence: usize,
    seen: Arc<AtomicUsize>,
    reached: Arc<Notify>,
    release: Arc<Notify>,
}

impl ManagerStateGate {
    fn new(target: ManagerStateKind, occurrence: usize) -> Self {
        Self {
            target,
            occurrence,
            seen: Arc::new(AtomicUsize::new(0)),
            reached: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }

    async fn block_if_target(&self, state: &ManagerState) {
        if manager_state_kind(state) != self.target {
            return;
        }
        let occurrence = self.seen.fetch_add(1, Relaxed) + 1;
        if occurrence == self.occurrence {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }

    async fn wait_reached(&self) {
        timeout(MANAGER_EVENT_TIMEOUT, self.reached.notified())
            .await
            .expect("timed out waiting for manager callback gate");
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

fn restart_network_config() -> NetworkConfig {
    NetworkConfig::mock(
        0,
        shared_relay_map(),
        None,
        None,
        None,
        Some(shared_address_lookup().clone()),
    )
}

async fn wait_for_manager_state(
    states: &mut UnboundedReceiver<ManagerState>,
    expected: ManagerState,
) {
    timeout(MANAGER_EVENT_TIMEOUT, async {
        loop {
            let state = states
                .recv()
                .await
                .expect("manager state channel closed before expected callback");
            if matches!(
                (&state, &expected),
                (ManagerState::Stopped, ManagerState::Stopped)
                    | (ManagerState::Starting, ManagerState::Starting)
                    | (ManagerState::Active, ManagerState::Active)
                    | (ManagerState::Failed, ManagerState::Failed)
            ) {
                return;
            }
        }
    })
    .await
    .expect("timed out waiting for manager state callback");
}

fn configure_relays(network_config: &NetworkConfig, relays: Vec<String>) {
    network_config
        .update(
            0,
            vec!["0.0.0.0".to_string()],
            Some(relays),
            None,
            None,
            None,
        )
        .expect("relay configuration should be valid");
}

async fn stalled_relay() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stalled relay listener should bind");
    let address = listener
        .local_addr()
        .expect("stalled relay listener should expose its address");
    (listener, format!("https://{address}"))
}

/// Builds a [MockHandle] whose callbacks count `get_contacts` invocations
/// into `get_contacts_call_count` so the test can prove the two-phase
/// commit does NOT consult `get_contacts` when rehydrating sessions.
async fn build_handle(
    identity: SecretKey,
    contacts_for_get_contacts: Vec<Contact>,
    contacts_for_get_contact: Vec<Contact>,
    codec_config: &CodecConfig,
    get_contacts_call_count: Arc<std::sync::atomic::AtomicUsize>,
) -> MockHandle {
    let relay_map = shared_relay_map();
    let network_config = NetworkConfig::mock(
        0,
        relay_map,
        None,
        None,
        None,
        Some(shared_address_lookup().clone()),
    );
    let screenshare = ScreenshareConfig::default();
    let overlay = Overlay::default();

    let mut mock: MockCoreCallbacks<MockCoreStatisticsCallback> = MockCoreCallbacks::new();

    mock.expect_session_status()
        .returning(|_, _| Box::pin(async {}));
    mock.expect_call_state().returning(|_| Box::pin(async {}));
    mock.expect_manager_state()
        .returning(|_| Box::pin(async {}));
    mock.expect_screenshare_started()
        .returning(|_, _| Box::pin(async {}));
    mock.expect_message_received()
        .returning(|_| Box::pin(async {}));
    mock.expect_get_accept_handle()
        .returning(|_, _, _| tokio::spawn(async { false }));
    mock.expect_statistics_callback()
        .returning(MockCoreStatisticsCallback::new);

    let list = contacts_for_get_contacts.clone();
    let counter = get_contacts_call_count.clone();
    mock.expect_get_contacts().returning(move || {
        let list = list.clone();
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Relaxed);
            list
        })
    });

    mock.expect_get_contact().returning(move |peer_id| {
        let contacts = contacts_for_get_contact.clone();
        Box::pin(async move {
            contacts
                .iter()
                .find(|c| c.get_peer_id().to_vec() == peer_id)
                .cloned()
        })
    });

    let mut telepathy: MockHandle = telepathy_core::internal::TelepathyHandle::new(
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        &network_config,
        &screenshare,
        &overlay,
        codec_config,
        mock,
    );
    *telepathy.inner.core_state.identity.write().await = Some(identity);
    telepathy.start_manager().await;
    telepathy.inner.core_state.manager_active.notified().await;
    telepathy
}

/// The two-phase identity-switch transaction must rehydrate sessions for
/// the TARGET contact snapshot the frontend passes to `commit_identity_switch`,
/// NOT the stale `get_contacts` value the callback would return while the
/// frontend is mid-mutation. This is the regression that motivated the
/// transaction: the previous one-shot `switch_identity_and_restart_manager`
/// always read contacts from `get_contacts`, so the new identity's session
/// set ended up populated from the OLD active profile's contacts.
///
/// This test proves the fix at the API contract level: `get_contacts` is
/// consulted exactly once during `begin_identity_switch` (to capture the
/// pre-transaction snapshot) and ZERO additional times during
/// `commit_identity_switch`. A regression that switched back to the
/// callback-driven `restart_manager_inner` path would fail the assertion
/// because the callback fires again on every restart.
#[tokio::test(flavor = "multi_thread")]
async fn commit_identity_switch_does_not_consult_get_contacts_during_commit() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_c = SecretKey::generate();
    let contact_c = Contact::new("client-c-target".to_string(), key_c.public().to_string())
        .expect("contact c invalid");

    // The stale list returned by get_contacts — represents the previous
    // active profile's contacts the frontend still exposes mid-mutation.
    let stale_contacts: Vec<Contact> = Vec::new();
    // The up-to-date list used for get_contact (singular) lookups so a
    // session for the target contact can fully establish if a partner
    // were attached; not required for the regression check.
    let lookup_contacts = vec![contact_c.clone()];

    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let telepathy = build_handle(
        key_a.clone(),
        stale_contacts,
        lookup_contacts,
        &codec_config,
        counter.clone(),
    )
    .await;

    // Sanity: start_manager itself does not call get_contacts.
    let calls_after_start = counter.load(Relaxed);
    assert_eq!(
        calls_after_start, 0,
        "manager startup must not seed sessions through get_contacts; \
         otherwise the regression scenario could not be distinguished from \
         the explicit-snapshot path"
    );

    // begin captures the pre-transaction snapshot via get_contacts AND stashes
    // the validated target payload (key + explicit contact list).
    telepathy
        .begin_identity_switch(key_a.to_bytes(), vec![contact_c])
        .await
        .expect("begin_identity_switch should succeed when slot is idle");
    let calls_after_begin = counter.load(Relaxed);
    assert_eq!(
        calls_after_begin, 1,
        "begin_identity_switch must snapshot the previous contact set via \
         get_contacts exactly once"
    );
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::IdentitySwitch,
        "the IdentitySwitch gate must be held between begin and commit"
    );

    // commit_identity_switch rehydrates sessions through the EXPLICIT
    // contacts snapshot validated and stashed at begin. It must NOT consult
    // get_contacts again.
    telepathy
        .commit_identity_switch()
        .await
        .expect("commit_identity_switch should succeed on the happy path");

    // Give the restarted manager a brief window to settle any async
    // session setup work; if commit had consulted get_contacts, the count
    // would already be > 1 by this point.
    let mut poll = interval(Duration::from_millis(50));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        poll.tick().await;
        if counter.load(Relaxed) > 1 {
            panic!(
                "commit_identity_switch must NOT consult get_contacts while \
                 rehydrating sessions; the two-phase contract requires the \
                 explicit contact snapshot to be the only source. A regression \
                 to the callback-driven restart_manager_inner path would \
                 re-introduce the stale-contacts bug."
            );
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    assert_eq!(
        counter.load(Relaxed),
        1,
        "get_contacts must remain at exactly 1 invocation across the full \
         begin+commit transaction"
    );

    assert_call_slot_idle(
        &super::common::ClientHarness {
            telepathy,
            is_active: Arc::new(AtomicBool::new(false)),
        },
        "the IdentitySwitch gate must be released after a successful commit",
    );
}

/// `cancel_identity_switch` is the path the frontend takes when its own
/// persistence failed between `begin` and `commit`. It must release the
/// gate without mutating the signing identity or session set.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_identity_switch_releases_gate_without_mutating_identity() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key = SecretKey::generate();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let telepathy = build_handle(key.clone(), Vec::new(), Vec::new(), &codec_config, counter).await;

    let identity_before = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("identity must be set before begin");

    telepathy
        .begin_identity_switch(key.to_bytes(), Vec::new())
        .await
        .expect("begin should succeed");
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::IdentitySwitch,
        "begin must hold the IdentitySwitch gate"
    );

    telepathy.cancel_identity_switch().await;

    let identity_after = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("identity must still be set after cancel");
    assert_eq!(
        identity_after.to_bytes(),
        identity_before.to_bytes(),
        "cancel must NOT mutate the signing identity"
    );

    assert_call_slot_idle(
        &super::common::ClientHarness {
            telepathy,
            is_active: Arc::new(AtomicBool::new(false)),
        },
        "cancel must release the IdentitySwitch gate so the backend can \
         accept new calls",
    );
}

/// `begin_identity_switch` must reject when the slot is already in a
/// non-idle state. Otherwise a profile switch could race an in-flight call
/// and leave the backend in an inconsistent state.
#[tokio::test(flavor = "multi_thread")]
async fn begin_identity_switch_rejects_when_slot_is_non_idle() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key = SecretKey::generate();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let telepathy = build_handle(key.clone(), Vec::new(), Vec::new(), &codec_config, counter).await;

    let acquired = telepathy
        .inner
        .core_state
        .call_slot
        .try_acquire(CallSlotState::ActiveDirect, None)
        .expect("acquire for the competing state should succeed");
    assert!(acquired, "test setup: slot must be acquired");

    let outcome = telepathy
        .begin_identity_switch(key.to_bytes(), Vec::new())
        .await;
    assert!(
        outcome.is_err(),
        "begin_identity_switch must reject when the slot is already in a \
         non-idle state; otherwise a profile switch could race an in-flight \
         call"
    );

    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::ActiveDirect,
        "a rejected begin must leave the slot in its prior state"
    );
}

// Suppress unused-import warnings for symbols referenced only by other
// modules in the same integration-test crate. Keeping these in scope makes
// future additions to this module cheaper.
#[allow(dead_code)]
fn _force_link_common_imports(
    _state: CallState,
    _status: SessionStatus,
    _manager: ManagerState,
    _active: Arc<AtomicBool>,
    _states: Arc<Mutex<Vec<CallState>>>,
) {
}

/// `cancel_identity_switch` without a preceding `begin_identity_switch`
/// must be a safe no-op for every non-idle slot state. The previous
/// implementation called `release()` unconditionally, so a stray or racing
/// cancel could invalidate an unrelated active call, pending negotiation,
/// room call, or audio test.
///
/// This test exercises each non-idle state by directly installing it on
/// the slot (simulating an unrelated owner) and proves cancel leaves the
/// slot in that exact state, not Idle.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_identity_switch_without_begin_is_noop_for_each_non_idle_state() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);

    for non_idle in [
        CallSlotState::PendingIncoming,
        CallSlotState::PendingOutgoing,
        CallSlotState::ActiveDirect,
        CallSlotState::RoomCall,
        CallSlotState::AudioTest,
    ] {
        let key = SecretKey::generate();
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let telepathy = build_handle(key, Vec::new(), Vec::new(), &codec_config, counter).await;

        let peer = iroh::PublicKey::from_bytes(&[1u8; 32]).unwrap();
        let installed = telepathy
            .inner
            .core_state
            .call_slot
            .try_acquire(non_idle, Some(peer))
            .expect("acquire for the unrelated state should succeed");
        assert!(
            installed,
            "test setup: slot must be acquired for {non_idle:?}"
        );

        // cancel without begin must not touch the slot.
        telepathy.cancel_identity_switch().await;

        assert_eq!(
            telepathy.inner.core_state.call_slot.current(),
            non_idle,
            "cancel without a pending transaction must leave the slot in \
             its prior state ({non_idle:?}); a stray cancel must not \
             invalidate an unrelated owner"
        );

        telepathy.shutdown().await;
    }
}

/// Concurrent commit and cancel: the transaction state machine serializes
/// terminal actions so only one consumes the pending transaction. A cancel
/// that arrives after commit has consumed the transaction is a no-op and
/// must NOT release a slot it no longer owns. The previous `release()`-based
/// cancel would clobber a future caller that re-acquired the slot.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_after_commit_is_noop_and_does_not_touch_slot() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key = SecretKey::generate();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let telepathy = build_handle(key.clone(), Vec::new(), Vec::new(), &codec_config, counter).await;

    telepathy
        .begin_identity_switch(key.to_bytes(), Vec::new())
        .await
        .expect("begin should succeed");

    // Commit consumes the transaction and releases the slot through
    // release_if_match keyed on the begin-captured snapshot.
    telepathy
        .commit_identity_switch()
        .await
        .expect("commit should succeed on the happy path");
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle,
        "commit must release the IdentitySwitch gate"
    );

    // Simulate a racing cancel that arrives after commit already consumed
    // the transaction. It must observe no pending transaction, do nothing,
    // and leave the slot Idle. If a future caller acquired the slot between
    // commit and this cancel, the cancel must NOT release that caller's slot.
    let peer = SecretKey::generate().public();
    telepathy
        .inner
        .core_state
        .call_slot
        .try_acquire(CallSlotState::AudioTest, Some(peer))
        .expect("future caller should acquire the slot");

    telepathy.cancel_identity_switch().await;

    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::AudioTest,
        "trailing cancel after commit must NOT release the unrelated \
         AudioTest owner; that is the regression the state-machine + \
         release_if_match design prevents"
    );
}

/// A failed `begin_identity_switch` (slot busy) must leave the slot in its
/// prior state AND a subsequent legitimate slot acquisition must succeed.
/// This is the regression for the wedge bug: previously, validation
/// happened inside commit AFTER the slot was reserved, so a malformed
/// target key left the slot permanently held because no commit/cancel
/// could release it. The new API validates at begin, before any slot is
/// reserved, so the slot can never be wedged by a malformed payload.
#[tokio::test(flavor = "multi_thread")]
async fn begin_rejection_preserves_slot_for_subsequent_acquisition() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key = SecretKey::generate();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let telepathy = build_handle(key.clone(), Vec::new(), Vec::new(), &codec_config, counter).await;

    // Hold the slot with an unrelated owner.
    let peer = SecretKey::generate().public();
    telepathy
        .inner
        .core_state
        .call_slot
        .try_acquire(CallSlotState::AudioTest, Some(peer))
        .expect("unrelated acquire should succeed");

    // begin must reject without touching the unrelated owner.
    let outcome = telepathy
        .begin_identity_switch(key.to_bytes(), Vec::new())
        .await;
    assert!(
        outcome.is_err(),
        "begin must reject when slot is held by an unrelated owner"
    );
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::AudioTest,
        "rejected begin must leave the unrelated owner in place"
    );

    // Release the unrelated owner; a fresh begin must succeed — proving
    // the prior rejection did not leave the slot wedged.
    telepathy
        .inner
        .core_state
        .call_slot
        .release()
        .expect("release should succeed");

    telepathy
        .begin_identity_switch(key.to_bytes(), Vec::new())
        .await
        .expect("subsequent begin after release must succeed");
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::IdentitySwitch,
        "subsequent begin must acquire the IdentitySwitch gate"
    );

    telepathy.cancel_identity_switch().await;
}

/// Production-path coverage for the generation-scoped restart handshake
/// (comment 3). `commit_identity_switch` drives `restart_manager_with_contacts`,
/// which captures `manager_generation` BEFORE signaling restart and waits
/// for an outcome with a strictly newer generation. Without the fix
/// (capturing from the outcome watch instead), an in-progress startup
/// generation could satisfy its own queued restart, leaving the
/// replacement iteration unsynchronized with the newly installed identity.
///
/// Generation counters are `pub(crate)` and not exposed to the integration
/// suite (comment 5), so this test verifies the production-observable
/// contract: commit completes within `MANAGER_RESTART_TIMEOUT`, installs
/// the target identity, and releases the gate — proving the waiter
/// unblocked on a real replacement iteration rather than the in-progress
/// one or a timeout.
#[tokio::test(flavor = "multi_thread")]
async fn commit_identity_switch_drives_replacement_manager_iteration() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let telepathy = build_handle(
        key_a.clone(),
        Vec::new(),
        Vec::new(),
        &codec_config,
        counter.clone(),
    )
    .await;

    telepathy
        .begin_identity_switch(key_b.to_bytes(), Vec::new())
        .await
        .expect("begin should succeed with idle slot");

    let start = std::time::Instant::now();
    telepathy
        .commit_identity_switch()
        .await
        .expect("commit should drive the replacement iteration and succeed");
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(45),
        "commit must complete well within MANAGER_RESTART_TIMEOUT, \
         took {elapsed:?}; a near-timeout elapsed time would indicate the \
         waiter failed to observe the replacement iteration's outcome",
    );

    let identity_after = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("identity must still be set after commit");
    assert_eq!(
        identity_after.to_bytes(),
        key_b.to_bytes(),
        "commit must install the target identity once the replacement \
         iteration becomes active",
    );

    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle,
        "commit must release the IdentitySwitch gate after the replacement \
         iteration publishes its outcome",
    );

    telepathy.shutdown().await;
}

/// Builds a [MockHandle] whose `manager_state` callback records every
/// observed state into `state_log`. Used by the queue-serialization tests
/// below to count how many session-manager iterations reached their active
/// milestone — one `ManagerState::Active` per iteration — and so prove that
/// each restart request drove its OWN replacement iteration rather than
/// being satisfied by another request's iteration.
async fn build_handle_with_state_log(
    identity: Option<SecretKey>,
    network_config: &NetworkConfig,
    codec_config: &CodecConfig,
    state_log: Arc<Mutex<Vec<ManagerState>>>,
    state_gate: Option<ManagerStateGate>,
) -> (MockHandle, UnboundedReceiver<ManagerState>) {
    let screenshare = ScreenshareConfig::default();
    let overlay = Overlay::default();
    let (state_sender, state_receiver) = unbounded_channel();

    let mut mock: MockCoreCallbacks<MockCoreStatisticsCallback> = MockCoreCallbacks::new();
    mock.expect_session_status()
        .returning(|_, _| Box::pin(async {}));
    mock.expect_call_state().returning(|_| Box::pin(async {}));
    let log_for_manager_state = state_log.clone();
    mock.expect_manager_state().returning(move |state| {
        let log = log_for_manager_state.clone();
        let sender = state_sender.clone();
        let gate = state_gate.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.block_if_target(&state).await;
            }
            log.lock().unwrap().push(state.clone());
            let _ = sender.send(state);
        })
    });
    mock.expect_screenshare_started()
        .returning(|_, _| Box::pin(async {}));
    mock.expect_message_received()
        .returning(|_| Box::pin(async {}));
    mock.expect_get_accept_handle()
        .returning(|_, _, _| tokio::spawn(async { false }));
    mock.expect_statistics_callback()
        .returning(MockCoreStatisticsCallback::new);

    // Return an empty contact list so restart_manager_inner's get_contacts
    // path does not dial peers (the test exercises the queue, not the
    // session-rehydration path).
    mock.expect_get_contacts()
        .returning(|| Box::pin(async { Vec::new() }));
    mock.expect_get_contact()
        .returning(|_| Box::pin(async { None }));

    let mut telepathy: MockHandle = telepathy_core::internal::TelepathyHandle::new(
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        network_config,
        &screenshare,
        &overlay,
        codec_config,
        mock,
    );
    *telepathy.inner.core_state.identity.write().await = identity;
    telepathy.start_manager().await;
    (telepathy, state_receiver)
}

fn count_active(state_log: &Arc<Mutex<Vec<ManagerState>>>) -> usize {
    state_log
        .lock()
        .unwrap()
        .iter()
        .filter(|state| matches!(state, ManagerState::Active))
        .count()
}

/// Committing an identity switch immediately after `start_manager()` must
/// drive a fresh session-manager iteration for the commit's restart request
/// — the initial iteration spawned by `start_manager()` cannot satisfy it.
///
/// In the command-channel handshake this is structural: the initial iteration
/// has no assigned requester and therefore never sends on any request's
/// one-shot ack channel, so the commit's `request_restart` await cannot
/// unblock until the manager loop drains the commit's request from the
/// queue and assigns it to a brand-new iteration.
///
/// The observable signal is the `ManagerState::Active` count: one for the
/// initial iteration, plus one for the commit's replacement iteration. A
/// regression that let the commit be satisfied by the initial iteration
/// (e.g. a watch-based handshake keyed on a generation the initial iter
/// already published) would leave the count at 1.
#[tokio::test(flavor = "multi_thread")]
async fn commit_identity_switch_after_start_manager_drives_replacement_iteration() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let state_log: Arc<Mutex<Vec<ManagerState>>> = Arc::new(Mutex::new(Vec::new()));
    let network_config = restart_network_config();
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(key_a.clone()),
        &network_config,
        &codec_config,
        state_log.clone(),
        None,
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;

    assert_eq!(
        count_active(&state_log),
        1,
        "the initial start_manager iteration must reach its active milestone \
         exactly once before commit"
    );

    telepathy
        .begin_identity_switch(key_b.to_bytes(), Vec::new())
        .await
        .expect("begin should succeed on idle slot");
    telepathy
        .commit_identity_switch()
        .await
        .expect("commit should drive its own replacement iteration");

    assert_eq!(
        count_active(&state_log),
        2,
        "commit must drive its own replacement iteration to its active \
         milestone; the initial start_manager iteration cannot satisfy the \
         commit's restart request"
    );

    let identity_after = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("identity must still be set after commit");
    assert_eq!(
        identity_after.to_bytes(),
        key_b.to_bytes(),
        "commit must install the target identity once its replacement \
         iteration becomes active"
    );

    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle,
        "commit must release the IdentitySwitch gate"
    );

    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_identity_switch_blocks_public_session_until_target_manager_is_active() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let previous_key = SecretKey::generate();
    let target_key = SecretKey::generate();
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let target_active_gate = ManagerStateGate::new(ManagerStateKind::Active, 2);
    let network_config = restart_network_config();
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(previous_key),
        &network_config,
        &codec_config,
        state_log,
        Some(target_active_gate.clone()),
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;
    let contact = Contact::new(
        "commit-transition-contact".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");

    telepathy
        .begin_identity_switch(target_key.to_bytes(), Vec::new())
        .await
        .expect("begin should acquire the identity switch gate");
    let telepathy = Arc::new(telepathy);
    let commit_telepathy = telepathy.clone();
    let commit = tokio::spawn(async move { commit_telepathy.commit_identity_switch().await });
    target_active_gate.wait_reached().await;
    assert!(
        telepathy.try_start_session(&contact).await.is_err(),
        "public session start must reject while target manager activation is pending"
    );
    telepathy.cancel_identity_switch().await;
    assert!(
        !commit.is_finished(),
        "cancel must not terminate an in-flight identity switch"
    );
    target_active_gate.release();
    commit
        .await
        .expect("commit task should not panic")
        .expect("commit should succeed after target manager activation");
    assert!(
        telepathy.try_start_session(&contact).await.is_ok(),
        "public session start should unblock after target manager activation and gate release"
    );
    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_session_start_serializes_with_identity_switch_begin() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key = SecretKey::generate();
    let counter = Arc::new(AtomicUsize::new(0));
    let mut telepathy =
        build_handle(key.clone(), Vec::new(), Vec::new(), &codec_config, counter).await;
    let (sender, mut receiver) = channel(1);
    let filler = SecretKey::generate().public();
    sender
        .send(filler)
        .await
        .expect("test channel should accept its filler");
    telepathy.inner.start_session = Some(sender);
    let contact = Contact::new(
        "serialized-session-contact".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");
    let telepathy = Arc::new(telepathy);
    let start_telepathy = telepathy.clone();
    let start_contact = contact.clone();
    let mut start =
        tokio::spawn(async move { start_telepathy.try_start_session(&start_contact).await });
    assert!(
        timeout(Duration::from_millis(100), &mut start)
            .await
            .is_err(),
        "public start should wait for bounded channel capacity while holding the session gate"
    );

    let begin_telepathy = telepathy.clone();
    let mut begin = tokio::spawn(async move {
        begin_telepathy
            .begin_identity_switch(key.to_bytes(), Vec::new())
            .await
    });
    assert!(
        timeout(Duration::from_millis(100), &mut begin)
            .await
            .is_err(),
        "identity begin must wait for the public session start to finish enqueueing"
    );
    let filler_received = receiver
        .recv()
        .await
        .expect("test channel should yield its filler");
    assert_eq!(filler_received, filler);
    let queued_peer = receiver
        .recv()
        .await
        .expect("public session start should enqueue before begin proceeds");
    assert_eq!(queued_peer, contact.peer_id);
    start
        .await
        .expect("public session task should not panic")
        .expect("public session start should succeed before begin");
    begin
        .await
        .expect("begin task should not panic")
        .expect("begin should succeed after public session enqueue");
    telepathy.cancel_identity_switch().await;
    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn aborted_commit_retains_recovery_for_explicit_retry() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let previous_key = SecretKey::generate();
    let target_key = SecretKey::generate();
    let target_active_gate = ManagerStateGate::new(ManagerStateKind::Active, 2);
    let network_config = restart_network_config();
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(previous_key.clone()),
        &network_config,
        &codec_config,
        Arc::new(Mutex::new(Vec::new())),
        Some(target_active_gate.clone()),
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;
    let contact = Contact::new(
        "aborted-commit-contact".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");

    telepathy
        .begin_identity_switch(target_key.to_bytes(), Vec::new())
        .await
        .expect("begin should acquire the identity switch gate");
    let telepathy = Arc::new(telepathy);
    let commit_telepathy = telepathy.clone();
    let commit = tokio::spawn(async move { commit_telepathy.commit_identity_switch().await });
    target_active_gate.wait_reached().await;
    commit.abort();
    assert!(commit.await.is_err(), "commit task should be cancelled");
    assert!(
        telepathy.try_start_session(&contact).await.is_err(),
        "aborted commit must retain the public session gate"
    );
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::IdentitySwitch,
        "aborted commit must retain the identity switch slot"
    );
    target_active_gate.release();
    telepathy
        .recover_identity_switch()
        .await
        .expect("explicit recovery should resolve an aborted commit");
    let restored_identity = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("recovery should retain an identity");
    assert_eq!(restored_identity.to_bytes(), previous_key.to_bytes());
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle
    );
    telepathy.shutdown().await;
}

/// Overlapping a regular `restart_manager()` with `commit_identity_switch()`
/// must serialize the two requests so that each is satisfied by its OWN
/// replacement iteration — never by the other's.
///
/// The previous generation-only handshake violated this: two requesters
/// that raced the `manager_generation` counter could both observe the
/// strictly-newer outcome published by the OTHER requester's replacement
/// iteration, unblock on a generation that was not their own, and release
/// the IdentitySwitch gate before their own replacement iteration had run.
///
/// The command channel replaces the shared generation outcome with
/// per-request one-shot acknowledgements. The manager loop assigns each
/// request to exactly one new iteration, and only that iteration can close
/// the channel. So a regular restart `R` and a concurrent commit `C` MUST
/// drive two distinct replacement iterations to their active milestones.
///
/// We assert the count of `ManagerState::Active` calls:
/// - 1 from the initial `start_manager` iteration.
/// - 1 from R's replacement iteration (count after R returns = 2).
/// - 1 from C's replacement iteration (count after C returns = 3).
///
/// A regression that let one request be ack'd by the other's iteration
/// would unblock both requesters at the same iteration (count = 2 when C
/// returns instead of 3) — the next iteration would run only later as a
/// wasted replacement, well outside C's await window.
#[tokio::test(flavor = "multi_thread")]
async fn overlapping_restart_manager_with_commit_drives_distinct_iterations() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let state_log: Arc<Mutex<Vec<ManagerState>>> = Arc::new(Mutex::new(Vec::new()));
    let network_config = restart_network_config();
    let third_active_gate = ManagerStateGate::new(ManagerStateKind::Active, 3);
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(key_a.clone()),
        &network_config,
        &codec_config,
        state_log.clone(),
        Some(third_active_gate.clone()),
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;
    let telepathy: Arc<MockHandle> = Arc::new(telepathy);

    assert_eq!(
        count_active(&state_log),
        1,
        "initial iteration must have reached its active milestone"
    );
    let r_telepathy = telepathy.clone();
    let r_handle = tokio::task::spawn(async move { r_telepathy.restart_manager().await });

    wait_for_manager_state(&mut manager_states, ManagerState::Stopped).await;

    let target_key_b = key_b.to_bytes();
    let c_telepathy = telepathy.clone();
    let c_handle = tokio::task::spawn(async move {
        c_telepathy
            .begin_identity_switch(target_key_b, Vec::new())
            .await
            .expect("begin should succeed on idle slot");
        c_telepathy.commit_identity_switch().await
    });
    // R unblocks the moment its OWN replacement iteration reaches the
    // active milestone. C is still waiting for ITS iteration.
    let r_outcome = r_handle.await.expect("restart_manager task panicked");
    r_outcome.expect("regular restart_manager should succeed on idle slot");
    third_active_gate.wait_reached().await;
    assert!(
        !c_handle.is_finished(),
        "C must remain pending while its own Active callback is gated"
    );
    third_active_gate.release();

    let c_outcome = c_handle.await.expect("commit task panicked");
    c_outcome.expect("commit should succeed");
    let count_after_c = count_active(&state_log);
    assert_eq!(
        count_after_c, 3,
        "C must drive its OWN replacement iteration to Active (initial + R's + \
         C's = 3); observed {count_after_c}. A count of 2 here would indicate \
         C was satisfied by R's iteration — the race the command-channel \
         handshake exists to prevent."
    );

    let identity_after = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("identity must be set after both requests complete");
    assert_eq!(
        identity_after.to_bytes(),
        key_b.to_bytes(),
        "commit must install its target identity"
    );

    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle,
        "commit must release the IdentitySwitch gate"
    );

    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_request_during_failure_backoff_owns_the_next_iteration() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let network_config = restart_network_config();
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        None,
        &network_config,
        &codec_config,
        state_log.clone(),
        None,
    )
    .await;

    wait_for_manager_state(&mut manager_states, ManagerState::Failed).await;
    *telepathy.inner.core_state.identity.write().await = Some(SecretKey::generate());

    timeout(MANAGER_EVENT_TIMEOUT, telepathy.restart_manager())
        .await
        .expect("restart request should wake failure backoff")
        .expect("assigned retry iteration should come online");
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;

    assert_eq!(
        count_active(&state_log),
        1,
        "the request received during backoff must own the first successful iteration"
    );
    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_during_unassigned_stalled_setup_assigns_the_received_request_next() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let network_config = restart_network_config();
    let good_relays = network_config
        .get_relays()
        .expect("integration network should have a relay");
    let (stalled_listener, stalled_url) = stalled_relay().await;
    configure_relays(&network_config, vec![stalled_url]);
    let stopped_gate = ManagerStateGate::new(ManagerStateKind::Stopped, 1);
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(SecretKey::generate()),
        &network_config,
        &codec_config,
        state_log,
        Some(stopped_gate.clone()),
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Starting).await;
    let (_stalled_connection, _) = timeout(MANAGER_EVENT_TIMEOUT, stalled_listener.accept())
        .await
        .expect("unassigned setup should connect to the non-responsive relay")
        .expect("stalled relay accept should succeed");

    let telepathy = Arc::new(telepathy);
    let restart_telepathy = telepathy.clone();
    let restart = tokio::spawn(async move { restart_telepathy.restart_manager().await });
    stopped_gate.wait_reached().await;
    configure_relays(&network_config, good_relays);
    stopped_gate.release();
    wait_for_manager_state(&mut manager_states, ManagerState::Stopped).await;

    timeout(MANAGER_EVENT_TIMEOUT, restart)
        .await
        .expect("restart should replace the cancelled unassigned setup")
        .expect("restart task should not panic")
        .expect("received request should own the next setup iteration");
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;
    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_cancels_unassigned_setup_parked_in_active_callback() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let network_config = restart_network_config();
    let active_gate = ManagerStateGate::new(ManagerStateKind::Active, 1);
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(SecretKey::generate()),
        &network_config,
        &codec_config,
        state_log,
        Some(active_gate.clone()),
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Starting).await;
    active_gate.wait_reached().await;

    let telepathy = Arc::new(telepathy);
    timeout(MANAGER_EVENT_TIMEOUT, telepathy.restart_manager())
        .await
        .expect("restart should cancel the parked unassigned Active callback")
        .expect("received request should own the replacement iteration");
    wait_for_manager_state(&mut manager_states, ManagerState::Stopped).await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;
    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn timed_out_stalled_setup_is_cancelled_before_the_next_restart_runs() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let network_config = restart_network_config();
    let good_relays = network_config
        .get_relays()
        .expect("integration network should have a relay");
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(SecretKey::generate()),
        &network_config,
        &codec_config,
        state_log,
        None,
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;

    let (stalled_listener, stalled_url) = stalled_relay().await;
    configure_relays(&network_config, vec![stalled_url]);
    let telepathy = Arc::new(telepathy);

    let first_telepathy = telepathy.clone();
    let first_restart = tokio::spawn(async move { first_telepathy.restart_manager().await });
    wait_for_manager_state(&mut manager_states, ManagerState::Stopped).await;
    wait_for_manager_state(&mut manager_states, ManagerState::Starting).await;
    let (_stalled_connection, _) = timeout(MANAGER_EVENT_TIMEOUT, stalled_listener.accept())
        .await
        .expect("stalled endpoint should connect to the non-responsive relay")
        .expect("stalled relay accept should succeed");

    let first_outcome = timeout(Duration::from_secs(55), first_restart)
        .await
        .expect("restart timeout should cancel its stalled iteration")
        .expect("first restart task should not panic");
    let first_error = first_outcome.expect_err("stalled restart should hit the production timeout");
    assert_eq!(
        first_error.to_string(),
        "Timed out waiting for the session manager to restart"
    );
    assert!(
        matches!(
            manager_states
                .try_recv()
                .expect("cancelled setup must publish Stopped before acknowledging timeout"),
            ManagerState::Stopped
        ),
        "cancelled setup must publish Stopped before acknowledging timeout"
    );

    configure_relays(&network_config, good_relays);
    timeout(MANAGER_EVENT_TIMEOUT, telepathy.restart_manager())
        .await
        .expect("next restart should run after timed-out setup cancellation")
        .expect("second restart should use the restored relay and succeed");
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;

    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rollback_setup_failure_requires_explicit_identity_switch_recovery() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let network_config = restart_network_config();
    let previous_key = SecretKey::generate();
    let target_key = SecretKey::generate();
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let recovery_active_gate = ManagerStateGate::new(ManagerStateKind::Active, 2);
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(previous_key.clone()),
        &network_config,
        &codec_config,
        state_log,
        Some(recovery_active_gate.clone()),
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;

    let reserved_port = UdpSocket::bind("0.0.0.0:0")
        .await
        .expect("UDP listener should reserve a collision port");
    let blocked_port = reserved_port
        .local_addr()
        .expect("UDP listener should expose its port")
        .port();
    let relays = network_config
        .get_relays()
        .expect("integration network should have a relay");
    configure_relays(&network_config, relays);
    network_config
        .update(
            blocked_port,
            vec!["0.0.0.0".to_string()],
            network_config.get_relays(),
            None,
            None,
            None,
        )
        .expect("collision configuration should be valid");

    telepathy
        .begin_identity_switch(target_key.to_bytes(), Vec::new())
        .await
        .expect("begin should acquire the identity switch gate");
    let commit_error = telepathy
        .commit_identity_switch()
        .await
        .expect_err("target and rollback setup should both fail on the reserved UDP port");
    assert!(
        matches!(commit_error, IdentitySwitchError::RollbackFailed { .. }),
        "commit must report both setup failures as a typed rollback failure"
    );
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::IdentitySwitch,
        "failed rollback must retain the identity switch gate for explicit recovery"
    );

    let contact = Contact::new(
        "recovery-guard-contact".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");
    assert!(
        telepathy.try_start_session(&contact).await.is_err(),
        "public session start must reject while recovery is required"
    );
    assert!(
        telepathy.start_call(&contact).await.is_err(),
        "public call start must reject while recovery is required"
    );
    assert!(
        telepathy.join_room(Vec::new()).await.is_err(),
        "public room start must reject while recovery is required"
    );
    assert!(
        telepathy.audio_test().await.is_err(),
        "public audio test must reject while recovery is required"
    );

    telepathy.cancel_identity_switch().await;
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::IdentitySwitch,
        "cancel must preserve the recovery-required payload and gate"
    );

    drop(reserved_port);
    let telepathy = Arc::new(telepathy);
    let recovery_telepathy = telepathy.clone();
    let recovery = tokio::spawn(async move { recovery_telepathy.recover_identity_switch().await });
    recovery_active_gate.wait_reached().await;
    assert!(
        telepathy.try_start_session(&contact).await.is_err(),
        "public session start must reject while recovery awaits manager activation"
    );
    recovery.abort();
    assert!(recovery.await.is_err(), "recovery task should be cancelled");
    assert!(
        telepathy.try_start_session(&contact).await.is_err(),
        "aborted recovery must retain the public session gate"
    );
    recovery_active_gate.release();
    telepathy
        .recover_identity_switch()
        .await
        .expect("recovery should restart the retained previous identity once the port is free");
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;
    let restored_identity = telepathy
        .inner
        .core_state
        .identity
        .read()
        .await
        .clone()
        .expect("recovery should restore an active previous identity");
    assert_eq!(
        restored_identity.to_bytes(),
        previous_key.to_bytes(),
        "recovery must reactivate the identity captured before the switch"
    );
    assert_eq!(
        telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle,
        "recovery must release the gate only after prior manager setup succeeds"
    );
    assert!(
        telepathy.try_start_session(&contact).await.is_ok(),
        "public session start should unblock after explicit recovery"
    );

    telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_during_setup_fails_assigned_and_buffered_restarts() {
    init_test_tracing();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let network_config = restart_network_config();
    let state_log = Arc::new(Mutex::new(Vec::new()));
    let (telepathy, mut manager_states) = build_handle_with_state_log(
        Some(SecretKey::generate()),
        &network_config,
        &codec_config,
        state_log,
        None,
    )
    .await;
    wait_for_manager_state(&mut manager_states, ManagerState::Active).await;

    let (stalled_listener, stalled_url) = stalled_relay().await;
    configure_relays(&network_config, vec![stalled_url]);
    let telepathy = Arc::new(telepathy);

    let assigned_telepathy = telepathy.clone();
    let assigned_restart = tokio::spawn(async move { assigned_telepathy.restart_manager().await });
    wait_for_manager_state(&mut manager_states, ManagerState::Stopped).await;
    wait_for_manager_state(&mut manager_states, ManagerState::Starting).await;
    let (_stalled_connection, _) = timeout(MANAGER_EVENT_TIMEOUT, stalled_listener.accept())
        .await
        .expect("assigned setup should connect to the non-responsive relay")
        .expect("stalled relay accept should succeed");

    let mut buffered_restart = Box::pin(telepathy.restart_manager());
    poll_fn(|context| match buffered_restart.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("buffered restart completed before shutdown"),
    })
    .await;
    let buffered_outcome = timeout(MANAGER_EVENT_TIMEOUT, async {
        let ((), outcome) = tokio::join!(telepathy.shutdown(), buffered_restart);
        outcome
    })
    .await
    .expect("shutdown should cancel endpoint setup and join the manager");

    let assigned_outcome = timeout(MANAGER_EVENT_TIMEOUT, assigned_restart)
        .await
        .expect("shutdown should resolve the assigned restart acknowledgement")
        .expect("assigned restart task should not panic");
    for outcome in [assigned_outcome, buffered_outcome] {
        let error = outcome.expect_err("shutdown must fail unfinished restart requests");
        assert!(
            error.to_string().contains("manager is shut down"),
            "unexpected shutdown restart error: {error}"
        );
    }
    wait_for_manager_state(&mut manager_states, ManagerState::Stopped).await;
}
