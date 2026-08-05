use super::common::{
    DEFAULT_SAMPLE_RATE, PendingAcceptProbe, TwoClientShutdownGuard, assert_no_busy_end,
    assert_no_call_ended_before_connected, build_client, build_client_with_accept_probe,
    build_client_with_lookup_contacts, call_state_snapshot, init_test_tracing,
    shared_address_lookup, shared_relay_map, wait_for_active_transport, wait_for_connected,
    wait_for_sessions, wait_for_stable_session_pair,
};

use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::state::SessionState;
use telepathy_core::types::Contact;
use telepathy_core::types::{CallState, CodecConfig, SessionStatus};
use tokio::time::sleep;
use tracing::info;

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

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let client_a = build_client_with_lookup_contacts(
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

    let client_b = build_client_with_lookup_contacts(
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

    tokio::join!(
        client_a.telepathy.start_session(&contact_b),
        client_b.telepathy.start_session(&contact_a),
    );

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

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

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("client_a should start a call after simultaneous session dialing");
    wait_for_connected(&call_states_a, "simultaneous-dial client_a").await;
    wait_for_connected(&call_states_b, "simultaneous-dial client_b").await;
    client_a.telepathy.end_call().await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_predecessor_promotes_same_identity_replacement_and_allows_call() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let (key_a, key_b) = loop {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        if key_a.public() < key_b.public() {
            break (key_a, key_b);
        }
    };
    let replacement_key_b = key_b.clone();
    let contact_a = Contact::new(
        "replacement-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "replacement-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_old_b = Arc::new(Mutex::new(Vec::new()));
    let call_states_replacement_b = Arc::new(Mutex::new(Vec::new()));

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
    let old_client_b = build_client(
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
        call_states_old_b,
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    old_client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &old_client_b, &contact_a).await;
    wait_for_active_transport(&client_a, "client_a predecessor").await;
    wait_for_active_transport(&old_client_b, "old client_b").await;

    let predecessor_id = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_b)
        .map(|state| state.id())
        .expect("client_a should register the predecessor");

    let replacement_client_b = build_client_with_lookup_contacts(
        relay_map,
        replacement_key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_replacement_b.clone(),
    )
    .await;
    replacement_client_b
        .telepathy
        .start_session(&contact_a)
        .await;
    replacement_client_b
        .session_status_probe
        .wait_for(
            peer_a.as_bytes(),
            SessionStatus::Connected {
                relayed: false,
                remote_address: String::new(),
            },
        )
        .await;

    assert_eq!(
        client_a
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .get(&peer_b)
            .map(|state| state.id()),
        Some(predecessor_id),
        "the retained candidate must wait while the predecessor remains live"
    );

    old_client_b.telepathy.shutdown().await;

    // Poll the session map directly: the connection-level Connected status can
    // precede the retained candidate's promotion, so it cannot gate this assert.
    let promoted_id = {
        let promoted = async {
            loop {
                let current = client_a
                    .telepathy
                    .inner
                    .session_states
                    .read()
                    .await
                    .get(&peer_b)
                    .map(|state| state.id());
                if current != Some(predecessor_id) {
                    return current.expect("client_a should promote the retained candidate");
                }
                sleep(Duration::from_millis(25)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), promoted)
            .await
            .unwrap_or_else(|_| {
                panic!("the completed predecessor must be replaced by the fresh candidate")
            })
    };
    assert_ne!(
        promoted_id, predecessor_id,
        "the completed predecessor must be replaced by the fresh candidate"
    );

    replacement_client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("the promoted session should carry a new call");
    wait_for_connected(&call_states_replacement_b, "replacement client_b").await;
    wait_for_connected(&call_states_a, "client_a after handoff").await;

    replacement_client_b.telepathy.end_call().await;
    replacement_client_b.telepathy.shutdown().await;
    client_a.telepathy.shutdown().await;
}

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

/// Reproduces the system-test failure
/// `test_call_drain_audio_frame_indices_strictly_increasing[iter-1-clean]`
/// (workflow run 30717774611, sweep 0, seed 30717774611-0): both sides issued
/// `start_session` simultaneously; Alice's dial installed a listener session
/// on Bob and her `Hello` opened Bob's accept prompt on it, then Bob's slower
/// outbound dial completed and `session_collision_kept_new` tore the listener
/// session down mid-prompt. Bob emitted `accept_call_canceled`, so the
/// subsequent `accept_call` was rejected with "unknown accept_call
/// request_id" and the in-flight call offer was lost.
///
/// Deterministic ordering via Bob's `SessionStatusProbe`:
/// 1. Bob's outbound dial is parked at its `Connecting` status emission,
///    which `open_session` awaits before connecting — the dial is in-flight
///    (past the `ignored_redundant_outgoing` guard) while no session exists.
/// 2. Alice's dial then installs the listener session on Bob and her `Hello`
///    opens Bob's accept prompt on that session.
/// 3. Releasing the park lets Bob's dial complete; collision resolution runs
///    while the prompt is still pending.
///
/// The prompt must survive collision resolution. On the broken code the
/// kept-new teardown cancels it, which is exactly the cancellation the system
/// test observed.
#[tokio::test(flavor = "multi_thread")]
async fn session_collision_kept_new_preserves_pending_accept_prompt() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    // Bob sorts before Alice so Bob's outbound (client) connection wins the
    // collision on Bob via `should_keep_new_session` — the same geometry as
    // the system-test failure (`connection.side.client=true` in the
    // `session_collision_kept_new` log line).
    let (key_a, key_b) = loop {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        if key_b.public() < key_a.public() {
            break (key_a, key_b);
        }
    };
    let contact_a = Contact::new(
        "prompt-collision-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "prompt-collision-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();

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

    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    // Park Bob's `Connecting` emission, then launch his outbound dial. The
    // manager spawns `open_session` (no session exists yet, so the dial is not
    // coalesced) and it blocks inside the status callback before connecting.
    client_b.session_status_probe.park_connecting();
    client_b.telepathy.start_session(&contact_a).await;
    client_b
        .session_status_probe
        .wait_for(peer_a.as_bytes(), SessionStatus::Connecting)
        .await;

    // Alice's dial installs the listener session on Bob while his own dial is
    // still parked — the glare window from the system test.
    client_a.telepathy.start_session(&contact_b).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Alice's Hello opens Bob's accept prompt on the listener session.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    accept_probe_b.wait_opened().await;
    assert_eq!(
        accept_probe_b.opened.load(Relaxed),
        1,
        "bob should have exactly one pending accept prompt before the collision"
    );

    // Let Bob's in-flight dial complete; collision resolution now runs while
    // the accept prompt is still pending on the listener session.
    client_b.session_status_probe.release_connecting();
    wait_for_stable_session_pair(&client_b, &peer_a, &client_a, &peer_b, None).await;

    // Grace window for a wrongful cancellation to surface, then assert the
    // prompt survived. On the broken code the kept-new teardown fires the
    // prompt's cancel token (`session_stopped_during_accept_prompt`) during
    // the swap above.
    sleep(Duration::from_secs(1)).await;
    assert_eq!(
        accept_probe_b.cancelled.load(Relaxed),
        0,
        "session collision must not cancel the pending accept prompt; \
         cancelling it drops the in-flight call offer — the system-test \
         failure mode where accept_call is rejected with \
         'unknown accept_call request_id'"
    );

    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Recreates the system-test strand (`test_session_simultaneous_dial_then_call`,
/// caller `CallEnded: "did not respond"` while the callee Connected): a call
/// Hello that arrives on a deferred same-identity candidate connection has no
/// reader — the candidate parks until the predecessor session finishes — so the
/// caller waits out its full HelloAck timeout even though the callee is right
/// there holding an active call with the same identity. The correct behavior
/// is a prompt terminal answer (Busy) from the parked candidate.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "recreation for the deferred-candidate starvation follow-up; fails until the parked candidate answers"]
async fn call_on_deferred_same_identity_candidate_starves_without_reader() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    // Alice sorts before Bob so Alice-dialed connections become deferred
    // candidates on Bob (`should_keep_new_session` keeps Bob's existing session).
    let (key_a, key_b) = loop {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        if key_a.public() < key_b.public() {
            break (key_a, key_b);
        }
    };
    let replacement_key_a = key_a.clone();
    let contact_a = Contact::new(
        "deferred-candidate-caller-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "deferred-candidate-callee-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_b = contact_b.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let call_states_replacement_a = Arc::new(Mutex::new(Vec::new()));
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

    // Establish the active call on the session Bob will keep.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the call");
    accept_probe_b.wait_opened().await;
    accept_probe_b.accept();
    wait_for_connected(&call_states_a, "alice original call").await;
    wait_for_connected(&call_states_b, "bob original call").await;

    // The same identity reconnects (new process instance); on Bob it defers
    // behind the session that owns the active call.
    let replacement_client_a = build_client(
        relay_map,
        replacement_key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        call_states_replacement_a.clone(),
    )
    .await;
    replacement_client_a
        .telepathy
        .start_session(&contact_b)
        .await;
    replacement_client_a
        .session_status_probe
        .wait_for(
            peer_b.as_bytes(),
            SessionStatus::Connected {
                relayed: false,
                remote_address: String::new(),
            },
        )
        .await;

    // A call on the deferred candidate connection must be answered, not
    // starved: the caller should terminalize promptly (Busy), not after the
    // full HelloAck timeout.
    let started = std::time::Instant::now();
    replacement_client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("replacement should start its call");
    let terminal = async {
        loop {
            let states = call_state_snapshot(&call_states_replacement_a);
            if states
                .iter()
                .any(|state| matches!(state, CallState::CallEnded(_, _)))
            {
                return states;
            }
            sleep(Duration::from_millis(50)).await;
        }
    };
    let states = tokio::time::timeout(Duration::from_secs(12), terminal)
        .await
        .unwrap_or_else(|_| panic!("replacement call should terminalize"));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a call on a deferred candidate must be answered promptly (Busy), not starve for the full HelloAck timeout; states={states:?}"
    );
    assert!(
        states.iter().any(|state| matches!(
            state,
            CallState::CallEnded(message, _) if message.contains("busy")
        )),
        "the deferred candidate should answer Busy while the predecessor owns an active call; states={states:?}"
    );

    // Bob's original call is unaffected.
    assert!(
        !call_state_snapshot(&call_states_b)
            .iter()
            .any(|state| matches!(state, CallState::CallEnded(_, _))),
        "bob's active call must survive the deferred candidate"
    );

    client_a.telepathy.end_call().await;
    replacement_client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_a.telepathy.shutdown().await;
}
