#![cfg(feature = "integration-testing")]
#![allow(
    clippy::empty_line_after_doc_comments,
    clippy::empty_line_after_outer_attr
)]

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
use telepathy_audio::devices::{AudioHost, DeviceDirection, DeviceError};
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_audio::io::StreamErrorCallback;
use telepathy_audio::{CpalError, CpalErrorKind};
use telepathy_core::internal::TelepathyHandle;
use telepathy_core::internal::callbacks::{MockCoreCallbacks, MockCoreStatisticsCallback};
use telepathy_core::internal::state::CallSlotState;
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

pub(super) static TEST_TRACING_INIT: Once = Once::new();
pub(super) static RELAY_INIT: Once = Once::new();
pub(super) static RELAY_DETAILS: OnceLock<RelayMap> = OnceLock::new();
/// Single shared in-process address lookup. `setup_endpoint` registers each
/// peer's `addr()` here right after bind, so every test client resolves the
/// others from the same map.
pub(super) static SHARED_ADDRESS_LOOKUP: OnceLock<MemoryLookup> = OnceLock::new();

pub(super) const SEQUENCED_STEP: f32 = 1.0 / 4096.0;
pub(super) const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub(super) const MOCK_DEVICE_ID: &str = "mock";
pub(super) const STALE_INPUT_DEVICE_ID: &str = "stale-input";
pub(super) const STALE_OUTPUT_DEVICE_ID: &str = "stale-output";

pub(super) type MockTelepathyHandle<H, I, O> = TelepathyHandle<
    MockCoreCallbacks<MockCoreStatisticsCallback>,
    MockCoreStatisticsCallback,
    H,
    I,
    O,
>;

pub(super) struct ClientHarness<H, I, O>
where
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    pub(super) telepathy: MockTelepathyHandle<H, I, O>,
    pub(super) is_active: Arc<AtomicBool>,
}

#[derive(Debug, Readable, Writable)]
pub(super) enum WireProtocolMessage {
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
pub(super) struct WireAudioHeader {
    sample_rate: u32,
    codec_enabled: bool,
    vbr: bool,
    residual_bits: f64,
}

#[derive(Debug, Readable, Writable)]
pub(super) enum WireGoodbyeReason {
    SessionStopped,
    AudioDeviceError,
    Error,
    None,
}

#[derive(Debug, Readable, Writable)]
pub(super) struct WireAttachment {
    name: String,
    data: Vec<u8>,
}

pub(super) struct RawRoomPeer {
    endpoint: Endpoint,
    connection: Connection,
    control_send: FramedWrite<SendStream, LengthDelimitedCodec>,
    control_recv: FramedRead<RecvStream, LengthDelimitedCodec>,
}

#[derive(Debug, Clone)]
pub(super) struct SequencedInput {
    counter: Arc<AtomicUsize>,
    sample_rate: u32,
}

impl SequencedInput {
    pub(super) fn new(sample_rate: u32) -> Self {
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
pub(super) struct RecordingOutput {
    log: Arc<Mutex<Vec<usize>>>,
}

impl RecordingOutput {
    pub(super) fn new(log: Arc<Mutex<Vec<usize>>>) -> Self {
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
pub(super) struct StreamErrorProbe {
    callback: Arc<Mutex<Option<StreamErrorCallback>>>,
    ready: Arc<Notify>,
}

impl StreamErrorProbe {
    pub(super) fn new() -> Self {
        Self {
            callback: Arc::new(Mutex::new(None)),
            ready: Arc::new(Notify::new()),
        }
    }

    pub(super) fn capture(&self, callback: Option<StreamErrorCallback>) {
        *self.callback.lock().unwrap() = callback;
        self.ready.notify_one();
    }

    pub(super) async fn wait_captured(&self) {
        self.ready.notified().await;
    }

    pub(super) fn signal_setup_attempt(&self) {
        self.ready.notify_one();
    }

    pub(super) async fn wait_setup_attempted(&self) {
        self.ready.notified().await;
    }

    pub(super) fn trigger(&self, error: CpalError) {
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
pub(super) struct CallbackCapturingAudioHost {
    pub(super) input_error_probe: StreamErrorProbe,
    pub(super) output_error_probe: StreamErrorProbe,
    pub(super) device_selection_probe: DeviceSelectionProbe,
    /// When set, `open_output` returns a synchronous `DeviceError`
    /// (simulating an exclusively-held output device) without capturing
    /// the stream error callback.
    pub(super) fail_output_synchronously: Arc<AtomicBool>,
    pub(super) fail_input_synchronously: Arc<AtomicBool>,
}

impl CallbackCapturingAudioHost {
    pub(super) fn new(
        input_error_probe: StreamErrorProbe,
        output_error_probe: StreamErrorProbe,
    ) -> Self {
        Self {
            input_error_probe,
            output_error_probe,
            device_selection_probe: DeviceSelectionProbe::default(),
            fail_output_synchronously: Arc::new(AtomicBool::new(false)),
            fail_input_synchronously: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn with_device_selection_probe(mut self, probe: DeviceSelectionProbe) -> Self {
        self.device_selection_probe = probe;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeviceSelectionOperation {
    InputSampleRate,
    OpenInput,
    OpenOutput,
}

type DeviceSelectionAttempt = (DeviceSelectionOperation, Option<String>);

#[derive(Debug, Clone, Default)]
pub(super) struct DeviceSelectionProbe {
    attempts: Arc<Mutex<Vec<DeviceSelectionAttempt>>>,
    changed: Arc<Notify>,
}

impl DeviceSelectionProbe {
    fn record(&self, operation: DeviceSelectionOperation, device_id: Option<&str>) {
        self.attempts
            .lock()
            .unwrap()
            .push((operation, device_id.map(str::to_owned)));
        self.changed.notify_one();
    }

    pub(super) async fn wait_for(
        &self,
        operation: DeviceSelectionOperation,
        device_id: &str,
        expected: usize,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if self
                .attempts
                .lock()
                .unwrap()
                .iter()
                .filter(|(attempted, id)| {
                    *attempted == operation && id.as_deref() == Some(device_id)
                })
                .count()
                >= expected
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {operation:?} with device {device_id:?}; attempts={:?}",
                self.snapshot()
            );
            tokio::select! { _ = self.changed.notified() => {}, _ = sleep(Duration::from_millis(100)) => {} }
        }
    }

    pub(super) fn snapshot(&self) -> Vec<DeviceSelectionAttempt> {
        self.attempts.lock().unwrap().clone()
    }

    pub(super) fn assert_no_default_attempt(&self, operation: DeviceSelectionOperation) {
        let attempts = self.snapshot();
        assert!(
            !attempts
                .iter()
                .any(|(attempted, id)| *attempted == operation && id.is_none()),
            "{operation:?} must not fall back to the default device; attempts={attempts:?}"
        );
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
        device_id: Option<&str>,
    ) -> Result<u32, telepathy_audio::devices::DeviceError> {
        self.device_selection_probe
            .record(DeviceSelectionOperation::InputSampleRate, device_id);
        if device_id == Some(STALE_INPUT_DEVICE_ID) {
            return Err(DeviceError::DeviceNotFound {
                direction: DeviceDirection::Input,
                id: STALE_INPUT_DEVICE_ID.to_string(),
            });
        }
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
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<
        (impl AudioInput + Send + 'static, u32, Self::InputStream),
        telepathy_audio::devices::DeviceError,
    > {
        self.device_selection_probe
            .record(DeviceSelectionOperation::OpenInput, device_id);
        if self.fail_input_synchronously.load(Relaxed) {
            self.input_error_probe.signal_setup_attempt();
            return Err(telepathy_audio::devices::DeviceError::NoOutputDevice);
        }
        self.input_error_probe.capture(error_callback);
        Ok((MockAudioInput::default(), DEFAULT_SAMPLE_RATE, ()))
    }

    fn open_output(
        &self,
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<
        (impl AudioOutput + Send + 'static, u32, Self::OutputStream),
        telepathy_audio::devices::DeviceError,
    > {
        self.device_selection_probe
            .record(DeviceSelectionOperation::OpenOutput, device_id);
        if device_id == Some(STALE_OUTPUT_DEVICE_ID) {
            self.output_error_probe.signal_setup_attempt();
            return Err(DeviceError::DeviceNotFound {
                direction: DeviceDirection::Output,
                id: STALE_OUTPUT_DEVICE_ID.to_string(),
            });
        }
        if self.fail_output_synchronously.load(Relaxed) {
            self.output_error_probe.signal_setup_attempt();
            return Err(telepathy_audio::devices::DeviceError::NoOutputDevice);
        }
        self.output_error_probe.capture(error_callback);
        Ok((MockAudioOutput, DEFAULT_SAMPLE_RATE, ()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RoomEventKind {
    Join,
    Leave,
}

#[derive(Debug, Clone, Default)]
pub(super) struct PendingAcceptProbe {
    pub(super) opened: Arc<AtomicUsize>,
    pub(super) cancelled: Arc<AtomicUsize>,
    pub(super) opened_notify: Arc<Notify>,
    pub(super) cancelled_notify: Arc<Notify>,
}

/// How many manager lifecycle cycles the mock `manager_state` callback accepts.
/// `Single` pins to one activation (2 active/starting + 1 stopped);
/// `Restartable` accepts any number so `restart_manager()` tests don't trip
/// mockall's strict call-count assertion.
#[derive(Debug, Clone, Copy)]
pub(super) enum ManagerLifecycle {
    Single,
    Restartable,
}

impl PendingAcceptProbe {
    pub(super) async fn wait_opened(&self) {
        wait_for_counter(&self.opened, &self.opened_notify, 1, "accept prompt opened").await;
    }

    pub(super) async fn wait_cancelled(&self) {
        wait_for_counter(
            &self.cancelled,
            &self.cancelled_notify,
            1,
            "accept prompt cancelled",
        )
        .await;
    }
}

/// In-process `MemoryLookup` registers each peer's `addr()` after bind so the
/// dial resolves without reaching the n0 PKARR relay. Regression: lookup
/// silently fails and dial hangs until `HELLO_TIMEOUT`.

/// Regression: a second `start_call` while a first outgoing dial is still pending must
/// be idempotent — no extra notify, no queued permit that re-enters
/// `negotiate_outgoing_call` after teardown, no phantom `Idle` flip.

/// Terminal teardown via `shutdown` -> `reset_sessions` must clear a pending
/// `PendingOutgoing` slot. Per-session `release_pending` no-ops on the empty
/// post-drain map; deterministic `clear_pending_direct` is the line of defense.

/// Mirrors `reset_sessions_clears_pending_outgoing_slot` for `PendingIncoming`.
/// Block the accept prompt via `PendingAcceptProbe`, then `shutdown` Bob before
/// it resolves. `reset_sessions` must clear the slot even though per-session
/// `release_pending` no-ops on the empty post-drain map.

/// Terminal reset clears a real public outgoing call while its callee remains
/// blocked on the acceptance prompt.

/// Full `restart_manager()` flow: slot ends `Idle`, a fresh session is registered
/// for the known contact, and a subsequent `start_call()` acquires a fresh
/// `PendingOutgoing` slot — not stuck in any pre-restart pending state.

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

/// Synchronously-failing `setup_output` (e.g. another process holds the exclusive
/// output device) must surface a single `CallState::CallEnded` to the dialer with
/// `CALL_END_AUDIO_DEVICE_FAILURE` copy and `remote == false`, so the frontend can
/// exit the connecting state.

/// Happy-path contrast for `setup_output_synchronous_failure_emits_call_ended`:
/// when the dialer's output device opens successfully, the same host still
/// produces `Connected`. Guards against an over-eager fix short-circuiting
/// `call_handshake`.

/// Happy-path baseline for the room-generation token: both clients `join_room`,
/// each side emits `Connected` and exactly one `RoomJoin` for the peer, slot is
/// `RoomCall` on both, and `RoomState.generation` is bumped. Locks in the
/// `room_owner`/`room_generation` invariants the controller enforces at teardown.

/// Regression for the `end_call` -> `join_room` cycle (R1): the post-rejoin must
/// produce a *second* `RoomJoin` (not be lost to stale `room_state` carry-over)
/// and must not emit a spurious `RoomLeave` after it — the failure mode in the
/// system-test artifact `test_room_end_releases_call_slot_for_rejoin`.

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

/// Slot-contention regression: a second `join_room` while the slot is `RoomCall`
/// must return `Err(CallAlreadyActive)`; after `end_call` a fresh `join_room`
/// re-acquires `RoomCall` and bumps the generation.

/// Synchronous output setup failure after room peers join must remove the
/// installed `RoomState` and release its `RoomCall` slot.

pub(super) async fn normal_call_stream_error_surfaces_local_message(trigger_input: bool) {
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

pub(super) async fn room_stream_error_surfaces_local_message(trigger_input: bool) {
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
        [RoomEventKind::Join, RoomEventKind::Leave],
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

pub(super) async fn room_stream_error_sends_audio_error_goodbye_on_control_stream(
    trigger_input: bool,
) {
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

/// Lock the `"{nickname} did not respond to the call"` copy the `HELLO_TIMEOUT`
/// arm of `negotiate_outgoing_call` emits. Positive end-to-end would require a
/// 10-second wait; we pin the formatter (the single source of the timeout copy).

/// Lock the natural peer-facing sentences produced by `peer_goodbye_reason_message`
/// for every `GoodbyeReason` variant. Pinning the formatter (the single source)
/// catches regressions that re-introduce raw wire wording.

/// A peer-driven normal hangup must reach the frontend as a *silent* `CallEnded`
/// so the dialog guard (`state.field0.isNotEmpty` in `lib/main.dart`) suppresses
/// the failure toast and the silent hangup tone plays instead.

/// Lock the `CallEndMessage::from_error` mapping for `SessionStopped`. A
/// `SessionStopped` error is hard to drive end-to-end (requires racing slot
/// release with `transition_pending_to_active`); pin the mapping at the helper
/// boundary. Audio-stream integration tests cover the production emission paths.

/// Lock the `CallEndMessage::from_error` mapping for generic non-audio,
/// non-session-stopped, non-timeout errors: must collapse to
/// `"The call ended unexpectedly"` regardless of which `ErrorKind` triggered it.

/// Catch-all: the *frontend* copy for every backend failure is a closed set of
/// user-facing sentences. Any internal wording that bleeds into `CallState::CallEnded`
/// is a regression.

pub(super) fn init_test_tracing() {
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

pub(super) fn shared_relay_map() -> &'static RelayMap {
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
pub(super) fn shared_address_lookup() -> &'static MemoryLookup {
    let _ = shared_relay_map();
    SHARED_ADDRESS_LOOKUP
        .get()
        .expect("shared_address_lookup called before shared_relay_map initialisation")
}

pub(super) async fn build_raw_room_endpoint(relay_map: &RelayMap, identity: SecretKey) -> Endpoint {
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

pub(super) async fn accept_raw_room_peer(endpoint: Endpoint) -> RawRoomPeer {
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

pub(super) async fn write_wire_message(
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

pub(super) async fn read_wire_message(
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
pub(super) async fn read_wire_message_skipping_keepalives(
    transport: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
) -> WireProtocolMessage {
    loop {
        let message = read_wire_message(transport).await;
        if !matches!(message, WireProtocolMessage::KeepAlive) {
            return message;
        }
    }
}

pub(super) async fn build_client<H, I, O>(
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

pub(super) async fn build_client_with_accept_probe<H, I, O>(
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_client_with_options<H, I, O>(
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
        telepathy,
        is_active,
    }
}

/// Returns mock callbacks that establish a telepathy instance with the provided
/// contacts. `is_active` flips to true on the first session-connected event.
/// `lifecycle` controls how many `manager_state` activations the mock accepts
/// (see `ManagerLifecycle`).
pub(super) fn construct_mock_callbacks(
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

pub(super) fn room_join_count(states: &[CallState], peer: &str) -> usize {
    states
        .iter()
        .filter(|state| matches!(state, CallState::RoomJoin(id) if id == peer))
        .count()
}

pub(super) fn room_leave_count(states: &[CallState], peer: &str) -> usize {
    states
        .iter()
        .filter(|state| matches!(state, CallState::RoomLeave(id) if id == peer))
        .count()
}

pub(super) async fn wait_for_room_join_count(
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

pub(super) fn sorted_room_members(a: &Contact, b: &Contact) -> Vec<String> {
    let mut members = vec![a.get_peer_id().to_string(), b.get_peer_id().to_string()];
    members.sort();
    members
}

pub(super) fn call_state_snapshot(call_states: &Arc<Mutex<Vec<CallState>>>) -> Vec<CallState> {
    call_states.lock().unwrap().clone()
}

pub(super) fn simulated_stream_error(message: &'static str) -> CpalError {
    CpalError::with_message(CpalErrorKind::DeviceNotAvailable, message)
}

pub(super) fn stream_error_scenario<'a>(
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

pub(super) fn assert_call_slot_idle<H, I, O>(client: &ClientHarness<H, I, O>, message: &str)
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

pub(super) async fn wait_for_call_ended_contains(
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

pub(super) fn assert_no_call_ended_contains(
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

pub(super) async fn wait_for_counter(
    counter: &AtomicUsize,
    notify: &Notify,
    expected: usize,
    label: &str,
) {
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

pub(super) async fn wait_for_connected(call_states: &Arc<Mutex<Vec<CallState>>>, label: &str) {
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
pub(super) async fn wait_for_active_transport<H, I, O>(client: &ClientHarness<H, I, O>, label: &str)
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

pub(super) fn assert_no_busy_end(states: &[CallState], label: &str) {
    assert!(
        !states.iter().any(|state| matches!(
            state,
            CallState::CallEnded(reason, true) if reason == "A call is already active"
        )),
        "{label} observed busy call end: {states:?}"
    );
}

pub(super) fn assert_no_call_ended_before_connected(states: &[CallState], label: &str) {
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

pub(super) fn room_event_sequence(states: &[CallState], peer: &str) -> Vec<RoomEventKind> {
    states
        .iter()
        .filter_map(|state| match state {
            CallState::RoomJoin(id) if id == peer => Some(RoomEventKind::Join),
            CallState::RoomLeave(id) if id == peer => Some(RoomEventKind::Leave),
            _ => None,
        })
        .collect()
}

pub(super) fn assert_room_event_sequence(
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

pub(super) async fn wait_for_room_leave_count(
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

pub(super) async fn wait_for_no_extra_room_leave(
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

pub(super) async fn wait_for_sessions<HA, IA, OA, HB, IB, OB>(
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
pub(super) async fn wait_for_stable_session_pair<HA, IA, OA, HB, IB, OB>(
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

pub(super) async fn wait_for_slot_idle<H, I, O>(client: &ClientHarness<H, I, O>, peer: &str)
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
pub(super) async fn assert_slot_remains_outside_direct_call_states<H, I, O>(
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
pub(super) async fn wait_for_slot_room_call<H, I, O>(client: &ClientHarness<H, I, O>, label: &str)
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
pub(super) async fn wait_for_slot_owned_by<H, I, O>(
    client: &ClientHarness<H, I, O>,
    peer: &PublicKey,
) where
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
pub(super) struct TwoClientShutdownGuard<
    'a,
    HA: AudioHost<InputStream = IA, OutputStream = OA> + Send + Sync + Clone + 'static,
    IA: Send + Sync + 'static,
    OA: Send + Sync + 'static,
    HB: AudioHost<InputStream = IB, OutputStream = OB> + Send + Sync + Clone + 'static,
    IB: Send + Sync + 'static,
    OB: Send + Sync + 'static,
> {
    pub(super) a: &'a ClientHarness<HA, IA, OA>,
    pub(super) b: &'a ClientHarness<HB, IB, OB>,
    pub(super) dropped: AtomicBool,
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
    pub(super) fn disarm(&self) {
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
pub(super) struct RawPeerShutdownGuard<'a, H, I, O>
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
    pub(super) fn disarm(&self) {
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
