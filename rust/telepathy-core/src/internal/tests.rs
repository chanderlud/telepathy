use super::*;
use crate::internal::callbacks::{MockCoreCallbacks, MockCoreStatisticsCallback};
use crate::types::{ManagerState, SessionStatus};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Once;
use std::sync::atomic::AtomicBool;
use telepathy_audio::MockAudioHost;
use tokio::time::interval;
use tracing::info;
use tracing_subscriber::EnvFilter;

static TEST_TRACING_INIT: Once = Once::new();

impl Contact {
    fn mock(is_room_only: bool, nickname: &str) -> (Self, SecretKey) {
        let key = SecretKey::generate();
        let peer_id = key.public();
        (
            Self {
                id: peer_id.to_string(),
                nickname: nickname.to_string(),
                peer_id,
                is_room_only,
                output_volume: 0_f32,
            },
            key,
        )
    }
}

impl<C, S, H> TelepathyCore<C, S, H>
where
    S: CoreStatisticsCallback + Send + Sync + 'static,
    C: CoreCallbacks<S> + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    fn mock(callbacks: C, network_config: &NetworkConfig, codec_config: &CodecConfig) -> Self {
        let screenshare_config = ScreenshareConfig::default();
        let overlay = Overlay::default();

        Self::new(
            AudioHost::new(),
            network_config,
            &screenshare_config,
            &overlay,
            codec_config,
            callbacks,
        )
    }
}

#[tokio::test]
async fn mock_callbacks() {
    run_test().await;
}

async fn run_test() {
    TEST_TRACING_INIT.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("telepathy_core=debug")),
            )
            .try_init();
    });

    // craft network config for the test instance
    let network_config_a = NetworkConfig::mock(40143, vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);

    let network_config_b = NetworkConfig::mock(40144, vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);

    // default codec config
    let codec_config = CodecConfig::new(true, true, 5.0);

    // create contacts & identities
    let (contact_a, key_a) = Contact::mock(false, "client-a");
    let (contact_b, key_b) = Contact::mock(false, "client-b");

    // set up client a
    let a_is_active = Arc::new(AtomicBool::new(false));
    let a_is_relayed = Arc::new(AtomicBool::new(false));
    let mock_a = construct_mock_callbacks(
        vec![contact_b.clone()],
        a_is_active.clone(),
        a_is_relayed.clone(),
    );
    let mut telepathy_a: TelepathyCore<_, _, MockAudioHost> =
        TelepathyCore::mock(mock_a, &network_config_a, &codec_config);
    *telepathy_a.core_state.identity.write().await = Some(key_a);
    let handle_a = telepathy_a.start_manager().await;
    telepathy_a.core_state.manager_active.notified().await;

    // set up client b
    let b_is_active = Arc::new(AtomicBool::new(false));
    let b_is_relayed = Arc::new(AtomicBool::new(false));
    let mock_b = construct_mock_callbacks(
        vec![contact_a.clone()],
        b_is_active.clone(),
        b_is_relayed.clone(),
    );
    let mut telepathy_b: TelepathyCore<_, _, MockAudioHost> =
        TelepathyCore::mock(mock_b, &network_config_b, &codec_config);
    *telepathy_b.core_state.identity.write().await = Some(key_b);
    let handle_b = telepathy_b.start_manager().await;
    telepathy_b.core_state.manager_active.notified().await;

    // a starts session with b
    telepathy_a
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.peer_id)
        .await
        .unwrap();

    // b starts session with a
    telepathy_b
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_a.peer_id)
        .await
        .unwrap();

    // poll for the session status callback to become connected
    let mut interval = interval(Duration::from_millis(100));
    loop {
        interval.tick().await;
        let a_active = a_is_active.load(Relaxed);
        let b_active = b_is_active.load(Relaxed);

        if a_active && b_active {
            info!("both clients got connected");
            break;
        }
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    // grab session states for inspection
    let b_session = telepathy_a
        .session_states
        .read()
        .await
        .get(&contact_b.peer_id)
        .cloned()
        .unwrap();
    let a_session = telepathy_b
        .session_states
        .read()
        .await
        .get(&contact_a.peer_id)
        .cloned()
        .unwrap();

    info!("session state a: {:?}", a_session);
    info!("session state b: {:?}", b_session);

    // direct connections should have been established
    assert!(!a_is_relayed.load(Relaxed));
    assert!(!b_is_relayed.load(Relaxed));

    // a_session.start_call.notify_one();

    tokio::time::sleep(Duration::from_secs(5)).await;

    // ensure shutdown is a success
    telepathy_a.shutdown().await;
    telepathy_b.shutdown().await;
    handle_a.unwrap().await.unwrap();
    handle_b.unwrap().await.unwrap();
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
        .withf(|a| matches!(a, ManagerState::Active))
        .once()
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
                if contact.peer_id.to_vec() == peer_id {
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

    mock
}
