#![cfg(feature = "integration-testing")]

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use iroh::address_lookup::memory::MemoryLookup;
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, PublicKey, RelayMap, RelayMode, SecretKey};
use speedy::{Readable, Writable};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread;
use std::time::Duration;
use telepathy_audio::devices::AudioHost;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_audio::io::StreamErrorCallback;
use telepathy_audio::{CpalError, CpalErrorKind};
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
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tracing::info;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

static TEST_TRACING_INIT: Once = Once::new();
static RELAY_INIT: Once = Once::new();
static RELAY_DETAILS: OnceLock<RelayMap> = OnceLock::new();
/// Single shared in-process address lookup. `setup_endpoint` registers each
/// peer's `addr()` here right after bind, so every test client resolves the
/// others from the same map.
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

#[derive(Debug, Readable, Writable)]
enum WireProtocolMessage {
    Hello {
        ringtone: Option<Vec<u8>>,
        audio_header: WireAudioHeader,
        room_hash: Option<u64>,
    },
    HelloAck {
        audio_header: WireAudioHeader,
    },
    Reject,
    Busy,
    Goodbye {
        reason: WireGoodbyeReason,
    },
    Chat {
        text: String,
        attachments: Vec<WireAttachment>,
    },
    KeepAlive,
    ScreenshareHeader {
        encoder_name: String,
    },
}

#[derive(Debug, Readable, Writable)]
struct WireAudioHeader {
    sample_rate: u32,
    codec_enabled: bool,
    vbr: bool,
    residual_bits: f64,
}

#[derive(Debug, Readable, Writable)]
enum WireGoodbyeReason {
    SessionStopped,
    AudioDeviceError,
    Error,
    None,
}

#[derive(Debug, Readable, Writable)]
struct WireAttachment {
    name: String,
    data: Vec<u8>,
}

struct RawRoomPeer {
    endpoint: Endpoint,
    connection: Connection,
    control_send: FramedWrite<SendStream, LengthDelimitedCodec>,
    control_recv: FramedRead<RecvStream, LengthDelimitedCodec>,
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

#[derive(Clone)]
struct StreamErrorProbe {
    callback: Arc<Mutex<Option<StreamErrorCallback>>>,
    ready: Arc<Notify>,
}

impl StreamErrorProbe {
    fn new() -> Self {
        Self {
            callback: Arc::new(Mutex::new(None)),
            ready: Arc::new(Notify::new()),
        }
    }

    fn capture(&self, callback: Option<StreamErrorCallback>) {
        *self.callback.lock().unwrap() = callback;
        self.ready.notify_one();
    }

    async fn wait_captured(&self) {
        self.ready.notified().await;
    }

    fn signal_setup_attempt(&self) {
        self.ready.notify_one();
    }

    async fn wait_setup_attempted(&self) {
        self.ready.notified().await;
    }

    fn trigger(&self, error: CpalError) {
        let mut callback = self
            .callback
            .lock()
            .unwrap()
            .take()
            .expect("stream error callback should be captured before triggering");
        callback(error);
    }
}

#[derive(Clone)]
struct CallbackCapturingAudioHost {
    input_error_probe: StreamErrorProbe,
    output_error_probe: StreamErrorProbe,
    /// When set, `open_output` returns a synchronous `DeviceError`
    /// (simulating an exclusively-held output device) without capturing
    /// the stream error callback.
    fail_output_synchronously: Arc<AtomicBool>,
    fail_input_synchronously: Arc<AtomicBool>,
}

impl CallbackCapturingAudioHost {
    fn new(input_error_probe: StreamErrorProbe, output_error_probe: StreamErrorProbe) -> Self {
        Self {
            input_error_probe,
            output_error_probe,
            fail_output_synchronously: Arc::new(AtomicBool::new(false)),
            fail_input_synchronously: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AudioHost for CallbackCapturingAudioHost {
    type InputStream = ();
    type OutputStream = ();

    fn list_input_devices(
        &self,
    ) -> Result<Vec<telepathy_audio::devices::AudioDeviceInfo>, telepathy_audio::devices::DeviceError>
    {
        Ok(vec![telepathy_audio::devices::AudioDeviceInfo {
            name: "Mock Input".to_string(),
            id: "mock".to_string(),
        }])
    }

    fn list_output_devices(
        &self,
    ) -> Result<Vec<telepathy_audio::devices::AudioDeviceInfo>, telepathy_audio::devices::DeviceError>
    {
        Ok(vec![telepathy_audio::devices::AudioDeviceInfo {
            name: "Mock Output".to_string(),
            id: "mock".to_string(),
        }])
    }

    fn list_all_devices(
        &self,
    ) -> Result<telepathy_audio::devices::AudioDeviceList, telepathy_audio::devices::DeviceError>
    {
        Ok(telepathy_audio::devices::AudioDeviceList {
            input_devices: self.list_input_devices()?,
            output_devices: self.list_output_devices()?,
        })
    }

    fn input_sample_rate(
        &self,
        _: Option<&str>,
    ) -> Result<u32, telepathy_audio::devices::DeviceError> {
        Ok(DEFAULT_SAMPLE_RATE)
    }

    fn output_sample_rate(
        &self,
        _: Option<&str>,
    ) -> Result<u32, telepathy_audio::devices::DeviceError> {
        Ok(DEFAULT_SAMPLE_RATE)
    }

    #[cfg(not(target_family = "wasm"))]
    fn open_input(
        &self,
        _: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<
        (impl AudioInput + Send + 'static, u32, Self::InputStream),
        telepathy_audio::devices::DeviceError,
    > {
        if self.fail_input_synchronously.load(Relaxed) {
            self.input_error_probe.signal_setup_attempt();
            return Err(telepathy_audio::devices::DeviceError::NoOutputDevice);
        }
        self.input_error_probe.capture(error_callback);
        Ok((MockAudioInput::default(), DEFAULT_SAMPLE_RATE, ()))
    }

    fn open_output(
        &self,
        _: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<
        (impl AudioOutput + Send + 'static, u32, Self::OutputStream),
        telepathy_audio::devices::DeviceError,
    > {
        if self.fail_output_synchronously.load(Relaxed) {
            self.output_error_probe.signal_setup_attempt();
            return Err(telepathy_audio::devices::DeviceError::NoOutputDevice);
        }
        self.output_error_probe.capture(error_callback);
        Ok((MockAudioOutput, DEFAULT_SAMPLE_RATE, ()))
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

/// How many manager lifecycle cycles the mock `manager_state` callback accepts.
/// `Single` pins to one activation (2 active/starting + 1 stopped);
/// `Restartable` accepts any number so `restart_manager()` tests don't trip
/// mockall's strict call-count assertion.
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

/// In-process `MemoryLookup` registers each peer's `addr()` after bind so the
/// dial resolves without reaching the n0 PKARR relay. Regression: lookup
/// silently fails and dial hangs until `HELLO_TIMEOUT`.
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

/// Regression: a second `start_call` while a first outgoing dial is still pending must
/// be idempotent — no extra notify, no queued permit that re-enters
/// `negotiate_outgoing_call` after teardown, no phantom `Idle` flip.
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

    // First outgoing dial moves the slot to PendingOutgoing.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("first start_call should succeed");
    // Second outgoing dial: must be an idempotent match — Ok(()), no extra notify.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("second start_call to same peer must succeed as an idempotent local start");

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_no_busy_end(&states_a, "alice");
    assert_no_busy_end(&states_b, "bob");
    assert_no_call_ended_before_connected(&states_a, "alice");
    assert_no_call_ended_before_connected(&states_b, "bob");

    // End the call cleanly. With the bug, the second start_call's queued permit would
    // re-enter negotiate_outgoing_call after the slot becomes Idle.
    client_a.telepathy.end_call().await;

    wait_for_slot_idle(&client_a, &contact_b.peer_id.to_string()).await;

    // Stability window: a phantom second dial would re-acquire the slot within a few
    // hundred ms. Without the bug, the slot must remain Idle because no permit was queued.
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

/// Terminal teardown via `shutdown` -> `reset_sessions` must clear a pending
/// `PendingOutgoing` slot. Per-session `release_pending` no-ops on the empty
/// post-drain map; deterministic `clear_pending_direct` is the line of defense.
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

    // Drive an outgoing dial through the public `start_call` API.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // Confirm we are exercising the real acquisition path, not a bypass.
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

    // Terminal teardown via `shutdown` -> `reset_sessions`. Per-session
    // `release_pending` no-ops on the empty post-drain map; deterministic
    // `clear_pending_direct` is what actually clears the slot.
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    // Per-session teardown runs asynchronously; re-check after a beat to catch a
    // delayed teardown re-pending the slot.
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

/// Mirrors `reset_sessions_clears_pending_outgoing_slot` for `PendingIncoming`.
/// Block the accept prompt via `PendingAcceptProbe`, then `shutdown` Bob before
/// it resolves. `reset_sessions` must clear the slot even though per-session
/// `release_pending` no-ops on the empty post-drain map.
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
    // task receives the `Hello`, runs `is_session_still_current`, acquires
    // `PendingIncoming`, and shows the accept prompt (blocked by the probe).
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    accept_probe_b.wait_opened().await;

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

    // `reset_sessions` cancels the session's `stop_session` token and drains
    // `session_states`; cancellation reaches the prompt, the session returns
    // `SessionStopped`, and `clear_pending_direct` must leave the slot `Idle`.
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

/// Terminal reset clears a real public outgoing call while its callee remains
/// blocked on the acceptance prompt.
#[tokio::test(flavor = "multi_thread")]
async fn reset_sessions_cancels_public_pending_outgoing_acceptance() {
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

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    accept_probe_b.wait_cancelled().await;

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
        "call slot must be Idle after reset_sessions clears the public outgoing call; got {after:?}"
    );
    assert_eq!(
        after.direct_peer, None,
        "no peer should own the slot after reset_sessions; got {after:?}"
    );
}

/// Full `restart_manager()` flow: slot ends `Idle`, a fresh session is registered
/// for the known contact, and a subsequent `start_call()` acquires a fresh
/// `PendingOutgoing` slot — not stuck in any pre-restart pending state.
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

    // `client_a` needs a multi-lifecycle mock; `client_b` uses the standard
    // single-lifecycle builder.
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

    // `client_b`'s mock pins `manager_state` to a single lifecycle, so a
    // panic elsewhere would leave an unmet `Stopped` expectation. The
    // guard keeps the diagnostic chain clean.
    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // `restart_manager` rejects a non-idle slot; drain any in-flight dial first.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;

    let pre_restart_session_id = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_b)
        .map(|s| s.id())
        .expect("client_a should have a session for contact_b before restart");

    // Timeout: a regression that hangs waiting for the new `manager_active`
    // notification would stall the test.
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

    // Wait for the full post-restart session pair to stabilize; `restart_manager`
    // re-spawns asynchronously after the new manager activates and `client_b`'s
    // pre-restart transport may still be tearing down.
    wait_for_stable_session_pair(
        &client_a,
        &peer_id_b,
        &client_b,
        &contact_a.get_peer_id(),
        Some(pre_restart_session_id),
    )
    .await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("start_call after restart_manager should succeed");

    wait_for_slot_owned_by(&client_a, &peer_id_b).await;

    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;

    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

// The previous `stale_session_receives_hello_sends_immediate_busy_response` test
// was removed: production no longer sends `Busy` from a stale session (a fresh
// replacement may be ready to serve the dialer). See the
// `stale_session_with_fresh_replacement_*` and `stale_session_with_no_replacement_*`
// tests for the new behaviour.

/// Test A — stale session with a fresh replacement session in the map.
///
/// The stale listener must NOT send `Busy` (the fresh replacement serves the dialer
/// on its own connection) and must NOT close its connection (the dialer's `Hello`
/// is on the stale connection; a premature close surfaces as a transport error).
///
/// Asserted invariants:
///   1. Alice does not observe an `is busy` `CallEnded`.
///   2. Bob's current map entry for Alice is the fresh id we inserted.
///
/// Alice's own session id is intentionally not asserted: real two-sided dialling
/// against the shared relay can legitimately swap or tear down her session.
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

    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Warm the transport before mutating Bob's map so post-dial timing
    // measures the stale-Hello round trip, not first-packet warmup.
    wait_for_active_transport(&client_a, "client_a").await;
    wait_for_active_transport(&client_b, "client_b").await;

    let stale_b_id = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_id_a)
        .map(|s| s.id())
        .expect("client_b should have a session for contact_a");

    // Replace Bob's map entry with a fresh `SessionState` (no live task).
    // The original session task is stale — still listening on its connection.
    {
        let mut states = client_b.telepathy.inner.session_states.write().await;
        let fresh: Arc<SessionState> = Arc::new(SessionState::new_for_test());
        states.insert(peer_id_a, fresh);
    }

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

    // Alice's session sends `Hello` to Bob's stale connection. The stale
    // session task sees the fresh map entry has a different id and must
    // NOT send `Busy` and must NOT close the connection.
    let dial_started_at = std::time::Instant::now();
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // 8s budget (well below the 10s `HELLO_TIMEOUT`) absorbs relay-contention
    // jitter. We assert NO `is busy`; the post-dial `did not respond` outcome
    // would only fire after `HELLO_TIMEOUT` and is outside the window.
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

    // Disarm guard, shut down before assertions to satisfy mock
    // `Stopped` lifecycle even on downstream panic.
    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

    assert!(
        !observed_busy,
        "Alice must not observe an 'is busy' CallEnded; \
         the stale session with a fresh replacement must not send Busy. \
         states = {states_a:?}"
    );

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
/// The stale listener must NOT send `Busy` and must close its own connection so
/// Alice's read returns a transport error promptly (well before the 10s
/// `HELLO_TIMEOUT`). The dialer sees NO `CallEnded` (slot is `PendingOutgoing`,
/// not `ActiveDirect`).
///
/// Asserted invariants:
///   1. Alice does not observe an `is busy` `CallEnded` within 8s.
///   2. Alice does not observe a `did not respond` `CallEnded` within 8s.
///   3. Bob's current map entry for Alice is `None`.
///
/// Alice's own session id is intentionally not asserted (see Test A).
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

    // Drain Bob's current session entry. The live session task remains running
    // on its connection with no fresh session in the map; the "no fresh session"
    // branch should close the stale connection. We do NOT call
    // `stop_session.cancel()` so the map-check branch is exercised in
    // isolation, without the concurrent `SessionStopped` path.
    {
        let mut states = client_b.telepathy.inner.session_states.write().await;
        states.remove(&peer_id_a);
    }

    // Alice's session sends `Hello` to Bob's stale connection. The stale
    // session task sees no entry and must close the connection instead of
    // sending `Busy`. Alice's read returns a transport error and the session
    // loop breaks — no `CallEnded` is fired (slot is `PendingOutgoing`,
    // not `ActiveDirect`).
    let dial_started_at = std::time::Instant::now();
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // Within the 8s window (well below the 10s `HELLO_TIMEOUT`), Alice must
    // NOT observe `is busy` or `did not respond` CallEnded emissions.
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

    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;

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
async fn audio_test_input_stream_error_propagates_and_clears_state() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(false, false, 5.0);

    let key = SecretKey::generate();
    let contact = Contact::new("audio-test-client".to_string(), key.public().to_string())
        .expect("contact invalid");

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let client = build_client_with_options(
        relay_map,
        key,
        vec![contact],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Restartable,
    )
    .await;

    let (result, ()) = tokio::join!(async { client.telepathy.audio_test().await }, async {
        input_error_probe.wait_captured().await;
        input_error_probe.trigger(simulated_stream_error(
            "simulated input device disconnected",
        ));
    });

    let error = result.expect_err("audio_test should fail when input stream errors");
    assert!(
        error
            .to_string()
            .contains("simulated input device disconnected"),
        "expected propagated disconnect error, got {error}"
    );
    assert_call_slot_idle(
        &client,
        "audio_test should leave the call slot idle after cleanup",
    );

    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_test_output_stream_error_propagates_and_clears_state() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(false, false, 5.0);

    let key = SecretKey::generate();
    let contact = Contact::new("audio-test-client".to_string(), key.public().to_string())
        .expect("contact invalid");

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let client = build_client_with_options(
        relay_map,
        key,
        vec![contact],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe, output_error_probe.clone()),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Restartable,
    )
    .await;

    let (result, ()) = tokio::join!(async { client.telepathy.audio_test().await }, async {
        output_error_probe.wait_captured().await;
        output_error_probe.trigger(simulated_stream_error(
            "simulated output device disconnected",
        ));
    });

    let error = result.expect_err("audio_test should fail when output stream errors");
    assert!(
        error
            .to_string()
            .contains("simulated output device disconnected"),
        "expected propagated disconnect error, got {error}"
    );
    assert_call_slot_idle(
        &client,
        "audio_test should leave the call slot idle after cleanup",
    );

    client.telepathy.shutdown().await;
}

/// Synchronously-failing `setup_output` (e.g. another process holds the exclusive
/// output device) must surface a single `CallState::CallEnded` to the dialer with
/// `CALL_END_AUDIO_DEVICE_FAILURE` copy and `remote == false`, so the frontend can
/// exit the connecting state.
#[tokio::test(flavor = "multi_thread")]
async fn setup_output_synchronous_failure_emits_call_ended() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "setup-output-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "setup-output-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let accept_probe_b = PendingAcceptProbe::default();

    let dialer_host =
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe.clone());
    dialer_host.fail_output_synchronously.store(true, Relaxed);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        dialer_host,
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
        .expect("bob should accept the call");

    wait_for_call_ended_contains(&call_states_a, "Audio device error", false, "alice").await;

    let states_a = call_state_snapshot(&call_states_a);
    let ended_count = states_a
        .iter()
        .filter(|state| matches!(state, CallState::CallEnded(_, _)))
        .count();
    assert_eq!(
        ended_count, 1,
        "expected exactly one CallEnded on the dialer; states were {states_a:?}"
    );
    assert!(
        states_a
            .iter()
            .all(|state| !matches!(state, CallState::Connected)),
        "dialer must never reach Connected when setup_output fails synchronously; states were {states_a:?}"
    );
    assert_no_call_ended_contains(&call_states_a, "no output device", "alice");
    assert_call_slot_idle(
        &client_a,
        "dialer should leave the call slot idle after synchronous setup_output failure",
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Happy-path contrast for `setup_output_synchronous_failure_emits_call_ended`:
/// when the dialer's output device opens successfully, the same host still
/// produces `Connected`. Guards against an over-eager fix short-circuiting
/// `call_handshake`.
#[tokio::test(flavor = "multi_thread")]
async fn setup_output_synchronous_flag_disabled_still_connects() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "setup-output-happy-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "setup-output-happy-client-b".to_string(),
        key_b.public().to_string(),
    )
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
        .expect("bob should accept the call");

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_call_input_stream_error_surfaces_local_message() {
    normal_call_stream_error_surfaces_local_message(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_call_output_stream_error_surfaces_local_message() {
    normal_call_stream_error_surfaces_local_message(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_input_stream_error_surfaces_local_message() {
    room_stream_error_surfaces_local_message(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_output_stream_error_surfaces_local_message() {
    room_stream_error_surfaces_local_message(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_input_stream_error_sends_audio_error_goodbye_on_control_stream() {
    room_stream_error_sends_audio_error_goodbye_on_control_stream(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_output_stream_error_sends_audio_error_goodbye_on_control_stream() {
    room_stream_error_sends_audio_error_goodbye_on_control_stream(false).await;
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

/// Happy-path baseline for the room-generation token: both clients `join_room`,
/// each side emits `Connected` and exactly one `RoomJoin` for the peer, slot is
/// `RoomCall` on both, and `RoomState.generation` is bumped. Locks in the
/// `room_owner`/`room_generation` invariants the controller enforces at teardown.
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

    wait_for_slot_room_call(&client_a, "client_a_pre_join").await;
    wait_for_slot_room_call(&client_b, "client_b_pre_join").await;

    // White-box check that `join_room` bumped the generation counter and stored
    // a matching value in `RoomState`.
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

/// Regression for the `end_call` -> `join_room` cycle (R1): the post-rejoin must
/// produce a *second* `RoomJoin` (not be lost to stale `room_state` carry-over)
/// and must not emit a spurious `RoomLeave` after it — the failure mode in the
/// system-test artifact `test_room_end_releases_call_slot_for_rejoin`.
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

    // `client_a` and `client_b` mock callbacks pin a single lifecycle; the guard
    // keeps the diagnostic chain clean if a downstream assertion panics.
    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

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

    // Force the stale-permit race: in production an incoming room `Hello`
    // can win `session_inner`'s `select!` while a queued room-start
    // `start_call.notify_one()` stays latched. Without `room_handshake`'s
    // terminal cleanup the permit survives into the next `session_inner`
    // iteration and the session task treats it as a fresh direct-call
    // intent, flipping the slot to `PendingOutgoing`/`ActiveDirect`.
    let a_session_for_b = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&contact_b.get_peer_id())
        .cloned()
        .expect("client_a should have a session for contact_b while in the room");
    a_session_for_b.start_call.notify_one();
    let b_session_for_a = client_b
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&contact_a.get_peer_id())
        .cloned()
        .expect("client_b should have a session for contact_a while in the room");
    b_session_for_a.start_call.notify_one();

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

    // Stability window: with the seeded permit, `room_handshake` must discard
    // it before returning control to `session_inner`, otherwise the slot
    // flips to `PendingOutgoing`/`ActiveDirect` for the former room peer.
    assert_slot_remains_outside_direct_call_states(
        &client_a,
        &contact_b.get_peer_id(),
        "client_a",
        Duration::from_secs(2),
    )
    .await;
    assert_slot_remains_outside_direct_call_states(
        &client_b,
        &contact_a.get_peer_id(),
        "client_b",
        Duration::from_secs(2),
    )
    .await;

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

    // The post-rejoin window must not produce a spurious `RoomLeave` after
    // the second `RoomJoin` — exact failure mode from the system-test artifacts.
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
    // Intermediate `end_call` -> `join_room` is a local slot transition (Idle
    // asserted above), not a wire `RoomLeave` — locked in as `Join, Join`.
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

    shutdown_guard.disarm();
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Primary regression for the failing system-test artifact
/// `test_room_peer_leave_and_rejoin` (R2/R5).
///
/// Three clients in a room. One leaves, the remaining two observe exactly one
/// `RoomLeave(leaver)`. The leaver rejoins, the remaining two observe a second
/// `RoomJoin(leaver)` and — critically — no extra `RoomLeave` after it. The exact
/// ordered sequence `[Join, Leave, Join]` for the leaver on each remaining client
/// locks in the fix: a stale `connection_id`-keyed `Leave` would emit
/// `RoomLeave(leaver)` after `RoomJoin(leaver)`, producing `[Join, Leave, Join, Leave]`
/// and breaking the mesh.
///
/// The R2 in-place race (fast `end_call` -> `join_room` on the same transport) is
/// blocked at the public API layer; this test reproduces the same race shape with a
/// real `stop_session`/`start_session` for the leaver's two sessions, producing a
/// fresh `connection_id` on both sides — exactly the condition the
/// `room_leave_stale_connection` branch in `room_controller` detects. The 3-second
/// post-rejoin window is the concrete guard.
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

    // Sorted three-member room, matching how production callers sort the member list.
    let mut room_members = vec![peer_a.clone(), peer_b.clone(), peer_c.clone()];
    room_members.sort();

    // `ManagerLifecycle::Single` is safe here: `stop_session`/`start_session`
    // don't plumb `manager_state` events, so the strict single-lifecycle mock
    // holds (2 starting/active + 1 stopped per client).
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

    // All three join the same room. `join_room` auto-accepts (no accept prompt for
    // room calls), so `build_client` is sufficient and the single-lifecycle mock works.
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

    // Client C leaves via `end_call`, then full `stop_session`/`start_session` for
    // both A and B before re-joining. `end_call` alone cannot be followed by an
    // in-place `join_room` because A and B are still in `RoomCall` and the new
    // `room_handshake` would race the still-active slot.
    client_c.telepathy.end_call().await;
    wait_for_slot_idle(&client_c, &peer_c).await;
    wait_for_room_leave_count(&call_states_a, &peer_c, 1).await;
    wait_for_room_leave_count(&call_states_b, &peer_c, 1).await;
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

    // Fresh `connection_id` on both sides: the new `Join` is keyed by a different
    // `connection_id` than the old `Leave`, which is the condition
    // `room_leave_stale_connection` detects.
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
    // 3-second window catches a stale `Leave` from the previous transport that
    // races the new `Join` handler.
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

/// Slot-contention regression: a second `join_room` while the slot is `RoomCall`
/// must return `Err(CallAlreadyActive)`; after `end_call` a fresh `join_room`
/// re-acquires `RoomCall` and bumps the generation.
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

    let second = client_a.telepathy.join_room(room_members.clone()).await;
    assert!(
        second.is_err(),
        "second join_room while the slot is RoomCall must return Err; got {second:?}"
    );

    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a).await;

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

/// Synchronous input setup failure must remove the `RoomState` installed by
/// public `join_room` and release its `RoomCall` slot before any room processing
/// starts.
#[tokio::test(flavor = "multi_thread")]
async fn room_input_setup_synchronous_failure_clears_state_and_releases_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "room-input-setup-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "room-input-setup-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let input_failure_host =
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe);
    input_failure_host
        .fail_input_synchronously
        .store(true, Relaxed);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        input_failure_host,
        call_states_a.clone(),
    )
    .await;

    client_a
        .telepathy
        .join_room(sorted_room_members(&contact_a, &contact_b))
        .await
        .expect("join_room should return after spawning its controller");
    input_error_probe.wait_setup_attempted().await;

    wait_for_call_ended_contains(
        &call_states_a,
        "Audio device error",
        false,
        "room input setup failure",
    )
    .await;
    wait_for_slot_idle(&client_a, &contact_a.get_peer_id().to_string()).await;
    assert_eq!(
        client_a.telepathy.inner.current_room_generation().await,
        None,
        "synchronous input setup failure must clear RoomState"
    );
    assert_call_slot_idle(
        &client_a,
        "synchronous room input setup failure must release the call slot",
    );

    client_a.telepathy.shutdown().await;
}

/// Synchronous output setup failure after room peers join must remove the
/// installed `RoomState` and release its `RoomCall` slot.
#[tokio::test(flavor = "multi_thread")]
async fn room_output_setup_synchronous_failure_clears_state_and_releases_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "room-output-setup-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "room-output-setup-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let output_failure_host =
        CallbackCapturingAudioHost::new(input_error_probe, output_error_probe.clone());
    output_failure_host
        .fail_output_synchronously
        .store(true, Relaxed);
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        output_failure_host,
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
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should start joining room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room and send peer join");
    output_error_probe.wait_setup_attempted().await;

    wait_for_call_ended_contains(
        &call_states_a,
        "Audio device error",
        false,
        "room output setup failure",
    )
    .await;
    wait_for_slot_idle(&client_a, &contact_a.get_peer_id().to_string()).await;
    assert_eq!(
        client_a.telepathy.inner.current_room_generation().await,
        None,
        "synchronous output setup failure after peer join must clear RoomState"
    );
    assert_call_slot_idle(
        &client_a,
        "synchronous room output setup failure must release the call slot",
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

async fn normal_call_stream_error_surfaces_local_message(trigger_input: bool) {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "stream-error-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "stream-error-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let accept_probe_b = PendingAcceptProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe.clone()),
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
        .expect("bob should accept the call");

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;
    accept_probe_b.wait_cancelled().await;

    let (probe, expected_message, simulated_message) =
        stream_error_scenario(trigger_input, &input_error_probe, &output_error_probe);
    probe.wait_captured().await;
    probe.trigger(simulated_stream_error(simulated_message));

    wait_for_call_ended_contains(&call_states_a, expected_message, false, "alice").await;
    wait_for_call_ended_contains(&call_states_b, "Audio device error", true, "bob").await;
    assert_no_call_ended_contains(&call_states_b, simulated_message, "bob");

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

async fn room_stream_error_surfaces_local_message(trigger_input: bool) {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "room-stream-error-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "room-stream-error-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe.clone()),
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

    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

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

    let (probe, expected_message, simulated_message) =
        stream_error_scenario(trigger_input, &input_error_probe, &output_error_probe);
    probe.wait_captured().await;
    probe.trigger(simulated_stream_error(simulated_message));

    // Terminal contract: local `CallEnded`, remote `RoomLeave`, local slot release,
    // remote peer retains its `RoomCall` slot. No wall-clock budget.
    wait_for_call_ended_contains(&call_states_a, expected_message, false, "room client a").await;
    wait_for_room_leave_count(&call_states_b, &peer_a, 1).await;
    wait_for_slot_idle(&client_a, &peer_b).await;
    wait_for_slot_room_call(&client_b, "room client b after remote stream error").await;

    let states_b = call_state_snapshot(&call_states_b);
    assert_room_event_sequence(
        &states_b,
        &peer_a,
        &[RoomEventKind::Join, RoomEventKind::Leave],
    );
    assert!(
        states_b
            .iter()
            .all(|state| !matches!(state, CallState::CallEnded(_, _))),
        "remote room member should receive RoomLeave without ending its room call; states were {states_b:?}"
    );

    shutdown_guard.disarm();
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

async fn room_stream_error_sends_audio_error_goodbye_on_control_stream(trigger_input: bool) {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_b = Contact::new(
        "room-wire-error-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");

    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let raw_endpoint = build_raw_room_endpoint(relay_map, key_b).await;

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe.clone()),
        call_states_a.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    let mut raw_peer = accept_raw_room_peer(raw_endpoint).await;
    wait_for_active_transport(&client_a, "room wire client a").await;

    client_a
        .telepathy
        .join_room(vec![peer_b.clone()])
        .await
        .expect("client a should join room with raw peer");

    let hello = read_wire_message_skipping_keepalives(&mut raw_peer.control_recv).await;
    assert!(
        matches!(
            hello,
            WireProtocolMessage::Hello {
                room_hash: Some(_),
                ..
            }
        ),
        "room session must negotiate over the established control stream; got {hello:?}"
    );
    write_wire_message(
        &mut raw_peer.control_send,
        WireProtocolMessage::HelloAck {
            audio_header: WireAudioHeader {
                sample_rate: DEFAULT_SAMPLE_RATE,
                codec_enabled: true,
                vbr: true,
                residual_bits: 5.0,
            },
        },
    )
    .await;

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    let (probe, expected_message, simulated_message) =
        stream_error_scenario(trigger_input, &input_error_probe, &output_error_probe);
    probe.wait_captured().await;
    probe.trigger(simulated_stream_error(simulated_message));

    // Both the Telepathy client and the raw peer must be torn down so the
    // pinned `Stopped` lifecycle expectations are satisfied even on wire-protocol
    // assertion panic.
    let shutdown_guard = RawPeerShutdownGuard {
        client: &client_a,
        raw_endpoint: Some(raw_peer.endpoint.clone()),
        dropped: AtomicBool::new(false),
    };

    // Read the terminal Goodbye on the established control stream. Tolerate
    // control keepalives so a delayed scheduler cannot turn a valid keepalive
    // into a false failure to receive Goodbye.
    let goodbye = tokio::time::timeout(
        Duration::from_secs(10),
        read_wire_message_skipping_keepalives(&mut raw_peer.control_recv),
    )
    .await
    .expect("room audio error goodbye should arrive on the control stream");
    assert!(
        matches!(
            goodbye,
            WireProtocolMessage::Goodbye {
                reason: WireGoodbyeReason::AudioDeviceError,
            }
        ),
        "room audio error must send GoodbyeReason::AudioDeviceError on the existing control stream; got {goodbye:?}"
    );

    // No replacement stream: after the terminal Goodbye the room controller must
    // NOT open another bidirectional stream.
    let connection = raw_peer.connection.clone();
    let additional_stream =
        tokio::time::timeout(Duration::from_millis(750), connection.accept_bi()).await;
    assert!(
        !matches!(additional_stream, Ok(Ok(_))),
        "room audio error must not open a second bidirectional stream after Goodbye; got {additional_stream:?}"
    );

    wait_for_call_ended_contains(
        &call_states_a,
        expected_message,
        false,
        "room wire client a",
    )
    .await;

    shutdown_guard.disarm();
    client_a.telepathy.shutdown().await;
    raw_peer.endpoint.close().await;
}

// Call-end copy contract tests. Each path funnels through `CallEndMessage` in
// `rust/telepathy-core/src/internal/error.rs`. Expected strings must stay in
// sync with the `CALL_END_*` constants and the peer-message helpers. Regression
// guard against: (1) `format!(...)` / `error.to_string()` leaking internal
// wording into the frontend, (2) inconsistent copy across paths.

/// Lock the `"{nickname} is busy"` copy Alice observes when Bob's listener
/// rejects the incoming `Hello` with `Busy`.
#[tokio::test(flavor = "multi_thread")]
async fn outgoing_call_busy_emits_localized_copy() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("busy-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("busy-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));

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
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Force Bob's slot into `AudioTest` so his listener rejects Alice's `Hello` with
    // `Busy`. We use the slot API directly (rather than `audio_test()`) because the
    // latter drives a real call loop that would block on `end_call`.
    assert!(
        client_b
            .telepathy
            .inner
            .core_state
            .call_slot
            .try_acquire(CallSlotState::AudioTest, None)
            .expect("slot acquire should succeed"),
        "Bob's slot must be acquirable for the busy test setup"
    );

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    let busy_message = format!("{} is busy", contact_b.nickname());
    wait_for_call_ended_contains(&call_states_a, &busy_message, true, "alice").await;

    assert_no_call_ended_contains(&call_states_a, "Busy", "alice");

    // Release Bob's `AudioTest` slot so `shutdown` (which only touches
    // `PendingDirect*`/`ActiveDirect`/`RoomCall`) can take it cleanly.
    client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .release()
        .expect("slot release should succeed");

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Lock the `"{nickname} did not respond to the call"` copy the `HELLO_TIMEOUT`
/// arm of `negotiate_outgoing_call` emits. Positive end-to-end would require a
/// 10-second wait; we pin the formatter (the single source of the timeout copy).
#[test]
fn outgoing_call_did_not_respond_emits_localized_copy() {
    use telepathy_core::internal::error::peer_no_response_message;

    assert_eq!(
        peer_no_response_message("Bob"),
        "Bob did not respond to the call"
    );
    // Empty nickname: formatter still produces a user-facing sentence.
    assert_eq!(peer_no_response_message(""), " did not respond to the call");
    // Unicode nickname: contract must round-trip without mangling.
    assert_eq!(
        peer_no_response_message("Élise Müller"),
        "Élise Müller did not respond to the call"
    );
}

/// Lock the natural peer-facing sentences produced by `peer_goodbye_reason_message`
/// for every `GoodbyeReason` variant. Pinning the formatter (the single source)
/// catches regressions that re-introduce raw wire wording.
#[test]
fn outgoing_call_goodbye_emits_localized_copy() {
    use telepathy_core::internal::error::{GoodbyeReason, peer_goodbye_reason_message};

    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::SessionStopped),
        "Bob did not accept the call because the session was stopped"
    );
    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::AudioDeviceError),
        "Bob did not accept the call because of an audio device problem"
    );
    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::Error),
        "Bob did not accept the call because of an unexpected problem"
    );
    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::None),
        "Bob did not accept the call"
    );

    // Each variant produces a "{nickname} did not accept the call" prefix.
    for reason in [
        GoodbyeReason::SessionStopped,
        GoodbyeReason::AudioDeviceError,
        GoodbyeReason::Error,
        GoodbyeReason::None,
    ] {
        let rendered = peer_goodbye_reason_message("Bob", reason);
        assert!(
            rendered.starts_with("Bob did not accept the call"),
            "goodbye reason {reason:?} did not start with the expected user-facing prefix; got {rendered:?}"
        );
        // The snake-case wire name must never appear in the user-facing copy.
        let wire_name = format!("{reason:?}");
        assert!(
            !rendered.contains(&wire_name),
            "wire-format variant name {wire_name:?} leaked into user-facing copy {rendered:?}"
        );
    }
}

/// A peer-driven normal hangup must reach the frontend as a *silent* `CallEnded`
/// so the dialog guard (`state.field0.isNotEmpty` in `lib/main.dart`) suppresses
/// the failure toast and the silent hangup tone plays instead.
#[tokio::test(flavor = "multi_thread")]
async fn normal_hangup_emits_silent_call_ended_for_remote_peer() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "silent-hangup-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "silent-hangup-client-b".to_string(),
        key_b.public().to_string(),
    )
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

    // Drive a connected direct call.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;

    // Alice's controller writes `Goodbye { reason: GoodbyeReason::None }` and
    // returns `Silent`; Bob's controller converts to an empty user-facing
    // message via `from_goodbye_reason`.
    client_a.telepathy.end_call().await;

    // Exactly one remote silent `CallEnded` on Bob — frontend dialog guard
    // suppresses and silent hangup tone plays.
    wait_for_call_ended_contains(&call_states_b, "", true, "bob's silent hangup").await;

    let states_b = call_state_snapshot(&call_states_b);
    for state in &states_b {
        if let CallState::CallEnded(message, _) = state {
            assert_ne!(
                message, "The call ended unexpectedly",
                "peer-driven normal hangup must NOT render to the generic failure copy on the receiving peer"
            );
        }
    }

    let silent_end_count = states_b
        .iter()
        .filter(|state| matches!(state, CallState::CallEnded(message, true) if message.is_empty()))
        .count();
    assert_eq!(
        silent_end_count, 1,
        "expected exactly one remote silent CallEnded on bob; got {silent_end_count} in {states_b:?}"
    );

    assert_no_call_ended_before_connected(&states_b, "bob");

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Lock the `CallEndMessage::from_error` mapping for `SessionStopped`. A
/// `SessionStopped` error is hard to drive end-to-end (requires racing slot
/// release with `transition_pending_to_active`); pin the mapping at the helper
/// boundary. Audio-stream integration tests cover the production emission paths.
#[test]
fn session_stopped_error_emits_localized_copy() {
    use telepathy_core::internal::error::{
        CALL_END_SESSION_STOPPED, CallEndMessage, Error, ErrorKind,
    };

    let error: Error = ErrorKind::SessionStopped.into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_SESSION_STOPPED,
        "SessionStopped must produce the dedicated session-stopped copy"
    );
    assert_eq!(
        rendered, "The session was stopped",
        "exact wording must stay in sync with the user-facing template"
    );
    // Legacy `Display` produces "Session stopped" (no "The" prefix); the helper
    // must NOT pass that raw text through.
    assert_ne!(
        rendered,
        error.to_string(),
        "legacy Display wording leaked through CallEndMessage"
    );
}

/// Lock the `CallEndMessage::from_error` mapping for generic non-audio,
/// non-session-stopped, non-timeout errors: must collapse to
/// `"The call ended unexpectedly"` regardless of which `ErrorKind` triggered it.
#[test]
fn generic_controller_failure_emits_localized_copy() {
    use telepathy_core::internal::error::{CALL_END_GENERIC, CallEndMessage, Error, ErrorKind};

    // Every internal error kind that maps to the generic copy must produce exactly
    // the expected string and MUST NOT leak the raw `Display` wording.
    let error: Error = ErrorKind::MpscSend.into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_GENERIC,
        "MpscSend must produce the generic copy"
    );
    assert_ne!(
        rendered,
        error.to_string(),
        "raw Display wording must not leak through CallEndMessage"
    );
    assert!(
        !rendered.contains("mpsc"),
        "internal acronym leaked into user copy: {rendered}"
    );

    let error: Error = ErrorKind::TransportSend.into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_GENERIC,
        "TransportSend must produce the generic copy"
    );
    assert!(
        !rendered.contains("Transport"),
        "internal wording leaked into user copy: {rendered}"
    );

    let error: Error = ErrorKind::Poison("test lock").into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_GENERIC,
        "Poison must produce the generic copy"
    );
    assert!(
        !rendered.contains("Poison"),
        "internal wording leaked into user copy: {rendered}"
    );
}

/// Catch-all: the *frontend* copy for every backend failure is a closed set of
/// user-facing sentences. Any internal wording that bleeds into `CallState::CallEnded`
/// is a regression.
#[tokio::test(flavor = "multi_thread")]
async fn raw_internal_error_strings_never_reach_call_ended() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("raw-error-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("raw-error-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));

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
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Drive the busy path: Bob's slot is `AudioTest` so he rejects Alice's `Hello`
    // with `Busy`. Acquire the slot directly rather than driving `audio_test()`
    // (which blocks on a real call loop).
    assert!(
        client_b
            .telepathy
            .inner
            .core_state
            .call_slot
            .try_acquire(CallSlotState::AudioTest, None)
            .expect("slot acquire should succeed"),
        "Bob's slot must be acquirable for the busy test setup"
    );
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    wait_for_call_ended_contains(
        &call_states_a,
        &format!("{} is busy", contact_b.nickname()),
        true,
        "alice",
    )
    .await;
    // Release Bob's `AudioTest` slot before the call-state snapshot so
    // the shutdown path doesn't race with our walk.
    client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .release()
        .expect("slot release should succeed");

    // Walk every captured `CallEnded` and assert no known internal Display
    // string leaked through. Closed set: any new emission violating this contract
    // must add a row here AND be routed through `CallEndMessage`.
    let states = call_state_snapshot(&call_states_a);
    let forbidden_substrings = [
        // `ErrorKind::Poison` wording
        "Poison",
        // `ErrorKind::MpscSend` wording
        "mpsc",
        // `ErrorKind::TransportSend` / `TransportRecv` wording
        "Transport",
        // `ErrorKind::KanalSend` / `KanalReceive` / `KanalClose` wording
        "Kanal",
        // `ErrorKind::InvalidContactFormat` wording
        "Invalid contact format",
        // `ErrorKind::NoIdentityAvailable` / `NoEncoderAvailable` wording
        "No identity",
        "No encoder",
        // `ErrorKind::ManagerRestartDuringCall` wording
        "Cannot restart manager",
        // `ErrorKind::AttachmentsTooLarge` wording
        "Attachments too large",
        // `ErrorKind::AudioError` raw "Audio error: ..." prefix from
        // the legacy `Display` impl
        "Audio error:",
        // `ErrorKind::AudioInputStream` / `AudioOutputStream` raw
        // "Input stream error: ..." / "Output stream error: ..." prefix
        "Input stream error:",
        "Output stream error:",
        // `ErrorKind::DeviceError` raw "Device error: ..." prefix
        "Device error:",
        // `ErrorKind::BindError` wording
        "Bind error",
        // `ErrorKind::KeyParsing` wording
        "Key parsing",
        // `ErrorKind::Connection` wording
        "Connection error",
        // `ErrorKind::Poison` from anywhere via session-error wording
        "poisoned",
        // Wire-level GoodbyeReason strings that must NOT reach the
        // frontend copy (the renderer does its own mapping via
        // `CallEndMessage::from_goodbye_reason`).
        "an error occurred",
        "transport error",
        "session stopped",
        "audio device error",
    ];

    let mut violations: Vec<String> = Vec::new();
    for state in &states {
        if let CallState::CallEnded(message, _) = state {
            for forbidden in &forbidden_substrings {
                if message.contains(forbidden) {
                    violations.push(format!(
                        "CallEnded message {message:?} contains forbidden substring {forbidden:?}"
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "raw internal error strings leaked into CallEnded copy:\n  {}",
        violations.join("\n  ")
    );

    // Every observed `CallEnded` must match one of the closed user-facing copy
    // templates — the dual of the forbidden-substring check.
    for state in &states {
        if let CallState::CallEnded(message, _) = state {
            let known = [
                "A call is already active",
                "Audio device error",
                "The call ended unexpectedly",
                "The session was stopped",
                "The connection timed out",
            ];
            let is_user_facing_template = known.iter().any(|t| message == *t)
                || message.contains(" did not accept the call")
                || message.contains(" did not respond to the call")
                || message.contains(" is busy")
                || message.starts_with("Received an unexpected message from ");
            assert!(
                is_user_facing_template,
                "CallEnded message {message:?} did not match any known user-facing template; \
                 either a new user-facing template was added (extend this assertion) or \
                 internal wording leaked (route through CallEndMessage)"
            );
        }
    }

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
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
        // Initialise the shared address lookup eagerly so subsequent
        // `shared_address_lookup` calls see a populated `MemoryLookup`.
        SHARED_ADDRESS_LOOKUP.get_or_init(MemoryLookup::new);
        tokio::spawn(async move {
            let server = iroh::test_utils::run_relay_server().await.unwrap();
            RELAY_DETAILS.get_or_init(|| server.0);
            sleep(Duration::from_secs(u64::MAX)).await;
        });
    });

    RELAY_DETAILS.wait()
}

/// Returns the test-binary-wide `MemoryLookup`. Initialised via `shared_relay_map`
/// (called below to guarantee ordering) and reused by every subsequent call.
fn shared_address_lookup() -> &'static MemoryLookup {
    let _ = shared_relay_map();
    SHARED_ADDRESS_LOOKUP
        .get()
        .expect("shared_address_lookup called before shared_relay_map initialisation")
}

async fn build_raw_room_endpoint(relay_map: &RelayMap, identity: SecretKey) -> Endpoint {
    let mut crypto_provider = rustls::crypto::aws_lc_rs::default_provider();
    crypto_provider.kx_groups = vec![
        rustls::crypto::aws_lc_rs::kx_group::X25519MLKEM768,
        rustls::crypto::aws_lc_rs::kx_group::X25519,
        rustls::crypto::aws_lc_rs::kx_group::SECP256R1,
        rustls::crypto::aws_lc_rs::kx_group::SECP384R1,
    ];
    let endpoint = Endpoint::builder(presets::Empty)
        .secret_key(identity)
        .alpns(vec![b"telepathy/session/1".to_vec()])
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .address_lookup(shared_address_lookup().clone())
        .crypto_provider(Arc::new(crypto_provider))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await
        .expect("raw room peer endpoint should bind");
    endpoint.online().await;
    shared_address_lookup().add_endpoint_info(endpoint.addr());
    endpoint
}

async fn accept_raw_room_peer(endpoint: Endpoint) -> RawRoomPeer {
    let connection = endpoint
        .accept()
        .await
        .expect("raw room peer should receive a connection")
        .await
        .expect("raw room peer should accept the connection");
    let (send, recv) = connection
        .accept_bi()
        .await
        .expect("raw room peer should accept the established control stream");

    RawRoomPeer {
        endpoint,
        connection,
        control_send: LengthDelimitedCodec::builder()
            .length_field_type::<u64>()
            .new_write(send),
        control_recv: LengthDelimitedCodec::builder()
            .length_field_type::<u64>()
            .new_read(recv),
    }
}

async fn write_wire_message(
    transport: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    message: WireProtocolMessage,
) {
    transport
        .send(Bytes::from(
            message
                .write_to_vec()
                .expect("wire protocol message should serialize"),
        ))
        .await
        .expect("raw room peer should write control message");
}

async fn read_wire_message(
    transport: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
) -> WireProtocolMessage {
    let frame = transport
        .next()
        .await
        .expect("raw room peer should receive a control frame")
        .expect("raw room peer control frame should decode");
    WireProtocolMessage::read_from_buffer(&frame)
        .expect("raw room peer control message should deserialize")
}

/// Read wire control messages while skipping `KeepAlive` frames so a
/// scheduler-delayed keepalive does not get mistaken for the expected terminal
/// message.
async fn read_wire_message_skipping_keepalives(
    transport: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
) -> WireProtocolMessage {
    loop {
        let message = read_wire_message(transport).await;
        if !matches!(message, WireProtocolMessage::KeepAlive) {
            return message;
        }
    }
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

/// Returns mock callbacks that establish a telepathy instance with the provided
/// contacts. `is_active` flips to true on the first session-connected event.
/// `lifecycle` controls how many `manager_state` activations the mock accepts
/// (see `ManagerLifecycle`).
fn construct_mock_callbacks(
    contacts: Vec<Contact>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
) -> MockCoreCallbacks<MockCoreStatisticsCallback> {
    let mut mock: MockCoreCallbacks<MockCoreStatisticsCallback> = MockCoreCallbacks::new();

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
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Active | ManagerState::Starting))
                .times(2)
                .returning(|_| Box::pin(async move {}));

            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Stopped))
                .once()
                .returning(|_| Box::pin(async move {}));
        }
        ManagerLifecycle::Restartable => {
            // Each restart cycle emits one `Starting` and one `Active`;
            // `start_manager` may invoke this any number of times.
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Active | ManagerState::Starting))
                .times(..)
                .returning(|_| Box::pin(async move {}));

            // One `Stopped` per manager teardown (one per cycle + final shutdown).
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Stopped))
                .times(..)
                .returning(|_| Box::pin(async move {}));

            // `start_manager` may emit `Failed` before retrying on
            // `setup_endpoint`/main-loop error; accept any count so a transient
            // failure doesn't surface as a mockall "no matching expectation"
            // panic that masks the real cause.
            mock.expect_manager_state()
                .withf(|a| matches!(a, ManagerState::Failed))
                .times(..)
                .returning(|_| Box::pin(async move {}));
        }
    }

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

fn simulated_stream_error(message: &'static str) -> CpalError {
    CpalError::with_message(CpalErrorKind::DeviceNotAvailable, message)
}

fn stream_error_scenario<'a>(
    trigger_input: bool,
    input_error_probe: &'a StreamErrorProbe,
    output_error_probe: &'a StreamErrorProbe,
) -> (&'a StreamErrorProbe, &'static str, &'static str) {
    // Local `CallEnded` copy is direction-specific (input -> "Microphone error",
    // output -> "Speaker error" via `CallEndMessage::from_stream_error`). Remote
    // wire reason stays generic (`GoodbyeReason::AudioDeviceError` ->
    // "Audio device error" via `from_goodbye_reason`). Raw cpal/driver wording
    // must NOT reach the frontend on either side.
    if trigger_input {
        (
            input_error_probe,
            "Microphone error",
            "simulated input device disconnected",
        )
    } else {
        (
            output_error_probe,
            "Speaker error",
            "simulated output device disconnected",
        )
    }
}

fn assert_call_slot_idle<H, I, O>(client: &ClientHarness<H, I, O>, message: &str)
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let snapshot = client
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed");
    assert_eq!(snapshot.state, CallSlotState::Idle, "{message}");
}

async fn wait_for_call_ended_contains(
    call_states: &Arc<Mutex<Vec<CallState>>>,
    expected_message: &str,
    expected_remote: bool,
    label: &str,
) {
    let mut poll = interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        poll.tick().await;
        let states = call_state_snapshot(call_states);
        if states.iter().any(|state| {
            matches!(
                state,
                CallState::CallEnded(message, remote)
                    if *remote == expected_remote && message.contains(expected_message)
            )
        }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {label} CallEnded containing '{expected_message}' with remote={expected_remote}; states were {states:?}"
        );
    }
}

fn assert_no_call_ended_contains(
    call_states: &Arc<Mutex<Vec<CallState>>>,
    unexpected_message: &str,
    label: &str,
) {
    let states = call_state_snapshot(call_states);
    assert!(
        !states.iter().any(|state| {
            matches!(
                state,
                CallState::CallEnded(message, _) if message.contains(unexpected_message)
            )
        }),
        "{label} should not observe raw stream error text '{unexpected_message}'; states were {states:?}"
    );
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

/// Wait until the underlying transport is actually live on the given client.
/// `ClientHarness::is_active` is flipped to `true` on the first
/// `SessionStatus::Connected` callback, so this confirms the QUIC/relay path is
/// warm and not still doing first-packet setup.
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
    // Two-phase wait: confirm both sides have a session entry, then re-check after
    // a poll interval that the SessionState::id is unchanged. Guards against
    // returning during a session-collision replacement where the new owner has not
    // yet stabilized.
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

/// Waits until both clients have a `SessionState` registered for the indicated
/// peer AND session ids remain stable across at least one polling interval.
/// Optionally asserts the resulting id differs from a previous id (e.g. to confirm
/// `restart_manager` re-spawned the session).
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

/// Polls the call slot across a stability window and asserts it does NOT transition
/// into `PendingOutgoing` or `ActiveDirect` for `peer`. Post-room-teardown guard for
/// the stale-start race: a `start_call` permit latched on a session must be discarded
/// by `room_handshake` before returning control to `session_inner`.
async fn assert_slot_remains_outside_direct_call_states<H, I, O>(
    client: &ClientHarness<H, I, O>,
    peer: &PublicKey,
    label: &str,
    window: Duration,
) where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    let mut poll = interval(Duration::from_millis(20));
    let deadline = tokio::time::Instant::now() + window;
    loop {
        poll.tick().await;
        let snapshot = client
            .telepathy
            .inner
            .core_state
            .call_slot
            .snapshot()
            .expect("call slot snapshot should succeed");
        assert!(
            !(snapshot.direct_peer == Some(*peer)
                && matches!(
                    snapshot.state,
                    CallSlotState::PendingOutgoing | CallSlotState::ActiveDirect
                )),
            "{label}: slot entered {:?} for former room peer after end_call; \
             a stale SessionState::start_call permit survived the room teardown. \
             snapshot={snapshot:?}",
            snapshot.state
        );
        if tokio::time::Instant::now() >= deadline {
            return;
        }
    }
}

/// Waits until the call slot is in `RoomCall` state, indicating `join_room` has
/// installed a `RoomState` and acquired the slot for the room.
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

/// Waits until the call slot is owned by `peer` and in a non-idle pending or
/// active call state, then re-checks across one more poll interval to confirm it
/// does not flip to `Idle` (which would indicate a phantom second negotiation or
/// stale-state leak).
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

/// Cleanup guard for two-client tests. On drop it schedules shutdowns for both
/// clients so an aborted test reaches the same shutdown path as a successful one,
/// preventing `client_b`'s mock from being left with an unmet `Stopped` expectation
/// that would surface as a misleading secondary panic after the real assertion
/// failure. `Drop` runs on a multi-thread test worker that owns a tokio runtime
/// handle; `block_in_place` + `block_on` drive the async shutdowns synchronously
/// without cloning the `ClientHarness`.
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
    /// Marks the guard as already-handled so its `Drop` becomes a no-op. The
    /// success path calls this immediately before `drop(shutdown_guard)` so the
    /// explicit `shutdown` calls that follow are the only shutdowns that run;
    /// without it `Drop` would fire a redundant `shutdown` after each explicit call.
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
        // Success path sets `dropped` so this is a no-op. On panic the
        // guard's `Drop` best-effort shuts down both clients to avoid an
        // unmet `Stopped` expectation surfacing as a misleading secondary panic.
        if self.dropped.swap(true, Relaxed) {
            return;
        }
        let a = self.a;
        let b = self.b;
        let shutdown_both = || async move {
            a.telepathy.shutdown().await;
            b.telepathy.shutdown().await;
        };
        // `Handle::current()` panicking with no runtime is the desired
        // failure mode: a silently-no-op `drop` would leave `client_b`'s mock
        // with an unmet `Stopped` expectation.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(shutdown_both());
        });
    }
}

/// Cleanup guard for single-client + raw-peer tests. On drop it shuts down the
/// Telepathy client and closes the raw `Endpoint`, preventing mock expectation
/// panics (the `MockCoreCallbacks` `Stopped` lifecycle, etc.) from being raised
/// after the real wire-protocol assertion failure.
struct RawPeerShutdownGuard<'a, H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    client: &'a ClientHarness<H, I, O>,
    raw_endpoint: Option<Endpoint>,
    dropped: AtomicBool,
}

impl<'a, H, I, O> RawPeerShutdownGuard<'a, H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    /// Marks the guard as already-handled so its `Drop` becomes a no-op. The
    /// success path calls this immediately before `drop(shutdown_guard)` so the
    /// explicit shutdowns that follow are the only ones that run.
    fn disarm(&self) {
        self.dropped.store(true, Relaxed);
    }
}

impl<H, I, O> Drop for RawPeerShutdownGuard<'_, H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    fn drop(&mut self) {
        if self.dropped.swap(true, Relaxed) {
            return;
        }
        let client = self.client;
        let endpoint = self.raw_endpoint.take();
        let cleanup = || async move {
            client.telepathy.shutdown().await;
            if let Some(ep) = endpoint {
                ep.close().await;
            }
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(cleanup());
        });
    }
}
