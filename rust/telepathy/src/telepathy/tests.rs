use super::*;
use crate::flutter::callbacks::{
    FrbStatisticsCallback, MockFrbCallbacks, MockFrbStatisticsCallback,
};
use fast_log::Config;
use kanal::{bounded, unbounded};
use log::{LevelFilter, info};
use relay_server::{RelayInfo, spawn_relay};
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::thread::{sleep, spawn};
use telepathy_audio::AudioHost;
use tokio::sync::OnceCell;
use tokio::time::interval;

static RELAY: OnceCell<RelayInfo> = OnceCell::const_new();
static PROFILES: &[NetProfile] = &[
    NetProfile {
        name: "clean",
        netem_args: &[],
    },
    NetProfile {
        name: "wifi_jitter",
        netem_args: &["delay", "40ms", "20ms", "loss", "0.2%"],
    },
    NetProfile {
        name: "cellular",
        netem_args: &["delay", "120ms", "40ms", "loss", "1%"],
    },
    NetProfile {
        name: "satellite-ish",
        netem_args: &["delay", "600ms", "80ms", "loss", "0.5%"],
    },
    NetProfile {
        name: "reorder",
        netem_args: &["delay", "80ms", "20ms", "reorder", "15%", "50%"],
    },
    NetProfile {
        name: "bursty_loss",
        netem_args: &["loss", "gemodel", "2%", "20%", "10%", "10%"],
    },
];

#[derive(Clone, Debug)]
struct NetProfile {
    name: &'static str,
    // tc netem args, like: ["delay","120ms","30ms","loss","5%"]
    netem_args: &'static [&'static str],
}

struct NetemGuard;

impl NetemGuard {
    fn apply(profile: &NetProfile, ports: &[u16]) -> std::io::Result<Self> {
        // Always start clean
        let _ = Command::new("tc")
            .args(["qdisc", "del", "dev", "lo", "root"])
            .status();

        if profile.netem_args.is_empty() {
            return Ok(Self);
        }

        // Root prio with 3 bands, netem attached to band 3
        cmd(&["qdisc", "add", "dev", "lo", "root", "handle", "1:", "prio"])?;
        cmd([
            "qdisc", "add", "dev", "lo", "parent", "1:3", "handle", "30:", "netem",
        ]
        .into_iter()
        .chain(profile.netem_args.iter().copied())
        .collect::<Vec<_>>()
        .as_slice())?;

        // Filters: UDP protocol (17) + dest port -> band 3
        for p in ports {
            cmd(&[
                "filter",
                "add",
                "dev",
                "lo",
                "protocol",
                "ip",
                "parent",
                "1:",
                "prio",
                "3",
                "u32",
                "match",
                "ip",
                "protocol",
                "17",
                "0xff",
                "match",
                "ip",
                "dport",
                &p.to_string(),
                "0xffff",
                "flowid",
                "1:3",
            ])?;
        }

        Ok(Self)
    }
}

impl Drop for NetemGuard {
    fn drop(&mut self) {
        let _ = Command::new("tc")
            .args(["qdisc", "del", "dev", "lo", "root"])
            .status();
    }
}

impl Contact {
    fn mock(is_room_only: bool, nickname: &str) -> (Self, Keypair) {
        let key = Keypair::generate_ed25519();
        let peer_id = key.public().to_peer_id();
        (
            Self {
                id: peer_id.to_string(),
                nickname: nickname.to_string(),
                peer_id,
                is_room_only,
            },
            key,
        )
    }
}

impl<C, S> TelepathyCore<C, S>
where
    S: FrbStatisticsCallback + Send + Sync + 'static,
    C: FrbCallbacks<S> + Send + Sync + 'static,
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
async fn mock_callbacks_test_network_matrix() {
    for profile in PROFILES {
        let _net = NetemGuard::apply(profile, &[40143, 40144]).expect("netem setup failed");
        if timeout(Duration::from_secs(5), run_test()).await.is_err() {
            panic!("Test timed out with profile {profile:?}");
        }
    }
}

#[tokio::test]
async fn mock_callbacks() {
    run_test().await;
}

async fn run_test() {
    fast_log::init(
        Config::new()
            .file("mock_callbacks.log")
            .level(LevelFilter::Debug),
    )
    .unwrap();

    // get local relay
    let relay: &RelayInfo = relay().await;

    // craft network config for the test instance
    let network_config_a = NetworkConfig::mock(
        "127.0.0.1:40142".parse().unwrap(),
        relay.peer_id,
        40143,
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
    );

    let network_config_b = NetworkConfig::mock(
        "127.0.0.1:40142".parse().unwrap(),
        relay.peer_id,
        40144,
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
    );

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
    let mut telepathy_a = TelepathyCore::mock(mock_a, &network_config_a, &codec_config);
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
    let mut telepathy_b = TelepathyCore::mock(mock_b, &network_config_b, &codec_config);
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
) -> MockFrbCallbacks<MockFrbStatisticsCallback> {
    let mut mock: MockFrbCallbacks<MockFrbStatisticsCallback> = MockFrbCallbacks::new();

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
    mock.expect_manager_active()
        .withf(|a, b| *a && *b)
        .once()
        .returning(|_, _| Box::pin(async move {}));

    // ensure manager deactivates
    mock.expect_manager_active()
        .withf(|a, b| !a && !b)
        .once()
        .returning(|_, _| Box::pin(async move {}));

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
                if contact.peer_id.to_bytes() == peer_id {
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

async fn relay() -> &'static RelayInfo {
    RELAY
        .get_or_init(|| async {
            // panic on error here is ok in tests
            spawn_relay(true).await.expect("failed to start relay")
        })
        .await
}

fn cmd(args: &[&str]) -> std::io::Result<()> {
    let st = Command::new("tc").args(args).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "tc failed"))
    }
}
