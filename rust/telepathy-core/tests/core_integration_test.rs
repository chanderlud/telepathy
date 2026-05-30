#![cfg(feature = "integration-testing")]

use iroh::{RelayMap, SecretKey};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread;
use std::time::Duration;
use telepathy_audio::devices::AudioHost;
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_audio::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::callbacks::{MockCoreCallbacks, MockCoreStatisticsCallback};
use telepathy_core::internal::core::TelepathyCore;
use telepathy_core::overlay::Overlay;
use telepathy_core::types::Contact;
use telepathy_core::types::{
    CodecConfig, ManagerState, NetworkConfig, ScreenshareConfig, SessionStatus,
};
use tokio::task::JoinHandle;
use tokio::time::{interval, sleep};
use tracing::info;
use tracing_subscriber::EnvFilter;

static TEST_TRACING_INIT: Once = Once::new();
static RELAY_INIT: Once = Once::new();
static RELAY_DETAILS: OnceLock<RelayMap> = OnceLock::new();

const SEQUENCED_STEP: f32 = 1.0 / 4096.0;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

struct ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
{
    telepathy:
        TelepathyCore<MockCoreCallbacks<MockCoreStatisticsCallback>, MockCoreStatisticsCallback, H>,
    handle: Option<JoinHandle<()>>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
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

fn init_test_tracing() {
    TEST_TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("telepathy_core=debug")),
            )
            .try_init();
    });
}

fn shared_relay_map() -> &'static RelayMap {
    RELAY_INIT.call_once(|| {
        tokio::spawn(async move {
            let server = iroh::test_utils::run_relay_server().await.unwrap();
            RELAY_DETAILS.get_or_init(|| server.0);
            // keep the relay server running forever
            sleep(Duration::from_secs(u64::MAX)).await;
        });
    });

    RELAY_DETAILS.wait()
}

async fn build_client<H>(
    relay_map: &RelayMap,
    identity: SecretKey,
    contacts: Vec<Contact>,
    codec_config: &CodecConfig,
    host: H,
) -> ClientHarness<H>
where
    H: AudioHost + Send + Sync + Clone + 'static,
{
    let network_config = NetworkConfig::mock(relay_map, None, None);
    let screenshare = ScreenshareConfig::default();
    let overlay = Overlay::default();

    let is_active = Arc::new(AtomicBool::new(false));
    let is_relayed = Arc::new(AtomicBool::new(false));
    let mock = construct_mock_callbacks(contacts, is_active.clone(), is_relayed.clone());

    let mut telepathy = TelepathyCore::new(
        host,
        &network_config,
        &screenshare,
        &overlay,
        codec_config,
        mock,
    );
    *telepathy.core_state.identity.write().await = Some(identity);
    let handle = telepathy.start_manager().await;
    telepathy.core_state.manager_active.notified().await;

    ClientHarness {
        telepathy,
        handle,
        is_active,
        is_relayed,
    }
}

async fn wait_for_connected(a_is_active: &AtomicBool, b_is_active: &AtomicBool) {
    let mut poll = interval(Duration::from_millis(100));
    loop {
        poll.tick().await;
        let a_active = a_is_active.load(Relaxed);
        let b_active = b_is_active.load(Relaxed);

        if a_active && b_active {
            info!("both clients got connected");
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
        MockAudioHost::new(MockAudioInput::default(), MockAudioOutput),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(MockAudioInput::default(), MockAudioOutput),
    )
    .await;

    client_a
        .telepathy
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.get_peer_id())
        .await
        .unwrap();

    wait_for_connected(&client_a.is_active, &client_b.is_active).await;

    tokio::time::sleep(Duration::from_secs(1)).await;

    let b_session = client_a
        .telepathy
        .session_states
        .read()
        .await
        .get(&contact_b.get_peer_id())
        .cloned()
        .unwrap();
    let a_session = client_b
        .telepathy
        .session_states
        .read()
        .await
        .get(&contact_a.get_peer_id())
        .cloned()
        .unwrap();

    info!("session state a: {:?}", a_session);
    info!("session state b: {:?}", b_session);

    assert!(!client_a.is_relayed.load(Relaxed));
    assert!(!client_b.is_relayed.load(Relaxed));

    a_session.start_call.notify_one();

    tokio::time::sleep(Duration::from_secs(5)).await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_a.handle.unwrap().await.unwrap();
    client_b.handle.unwrap().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
        MockAudioHost::new(SequencedInput::new(DEFAULT_SAMPLE_RATE), MockAudioOutput),
    )
    .await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            RecordingOutput::new(playback_log.clone()),
        ),
    )
    .await;

    client_a
        .telepathy
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.get_peer_id())
        .await
        .unwrap();

    wait_for_connected(&client_a.is_active, &client_b.is_active).await;

    client_a.telepathy.core_state.set_input_volume(0.0);

    let b_session = client_a
        .telepathy
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
    client_a.handle.unwrap().await.unwrap();
    client_b.handle.unwrap().await.unwrap();

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

/// returns mock callbacks that will establish a telepathy instance with the provided contacts
/// sets is_active to true when the first session connected event is received
fn construct_mock_callbacks(
    contacts: Vec<Contact>,
    is_active: Arc<AtomicBool>,
    is_relayed: Arc<AtomicBool>,
) -> MockCoreCallbacks<MockCoreStatisticsCallback> {
    let mut mock: MockCoreCallbacks<MockCoreStatisticsCallback> = MockCoreCallbacks::new();

    // handle session status callbacks
    mock.expect_session_status().returning(move |status, peer| {
        info!("session status got called {status:?} {peer}");
        let is_active_clone = is_active.clone();
        let is_relayed_clone = is_relayed.clone();
        Box::pin(async move {
            match status {
                SessionStatus::Connected { relayed, .. } => {
                    is_active_clone.store(true, Relaxed);
                    is_relayed_clone.store(relayed, Relaxed);
                }
                _ => (),
            }
        })
    });

    // ensure manager activates
    mock.expect_manager_state()
        .withf(|a| matches!(a, ManagerState::Active | ManagerState::Starting))
        .times(2)
        .returning(|_| Box::pin(async move {}));

    // ensure manager deactivates
    mock.expect_manager_state()
        .withf(|a| matches!(a, ManagerState::Stopped))
        .once()
        .returning(|_| Box::pin(async move {}));

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

    // immediately accepts prompt for calls
    mock.expect_get_accept_handle().returning(move |_, _, _| {
        info!("accept call called");
        tokio::spawn(async move { true })
    });

    mock.expect_call_state().returning(|state| {
        info!("got call state: {state:?}");
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
