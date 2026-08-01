use iroh::address_lookup::memory::MemoryLookup;
use iroh::{PublicKey, RelayMap, SecretKey};
use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Condvar, Mutex, Once, OnceLock};
use std::thread;
use std::time::Duration;
use telepathy_audio::devices::{AudioHost, DeviceDirection, DeviceError};
use telepathy_audio::devices::{MockAudioInput, MockAudioOutput};
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
use tokio::sync::{Notify, watch};
use tokio::time::{interval, sleep};
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

pub(super) type MockTelepathyHandle<H> = TelepathyHandle<MockCoreCallbacks, H>;

pub(super) struct ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
{
    pub(super) telepathy: MockTelepathyHandle<H>,
    pub(super) is_active: Arc<AtomicBool>,
    pub(super) contact_lookup_probe: ContactLookupProbe,
    pub(super) session_status_probe: SessionStatusProbe,
}

impl<H> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
{
    pub(super) async fn stop_session_and_wait_for_runtime(&self, contact: &Contact) {
        let runtime_applied = self.telepathy.inner.core_state.manager_active.notified();
        tokio::pin!(runtime_applied);
        runtime_applied.as_mut().enable();
        self.telepathy.stop_session(contact).await;
        runtime_applied.await;
    }
}

#[derive(Clone, Default)]
pub(super) struct ContactLookupProbe {
    counts: Arc<Mutex<HashMap<Vec<u8>, usize>>>,
    changed: Arc<Notify>,
}

impl ContactLookupProbe {
    fn record(&self, peer_id: &[u8]) {
        *self
            .counts
            .lock()
            .unwrap()
            .entry(peer_id.to_vec())
            .or_default() += 1;
        self.changed.notify_waiters();
    }

    pub(super) async fn wait_for(&self, peer_id: &[u8], expected: usize) {
        let wait = async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self
                    .counts
                    .lock()
                    .unwrap()
                    .get(peer_id)
                    .copied()
                    .unwrap_or_default()
                    >= expected
                {
                    return;
                }
                changed.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .is_err()
        {
            let observed = self
                .counts
                .lock()
                .unwrap()
                .get(peer_id)
                .copied()
                .unwrap_or_default();
            panic!(
                "timed out waiting for {expected} contact lookups for {peer_id:?}, got {observed}"
            );
        }
    }

    pub(super) fn count(&self, peer_id: &[u8]) -> usize {
        self.counts
            .lock()
            .unwrap()
            .get(peer_id)
            .copied()
            .unwrap_or_default()
    }
}

#[derive(Clone)]
pub(super) struct SessionStatusProbe {
    statuses: Arc<Mutex<HashMap<Vec<u8>, SessionStatus>>>,
    connected_counts: Arc<Mutex<HashMap<Vec<u8>, usize>>>,
    changed: Arc<Notify>,
    park_connecting: Arc<AtomicBool>,
    connecting_released: watch::Sender<bool>,
}

impl Default for SessionStatusProbe {
    fn default() -> Self {
        let (connecting_released, _) = watch::channel(false);
        Self {
            statuses: Arc::default(),
            connected_counts: Arc::default(),
            changed: Arc::new(Notify::new()),
            park_connecting: Arc::new(AtomicBool::new(false)),
            connecting_released,
        }
    }
}

impl SessionStatusProbe {
    fn record(&self, peer_id: &[u8], status: SessionStatus) -> bool {
        let is_connecting = matches!(status, SessionStatus::Connecting);
        if matches!(status, SessionStatus::Connected { .. }) {
            *self
                .connected_counts
                .lock()
                .unwrap()
                .entry(peer_id.to_vec())
                .or_default() += 1;
        }
        self.statuses
            .lock()
            .unwrap()
            .insert(peer_id.to_vec(), status);
        self.changed.notify_waiters();
        is_connecting && self.park_connecting.load(Relaxed)
    }

    pub(super) fn connected_count(&self, peer_id: &[u8]) -> usize {
        self.connected_counts
            .lock()
            .unwrap()
            .get(peer_id)
            .copied()
            .unwrap_or_default()
    }

    pub(super) async fn wait_for_connected_after(&self, peer_id: &[u8], previous: usize) {
        let wait = async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.connected_count(peer_id) > previous {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for a new Connected session status for {peer_id:?}; previous={previous}, current={}",
                    self.connected_count(peer_id)
                )
            });
    }

    pub(super) fn park_connecting(&self) {
        self.park_connecting.store(true, Relaxed);
        let _ = self.connecting_released.send(true);
    }

    pub(super) fn release_connecting(&self) {
        self.park_connecting.store(false, Relaxed);
        let _ = self.connecting_released.send(false);
    }

    async fn wait_for_connecting_release(&self) {
        let mut released = self.connecting_released.subscribe();
        loop {
            if !self.park_connecting.load(Relaxed) || !*released.borrow() {
                return;
            }
            if released.changed().await.is_err() {
                return;
            }
        }
    }

    pub(super) async fn wait_for(&self, peer_id: &[u8], expected: SessionStatus) {
        let wait = async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if let Some(status) = self.statuses.lock().unwrap().get(peer_id) {
                    if std::mem::discriminant(status) == std::mem::discriminant(&expected) {
                        return;
                    }
                }
                changed.await;
            }
        };
        if tokio::time::timeout(Duration::from_secs(60), wait)
            .await
            .is_err()
        {
            let observed = self.statuses.lock().unwrap().get(peer_id).cloned();
            panic!(
                "timed out waiting for {expected:?} session status for {peer_id:?}, got {observed:?}"
            );
        }
    }
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

/// Blocks `input_sample_rate` after recording its device selection. Tests use
/// this to close the control stream before a setup failure resumes.
#[derive(Clone, Default)]
pub(super) struct InputSampleRateGate {
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl InputSampleRateGate {
    pub(super) fn release(&self) {
        let (released, wake) = &*self.released;
        *released.lock().unwrap() = true;
        wake.notify_all();
    }

    fn wait(&self) {
        let (released, wake) = &*self.released;
        let mut released = released.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
    }
}

/// Blocks `open_output` after recording its device selection. Tests use this
/// to gate the production `setup_output` path through the host instead of
/// locking `CoreState::output_device`, which is `pub(crate)`.
#[derive(Clone, Default)]
pub(super) struct OutputOpenGate {
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl OutputOpenGate {
    pub(super) fn release(&self) {
        let (released, wake) = &*self.released;
        *released.lock().unwrap() = true;
        wake.notify_all();
    }

    fn wait(&self) {
        let (released, wake) = &*self.released;
        let mut released = released.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
    }
}

/// Parks the `Waiting` call-state observation inside the room controller's
/// `deliver_room_observation` so a test can drive teardown while the
/// controller is still mid-callback. Tests observe `wait_for_waiting` before
/// triggering `end_call`, then `release` only after teardown has settled.
#[derive(Clone, Default)]
pub(super) struct WaitingCallbackGate {
    released: Arc<AtomicBool>,
    parked: Arc<Notify>,
    saw_waiting: Arc<AtomicBool>,
    saw_notify: Arc<Notify>,
}

impl WaitingCallbackGate {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn release(&self) {
        self.released.store(true, Relaxed);
        self.parked.notify_one();
    }

    pub(super) async fn wait_for_waiting(&self) {
        if self.saw_waiting.load(Relaxed) {
            return;
        }
        self.saw_notify.notified().await;
    }

    async fn wait(&self) {
        if self.released.load(Relaxed) {
            return;
        }
        self.parked.notified().await;
    }

    fn mark_waiting(&self) {
        self.saw_waiting.store(true, Relaxed);
        self.saw_notify.notify_one();
    }
}

/// Parks the `Connected` call-state observation inside `call_controller`'s
/// `deliver_callback_against_teardown` so a test can drive `end_call` while the
/// controller is still mid-callback. Tests observe `wait_for_connected` before
/// triggering `end_call`, then `release` only after teardown has settled.
#[derive(Clone, Default)]
pub(super) struct ConnectedCallbackGate {
    released: Arc<AtomicBool>,
    parked: Arc<Notify>,
    saw_connected: Arc<AtomicBool>,
    saw_notify: Arc<Notify>,
}

impl ConnectedCallbackGate {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn release(&self) {
        self.released.store(true, Relaxed);
        self.parked.notify_one();
    }

    pub(super) async fn wait_for_connected(&self) {
        if self.saw_connected.load(Relaxed) {
            return;
        }
        self.saw_notify.notified().await;
    }

    async fn wait(&self) {
        if self.released.load(Relaxed) {
            return;
        }
        self.parked.notified().await;
    }

    fn mark_connected(&self) {
        self.saw_connected.store(true, Relaxed);
        self.saw_notify.notify_one();
    }
}

/// Parks the terminal `CallEnded` observation so a test can verify the call
/// slot is released while the Dart callback is still parked. Reproduces the
/// wedge bug: a stalled frontend must NOT keep backend ownership held.
#[derive(Clone, Default)]
pub(super) struct CallEndedPark {
    released: Arc<AtomicBool>,
    parked: Arc<Notify>,
    saw_call_ended: Arc<AtomicBool>,
    saw_notify: Arc<Notify>,
}

impl CallEndedPark {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn release(&self) {
        self.released.store(true, Relaxed);
        self.parked.notify_one();
    }

    pub(super) async fn wait_for_call_ended(&self) {
        if self.saw_call_ended.load(Relaxed) {
            return;
        }
        self.saw_notify.notified().await;
    }

    async fn wait(&self) {
        if self.released.load(Relaxed) {
            return;
        }
        self.parked.notified().await;
    }

    fn mark_call_ended(&self) {
        self.saw_call_ended.store(true, Relaxed);
        self.saw_notify.notify_one();
    }
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

#[derive(Debug, Clone)]
struct ControlledInput {
    inner: MockAudioInput,
    panic_on_read: Arc<AtomicBool>,
}

impl AudioInput for ControlledInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, telepathy_audio::Error> {
        assert!(
            !self.panic_on_read.swap(false, Relaxed),
            "simulated room input task panic"
        );
        self.inner.read_into(dst)
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
    pub(super) fail_input_immediately: Arc<AtomicBool>,
    pub(super) panic_input: Arc<AtomicBool>,
    input_sample_rate_gate: Option<InputSampleRateGate>,
    output_open_gate: Option<OutputOpenGate>,
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
            fail_input_immediately: Arc::new(AtomicBool::new(false)),
            panic_input: Arc::new(AtomicBool::new(false)),
            input_sample_rate_gate: None,
            output_open_gate: None,
        }
    }

    pub(super) fn with_device_selection_probe(mut self, probe: DeviceSelectionProbe) -> Self {
        self.device_selection_probe = probe;
        self
    }

    pub(super) fn with_input_sample_rate_gate(mut self, gate: InputSampleRateGate) -> Self {
        self.input_sample_rate_gate = Some(gate);
        self
    }

    pub(super) fn with_output_open_gate(mut self, gate: OutputOpenGate) -> Self {
        self.output_open_gate = Some(gate);
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
        if let Some(gate) = &self.input_sample_rate_gate {
            gate.wait();
        }
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
        if self.fail_input_immediately.load(Relaxed) {
            let mut callback = error_callback.expect("input error callback should be installed");
            callback(simulated_stream_error("input unavailable during open"));
        } else {
            self.input_error_probe.capture(error_callback);
        }
        Ok((
            ControlledInput {
                inner: MockAudioInput::default(),
                panic_on_read: self.panic_input.clone(),
            },
            DEFAULT_SAMPLE_RATE,
            (),
        ))
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
        if let Some(gate) = &self.output_open_gate {
            gate.wait();
        }
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
/// `Single` pins to one activation; `RevisionCycles` pins manager retries caused
/// by desired-runtime revision changes;
/// `Restartable` accepts any number so `restart_manager()` tests don't trip
/// mockall's strict call-count assertion.
#[derive(Debug, Clone)]
pub(super) enum ManagerLifecycle {
    Single,
    RevisionCycles(usize),
    Restartable,
    StartingGate(ManagerStartingGate),
    ActiveGate(ManagerActiveGate),
}

#[derive(Debug, Clone, Default)]
pub(super) struct ManagerStartingGate {
    started: Arc<AtomicBool>,
    started_notify: Arc<Notify>,
    released: Arc<AtomicBool>,
    released_notify: Arc<Notify>,
}

impl ManagerStartingGate {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) async fn wait_started(&self) {
        let notified = self.started_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.started.load(Relaxed) {
            return;
        }
        notified.await;
    }

    pub(super) fn release(&self) {
        self.released.store(true, Relaxed);
        self.released_notify.notify_waiters();
    }

    async fn wait_released(&self) {
        let notified = self.released_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.released.load(Relaxed) {
            return;
        }
        self.started.store(true, Relaxed);
        self.started_notify.notify_waiters();
        notified.await;
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ManagerActiveGate {
    active: Arc<AtomicBool>,
    active_notify: Arc<Notify>,
    released: Arc<AtomicBool>,
    released_notify: Arc<Notify>,
}

impl ManagerActiveGate {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) async fn wait_active(&self) {
        let notified = self.active_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.active.load(Relaxed) {
            return;
        }
        notified.await;
    }

    pub(super) fn release(&self) {
        self.released.store(true, Relaxed);
        self.released_notify.notify_waiters();
    }

    async fn wait_released(&self) {
        let notified = self.released_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.released.load(Relaxed) {
            return;
        }
        self.active.store(true, Relaxed);
        self.active_notify.notify_waiters();
        notified.await;
    }
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

pub(super) async fn build_client<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
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

pub(super) async fn build_client_with_lookup_contacts<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
{
    build_client_with_options_and_initial_contacts(
        relay_map,
        identity,
        contacts,
        vec![],
        codec_config,
        host,
        call_states,
        None,
        ManagerLifecycle::Single,
    )
    .await
}

pub(super) async fn build_client_with_accept_probe<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: PendingAcceptProbe,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
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

/// Like `build_client`, but parks the `Waiting` call-state observation on
/// `waiting_gate` until the test releases it. Used to reproduce the local
/// hangup race where `end_call` arrives while the controller is still
/// mid-Waiting.
#[allow(clippy::too_many_arguments)]
pub(super) async fn build_client_with_waiting_gate<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    waiting_gate: WaitingCallbackGate,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
    let session_status_probe = SessionStatusProbe::default();
    let mock = construct_mock_callbacks(
        contacts,
        is_active.clone(),
        is_relayed.clone(),
        call_states,
        None,
        ManagerLifecycle::Single,
        Some(waiting_gate),
        None,
        None,
        Some(session_status_probe.clone()),
    );

    let mut telepathy: MockTelepathyHandle<H> = TelepathyHandle::new(
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
        contact_lookup_probe: Default::default(),
        session_status_probe,
    }
}

/// Like `build_client`, but parks the `Connected` call-state observation on
/// `connected_gate` until the test releases it. Used to reproduce the
/// direct-call deadlock where `end_call` arrives while the controller is still
/// mid-`Connected` delivery.
#[allow(clippy::too_many_arguments)]
pub(super) async fn build_client_with_connected_gate<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    connected_gate: ConnectedCallbackGate,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
    let session_status_probe = SessionStatusProbe::default();
    let mock = construct_mock_callbacks(
        contacts,
        is_active.clone(),
        is_relayed.clone(),
        call_states,
        None,
        ManagerLifecycle::Single,
        None,
        Some(connected_gate),
        None,
        Some(session_status_probe.clone()),
    );

    let mut telepathy: MockTelepathyHandle<H> = TelepathyHandle::new(
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
        contact_lookup_probe: Default::default(),
        session_status_probe,
    }
}

/// Like `build_client`, but parks the terminal `CallEnded` call-state
/// observation on `call_ended_park` until the test releases it. Used to
/// verify the call slot and room state are released while the frontend
/// callback is still parked.
#[allow(clippy::too_many_arguments)]
pub(super) async fn build_client_with_call_ended_park<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    call_ended_park: CallEndedPark,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
    let session_status_probe = SessionStatusProbe::default();
    let mock = construct_mock_callbacks(
        contacts,
        is_active.clone(),
        is_relayed.clone(),
        call_states,
        None,
        ManagerLifecycle::Single,
        None,
        None,
        Some(call_ended_park),
        Some(session_status_probe.clone()),
    );

    let mut telepathy: MockTelepathyHandle<H> = TelepathyHandle::new(
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
        contact_lookup_probe: Default::default(),
        session_status_probe,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_client_with_options<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
{
    build_client_with_options_and_initial_contacts(
        relay_map,
        identity,
        contacts.clone(),
        contacts,
        codec_config,
        host,
        call_states,
        accept_probe,
        lifecycle,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_client_with_options_and_initial_contacts<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    initial_contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
    let contact_lookup_probe = ContactLookupProbe::default();
    let session_status_probe = SessionStatusProbe::default();
    let mock = construct_mock_callbacks_with_contact_lookup(
        contacts,
        initial_contacts,
        is_active.clone(),
        is_relayed.clone(),
        call_states,
        accept_probe,
        lifecycle,
        None,
        None,
        None,
        Some(contact_lookup_probe.clone()),
        Some(session_status_probe.clone()),
    );

    let mut telepathy: MockTelepathyHandle<H> = TelepathyHandle::new(
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
        contact_lookup_probe,
        session_status_probe,
    }
}

/// Returns mock callbacks that establish a telepathy instance with the provided
/// contacts. `is_active` flips to true on the first session-connected event.
/// `lifecycle` controls how many `manager_state` activations the mock accepts
/// (see `ManagerLifecycle`). `waiting_gate`, `connected_gate`, and
/// `call_ended_park`, when set, park the `Waiting`, `Connected`, and
/// `CallEnded` call-state observations respectively until the test releases
/// them.
#[allow(clippy::too_many_arguments)]
pub(super) fn construct_mock_callbacks(
    contacts: Vec<Contact>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
    waiting_gate: Option<WaitingCallbackGate>,
    connected_gate: Option<ConnectedCallbackGate>,
    call_ended_park: Option<CallEndedPark>,
    session_status_probe: Option<SessionStatusProbe>,
) -> MockCoreCallbacks {
    let initial_contacts = contacts.clone();
    construct_mock_callbacks_with_contact_lookup(
        contacts,
        initial_contacts,
        is_active,
        is_relayed,
        call_states,
        accept_probe,
        lifecycle,
        waiting_gate,
        connected_gate,
        call_ended_park,
        None,
        session_status_probe,
    )
}

#[allow(clippy::too_many_arguments)]
fn construct_mock_callbacks_with_contact_lookup(
    contacts: Vec<Contact>,
    initial_contacts: Vec<Contact>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
    call_states: Arc<Mutex<Vec<CallState>>>,
    accept_probe: Option<PendingAcceptProbe>,
    lifecycle: ManagerLifecycle,
    waiting_gate: Option<WaitingCallbackGate>,
    connected_gate: Option<ConnectedCallbackGate>,
    call_ended_park: Option<CallEndedPark>,
    contact_lookup_probe: Option<ContactLookupProbe>,
    session_status_probe: Option<SessionStatusProbe>,
) -> MockCoreCallbacks {
    let mut mock = MockCoreCallbacks::new();

    mock.expect_session_status()
        .returning(move |status, _peer| {
            info!("session status got called {status:?} {_peer}");
            let park_connecting = session_status_probe
                .as_ref()
                .is_some_and(|probe| probe.record(_peer.as_bytes(), status.clone()));
            let session_status_probe = session_status_probe.clone();
            let is_active_clone = is_active.clone();
            let is_relayed_clone = is_relayed.clone();
            Box::pin(async move {
                if park_connecting && let Some(probe) = session_status_probe {
                    probe.wait_for_connecting_release().await;
                }
                if let SessionStatus::Connected { relayed, .. } = status {
                    is_active_clone.store(true, Relaxed);
                    is_relayed_clone.store(relayed, Relaxed);
                } else if matches!(status, SessionStatus::Inactive) {
                    is_active_clone.store(false, Relaxed);
                }
            })
        });

    match lifecycle {
        ManagerLifecycle::Single | ManagerLifecycle::RevisionCycles(_) => {
            let cycles = match lifecycle {
                ManagerLifecycle::Single => 1,
                ManagerLifecycle::RevisionCycles(cycles) => cycles,
                ManagerLifecycle::Restartable
                | ManagerLifecycle::StartingGate(_)
                | ManagerLifecycle::ActiveGate(_) => unreachable!(),
            };

            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Starting))
                .times(cycles)
                .returning(|_| Box::pin(async move {}));

            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Active))
                .times(cycles)
                .returning(|_| Box::pin(async move {}));

            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Stopped))
                .times(cycles)
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
        ManagerLifecycle::StartingGate(gate) => {
            let starting_gate = gate.clone();
            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Starting))
                .times(..)
                .returning(move |_| {
                    let gate = starting_gate.clone();
                    Box::pin(async move { gate.wait_released().await })
                });

            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Active | ManagerState::Stopped))
                .times(..)
                .returning(|_| Box::pin(async move {}));

            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Failed))
                .times(..)
                .returning(|_| Box::pin(async move {}));
        }
        ManagerLifecycle::ActiveGate(gate) => {
            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Starting))
                .times(..)
                .returning(|_| Box::pin(async move {}));

            let active_gate = gate.clone();
            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Active))
                .times(..)
                .returning(move |_| {
                    let gate = active_gate.clone();
                    Box::pin(async move { gate.wait_released().await })
                });

            mock.expect_manager_state()
                .withf(|state| matches!(state, ManagerState::Stopped | ManagerState::Failed))
                .times(..)
                .returning(|_| Box::pin(async move {}));
        }
    }

    let contacts_clone = initial_contacts.clone();
    mock.expect_get_contacts().returning(move || {
        let contacts_clone = contacts_clone.clone();
        Box::pin(async move { contacts_clone })
    });

    mock.expect_get_contact().returning(move |peer_id| {
        let contacts_clone = contacts.clone();
        let contact_lookup_probe = contact_lookup_probe.clone();
        Box::pin(async move {
            let contact = contacts_clone
                .iter()
                .find(|contact| contact.get_peer_id().as_bytes() == peer_id.as_slice())
                .cloned();
            if let Some(probe) = contact_lookup_probe {
                probe.record(&peer_id);
            }
            contact
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
        let call_states = call_states.clone();
        let waiting_gate = waiting_gate.clone();
        let connected_gate = connected_gate.clone();
        let call_ended_park = call_ended_park.clone();
        Box::pin(async move {
            if matches!(state, CallState::Waiting)
                && let Some(gate) = waiting_gate.as_ref()
            {
                gate.mark_waiting();
                gate.wait().await;
            }
            if matches!(state, CallState::Connected)
                && let Some(gate) = connected_gate.as_ref()
            {
                gate.mark_connected();
                gate.wait().await;
            }
            if matches!(state, CallState::CallEnded(_, _))
                && let Some(park) = call_ended_park.as_ref()
            {
                park.mark_call_ended();
                park.wait().await;
            }
            call_states.lock().unwrap().push(state);
        })
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

pub(super) fn assert_call_slot_idle<H>(client: &ClientHarness<H>, message: &str)
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
pub(super) async fn wait_for_active_transport<H>(client: &ClientHarness<H>, label: &str)
where
    H: AudioHost + Send + Sync + Clone + 'static,
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

pub(super) async fn wait_for_sessions<HA, HB>(
    a: &ClientHarness<HA>,
    a_peer: &Contact,
    b: &ClientHarness<HB>,
    b_peer: &Contact,
) where
    HA: AudioHost + Send + Sync + Clone + 'static,
    HB: AudioHost + Send + Sync + Clone + 'static,
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
pub(super) async fn wait_for_stable_session_pair<HA, HB>(
    a: &ClientHarness<HA>,
    a_peer: &PublicKey,
    b: &ClientHarness<HB>,
    b_peer: &PublicKey,
    require_a_id_change: Option<Uuid>,
) where
    HA: AudioHost + Send + Sync + Clone + 'static,
    HB: AudioHost + Send + Sync + Clone + 'static,
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

pub(super) async fn wait_for_slot_idle<H>(client: &ClientHarness<H>, peer: &str)
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
pub(super) async fn assert_slot_remains_outside_direct_call_states<H>(
    client: &ClientHarness<H>,
    peer: &PublicKey,
    label: &str,
    window: Duration,
) where
    H: AudioHost + Send + Sync + Clone + 'static,
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
pub(super) async fn wait_for_slot_room_call<H>(client: &ClientHarness<H>, label: &str)
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
pub(super) async fn wait_for_slot_owned_by<H>(client: &ClientHarness<H>, peer: &PublicKey)
where
    H: AudioHost + Send + Sync + Clone + 'static,
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
    HA: AudioHost + Send + Sync + Clone + 'static,
    HB: AudioHost + Send + Sync + Clone + 'static,
> {
    pub(super) a: &'a ClientHarness<HA>,
    pub(super) b: &'a ClientHarness<HB>,
    pub(super) dropped: AtomicBool,
}

impl<HA, HB> TwoClientShutdownGuard<'_, HA, HB>
where
    HA: AudioHost + Send + Sync + Clone + 'static,
    HB: AudioHost + Send + Sync + Clone + 'static,
{
    /// Marks the guard as already-handled so its `Drop` becomes a no-op. The
    /// success path calls this immediately before `drop(shutdown_guard)` so the
    /// explicit `shutdown` calls that follow are the only shutdowns that run;
    /// without it `Drop` would fire a redundant `shutdown` after each explicit call.
    pub(super) fn disarm(&self) {
        self.dropped.store(true, Relaxed);
    }
}

impl<HA, HB> Drop for TwoClientShutdownGuard<'_, HA, HB>
where
    HA: AudioHost + Send + Sync + Clone + 'static,
    HB: AudioHost + Send + Sync + Clone + 'static,
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
