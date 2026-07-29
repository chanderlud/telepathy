use super::common::{
    ContactLookupGate, DEFAULT_SAMPLE_RATE, ManagerLifecycle, PendingAcceptProbe,
    TwoClientShutdownGuard, assert_no_busy_end, assert_no_call_ended_before_connected,
    build_client, build_client_with_accept_probe, build_client_with_contact_lookup_gate,
    build_client_with_lookup_contacts, call_state_snapshot, init_test_tracing,
    shared_address_lookup, shared_relay_map, wait_for_active_transport, wait_for_connected,
    wait_for_sessions,
};

use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::state::SessionState;
use telepathy_core::types::Contact;
use telepathy_core::types::{CallState, CodecConfig, SessionStatus};
use tokio::time::{sleep, timeout};
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
async fn profile_switch_cancels_dial_blocked_after_connect_and_refreshes_invitation() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("switch-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("switch-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let gate = ContactLookupGate::new(contact_b.get_peer_id().to_vec(), 2);

    let client_a = build_client_with_contact_lookup_gate(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        Vec::new(),
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
        ManagerLifecycle::Restartable,
        gate.clone(),
    )
    .await;
    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a],
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
    let initial_invitation = client_a
        .telepathy
        .node_addr()
        .await
        .expect("active manager should expose an invitation");

    client_a.telepathy.start_session(&contact_b).await;
    gate.wait_blocked().await;

    let prepared = client_a
        .telepathy
        .prepare_identity_switch(SecretKey::generate().to_bytes(), Vec::new())
        .await
        .expect("identity switch should prepare");
    timeout(Duration::from_secs(5), prepared.commit())
        .await
        .expect("profile switch must cancel the delayed old dial")
        .expect("profile switch should succeed");

    assert!(
        client_a
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .is_empty(),
        "the canceled old connection must not initialize a stale session"
    );
    let restarted_invitation = client_a
        .telepathy
        .node_addr()
        .await
        .expect("restarted manager should expose its current invitation");
    assert_ne!(
        restarted_invitation, initial_invitation,
        "the restarted invitation must use the switched identity"
    );
    timeout(Duration::from_secs(5), client_a.telepathy.restart_manager())
        .await
        .expect("explicit manager restart should be bounded")
        .expect("explicit manager restart should succeed");
    assert!(
        client_a.telepathy.node_addr().await.is_some(),
        "active manager should expose its current invitation after restart"
    );

    gate.release();
    client_b.telepathy.shutdown().await;
    timeout(Duration::from_secs(5), client_a.telepathy.shutdown())
        .await
        .expect("shutdown must not wait for the canceled dial");
    assert_eq!(
        client_a.telepathy.node_addr().await,
        None,
        "inactive manager must not expose a direct invitation"
    );
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
    let connected_before_replacement = client_a
        .session_status_probe
        .connected_count(peer_b.as_bytes());

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
    client_a
        .session_status_probe
        .wait_for_connected_after(peer_b.as_bytes(), connected_before_replacement)
        .await;

    let promoted_id = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_b)
        .map(|state| state.id())
        .expect("client_a should promote the retained candidate");
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
