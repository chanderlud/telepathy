#![cfg(feature = "integration-testing")]

use iroh::address_lookup::memory::MemoryLookup;
use iroh::{PublicKey, RelayMap, SecretKey};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread;
use std::time::Duration;
use telepathy_audio::devices::AudioHost;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_core::internal::TelepathyHandle;
use telepathy_core::internal::callbacks::{MockCoreCallbacks, MockCoreStatisticsCallback};
use telepathy_core::internal::state::{CallSlotState, SessionState};
use telepathy_core::overlay::Overlay;
use telepathy_core::types::Contact;
use telepathy_core::types::{
    CallState, CodecConfig, ManagerState, NetworkConfig, ScreenshareConfig, SessionStatus,
};
use tokio::sync::Notify;
use tokio::time::{interval, sleep};
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

static TEST_TRACING_INIT: Once = Once::new();
static RELAY_INIT: Once = Once::new();
static RELAY_DETAILS: OnceLock<RelayMap> = OnceLock::new();
/// Single shared in-process address lookup. Initialised alongside the
/// relay server so every test client sees the same `BTreeMap`.
///
/// `setup_endpoint` calls `add_endpoint_info(endpoint.addr())` against
/// this lookup right after binding, so any client that dials another
/// will resolve that peer's `EndpointInfo` from the same map.
static SHARED_ADDRESS_LOOKUP: OnceLock<MemoryLookup> = OnceLock::new();

const SEQUENCED_STEP: f32 = 1.0 / 4096.0;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

type MockTelepathyHandle<H, I, O> = TelepathyHandle<
    MockCoreCallbacks<MockCoreStatisticsCallback>,
    MockCoreStatisticsCallback,
    H,
    I,
    O,
>;

struct ClientHarness<H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    telepathy: MockTelepathyHandle<H, I, O>,
    is_active: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct SequencedInput {
    counter: Arc<AtomicUsize>,
    sample_rate: u32,
}

impl SequencedInput {
    fn new(sample_rate: u32) -> Self {
        Self {
            counter: Arc::new(AtomicUsize::new(1)),
            sample_rate,
        }
    }
}

impl AudioInput for SequencedInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, telepathy_audio::Error> {
        let frame_seconds = dst.len() as f64 / self.sample_rate as f64;
        if frame_seconds.is_normal() || frame_seconds > 0.0 {
            thread::sleep(Duration::from_secs_f64(frame_seconds));
        }
        let idx = self.counter.fetch_add(1, Relaxed);
        let dc = idx as f32 * SEQUENCED_STEP;
        dst.fill(dc);
        Ok(dst.len())
    }
}

#[derive(Debug, Clone)]
struct RecordingOutput {
    log: Arc<Mutex<Vec<usize>>>,
}

impl RecordingOutput {
    fn new(log: Arc<Mutex<Vec<usize>>>) -> Self {
        Self { log }
    }
}

impl AudioOutput for RecordingOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, telepathy_audio::Error> {
        let idx = (samples[0] / SEQUENCED_STEP).round() as usize;
        self.log.lock().unwrap().push(idx);
        Ok(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomEventKind {
    Join,
    Leave,
}

#[derive(Debug, Clone, Default)]
struct PendingAcceptProbe {
    opened: Arc<AtomicUsize>,
    cancelled: Arc<AtomicUsize>,
    opened_notify: Arc<Notify>,
    cancelled_notify: Arc<Notify>,
}

/// How many manager lifecycle cycles the mock `manager_state` callback
/// should accept. The standard `SingleLifecycle` mirrors the production
/// expectation of one activation (2 `Active`/`Starting` events) followed by
/// one `Stopped` on shutdown. `RestartableLifecycle` permits any number of
/// activations and `Stopped`/`Failed` events so tests that exercise
/// `restart_manager()` (which stops the existing manager and spawns a new
/// one) do not trip mockall's strict call-count assertion.
#[derive(Debug, Clone, Copy)]
enum ManagerLifecycle {
    Single,
    Restartable,
}

impl PendingAcceptProbe {
    async fn wait_opened(&self) {
        wait_for_counter(&self.opened, &self.opened_notify, 1, "accept prompt opened").await;
    }

    async fn wait_cancelled(&self) {
        wait_for_counter(
            &self.cancelled,
            &self.cancelled_notify,
            1,
            "accept prompt cancelled",
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn session_collision_doesnt_fail() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact a invalid");

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    client_a
        .telepathy
        .inner
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.get_peer_id())
        .await
        .unwrap();

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let b_session = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&contact_b.get_peer_id())
        .cloned()
        .unwrap();
    let a_session = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&contact_a.get_peer_id())
        .cloned()
        .unwrap();

    info!("session state a: {:?}", a_session);
    info!("session state b: {:?}", b_session);

    a_session.start_call.notify_one();

    tokio::time::sleep(Duration::from_secs(5)).await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Locks in the in-process `MemoryLookup` address discovery path.
///
/// `shared_relay_map()` boots the relay server and the shared
/// `MemoryLookup` is initialised in the same `Once`. After both
/// clients bind, `setup_endpoint` registers each peer's `addr()`
/// against the shared lookup, so the dial that follows can resolve
/// the remote peer without reaching the n0 PKARR relay. The
/// assertions verify both that the registration happened (the
/// lookup contains entries for both peers) and that the resulting
/// end-to-end call reaches `Connected` — the regression scenario is
/// the in-process lookup silently failing and the dial hanging
/// until `HELLO_TIMEOUT`.
#[tokio::test(flavor = "multi_thread")]
async fn memory_address_lookup_resolves_peer_over_relay() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let lookup = shared_address_lookup();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("lookup-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("lookup-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let accept_probe_b = PendingAcceptProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client_with_accept_probe(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
        accept_probe_b.clone(),
    )
    .await;

    // `setup_endpoint` is expected to register each peer's `addr()` in the
    // shared `MemoryLookup` immediately after `bind()`. The lookup must
    // therefore hold entries for both public keys before any dial is
    // attempted. This is the assertion that locks in the new code path:
    // a regression where the registration step is skipped (e.g. by
    // re-introducing the PkarrPublisher branch) would leave these
    // lookups empty and the dial would hang on `HELLO_TIMEOUT`.
    assert!(
        lookup.get_endpoint_info(contact_a.get_peer_id()).is_some(),
        "shared MemoryLookup must contain an entry for client-a after bind"
    );
    assert!(
        lookup.get_endpoint_info(contact_b.get_peer_id()).is_some(),
        "shared MemoryLookup must contain an entry for client-b after bind"
    );

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    accept_probe_b.wait_opened().await;

    client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("bob should match the pending incoming call");

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;
    accept_probe_b.wait_cancelled().await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_no_busy_end(&states_a, "alice");
    assert_no_busy_end(&states_b, "bob");
    assert_no_call_ended_before_connected(&states_a, "alice");
    assert_no_call_ended_before_connected(&states_b, "bob");

    client_a.telepathy.end_call().await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn call_simultaneous_dial_matches_pending_incoming_and_connects() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let accept_probe_b = PendingAcceptProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client_with_accept_probe(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
        accept_probe_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    accept_probe_b.wait_opened().await;

    client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("bob should match the pending incoming call");

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;
    accept_probe_b.wait_cancelled().await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_no_busy_end(&states_a, "alice");
    assert_no_busy_end(&states_b, "bob");
    assert_no_call_ended_before_connected(&states_a, "alice");
    assert_no_call_ended_before_connected(&states_b, "bob");
    assert_eq!(accept_probe_b.opened.load(Relaxed), 1);
    assert_eq!(accept_probe_b.cancelled.load(Relaxed), 1);

    client_a.telepathy.end_call().await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Regression test for the repeated-`start_call` queueing bug.
///
/// Calling `start_call` again while the first outgoing dial to the same peer is still pending
/// must be an idempotent local start: the second call returns success, does not send another
/// `state.start_call.notify_one()`, and does not queue a stale permit that re-enters
/// `negotiate_outgoing_call` after the present call ends. Without the fix the queued permit
/// would re-fire the dial after teardown and the slot would briefly leave `Idle` for a
/// phantom second negotiation.
#[tokio::test(flavor = "multi_thread")]
async fn repeated_start_call_same_outgoing_does_not_queue_stale_permit() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // First outgoing dial. Slot moves Idle -> PendingOutgoing; the session task
    // observes the notify and starts negotiating.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("first start_call should succeed");
    // Second outgoing dial to the same peer while the first is still pending. The slot is
    // already PendingOutgoing for this peer, so the second call must be an idempotent
    // match: Ok(()), no extra notify.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("second start_call to same peer must succeed as an idempotent local start");

    // The call should connect normally — the second match must not have corrupted the
    // negotiation in any way.
    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_no_busy_end(&states_a, "alice");
    assert_no_busy_end(&states_b, "bob");
    assert_no_call_ended_before_connected(&states_a, "alice");
    assert_no_call_ended_before_connected(&states_b, "bob");

    // End the call cleanly. With the bug, the second start_call's queued notify permit
    // would re-enter negotiate_outgoing_call after the slot becomes Idle, briefly
    // re-acquiring it for a phantom second dial.
    client_a.telepathy.end_call().await;

    wait_for_slot_idle(&client_a, &contact_b.peer_id.to_string()).await;

    // Stability window: any phantom second dial would have re-acquired the slot within
    // a few hundred ms. Without the bug, the slot must remain Idle because no permit was
    // queued.
    sleep(Duration::from_secs(2)).await;

    let final_snapshot = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed after teardown");
    assert_eq!(
        final_snapshot.state,
        CallSlotState::Idle,
        "slot must remain Idle after the call ended; a stale second start_call permit would have re-acquired it for a phantom negotiation. snapshot={:?}",
        final_snapshot
    );

    // Defensive secondary check: a phantom second negotiation would have produced a
    // second end-to-end hello-ack timeout or hello failure, which manifests as an extra
    // CallEnded before any second Connected. Verify the call-state log shows the single
    // expected Connected -> ended transition, not a phantom re-dial.
    let states_a_after = call_state_snapshot(&call_states_a);
    let connected_count = states_a_after
        .iter()
        .filter(|state| matches!(state, CallState::Connected))
        .count();
    assert_eq!(
        connected_count, 1,
        "exactly one Connected event should be observed; got {connected_count} in {states_a_after:?}"
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Regression test for the terminal-teardown pending-slot leak.
///
/// `start_call` is the only production path that acquires a `PendingOutgoing` slot
/// for an outgoing dial. After it acquires the slot and notifies the session
/// task, the session enters `negotiate_outgoing_call` and matches the same slot.
/// Terminal teardown via `shutdown` (which goes through `reset_sessions` internally)
/// must clear the pending slot, even though the session's `is_session_still_current`
/// guard sees an empty map and the per-session `release_pending` would no-op.
#[tokio::test(flavor = "multi_thread")]
async fn reset_sessions_clears_pending_outgoing_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_b = contact_b.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Drive an outgoing dial through the public `start_call` API. The slot moves
    // Idle -> PendingOutgoing; the session task is notified and will match.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // Verify the slot is now PendingOutgoing before we trigger teardown. The
    // public `start_call` is the production entry point; verifying this state
    // confirms we are exercising the real acquisition path, not a bypass.
    let before = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed while pending");
    assert_eq!(
        before.state,
        CallSlotState::PendingOutgoing,
        "slot should be PendingOutgoing after start_call; got {before:?}"
    );
    assert_eq!(before.direct_peer, Some(peer_id_b));

    // Terminal teardown via the public `shutdown` API. This is the same path a
    // real user would hit, and it goes through `reset_sessions` internally. The
    // per-session `is_session_still_current` guard sees the empty post-drain map
    // and the per-session `release_pending` would no-op; the deterministic
    // `clear_pending_direct` in `reset_sessions` is what actually clears the
    // slot. The slot must end up `Idle` with no owner.
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    // Stability window: per-session teardown runs asynchronously. Wait for the
    // slot to become `Idle` and then re-check after a beat to catch any race
    // where a delayed teardown could re-pend it.
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;
    sleep(Duration::from_millis(200)).await;

    let after = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed after reset_sessions");
    assert_eq!(
        after.state,
        CallSlotState::Idle,
        "call slot must be Idle after reset_sessions clears the pending slot; got {after:?}"
    );
    assert_eq!(
        after.direct_peer, None,
        "no peer should own the slot after reset_sessions; got {after:?}"
    );
}

/// Regression test for the terminal-teardown pending-incoming-slot leak.
///
/// Mirrors `reset_sessions_clears_pending_outgoing_slot` for the `PendingIncoming`
/// state. Alice calls Bob, Bob's session task receives the `Hello` and acquires
/// `PendingIncoming` to show the accept prompt. We block the accept prompt via
/// the `PendingAcceptProbe` and then call `shutdown` on Bob before the prompt
/// resolves. The deterministic `clear_pending_direct` in `reset_sessions` must
/// clear the slot even though the per-session `is_session_still_current` guard
/// sees the empty post-drain map and the per-session `release_pending` would
/// no-op.
#[tokio::test(flavor = "multi_thread")]
async fn reset_sessions_clears_pending_incoming_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_a = contact_a.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let accept_probe_b = PendingAcceptProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client_with_accept_probe(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
        accept_probe_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Drive the call through the public `start_call` API on Alice. Bob's session
    // task receives the `Hello`, runs the new `is_session_still_current` guard,
    // acquires `PendingIncoming`, and shows the accept prompt (blocked by the
    // probe).
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    accept_probe_b.wait_opened().await;

    // Verify the slot is now `PendingIncoming` for Alice on Bob. This confirms
    // the production acquisition path (session task -> `acquire_incoming_call_slot`)
    // was exercised, not a manual bypass.
    let before = client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed while pending incoming");
    assert_eq!(
        before.state,
        CallSlotState::PendingIncoming,
        "slot should be PendingIncoming after the Hello arrived; got {before:?}"
    );
    assert_eq!(before.direct_peer, Some(peer_id_a));

    // Terminal teardown via the public `shutdown` API. The session task is
    // blocked waiting for the accept prompt; `reset_sessions` cancels the
    // session's `stop_session` token and drains `session_states`. The
    // cancellation reaches the prompt (via `cancel_prompt.notify_one()` in
    // `abort_negotiation_session_stopped`), the session task returns
    // `SessionStopped`, and the deterministic `clear_pending_direct` in
    // `reset_sessions` must leave the slot in `Idle` with no owner.
    client_b.telepathy.shutdown().await;
    client_a.telepathy.shutdown().await;

    wait_for_slot_idle(&client_b, &peer_id_a.to_string()).await;
    sleep(Duration::from_millis(200)).await;

    let after = client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed after reset_sessions");
    assert_eq!(
        after.state,
        CallSlotState::Idle,
        "call slot must be Idle after reset_sessions clears the pending incoming slot; got {after:?}"
    );
    assert_eq!(
        after.direct_peer, None,
        "no peer should own the slot after reset_sessions; got {after:?}"
    );
}

/// Regression test for the queued-session-work path through terminal teardown.
///
/// A session that has already been selected for queued work (`start_call.notify_one()`)
/// can resume inside `negotiate_outgoing_call` AFTER `reset_sessions` has performed
/// its terminal barrier. The per-session `release_pending` guard sees an empty
/// `session_states` and would no-op; the slot must still be cleared by the
/// deterministic `clear_pending_direct` in `reset_sessions`.
///
/// This test exercises that real queued-session-work path: it queues a
/// `start_call.notify_one()` on a live `SessionState` so the session task will
/// reach `negotiate_outgoing_call`, and then calls `shutdown` (which goes through
/// `reset_sessions`). The negotiation-entry cancellation guard added to
/// `negotiate_outgoing_call` must observe the cancellation and return
/// `OutgoingNegotiationOutcome::SessionStopped` without acquiring a slot. The
/// terminal `clear_pending_direct` in `reset_sessions` must then leave the slot
/// in `Idle`.
///
/// Without the guard, a drained session task that already entered
/// `negotiate_outgoing_call` could re-pend the slot after the terminal barrier,
/// and the per-session `release_pending` would no-op because the session is no
/// longer the current map entry — leaving the slot stuck in `PendingOutgoing`
/// after `shutdown` returns.
#[tokio::test(flavor = "multi_thread")]
async fn reset_sessions_drains_queued_start_call_after_terminal_force_clear() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_b = contact_b.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Queue a start_call permit on the live session for client_a. Using the
    // session's `start_call` Notify directly (rather than the public `start_call`
    // API) ensures the slot is NOT pre-acquired here — the negotiation-entry
    // cancellation guard in `negotiate_outgoing_call` is the line of defense
    // being tested, not the public-path write lock.
    let a_session = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&contact_b.get_peer_id())
        .cloned()
        .expect("client_a should have a session for contact_b");
    a_session.start_call.notify_one();

    // State-driven wait instead of a fixed sleep: poll the call slot until
    // it transitions to `PendingOutgoing`, which proves the session task has
    // entered `negotiate_outgoing_call` and called
    // `PendingDirectCallSlot::try_acquire_outgoing`. The cancellation guard
    // under test fires inside that function; we need the session task to
    // have reached it before triggering shutdown, so the test does not race
    // the slot acquisition on a slow machine.
    wait_for_slot_pending_outgoing(&client_a, &peer_id_b.to_string()).await;

    // Trigger terminal teardown on client_a. `shutdown` calls `reset_sessions`
    // which drains `session_states`, cancels each session's `stop_session` token,
    // and force-clears any pending slot. The session task in
    // `negotiate_outgoing_call` will observe the cancellation via the new guard
    // and return `SessionStopped` without acquiring a slot — closing the
    // queued-work race window.
    let core_a = client_a.telepathy.inner.clone();
    let reset_task = tokio::spawn(async move {
        core_a.shutdown().await;
    });

    // Wait for the slot to settle to `Idle`. A regression where
    // `negotiate_outgoing_call` re-pended the slot after the final force-clear
    // would leave the slot in `PendingOutgoing` here, and the wait would time
    // out.
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;

    // Stability window: any phantom queued-work acquisition that survived the
    // final force-clear would re-pend the slot within a few hundred ms. The
    // cancellation guard must have prevented that.
    sleep(Duration::from_millis(200)).await;

    let after = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed after reset_sessions");
    assert_eq!(
        after.state,
        CallSlotState::Idle,
        "call slot must be Idle after reset_sessions; a queued start_call permit that resumed negotiate_outgoing_call after the terminal force-clear would have re-pended it. snapshot={after:?}"
    );
    assert_eq!(
        after.direct_peer, None,
        "no peer should own the slot after reset_sessions; got {after:?}"
    );

    // Ensure the reset task finishes so we don't leak the manager task. We do
    // not assert on its result — `reset_sessions` returns `()`, but the test
    // already validated the observable slot state above.
    let _ = reset_task.await;
}

/// Regression test for the public `restart_manager()` flow.
///
/// `restart_manager()` does more than `reset_sessions()`: it checks the
/// slot is idle, calls `reset_sessions()`, signals the manager to
/// tear down, waits for the new manager to come online, clears the
/// peer output volume cache, and re-spawns sessions for all known
/// contacts. This test exercises the full public flow and asserts:
/// 1. The slot must end up `Idle` after restart (no stale ownership).
/// 2. A *new* session is registered for the known contact (re-spawn
///    loop, not a no-op).
/// 3. A subsequent `start_call()` succeeds and acquires a fresh
///    `PendingOutgoing` slot owned by the contact — the slot must not
///    be stuck in a pre-restart `PendingIncoming`/`PendingOutgoing`.
/// 4. The post-restart session is stable end-to-end (both sides have
///    attached) before the next start_call, so a slow `client_b`
///    teardown cannot make the dialing half observe a half-orphaned
///    session.
#[tokio::test(flavor = "multi_thread")]
async fn restart_manager_recovers_slot_respawns_sessions_and_allows_fresh_start_call() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_b = contact_b.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    // `client_a` exercises `restart_manager`, so it needs a mock that
    // permits multiple manager lifecycles. `client_b` does not, so it
    // uses the standard single-lifecycle builder.
    let client_a = build_client_with_options(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
        None,
        ManagerLifecycle::Restartable,
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    // Ensure shutdown runs on the success path even if an assertion
    // below panics — the test's `client_b` mock pins `manager_state` to
    // a single lifecycle, so an aborted run otherwise leaves an unmet
    // `Stopped` expectation that surfaces as a misleading secondary
    // panic. Reaching shutdown first keeps the diagnostic chain clean.
    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // End any in-flight dial before restart. `restart_manager` rejects
    // a restart while the slot is non-idle; the public path requires
    // `Idle`. We exercise the same production flow a user would hit
    // when a pending dial is cancelled before the restart request.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;

    // Capture the pre-restart session id; the helper asserts the
    // post-restart id differs, proving the session was re-spawned.
    let pre_restart_session_id = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_b)
        .map(|s| s.id())
        .expect("client_a should have a session for contact_b before restart");

    // Wrap the restart call in a timeout: a regression that hangs
    // waiting for the new `manager_active` notification would otherwise
    // stall the test. The public path awaits the new manager
    // notification before returning.
    tokio::time::timeout(
        Duration::from_secs(15),
        client_a.telepathy.restart_manager(),
    )
    .await
    .expect("restart_manager should not hang waiting for the new manager to come online")
    .expect("restart_manager should succeed while the slot is idle");

    let after_restart = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed after restart");
    assert_eq!(
        after_restart.state,
        CallSlotState::Idle,
        "call slot must be Idle after restart_manager; got {after_restart:?}"
    );
    assert_eq!(
        after_restart.direct_peer, None,
        "no peer should own the slot after restart_manager; got {after_restart:?}"
    );

    // Wait for the *full* post-restart session pair to stabilize, not
    // just the dialing side. `restart_manager` re-spawns sessions
    // asynchronously after the new manager activates, and `client_b`'s
    // pre-restart transport may still be tearing down — a one-sided
    // wait resolved while the remote was mid-replace.
    wait_for_stable_session_pair(
        &client_a,
        &peer_id_b,
        &client_b,
        &contact_a.get_peer_id(),
        Some(pre_restart_session_id),
    )
    .await;

    // The new session must succeed and the slot must end up owned by
    // the contact we asked to call. If the slot were stuck in a stale
    // non-idle state from a pre-restart leak, `start_call` would return
    // `CallAlreadyActive` and the test would fail at the `expect`
    // below; if ownership leaked, the `direct_peer` check would catch
    // it.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("start_call after restart_manager should succeed");

    // State-driven wait instead of a fixed sleep: the post-restart
    // session task must observe the `start_call` notify and acquire
    // the slot. Wait until the slot is owned by the right peer and in
    // a non-idle call state, then assert it remains stable.
    wait_for_slot_owned_by(&client_a, &peer_id_b).await;

    // End the call cleanly so the slot reaches `Idle` before shutdown.
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;

    // Disarm the guard before dropping it so the guard's `Drop` is a
    // no-op and only the explicit `shutdown` calls below drive the
    // shutdown. Without the disarm, `drop(shutdown_guard)` would call
    // `shutdown` on each client, and the explicit `shutdown` calls
    // would be a redundant second shutdown on each client — exactly
    // the double-shutdown the `dropped` flag exists to prevent.
    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

// The previous `stale_session_receives_hello_sends_immediate_busy_response`
// test was removed: it asserted that a stale session would reply with
// `Busy` on the wire so the dialer doesn't have to wait through
// `HELLO_TIMEOUT`. Production no longer sends `Busy` from a stale session
// (it would be a lie: a fresh replacement session may be ready to serve
// the dialer on its own connection). See `stale_session_with_fresh_replacement_*`
// and `stale_session_with_no_replacement_*` for the new behaviour.

/// Test A — stale session with a fresh replacement session in the map.
///
/// Mirrors the two-client setup of the removed test, but instead of
/// draining Bob's map (which exercises the "no fresh session" branch),
/// it inserts a fresh `SessionState` for `peer_id_a` so Bob's
/// `session_states` map holds both the original (now-stale) session and
/// a new entry with a different id. The listener transport for the
/// original session is still live, so a `Hello` from Alice arrives on
/// Bob's old connection and is handled by the stale session task.
///
/// Production contract: the stale session must NOT send `Busy` (a lie:
/// from the caller's perspective the peer is reachable via the fresh
/// session on its own connection). The stale session also must NOT
/// close its connection — the fresh session "owns/closes the relevant
/// connection" only in the sense that it serves the dialer on its own
/// connection; closing the stale connection here would be wrong because
/// the dialer's `Hello` is on the stale connection and a premature
/// close would surface as a transport error to Alice.
///
/// Note on Alice's own session: the test only mutates Bob's map to
/// fake the stale/fresh pair. Alice's live session is an
/// uncontrolled, collision-susceptible artifact (real two-sided
/// dialling against the shared relay can legitimately swap or tear
/// down her session), so this test deliberately does NOT assert on
/// Alice's session id. The asserted invariants are:
///   1. Alice does not observe an `is busy` `CallEnded`
///      (no `Busy` from the stale session), and
///   2. Bob's current map entry for Alice is the fresh id we inserted
///      (the stale session's "fresh session exists" branch did not
///      evict it).
#[tokio::test(flavor = "multi_thread")]
async fn stale_session_with_fresh_replacement_does_not_send_busy() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_a = contact_a.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    // Guard shutdown on panic so the mock callbacks' pinned `Stopped`
    // expectations are satisfied before any assertion panic.
    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Warm the transport before mutating Bob's map and dialing, so the
    // post-dial timing measures the stale-`Hello` round trip, not
    // first-packet QUIC/relay warmup.
    wait_for_active_transport(&client_a, "client_a").await;
    wait_for_active_transport(&client_b, "client_b").await;

    // Capture Bob's live session id. This session task is still running
    // on its connection; we will turn it into a "stale" session by
    // inserting a different `SessionState` in its slot.
    let stale_b_id = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_a)
        .map(|s| s.id())
        .expect("client_b should have a session for contact_a");

    // Simulate a `session_collision_kept_new` state without actually
    // dialing a new connection: replace Bob's current map entry for
    // `peer_id_a` with a fresh `SessionState` (different id) that has
    // no live task. The original session task (`stale_b_id`) is now
    // stale — its `stop_session` is NOT cancelled, so it is still
    // listening for messages on its existing connection. The fresh
    // entry exists in the map, so the production code's
    // "fresh session exists" branch should kick in and the stale
    // session should NOT close its connection.
    {
        let mut states = client_b.telepathy.inner.session_states.write().await;
        let fresh: Arc<SessionState> = Arc::new(SessionState::new_for_test());
        states.insert(peer_id_a, fresh);
    }

    // Sanity: the fresh entry id differs from the stale id.
    let fresh_id = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_a)
        .map(|s| s.id())
        .expect("client_b should have a fresh session after insert");
    assert_ne!(
        fresh_id, stale_b_id,
        "fresh entry id should differ from the captured stale id; \
         fresh={fresh_id:?}, stale={stale_b_id:?}"
    );

    // Drive an outgoing dial through the public `start_call` API. The
    // slot moves Idle -> PendingOutgoing; Alice's session task sends
    // `Hello` to Bob's existing (stale) connection. The stale session
    // task receives the `Hello`, runs the `is_session_still_current`
    // guard, sees the fresh entry has a different id, and (with the
    // fix) does NOT send `Busy` and does NOT close the connection.
    let dial_started_at = std::time::Instant::now();
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // The stale session must NOT emit a `Busy`. The wire-level outcome
    // we care about is "no `is busy` CallEnded fires", which we can
    // observe by snapshotting Alice's call-state log. The 8s budget
    // matches the old test (well below the 10s `HELLO_TIMEOUT`) and
    // gives the relay/thread contention headroom. The post-dial
    // `did not respond` outcome is expected here because Alice's
    // `Hello` went to the stale connection, which is now silent —
    // the assertion is that we do NOT observe `is busy`, NOT that
    // we observe `did not respond` within 8s (the latter would only
    // fire after `HELLO_TIMEOUT`, which is 10s).
    let busy_message = format!("{} is busy", contact_b.nickname());
    let observe_window = Duration::from_secs(8);
    let observe_deadline = tokio::time::Instant::now() + observe_window;
    while tokio::time::Instant::now() < observe_deadline {
        let states = call_state_snapshot(&call_states_a);
        assert!(
            !states.iter().any(|state| {
                matches!(state, CallState::CallEnded(reason, true) if reason == &busy_message)
            }),
            "Alice must NOT observe an 'is busy' CallEnded; the stale session must not lie. \
             elapsed since dial = {:?}; states = {:?}",
            dial_started_at.elapsed(),
            states
        );
        sleep(Duration::from_millis(100)).await;
    }

    // Capture the post-dial state BEFORE shutdown. We need to know:
    // - the fresh entry is still in Bob's map (the production code's
    //   "fresh session exists" branch did not evict it).
    // Note: we do NOT capture Alice's session id — Alice's live session
    // is subject to real collision/transport resolution and is not
    // controlled by this test (see the doc comment above).
    let current_b_id_after = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_a)
        .map(|s| s.id());
    let states_a = call_state_snapshot(&call_states_a);
    let observed_busy = states_a.iter().any(
        |state| matches!(state, CallState::CallEnded(reason, true) if reason == &busy_message),
    );

    // Disarm the shutdown guard and shut both clients down BEFORE the
    // assertion phase. See the removed test for the rationale: this
    // keeps the `ManagerLifecycle::Single` mocks' `Stopped`
    // expectations satisfied even if a downstream assertion panics.
    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    // Primary assertion: Alice did NOT observe `is busy`. The stale
    // session must not lie about the peer being busy.
    assert!(
        !observed_busy,
        "Alice must not observe an 'is busy' CallEnded; \
         the stale session with a fresh replacement must not send Busy. \
         states = {states_a:?}"
    );

    // The fresh entry should still be in Bob's map — the stale
    // session's "fresh session exists" branch does not evict it. The
    // live replacement's id is the fresh id we inserted, NOT the
    // captured stale id.
    assert_eq!(
        current_b_id_after,
        Some(fresh_id),
        "fresh entry should still be the current map entry on Bob; \
         after={current_b_id_after:?}, expected_fresh={fresh_id:?}"
    );
    assert_ne!(
        current_b_id_after,
        Some(stale_b_id),
        "stale id should not have re-asserted itself as the current map entry"
    );
}

/// Test B — stale session with no replacement session in the map.
///
/// Mirrors the removed test's setup, including the drain. Bob's current
/// map entry for `peer_id_a` is removed, leaving the stale session task
/// live on its connection with no replacement session.
///
/// Production contract: the stale session must NOT send `Busy`. With
/// no fresh session in the map, nothing else will close the stale
/// connection, so the stale session must close its own connection so
/// Alice's read returns a transport error promptly (well before the
/// 10s `HELLO_TIMEOUT`). The dialer therefore sees NO `CallEnded`
/// (the slot is `PendingOutgoing`, not `ActiveDirect`, so the
/// `session_error_while_call_active` path does not emit a
/// `CallEnded`), and crucially does NOT observe the
/// `"{nickname} did not respond to the call"` `HELLO_TIMEOUT` branch.
///
/// Note on Alice's own session: the test only mutates Bob's map (via
/// the drain). Alice's live session is an uncontrolled,
/// collision-susceptible artifact (real two-sided dialling against the
/// shared relay can legitimately swap or tear down her session), so
/// this test deliberately does NOT assert on Alice's session id. The
/// asserted invariants are:
///   1. Alice does not observe an `is busy` `CallEnded`
///      (no `Busy` from the stale session),
///   2. Alice does not observe a `did not respond` `CallEnded` within
///      8s (the stale session closed the connection well before the
///      10s `HELLO_TIMEOUT`), and
///   3. Bob's current map entry for Alice is `None` (the drain took
///      effect and the stale session's "no fresh session" branch did
///      not re-insert it).
#[tokio::test(flavor = "multi_thread")]
async fn stale_session_with_no_replacement_closes_connection_promptly() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_a = contact_a.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    wait_for_active_transport(&client_a, "client_a").await;
    wait_for_active_transport(&client_b, "client_b").await;

    // Drain Bob's current session entry. The live session task is
    // still running on its connection with no fresh session in the
    // map; `is_session_still_current` will return `false` for it, so
    // it is now "stale". The production code's "no fresh session"
    // branch should close the stale connection.
    //
    // The test deliberately does NOT call `stop_session.cancel()` on
    // Bob's session here — the map-check branch in
    // `is_session_still_current` is exercised in isolation, without
    // the concurrent `SessionStopped` path that cancellation would
    // also drive. Conflating the two would hide a regression that
    // affected only the map-check branch.
    {
        let mut states = client_b.telepathy.inner.session_states.write().await;
        states.remove(&peer_id_a);
    }

    // Drive an outgoing dial through the public `start_call` API. The
    // slot moves Idle -> PendingOutgoing; Alice's session task sends
    // `Hello` to Bob's existing (stale) connection. The stale session
    // task receives the `Hello`, runs the `is_session_still_current`
    // guard, sees no entry, and (with the fix) closes the connection
    // instead of sending `Busy`. Alice's read returns a transport
    // error, the session loop breaks, and no `CallEnded` is fired
    // (the slot is `PendingOutgoing`, not `ActiveDirect`).
    let dial_started_at = std::time::Instant::now();
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // Within the 8s window (well below the 10s `HELLO_TIMEOUT`),
    // Alice must NOT observe:
    // - `CallEnded("is busy", true)` — the stale session must not lie
    //   about the peer being busy.
    // - `CallEnded("did not respond to the call", true)` — the
    //   connection closed promptly, so the `HELLO_TIMEOUT` branch
    //   must not fire. The `HELLO_TIMEOUT` arm of
    //   `negotiate_outgoing_call` would emit this CallEnded at 10s;
    //   we poll for 8s to confirm the close happens first.
    let busy_message = format!("{} is busy", contact_b.nickname());
    let did_not_respond_message = format!("{} did not respond to the call", contact_b.nickname());
    let observe_window = Duration::from_secs(8);
    let observe_deadline = tokio::time::Instant::now() + observe_window;
    while tokio::time::Instant::now() < observe_deadline {
        let states = call_state_snapshot(&call_states_a);
        assert!(
            !states.iter().any(|state| {
                matches!(state, CallState::CallEnded(reason, true) if reason == &busy_message)
            }),
            "Alice must NOT observe an 'is busy' CallEnded; the stale session must not lie. \
             elapsed since dial = {:?}; states = {:?}",
            dial_started_at.elapsed(),
            states
        );
        assert!(
            !states.iter().any(|state| {
                matches!(state, CallState::CallEnded(reason, true) if reason == &did_not_respond_message)
            }),
            "Alice must NOT observe a 'did not respond' CallEnded within 8s; \
             the stale session with no replacement must close the connection \
             promptly (well before the 10s HELLO_TIMEOUT). \
             elapsed since dial = {:?}; states = {:?}",
            dial_started_at.elapsed(),
            states
        );
        sleep(Duration::from_millis(100)).await;
    }

    // Capture the post-drain state for the secondary assertions.
    // Note: we do NOT capture Alice's session id — Alice's live session
    // is subject to real collision/transport resolution and is not
    // controlled by this test (see the doc comment above).
    let current_b_id_after = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_a)
        .map(|s| s.id());
    let states_a = call_state_snapshot(&call_states_a);
    let observed_busy = states_a.iter().any(
        |state| matches!(state, CallState::CallEnded(reason, true) if reason == &busy_message),
    );
    let observed_did_not_respond = states_a.iter().any(|state| {
        matches!(
            state,
            CallState::CallEnded(reason, true) if reason == &did_not_respond_message
        )
    });

    // Disarm the guard and shut down before asserting (see the
    // removed test for the rationale).
    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    // Defensive secondary assertions: lock in the outcomes we already
    // guarded during the observe window. The bug being fixed is
    // exactly that the stale session either (a) lied with `Busy`, or
    // (b) said nothing at all and let the dialer fall through to the
    // `HELLO_TIMEOUT` "did not respond" branch. With the fix, neither
    // of those happens.
    assert!(
        !observed_busy,
        "Alice must not observe an 'is busy' CallEnded; \
         the stale session with no replacement must not send Busy. \
         states = {states_a:?}"
    );
    assert!(
        !observed_did_not_respond,
        "Alice must not observe a 'did not respond' CallEnded within 8s; \
         the stale session with no replacement must close the connection \
         promptly so the dialer does not fall through to the HELLO_TIMEOUT branch. \
         states = {states_a:?}"
    );

    // Confirm the drain actually took effect: the live listener
    // session that received the `Hello` is no longer the current map
    // entry for `peer_id_a` on Bob.
    assert!(
        current_b_id_after.is_none(),
        "drain should have removed Bob's session entry; after={current_b_id_after:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_frames_play_in_order() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(false, false, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let playback_log = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            SequencedInput::new(DEFAULT_SAMPLE_RATE),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            RecordingOutput::new(playback_log.clone()),
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    client_a
        .telepathy
        .inner
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.get_peer_id())
        .await
        .unwrap();

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a.telepathy.inner.core_state.set_input_volume(0.0);

    let b_session = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&contact_b.get_peer_id())
        .cloned()
        .unwrap();

    b_session.start_call.notify_one();

    tokio::time::sleep(Duration::from_secs(5)).await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    let log = playback_log.lock().unwrap();
    assert!(
        log.len() >= 30,
        "expected at least 30 playback frames, got {}",
        log.len()
    );
    assert!(
        *log.first().unwrap() <= 50,
        "expected first recovered index near stream start, got {}",
        log.first().unwrap()
    );
    for window in log.windows(2) {
        assert!(
            window[1] > window[0],
            "playback index out of order: {} followed by {}",
            window[0],
            window[1]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn room_two_peers_join_emits_remote_room_join() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a, 0, Duration::from_secs(1)).await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_room_event_sequence(&states_a, &peer_b, &[RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a, &[RoomEventKind::Join]);

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_two_peers_join_remains_stable_without_leave() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 0, Duration::from_secs(2)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a, 0, Duration::from_secs(2)).await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        0,
        "client a should not observe a room leave while the room stays stable"
    );
    assert_eq!(
        room_leave_count(&states_b, &peer_a),
        0,
        "client b should not observe a room leave while the room stays stable"
    );
    assert_room_event_sequence(&states_a, &peer_b, &[RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a, &[RoomEventKind::Join]);

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_peer_disconnect_emits_room_leave_once() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    client_b.is_active.store(false, Relaxed);
    client_b.telepathy.stop_session(&contact_a).await;

    wait_for_room_leave_count(&call_states_a, &peer_b, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 1, Duration::from_secs(1)).await;

    let states_a = call_state_snapshot(&call_states_a);
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        1,
        "peer b should leave exactly once after a disconnect"
    );
    assert_room_event_sequence(
        &states_a,
        &peer_b,
        &[RoomEventKind::Join, RoomEventKind::Leave],
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_peer_disconnect_then_rejoin_emits_leave_then_join() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    client_b.is_active.store(false, Relaxed);
    client_b.telepathy.stop_session(&contact_a).await;
    wait_for_room_leave_count(&call_states_a, &peer_b, 1).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 1, Duration::from_secs(1)).await;

    let states_a = call_state_snapshot(&call_states_a);
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        1,
        "peer b should emit one room leave before rejoining"
    );
    assert_room_event_sequence(
        &states_a,
        &peer_b,
        &[
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_multiple_quick_reconnects_do_not_emit_stale_room_leave() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    client_b.is_active.store(false, Relaxed);
    client_b.telepathy.stop_session(&contact_a).await;
    sleep(Duration::from_millis(500)).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;

    client_b.is_active.store(false, Relaxed);
    client_b.telepathy.stop_session(&contact_a).await;
    sleep(Duration::from_millis(500)).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    wait_for_room_join_count(&call_states_a, &peer_b, 3).await;

    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 2, Duration::from_secs(2)).await;

    let states_a = call_state_snapshot(&call_states_a);
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        2,
        "quick reconnects should emit one room leave per real disconnect"
    );
    assert_room_event_sequence(
        &states_a,
        &peer_b,
        &[
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_reconnect_does_not_emit_stale_room_leave() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let mut room_members = vec![
        contact_a.get_peer_id().to_string(),
        contact_b.get_peer_id().to_string(),
    ];
    room_members.sort();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    assert!(
        client_a
            .telepathy
            .join_room(room_members.clone())
            .await
            .is_ok(),
        "client a should join room"
    );
    assert!(
        client_b.telepathy.join_room(room_members).await.is_ok(),
        "client b should join room"
    );

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    // Simulate a transport drop and reconnect while the room call stays active.
    client_b.is_active.store(false, Relaxed);
    client_b.telepathy.stop_session(&contact_a).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;

    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 1, Duration::from_secs(2)).await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    let states_a = call_state_snapshot(&call_states_a);
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        1,
        "reconnect should emit one room leave for the real disconnect and no stale extra leave"
    );
    assert!(
        room_join_count(&states_a, &peer_b) >= 2,
        "peer should rejoin the room after reconnecting"
    );
    assert_room_event_sequence(
        &states_a,
        &peer_b,
        &[
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );
}

/// Happy-path regression for the room path.
///
/// Two mutual contacts both `join_room`. Asserts each side emits
/// `CallState::Connected` and exactly one `RoomJoin` for the peer, and
/// that the call slot is `RoomCall` on both clients. This is the
/// baseline test for the room-generation token: the new `RoomState` is
/// installed once, no `end_call` -> `join_room` cycle happens, and the
/// `room_owner`/`room_generation` invariants the controller enforces at
/// teardown must hold.
#[tokio::test(flavor = "multi_thread")]
async fn two_client_room_join_connects_and_reports_join() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    // Both clients move the slot into `RoomCall` and install a `RoomState`.
    wait_for_slot_room_call(&client_a, "client_a_pre_join").await;
    wait_for_slot_room_call(&client_b, "client_b_pre_join").await;

    // Capture the room-generation token captured by the controller so the
    // teardown's `release_if_match` can be checked for the right owner.
    // `RoomCall` does not carry a generation snapshot directly, but the
    // `RoomState` does; reading it here is a white-box check that
    // `join_room` bumped the counter and stored a matching value.
    let generation_a = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_a should have an installed RoomState after wait_for_slot_room_call");
    let generation_b = client_b
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_b should have an installed RoomState after wait_for_slot_room_call");
    assert!(
        generation_a > 0,
        "client_a room generation should be a positive value after join_room; got {generation_a}"
    );
    assert!(
        generation_b > 0,
        "client_b room generation should be a positive value after join_room; got {generation_b}"
    );

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a, 0, Duration::from_secs(1)).await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_eq!(
        room_join_count(&states_a, &peer_b),
        1,
        "client a should observe exactly one RoomJoin for client b; got states={states_a:?}"
    );
    assert_eq!(
        room_join_count(&states_b, &peer_a),
        1,
        "client b should observe exactly one RoomJoin for client a; got states={states_b:?}"
    );
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        0,
        "client a should not observe a RoomLeave while the room is stable; got states={states_a:?}"
    );
    assert_eq!(
        room_leave_count(&states_b, &peer_a),
        0,
        "client b should not observe a RoomLeave while the room is stable; got states={states_b:?}"
    );
    assert_room_event_sequence(&states_a, &peer_b, &[RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a, &[RoomEventKind::Join]);

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Regression test for the `end_call` -> `join_room` cycle (R1).
///
/// Joins a room on both clients, waits for `RoomJoin`, calls `end_call()`
/// on both, then re-joins. The post-rejoin must produce a *second*
/// `RoomJoin` (not be lost to a stale `room_state` carry-over) and must
/// not emit a spurious `RoomLeave` after the second `RoomJoin` — that
/// was the exact failure mode in the system-test artifact
/// `test_room_end_releases_call_slot_for_rejoin`.
#[tokio::test(flavor = "multi_thread")]
async fn room_end_releases_slot_and_allows_rejoin() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // First join: both sides acquire `RoomCall` and install a `RoomState`.
    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room (first)");
    client_b
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client b should join room (first)");
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    let first_generation_a = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_a should have RoomState after first join");
    let first_generation_b = client_b
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_b should have RoomState after first join");

    // Tear the room down via the public `end_call` API. `wait_for_slot_idle`
    // is the state-driven precondition for the rejoin: the controller's
    // teardown must have cleared the slot and the `RoomState` so a fresh
    // `join_room` can re-acquire both atomically.
    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a).await;
    wait_for_slot_idle(&client_b, &peer_b).await;
    let after_end_a = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .is_none();
    let after_end_b = client_b
        .telepathy
        .inner
        .current_room_generation()
        .await
        .is_none();
    assert!(
        after_end_a,
        "client_a room_state should be cleared after end_call; a stale controller would still be holding the entry"
    );
    assert!(
        after_end_b,
        "client_b room_state should be cleared after end_call; a stale controller would still be holding the entry"
    );

    // Re-join. The new room must have a strictly greater generation
    // (counter-bumped in `join_room`) and a fresh `RoomState` must be
    // installed. Asserting both captures the R1 fix: a stale controller's
    // late teardown must not clobber the new room.
    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should re-join room");
    client_b
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client b should re-join room");
    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 2).await;
    let second_generation_a = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_a should have RoomState after re-join");
    let second_generation_b = client_b
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_b should have RoomState after re-join");
    assert!(
        second_generation_a > first_generation_a,
        "re-join should bump the room generation; first={first_generation_a}, second={second_generation_a}"
    );
    assert!(
        second_generation_b > first_generation_b,
        "re-join should bump the room generation; first={first_generation_b}, second={second_generation_b}"
    );

    // Critical: the post-rejoin window must not produce a spurious
    // `RoomLeave` after the second `RoomJoin`. This is the exact
    // failure mode the system test artifacts reported.
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 0, Duration::from_secs(3)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a, 0, Duration::from_secs(3)).await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_eq!(
        room_leave_count(&states_a, &peer_b),
        0,
        "client a should not observe a RoomLeave for client b across the end_call -> join_room cycle; got states={states_a:?}"
    );
    assert_eq!(
        room_leave_count(&states_b, &peer_a),
        0,
        "client b should not observe a RoomLeave for client a across the end_call -> join_room cycle; got states={states_b:?}"
    );
    // The exact ordered sequence we are locking in: Join, Join (no Leave).
    // The intermediate `end_call` -> `join_room` cycle is observed only
    // as a stable slot state transition (we asserted `Idle` above), not
    // as a wire `RoomLeave` event — the controller tears the room down
    // locally without emitting a `RoomLeave` callback to the UI.
    assert_room_event_sequence(
        &states_a,
        &peer_b,
        &[RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_b,
        &peer_a,
        &[RoomEventKind::Join, RoomEventKind::Join],
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Primary regression test for the failing system-test artifact
/// `test_room_peer_leave_and_rejoin` (R2/R5).
///
/// Three clients in a room. One client `end_call()`s (leaves). The
/// remaining two observe exactly one `RoomLeave(leaver)`. The leaver
/// then `join_room`s again, and the remaining two observe a *second*
/// `RoomJoin(leaver)` and — critically — no extra `RoomLeave` after it.
/// The exact ordered sequence `[Join, Leave, Join]` for the leaver on
/// each remaining client is what locks in the fix: a stale
/// connection_id-keyed `Leave` after a rejoin would emit a
/// `RoomLeave(leaver)` after the `RoomJoin(leaver)`, producing
/// `[Join, Leave, Join, Leave]` and breaking the mesh.
///
/// **Slot-busy constraint and test shape:** the R2 race the system-test
/// artifact described is a fast in-place `end_call` -> `join_room` on
/// the *same* transport, where a stale `Leave` keyed by the previous
/// `connection_id` races the new `Join` inside the same
/// `room_controller` loop. The production guard against that race lives
/// in `room_controller`'s `RoomMessage::Leave` arm: it matches
/// `peer_connections[peer] == leave.connection_id` and only emits
/// `RoomLeave` for the still-active connection, otherwise logging
/// `room_leave_stale_connection` and dropping the stale `Leave`. We
/// cannot exercise that in-place race in this integration test because
/// a fast `end_call` -> `join_room` while the other two clients are
/// still in `RoomCall` is blocked at the public API layer: the new
/// `join_room` has to be sequenced after a transport teardown on the
/// leaver so the receiving peers free the slot for the new
/// `room_handshake`. So this test reproduces the same race shape in
/// the closest way the integration harness allows: a real
/// `stop_session`/`start_session` for the leaver's two sessions,
/// which produces a fresh `connection_id` on both sides. The new
/// `Join` on the fresh transport and the previously-emitted `Leave`
/// from the old transport are now keyed by *different* connection
/// ids on the receiving peers, which is exactly the condition the
/// `room_leave_stale_connection` branch detects. The 3-second
/// post-rejoin stability window + `room_leave_count` assertion is
/// the concrete guard: if the controller ever regressed to emitting
/// `RoomLeave` for a stale `connection_id`, the post-rejoin window
/// would see a second `RoomLeave(C)` on A and B and the assertion
/// would fail.
///
/// The disconnect/reconnect pattern is also already covered by the
/// narrower 2-client `room_peer_disconnect_then_rejoin_emits_leave_then_join`
/// and `room_reconnect_does_not_emit_stale_room_leave` tests. This
/// test's added value over those is the 3-client mesh: the leaver
/// has two peers, both of which must observe the exact
/// `[Join, Leave, Join]` ordering, and the room-generation token
/// must stay monotonic across the leave-and-rejoin cycle.
#[tokio::test(flavor = "multi_thread")]
async fn room_peer_leave_and_rejoin_reestablishes_mesh() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let key_c = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let contact_c = Contact::new("room-client-c".to_string(), key_c.public().to_string())
        .expect("contact c invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let peer_c = contact_c.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let call_states_c = Arc::new(Mutex::new(Vec::new()));

    // Sorted three-member room, matching how production callers sort the
    // member list before passing it to `join_room`.
    let mut room_members = vec![peer_a.clone(), peer_b.clone(), peer_c.clone()];
    room_members.sort();

    // `ManagerLifecycle::Single` is safe for all three clients here, even
    // though C undergoes two `stop_session`/`start_session` cycles mid-test
    // (one per contact, A and B). The mock's `manager_state` expectation
    // counts the *manager* state machine events (`Starting`/`Active` on
    // boot, `Stopped` on shutdown, `Failed` on session-manager error), not
    // per-peer session lifecycle events. `stop_session` removes a
    // `SessionState` from the `session_states` map and cancels the
    // per-session token; `start_session` sends a public key to the
    // session-manager channel to spawn a new `SessionState`. Neither
    // path calls `callbacks.manager_state(...)` — the only call sites
    // are `start_manager` (`Starting`/`Active`/`Failed`) and the
    // `session_manager` loop's teardown (`Stopped`, only on
    // `shutdown`/`restart_manager`). Each test client boots once and
    // shuts down once, so the strict `Single` mock expectation of
    // `2` (`Starting`+`Active`) + `1` (`Stopped`) holds. If a future
    // refactor makes `stop_session`/`start_session` plumb through
    // `manager_state` (e.g. a transient `Failed` on session-manager
    // error), this test would trip the strict mock and would need to
    // be switched to `ManagerLifecycle::Restartable` for all three
    // clients — but the failure mode would be a mockall
    // "called 0 time(s)" panic on `Stopped` or
    // `times(2)`/`times(0)` mismatch on `Starting`/`Active`, both of
    // which are easy to attribute.
    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone(), contact_c.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_a.clone(),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone(), contact_c.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_b.clone(),
    )
    .await;

    let client_c = build_client(
        relay_map,
        key_c,
        vec![contact_a.clone(), contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_c.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_a.telepathy.start_session(&contact_c).await;
    client_b.telepathy.start_session(&contact_a).await;
    client_b.telepathy.start_session(&contact_c).await;
    client_c.telepathy.start_session(&contact_a).await;
    client_c.telepathy.start_session(&contact_b).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_sessions(&client_a, &contact_c, &client_c, &contact_a).await;
    wait_for_sessions(&client_b, &contact_c, &client_c, &contact_b).await;

    // All three join the same room. The `join_room` API auto-accepts
    // (no accept prompt for room calls), so `build_client` is sufficient
    // and a single `ManagerLifecycle::Single` mock works.
    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client b should join room");
    client_c
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client c should join room");

    // Each client must see the other two join the mesh.
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_a, &peer_c, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_c, 1).await;
    wait_for_room_join_count(&call_states_c, &peer_a, 1).await;
    wait_for_room_join_count(&call_states_c, &peer_b, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_c, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_c, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_c, &peer_a, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_c, &peer_b, 0, Duration::from_secs(1)).await;

    // Client C leaves the room via `end_call`, then does a full
    // `stop_session`/`start_session` for both A and B before
    // `join_room` again. The `end_call` alone cannot be followed by an
    // in-place `join_room` here because A and B are still in `RoomCall`
    // for the existing room and the new `room_handshake` on either
    // side would race the still-active slot. A transport teardown
    // for the leaver's two peers is what clears the slot on the
    // listening side and lets the new `room_controller` install a
    // fresh `RoomState`. This is the same pattern as the existing
    // `room_peer_disconnect_then_rejoin_emits_leave_then_join` test;
    // see the test's docstring for why this is the closest the
    // integration harness can get to the R2 in-place `end_call`
    // ->`join_room` race.
    client_c.telepathy.end_call().await;
    wait_for_slot_idle(&client_c, &peer_c).await;
    wait_for_room_leave_count(&call_states_a, &peer_c, 1).await;
    wait_for_room_leave_count(&call_states_b, &peer_c, 1).await;
    // Stability window: no extra `RoomLeave(C)` should arrive from a
    // late `Leave` message produced by the previous transport. The
    // 1-second window is sized to absorb relay-contention jitter
    // (single in-flight `Leave` only) without masking a regression
    // where the old transport's `Leave` arrives after the new
    // transport's `Join` (the R2 failure mode).
    wait_for_no_extra_room_leave(&call_states_a, &peer_c, 1, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_c, 1, Duration::from_secs(1)).await;

    let after_leave_a = call_state_snapshot(&call_states_a);
    let after_leave_b = call_state_snapshot(&call_states_b);
    assert_eq!(
        room_leave_count(&after_leave_a, &peer_c),
        1,
        "client a should observe exactly one RoomLeave(C) after C's end_call; got states={after_leave_a:?}"
    );
    assert_eq!(
        room_leave_count(&after_leave_b, &peer_c),
        1,
        "client b should observe exactly one RoomLeave(C) after C's end_call; got states={after_leave_b:?}"
    );

    // Re-join: tear down C's sessions to A and B, re-establish them
    // (a fresh `connection_id` on both sides), then `join_room`. The
    // remaining two clients must observe a *second* `RoomJoin(C)` and
    // — critically — no extra `RoomLeave(C)` after it. The fresh
    // transport means the new `Join` is keyed by a different
    // `connection_id` than the old `Leave` from `end_call`, which is
    // the exact condition the `room_leave_stale_connection` branch in
    // `room_controller` is meant to detect (see `internal/core.rs`).
    // The 3-second post-rejoin window is the concrete guard: a
    // regression where the controller emitted `RoomLeave` for a
    // stale `connection_id` would surface as a second
    // `RoomLeave(C)` on A or B inside that window and trip the
    // assertion below.
    client_c.is_active.store(false, Relaxed);
    client_c.telepathy.stop_session(&contact_a).await;
    client_c.telepathy.stop_session(&contact_b).await;
    client_c.telepathy.start_session(&contact_a).await;
    client_c.telepathy.start_session(&contact_b).await;
    wait_for_sessions(&client_c, &contact_a, &client_a, &contact_c).await;
    wait_for_sessions(&client_c, &contact_b, &client_b, &contact_c).await;
    client_c
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client c should re-join room");
    wait_for_room_join_count(&call_states_a, &peer_c, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_c, 2).await;
    // Stability window for the rejoin. A 3-second window is the
    // integration-test analog of the post-rejoin wait used in
    // `room_peer_disconnect_then_rejoin_emits_leave_then_join`; it
    // catches a stale `Leave` from the previous transport that races
    // the new `Join` handler. The window is large enough to absorb
    // relay-contention jitter on a single in-flight stale `Leave`,
    // and small enough that a regression producing multiple
    // `RoomLeave` events after the second `Join` cannot hide inside
    // it.
    wait_for_no_extra_room_leave(&call_states_a, &peer_c, 1, Duration::from_secs(3)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_c, 1, Duration::from_secs(3)).await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_eq!(
        room_leave_count(&states_a, &peer_c),
        1,
        "client a should observe exactly one RoomLeave(C) across leave+rejoin; got states={states_a:?}"
    );
    assert_eq!(
        room_leave_count(&states_b, &peer_c),
        1,
        "client b should observe exactly one RoomLeave(C) across leave+rejoin; got states={states_b:?}"
    );
    // The exact ordered sequence we are locking in for the leaver. The
    // exact bug being guarded against would produce a fourth
    // `RoomLeave(C)` after the second `RoomJoin(C)` (or a spurious
    // interleaving), failing this assertion.
    assert_room_event_sequence(
        &states_a,
        &peer_c,
        &[
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );
    assert_room_event_sequence(
        &states_b,
        &peer_c,
        &[
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_c.telepathy.shutdown().await;
}

/// Slot-contention regression for the room path.
///
/// A single client already in a room has the call slot acquired as
/// `RoomCall`; a second `join_room` must return `Err(CallAlreadyActive)`.
/// After `end_call()` and `wait_for_slot_idle`, a fresh `join_room`
/// succeeds and re-acquires `RoomCall`. This guards the
/// `try_acquire(RoomCall)` contention check and the clean release path
/// that releases the slot on `end_call`/`shutdown` teardown.
#[tokio::test(flavor = "multi_thread")]
async fn room_duplicate_join_is_busy_then_idempotent() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    // First `join_room` succeeds and acquires `RoomCall`.
    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("first join_room should succeed");
    wait_for_slot_room_call(&client_a, "after first join").await;

    // Second `join_room` while the slot is still `RoomCall` must fail
    // with the production `CallAlreadyActive` error. We do not assert
    // on the exact `ErrorKind` here (the public error type wraps it),
    // only that the call returns `Err` — a regression where the second
    // `join_room` silently succeeded would clobber the previous room's
    // `RoomState` and break the controller.
    let second = client_a.telepathy.join_room(room_members.clone()).await;
    assert!(
        second.is_err(),
        "second join_room while the slot is RoomCall must return Err; got {second:?}"
    );

    // `end_call` releases the slot. The state-driven `wait_for_slot_idle`
    // is the precondition for the next `join_room` to succeed.
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a).await;

    // Re-join after the slot is released. The new room must install a
    // fresh `RoomState` and re-acquire `RoomCall`. The generation must
    // be strictly greater than the first room's (counter-bumped in
    // `join_room`).
    let first_generation = 1u64; // the first room's generation was 1
    client_a
        .telepathy
        .join_room(room_members)
        .await
        .expect("post-end_call join_room should succeed");
    wait_for_slot_room_call(&client_a, "after post-end_call join").await;
    let second_generation = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_a should have RoomState after post-end_call join");
    assert!(
        second_generation > first_generation,
        "post-end_call join_room should bump the room generation; first={first_generation}, second={second_generation}"
    );

    client_a.telepathy.shutdown().await;
}

fn init_test_tracing() {
    TEST_TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("telepathy_core=info")),
            )
            .try_init();
    });
}

fn shared_relay_map() -> &'static RelayMap {
    RELAY_INIT.call_once(|| {
        // Initialise the shared address lookup eagerly so every test
        // client that reads `shared_address_lookup` afterwards observes
        // a populated `MemoryLookup` instead of a fresh one per call.
        SHARED_ADDRESS_LOOKUP.get_or_init(MemoryLookup::new);
        tokio::spawn(async move {
            let server = iroh::test_utils::run_relay_server().await.unwrap();
            RELAY_DETAILS.get_or_init(|| server.0);
            // keep the relay server running forever
            sleep(Duration::from_secs(u64::MAX)).await;
        });
    });

    RELAY_DETAILS.wait()
}

/// Returns the test-binary-wide `MemoryLookup`. Initialised the first
/// time `shared_relay_map` runs (which is the first test in the binary
/// that touches networking), and reused by every subsequent call so
/// address resolution works across all clients.
fn shared_address_lookup() -> &'static MemoryLookup {
    // `shared_relay_map()` is the canonical initialiser; calling it
    // first guarantees the lookup is populated before we hand it back.
    let _ = shared_relay_map();
    SHARED_ADDRESS_LOOKUP
        .get()
        .expect("shared_address_lookup called before shared_relay_map initialisation")
}

async fn build_client<H, I, O>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
) -> ClientHarness<H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    build_client_with_options(
        relay_map,
        identity,
        contacts,
        codec_config,
        host,
        call_states,
        None,
        ManagerLifecycle::Single,
    )
    .await
}

async fn build_client_with_accept_probe<H, I, O>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: PendingAcceptProbe,
) -> ClientHarness<H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    build_client_with_options(
        relay_map,
        identity,
        contacts,
        codec_config,
        host,
        call_states,
        Some(accept_probe),
        ManagerLifecycle::Single,
    )
    .await
}

async fn build_client_with_options<H, I, O>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
) -> ClientHarness<H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
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

    let is_active = Arc::new(AtomicBool::new(false));
    let is_relayed = Arc::new(AtomicBool::new(false));
    let mock = construct_mock_callbacks(
        contacts,
        is_active.clone(),
        is_relayed.clone(),
        call_states,
        accept_probe,
        lifecycle,
    );

    let mut telepathy: MockTelepathyHandle<H, I, O> = TelepathyHandle::new(
        host,
        &network_config,
        &screenshare,
        &overlay,
        codec_config,
        mock,
    );
    *telepathy.inner.core_state.identity.write().await = Some(identity);
    telepathy.start_manager().await;
    telepathy.inner.core_state.manager_active.notified().await;

    ClientHarness {
        telepathy: TelepathyHandle::from(telepathy),
        is_active,
    }
}

/// returns mock callbacks that will establish a telepathy instance with the provided contacts
/// sets is_active to true when the first session connected event is received
///
/// `lifecycle` controls how many `manager_state` activations the mock will
/// accept: `Single` pins to a single activation (2 `Active`/`Starting` and
/// 1 `Stopped`); `Restartable` accepts any number of activations, stops,
/// and `Failed` events so tests that exercise `restart_manager()` do not
/// trip mockall's strict call-count assertion.
fn construct_mock_callbacks(
    contacts: Vec<Contact>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
) -> MockCoreCallbacks<MockCoreStatisticsCallback> {
    let mut mock: MockCoreCallbacks<MockCoreStatisticsCallback> = MockCoreCallbacks::new();

    // handle session status callbacks
    mock.expect_session_status().returning(move |status, peer| {
        info!("session status got called {status:?} {peer}");
        let is_active_clone = is_active.clone();
        let is_relayed_clone = is_relayed.clone();
        Box::pin(async move {
            if let SessionStatus::Connected { relayed, .. } = status {
                is_active_clone.store(true, Relaxed);
                is_relayed_clone.store(relayed, Relaxed);
            }
        })
    });

    match lifecycle {
        ManagerLifecycle::Single => {
            // ensure manager activates (one `Starting` + one `Active`)
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Active | ManagerState::Starting))
                .times(2)
                .returning(|_| Box::pin(async move {}));

            // ensure manager deactivates
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Stopped))
                .once()
                .returning(|_| Box::pin(async move {}));
        }
        ManagerLifecycle::Restartable => {
            // Each restart cycle emits one `Starting` and one `Active`; the
            // outer `start_manager` loop can call this any number of times.
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Active | ManagerState::Starting))
                .times(..)
                .returning(|_| Box::pin(async move {}));

            // One `Stopped` per manager teardown (one per cycle plus the
            // final shutdown).
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Stopped))
                .times(..)
                .returning(|_| Box::pin(async move {}));

            // The outer loop in `start_manager` emits `ManagerState::Failed`
            // before retrying if `setup_endpoint` or the main loop errors.
            // Accepting any number keeps a transient setup failure (e.g.
            // relay hiccup) from surfacing as a mockall "no matching
            // expectation" panic that would mask the real cause.
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Failed))
                .times(..)
                .returning(|_| Box::pin(async move {}));
        }
    }

    // return the contacts
    let contacts_clone = contacts.clone();
    mock.expect_get_contacts().returning(move || {
        let contacts_clone = contacts_clone.clone();
        Box::pin(async move { contacts_clone })
    });

    mock.expect_get_contact().returning(move |peer_id| {
        let contacts_clone = contacts.clone();
        Box::pin(async move {
            for contact in contacts_clone.iter() {
                if contact.get_peer_id().to_vec() == peer_id {
                    return Some(contact.clone());
                }
            }

            None
        })
    });

    if let Some(probe) = accept_probe {
        mock.expect_get_accept_handle()
            .returning(move |_, _, cancel| {
                info!("accept call called with pending probe");
                let probe = probe.clone();
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    probe.opened.fetch_add(1, Relaxed);
                    probe.opened_notify.notify_waiters();
                    cancel.notified().await;
                    probe.cancelled.fetch_add(1, Relaxed);
                    probe.cancelled_notify.notify_waiters();
                    false
                })
            });
    } else {
        mock.expect_get_accept_handle().returning(move |_, _, _| {
            info!("accept call called");
            tokio::spawn(async move { true })
        });
    }

    mock.expect_call_state().returning(move |state| {
        info!("got call state: {state:?}");
        call_states.lock().unwrap().push(state);
        Box::pin(async move {})
    });

    mock.expect_statistics_callback().returning(|| {
        let mut mock = MockCoreStatisticsCallback::new();

        mock.expect_post()
            .returning(move |_| Box::pin(async move {}));

        mock
    });

    mock
}

fn room_join_count(states: &[CallState], peer: &str) -> usize {
    states
        .iter()
        .filter(|state| matches!(state, CallState::RoomJoin(id) if id == peer))
        .count()
}

fn room_leave_count(states: &[CallState], peer: &str) -> usize {
    states
        .iter()
        .filter(|state| matches!(state, CallState::RoomLeave(id) if id == peer))
        .count()
}

async fn wait_for_room_join_count(
    call_states: &Arc<Mutex<Vec<CallState>>>,
    peer: &str,
    expected: usize,
) {
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        poll.tick().await;
        let count = room_join_count(&call_states.lock().unwrap(), peer);
        if count >= expected {
            info!("observed {count} RoomJoin events for {peer}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {expected} RoomJoin events for {peer}, got {count}"
        );
    }
}

fn sorted_room_members(a: &Contact, b: &Contact) -> Vec<String> {
    let mut members = vec![a.get_peer_id().to_string(), b.get_peer_id().to_string()];
    members.sort();
    members
}

fn call_state_snapshot(call_states: &Arc<Mutex<Vec<CallState>>>) -> Vec<CallState> {
    call_states.lock().unwrap().clone()
}

async fn wait_for_counter(counter: &AtomicUsize, notify: &Notify, expected: usize, label: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if counter.load(Relaxed) >= expected {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {label} count to reach {expected}, got {}",
            counter.load(Relaxed)
        );
        tokio::select! {
            _ = notify.notified() => {}
            _ = sleep(Duration::from_millis(100)) => {}
        }
    }
}

async fn wait_for_connected(call_states: &Arc<Mutex<Vec<CallState>>>, label: &str) {
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        poll.tick().await;
        let states = call_state_snapshot(call_states);
        if states
            .iter()
            .any(|state| matches!(state, CallState::Connected))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {label} call state to connect; states were {states:?}"
        );
    }
}

/// Wait until the underlying transport is actually live on the given
/// client. `ClientHarness::is_active` is flipped to `true` on the first
/// `SessionStatus::Connected` callback (see `construct_mock_callbacks`),
/// so this confirms the QUIC/relay path is warm and not still doing
/// first-packet setup. The 60s budget mirrors `wait_for_connected` so a
/// transport that never comes up fails loudly instead of producing a
/// misleading downstream timing flake.
async fn wait_for_active_transport<H, I, O>(client: &ClientHarness<H, I, O>, label: &str)
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        poll.tick().await;
        if client.is_active.load(Relaxed) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {label} transport to become active; \
             is_active stayed false for 60s"
        );
    }
}

fn assert_no_busy_end(states: &[CallState], label: &str) {
    assert!(
        !states.iter().any(|state| matches!(
            state,
            CallState::CallEnded(reason, true) if reason == "A call is already active"
        )),
        "{label} observed busy call end: {states:?}"
    );
}

fn assert_no_call_ended_before_connected(states: &[CallState], label: &str) {
    let connected_index = states
        .iter()
        .position(|state| matches!(state, CallState::Connected))
        .unwrap_or_else(|| panic!("{label} never connected: {states:?}"));
    assert!(
        !states[..connected_index]
            .iter()
            .any(|state| matches!(state, CallState::CallEnded(_, _))),
        "{label} observed CallEnded before Connected: {states:?}"
    );
}

fn room_event_sequence(states: &[CallState], peer: &str) -> Vec<RoomEventKind> {
    states
        .iter()
        .filter_map(|state| match state {
            CallState::RoomJoin(id) if id == peer => Some(RoomEventKind::Join),
            CallState::RoomLeave(id) if id == peer => Some(RoomEventKind::Leave),
            _ => None,
        })
        .collect()
}

fn assert_room_event_sequence(
    states: &[CallState],
    peer: &str,
    expected: impl AsRef<[RoomEventKind]>,
) {
    let actual = room_event_sequence(states, peer);
    let expected = expected.as_ref();
    assert_eq!(
        actual.as_slice(),
        expected,
        "expected room events for {peer} to be {expected:?}, got {actual:?}"
    );
}

async fn wait_for_room_leave_count(
    call_states: &Arc<Mutex<Vec<CallState>>>,
    peer: &str,
    expected: usize,
) {
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        poll.tick().await;
        let count = room_leave_count(&call_state_snapshot(call_states), peer);
        if count >= expected {
            info!("observed {count} RoomLeave events for {peer}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {expected} RoomLeave events for {peer}, got {count}"
        );
    }
}

async fn wait_for_no_extra_room_leave(
    call_states: &Arc<Mutex<Vec<CallState>>>,
    peer: &str,
    expected: usize,
    stability_window: Duration,
) {
    wait_for_room_leave_count(call_states, peer, expected).await;
    let before = room_leave_count(&call_state_snapshot(call_states), peer);
    sleep(stability_window).await;
    let after = room_leave_count(&call_state_snapshot(call_states), peer);
    assert_eq!(
        after, before,
        "expected no extra RoomLeave events for {peer} during {:?}, got {} before and {} after",
        stability_window, before, after
    );
}

async fn wait_for_sessions<HA, IA, OA, HB, IB, OB>(
    a: &ClientHarness<HA, IA, OA>,
    a_peer: &Contact,
    b: &ClientHarness<HB, IB, OB>,
    b_peer: &Contact,
) where
    HA: AudioHost<InputStream = IA, OutputStream = OA> + Send + Sync + Clone + 'static,
    IA: Send + Sync + 'static,
    OA: Send + Sync + 'static,
    HB: AudioHost<InputStream = IB, OutputStream = OB> + Send + Sync + Clone + 'static,
    IB: Send + Sync + 'static,
    OB: Send + Sync + 'static,
{
    // Two-phase wait: first confirm both sides have a session entry, then re-check after
    // a poll interval that the SessionState::id is unchanged. This guards against
    // returning during a session-collision replacement where one entry has been swapped
    // but the new owner has not yet stabilized — callers that act on the session
    // immediately after `wait_for_sessions` would otherwise race the replacement.
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut prev_a_id = None;
    let mut prev_b_id = None;
    let mut both_present = false;
    loop {
        poll.tick().await;

        let a_id = a
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .get(&a_peer.get_peer_id())
            .map(|s| s.id());
        let b_id = b
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .get(&b_peer.get_peer_id())
            .map(|s| s.id());

        if !both_present && a_id.is_some() && b_id.is_some() {
            both_present = true;
            prev_a_id = a_id;
            prev_b_id = b_id;
            continue;
        }

        if both_present && a_id == prev_a_id && b_id == prev_b_id {
            info!("both clients have stable session state");
            break;
        }

        if a_id != prev_a_id || b_id != prev_b_id {
            // session entry swapped (collision replacement); restart the stability window
            both_present = a_id.is_some() && b_id.is_some();
            prev_a_id = a_id;
            prev_b_id = b_id;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for both clients to stabilize sessions; a_id={a_id:?}, b_id={b_id:?}"
        );
    }
}

/// Waits until both clients have a `SessionState` registered for the
/// indicated peer AND the session ids remain stable across at least one
/// polling interval. Optionally asserts that the resulting session id
/// differs from a previous id (e.g. to confirm `restart_manager` actually
/// re-spawned the session rather than leaving it in place).
///
/// This supersedes the single-sided `wait_for_session` helper: the
/// restart flow re-spawns sessions asynchronously after manager
/// activation, while the remote side may still be cleaning up its
/// pre-restart transport. A one-sided wait on the dialing client
/// resolved before the remote had a chance to attach, so callers that
/// act on both halves (e.g. asserting a post-restart slot acquisition
/// will succeed end-to-end) would race.
async fn wait_for_stable_session_pair<HA, IA, OA, HB, IB, OB>(
    a: &ClientHarness<HA, IA, OA>,
    a_peer: &PublicKey,
    b: &ClientHarness<HB, IB, OB>,
    b_peer: &PublicKey,
    require_a_id_change: Option<Uuid>,
) where
    HA: AudioHost<InputStream = IA, OutputStream = OA> + Send + Sync + Clone + 'static,
    IA: Send + Sync + 'static,
    OA: Send + Sync + 'static,
    HB: AudioHost<InputStream = IB, OutputStream = OB> + Send + Sync + Clone + 'static,
    IB: Send + Sync + 'static,
    OB: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut prev_a_id: Option<Uuid> = None;
    let mut prev_b_id: Option<Uuid> = None;
    let mut both_present = false;
    loop {
        poll.tick().await;

        let a_id = a
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .get(a_peer)
            .map(|s| s.id());
        let b_id = b
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .get(b_peer)
            .map(|s| s.id());

        if !both_present && a_id.is_some() && b_id.is_some() {
            both_present = true;
            prev_a_id = a_id;
            prev_b_id = b_id;
            continue;
        }

        if both_present && a_id == prev_a_id && b_id == prev_b_id {
            if let Some(prev) = require_a_id_change {
                assert_ne!(
                    a_id,
                    Some(prev),
                    "client_a session id was not replaced across the restart; \
                     expected a new id distinct from {prev:?}, got {a_id:?}"
                );
            }
            info!("both clients have stable post-restart session state");
            return;
        }

        if a_id != prev_a_id || b_id != prev_b_id {
            // session entry swapped (collision replacement or restart);
            // restart the stability window
            both_present = a_id.is_some() && b_id.is_some();
            prev_a_id = a_id;
            prev_b_id = b_id;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for stable post-restart session pair; a_id={a_id:?}, b_id={b_id:?}"
        );
    }
}

/// Waits until the call slot transitions to `PendingOutgoing` for `peer`.
///
/// This is the state-driven replacement for a fixed settle sleep: the
/// session task observes its queued `start_call` notify, enters
/// `negotiate_outgoing_call`, and acquires the outgoing slot via
/// `PendingDirectCallSlot::try_acquire_outgoing` — that acquisition is
/// exactly the `PendingOutgoing` transition. Tests that need the
/// session task to have reached this acquisition point can poll until
/// the slot state is `PendingOutgoing` rather than sleeping for an
/// arbitrary duration that races machine speed.
async fn wait_for_slot_pending_outgoing<H, I, O>(client: &ClientHarness<H, I, O>, peer: &str)
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(20));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        poll.tick().await;
        let snapshot = client
            .telepathy
            .inner
            .core_state
            .call_slot
            .snapshot()
            .expect("call slot snapshot should succeed");
        if snapshot.state == CallSlotState::PendingOutgoing {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for call slot to reach PendingOutgoing for peer {peer}; \
             the session task should have entered negotiate_outgoing_call and called \
             PendingDirectCallSlot::try_acquire_outgoing. last snapshot={snapshot:?}"
        );
    }
}

async fn wait_for_slot_idle<H, I, O>(client: &ClientHarness<H, I, O>, peer: &str)
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(50));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        poll.tick().await;
        let snapshot = client
            .telepathy
            .inner
            .core_state
            .call_slot
            .snapshot()
            .expect("call slot snapshot should succeed");
        if snapshot.state == CallSlotState::Idle {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for call slot to become Idle for peer {peer}; last snapshot={snapshot:?}"
        );
    }
}

/// Waits until the call slot is in `RoomCall` state, indicating that
/// `join_room` has installed a `RoomState` and acquired the slot for the
/// room. Mirrors `wait_for_slot_idle` and `wait_for_slot_pending_outgoing`
/// for the room-specific slot state.
async fn wait_for_slot_room_call<H, I, O>(client: &ClientHarness<H, I, O>, label: &str)
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(50));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        poll.tick().await;
        let snapshot = client
            .telepathy
            .inner
            .core_state
            .call_slot
            .snapshot()
            .expect("call slot snapshot should succeed");
        if snapshot.state == CallSlotState::RoomCall {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for call slot to reach RoomCall for {label}; last snapshot={snapshot:?}"
        );
    }
}

/// Waits until the call slot is owned by `peer` and in a non-idle
/// pending or active call state. This replaces the old fixed-sleep
/// stability window after a fresh `start_call`: the post-restart session
/// task observes the `start_call` notify and acquires the slot
/// asynchronously, so the assertion is state-driven rather than
/// time-driven. Once observed, the slot is re-checked across one more
/// poll interval to confirm it does not flip to `Idle` (which would
/// indicate a phantom second negotiation that immediately ended or a
/// stale-state leak clobbering the acquisition).
async fn wait_for_slot_owned_by<H, I, O>(client: &ClientHarness<H, I, O>, peer: &PublicKey)
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(50));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut observed: Option<CallSlotState> = None;
    loop {
        poll.tick().await;
        let snapshot = client
            .telepathy
            .inner
            .core_state
            .call_slot
            .snapshot()
            .expect("call slot snapshot should succeed");
        if snapshot.direct_peer == Some(*peer)
            && matches!(
                snapshot.state,
                CallSlotState::PendingOutgoing | CallSlotState::ActiveDirect
            )
        {
            if observed == Some(snapshot.state) {
                return;
            }
            observed = Some(snapshot.state);
            continue;
        }
        if observed.is_some() {
            assert_ne!(
                snapshot.state,
                CallSlotState::Idle,
                "slot flipped to Idle after a successful start_call; \
                 a stale pre-restart state leaking through would manifest as \
                 either a flip to Idle or a different owning peer. \
                 snapshot={snapshot:?}"
            );
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for call slot to be owned by {peer} in a \
             non-idle state; last snapshot={snapshot:?}"
        );
    }
}

/// Test-harness cleanup guard for two-client tests. On drop it
/// schedules shutdowns for both clients so an aborted test reaches
/// the same shutdown path as a successful one. This prevents
/// `client_b`'s mock from being left with an unmet `Stopped`
/// expectation that would surface as a misleading secondary panic
/// after the real assertion failure has already been reported.
///
/// The test is declared `flavor = "multi_thread"`, so `Drop` runs on
/// a worker thread that owns a tokio runtime handle. We use
/// `block_in_place` + `block_on` to drive the async shutdowns
/// synchronously without needing to clone the `ClientHarness`.
struct TwoClientShutdownGuard<
    'a,
    HA: AudioHost<InputStream = IA, OutputStream = OA> + Send + Sync + Clone + 'static,
    IA: Send + Sync + 'static,
    OA: Send + Sync + 'static,
    HB: AudioHost<InputStream = IB, OutputStream = OB> + Send + Sync + Clone + 'static,
    IB: Send + Sync + 'static,
    OB: Send + Sync + 'static,
> {
    a: &'a ClientHarness<HA, IA, OA>,
    b: &'a ClientHarness<HB, IB, OB>,
    dropped: AtomicBool,
}

impl<HA, IA, OA, HB, IB, OB> TwoClientShutdownGuard<'_, HA, IA, OA, HB, IB, OB>
where
    HA: AudioHost<InputStream = IA, OutputStream = OA> + Send + Sync + Clone + 'static,
    IA: Send + Sync + 'static,
    OA: Send + Sync + 'static,
    HB: AudioHost<InputStream = IB, OutputStream = OB> + Send + Sync + Clone + 'static,
    IB: Send + Sync + 'static,
    OB: Send + Sync + 'static,
{
    /// Marks the guard as already-handled so its `Drop` becomes a
    /// no-op. The success path calls this immediately before
    /// `drop(shutdown_guard)` so the explicit `shutdown` calls that
    /// follow are the only shutdowns that run; otherwise `Drop` would
    /// fire a redundant `shutdown` on each client after the explicit
    /// calls and we would hit the double-shutdown path.
    fn disarm(&self) {
        self.dropped.store(true, Relaxed);
    }
}

impl<HA, IA, OA, HB, IB, OB> Drop for TwoClientShutdownGuard<'_, HA, IA, OA, HB, IB, OB>
where
    HA: AudioHost<InputStream = IA, OutputStream = OA> + Send + Sync + Clone + 'static,
    IA: Send + Sync + 'static,
    OA: Send + Sync + 'static,
    HB: AudioHost<InputStream = IB, OutputStream = OB> + Send + Sync + Clone + 'static,
    IB: Send + Sync + 'static,
    OB: Send + Sync + 'static,
{
    fn drop(&mut self) {
        // The success path explicitly drops the guard before the
        // explicit `shutdown` calls, which sets `dropped` to `true` so
        // this `Drop` becomes a no-op on the success path. When a test
        // panics before that explicit drop, the guard's `Drop` runs
        // and best-effort shuts down both clients to avoid leaving
        // `client_b`'s mock with an unmet `Stopped` expectation that
        // would surface as a misleading secondary panic after the real
        // assertion failure. We use `block_in_place` so the async
        // shutdowns run synchronously on the multi-threaded test
        // runtime; the test is declared `flavor = "multi_thread"` so
        // this is available.
        if self.dropped.swap(true, Relaxed) {
            return;
        }
        let a = self.a;
        let b = self.b;
        let shutdown_both = || async move {
            a.telepathy.shutdown().await;
            b.telepathy.shutdown().await;
        };
        // `Handle::current()` panics with a descriptive message if no
        // runtime is present, which is the desired failure mode here: a
        // silently-no-op `drop` would leave `client_b`'s mock with an
        // unmet `Stopped` expectation and surface as a misleading
        // secondary panic after the real assertion failure.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(shutdown_both());
        });
    }
}
