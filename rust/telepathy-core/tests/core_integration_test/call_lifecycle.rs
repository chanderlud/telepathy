use super::common::{
    CallEndedPark, ConnectedCallbackGate, DEFAULT_SAMPLE_RATE, ManagerLifecycle,
    PendingAcceptProbe, TwoClientShutdownGuard, assert_call_slot_idle, assert_no_busy_end,
    assert_no_call_ended_before_connected, build_client, build_client_with_accept_probe,
    build_client_with_call_ended_park, build_client_with_connected_gate, build_client_with_options,
    build_client_with_options_and_initial_contacts, call_state_snapshot, init_test_tracing,
    shared_relay_map, wait_for_active_transport, wait_for_connected, wait_for_sessions,
    wait_for_slot_idle, wait_for_slot_owned_by, wait_for_stable_session_pair,
};

use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::state::{CallSlotAcquireResult, CallSlotState};
use telepathy_core::types::Contact;
use telepathy_core::types::{CallState, CodecConfig, SessionStatus};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_start_call_before_acquisition_leaves_slot_idle() {
    init_test_tracing();
    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let contact = Contact::new(
        "cancelled peer".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");

    client
        .telepathy
        .start_call_with_operation(&contact, &cancelled)
        .await
        .expect("a cancelled operation is a successful no-op");

    assert_call_slot_idle(
        &client,
        "cancelling before direct-call acquisition must leave the slot idle",
    );
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn start_call_waits_for_trusted_session_attempt_and_cancellation_leaves_no_call() {
    init_test_tracing();
    let key_a = SecretKey::generate();
    let contact_b = Contact::new(
        "unavailable peer".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");
    let peer_b = contact_b.get_peer_id();
    let client = build_client(
        shared_relay_map(),
        key_a,
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client.telepathy.start_session(&contact_b).await;
    let operation = CancellationToken::new();
    let call = client
        .telepathy
        .start_call_with_operation(&contact_b, &operation);
    tokio::pin!(call);
    let first_poll = std::future::poll_fn(|context| Poll::Ready(call.as_mut().poll(context))).await;
    assert!(
        matches!(first_poll, Poll::Pending),
        "a trusted in-flight session attempt must make start_call wait"
    );

    client
        .session_status_probe
        .wait_for(peer_b.as_bytes(), SessionStatus::Connecting)
        .await;

    let second_poll =
        std::future::poll_fn(|context| Poll::Ready(call.as_mut().poll(context))).await;
    assert!(
        matches!(second_poll, Poll::Pending),
        "start_call must remain pending while the trusted session attempt is still in flight"
    );

    operation.cancel();
    call.await
        .expect("cancelling while waiting for a session should be a no-op");
    assert_call_slot_idle(
        &client,
        "cancelling while waiting for session availability must not claim a call slot",
    );
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_wakes_waiting_start_call_without_claiming_call_slot() {
    init_test_tracing();
    let key_a = SecretKey::generate();
    let contact_b = Contact::new(
        "shutdown peer".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");
    let peer_b = contact_b.get_peer_id();
    let client = build_client(
        shared_relay_map(),
        key_a,
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client.telepathy.start_session(&contact_b).await;
    client
        .session_status_probe
        .wait_for(peer_b.as_bytes(), SessionStatus::Connecting)
        .await;

    let call = client.telepathy.start_call(&contact_b);
    tokio::pin!(call);
    let first_poll = std::future::poll_fn(|context| Poll::Ready(call.as_mut().poll(context))).await;
    assert!(
        matches!(first_poll, Poll::Pending),
        "start_call must wait on an in-flight direct session attempt"
    );
    assert_call_slot_idle(
        &client,
        "waiting for an unreachable session must not claim the call slot",
    );

    tokio::time::timeout(Duration::from_secs(5), client.telepathy.shutdown())
        .await
        .expect("shutdown should complete promptly while start_call is waiting");

    let shutdown_result = tokio::time::timeout(
        Duration::from_secs(5),
        std::future::poll_fn(|context| Poll::Ready(call.as_mut().poll(context))),
    )
    .await
    .expect("shutdown must wake the waiting start_call promptly");
    let error = match shutdown_result {
        Poll::Ready(result) => result.expect_err("shutdown must not create a call"),
        Poll::Pending => panic!("shutdown must not leave start_call pending"),
    };
    assert!(
        error.to_string().contains("Runtime not ready") || error.to_string().contains("No session"),
        "expected shutdown to end the call attempt cleanly, got {error}"
    );
    assert_call_slot_idle(
        &client,
        "shutdown must leave the call slot idle after waking start_call",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn start_call_without_trusted_attempt_returns_no_session_promptly() {
    init_test_tracing();
    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let absent = Contact::new(
        "absent peer".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");

    let error = tokio::time::timeout(Duration::from_secs(1), client.telepathy.start_call(&absent))
        .await
        .expect("an absent peer without a trusted attempt must not wait")
        .expect_err("absent peer should not start a call");
    assert!(
        error.to_string().contains("No session"),
        "expected NoSessionForContact, got {error}"
    );
    assert_call_slot_idle(&client, "absent peer must leave the call slot idle");
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_session_sender_terminalizes_its_attempt() {
    init_test_tracing();
    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let contact = Contact::new(
        "closed sender peer".to_string(),
        SecretKey::generate().public().to_string(),
    )
    .expect("contact should be valid");

    client.telepathy.shutdown().await;
    assert!(
        client.telepathy.try_start_session(&contact).await.is_err(),
        "a closed manager sender must report RuntimeNotReady instead of stranding an attempt"
    );
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        client.telepathy.start_call(&contact),
    )
    .await
    .expect("terminal closed-sender attempt must not leave start_call waiting")
    .expect_err("closed sender cannot establish a session");
    assert!(
        error.to_string().contains("Runtime not ready") || error.to_string().contains("No session")
    );
    assert_call_slot_idle(&client, "closed sender must not leave a latent call slot");
}

#[tokio::test(flavor = "multi_thread")]
async fn self_session_request_terminalizes_without_stranding_start_call() {
    init_test_tracing();
    let identity = SecretKey::generate();
    let self_contact = Contact::new("self".to_string(), identity.public().to_string())
        .expect("self contact should be valid");
    let client = build_client(
        shared_relay_map(),
        identity,
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client.telepathy.start_session(&self_contact).await;
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        client.telepathy.start_call(&self_contact),
    )
    .await
    .expect("self-dial terminalization must wake start_call")
    .expect_err("self dial must not create a session");
    assert!(error.to_string().contains("No session"));
    assert_call_slot_idle(&client, "self dial must leave no call slot");
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_start_session_then_start_call_waits_for_publication_and_connects() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("publication-a".to_string(), key_a.public().to_string())
        .expect("contact a should be valid");
    let contact_b = Contact::new("publication-b".to_string(), key_b.public().to_string())
        .expect("contact b should be valid");
    let states_a = Arc::new(Mutex::new(Vec::new()));
    let states_b = Arc::new(Mutex::new(Vec::new()));
    let accept_probe = PendingAcceptProbe::default();
    let client_a = build_client_with_options_and_initial_contacts(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        vec![],
        &codec,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        states_a.clone(),
        None,
        ManagerLifecycle::Single,
    )
    .await;
    let client_b = build_client_with_options_and_initial_contacts(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        vec![],
        &codec,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        states_b.clone(),
        Some(accept_probe.clone()),
        ManagerLifecycle::Single,
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("start_call must wait for the public session attempt to publish");
    accept_probe.wait_opened().await;
    client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("callee should accept the published session call");
    wait_for_connected(&states_a, "publication caller").await;
    wait_for_connected(&states_b, "publication callee").await;

    client_a.telepathy.end_call().await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_idempotent_retry_preserves_original_pending_call_generation() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("retry-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("retry-client-b".to_string(), key_b.public().to_string())
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
        .expect("original outgoing call should start");
    accept_probe_b.wait_opened().await;

    let original_owner = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("original pending call slot snapshot should succeed");
    assert_eq!(original_owner.state, CallSlotState::PendingOutgoing);
    assert_eq!(original_owner.direct_peer, Some(contact_b.get_peer_id()));

    // Hold the session map write lock so manually polling the retry proves it has
    // observed its initial non-cancelled state and is waiting before acquisition.
    let session_lock = client_a.telepathy.inner.session_states.write().await;
    let retry_operation = CancellationToken::new();
    let retry = client_a
        .telepathy
        .start_call_with_operation(&contact_b, &retry_operation);
    tokio::pin!(retry);
    let first_poll =
        std::future::poll_fn(|context| Poll::Ready(retry.as_mut().poll(context))).await;
    assert!(
        matches!(first_poll, Poll::Pending),
        "retry must wait for the session lock after passing its initial cancellation check"
    );
    retry_operation.cancel();
    drop(session_lock);

    retry
        .await
        .expect("cancelled idempotent retry should complete without error");

    let owner_after_retry = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("call slot snapshot should succeed after retry cancellation");
    assert_eq!(
        owner_after_retry, original_owner,
        "cancelling a matched retry must not release or replace original call ownership"
    );

    client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("peer should be able to complete original pending call");
    wait_for_connected(&call_states_a, "original caller").await;
    wait_for_connected(&call_states_b, "peer").await;

    let connected_owner = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("connected call slot snapshot should succeed");
    assert_eq!(connected_owner.state, CallSlotState::ActiveDirect);
    assert_eq!(connected_owner.direct_peer, original_owner.direct_peer);
    assert_eq!(
        connected_owner.generation, original_owner.generation,
        "original call must retain its generation through cancelled matched retry"
    );

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

#[tokio::test(flavor = "multi_thread")]
async fn remote_shutdown_during_pending_acceptance_ends_call_and_clears_caller_state() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("Alice Caller".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("Bob Callee".to_string(), key_b.public().to_string())
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
        call_states_b,
        accept_probe_b.clone(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_active_transport(&client_a, "caller before remote shutdown").await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("caller should start the outgoing call");
    accept_probe_b.wait_opened().await;
    let caller_pending = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("caller slot snapshot should succeed");
    assert_eq!(caller_pending.state, CallSlotState::PendingOutgoing);
    assert_eq!(caller_pending.direct_peer, Some(peer_b));
    let callee_pending = client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("callee slot snapshot should succeed");
    assert_eq!(callee_pending.state, CallSlotState::PendingIncoming);
    assert_eq!(callee_pending.direct_peer, Some(peer_a));

    client_b.telepathy.shutdown().await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let caller_states = call_state_snapshot(&call_states_a);
        let terminal_observed = caller_states
            .iter()
            .any(|state| matches!(state, CallState::CallEnded(_, true)));
        let slot_idle =
            client_a.telepathy.inner.core_state.call_slot.current() == CallSlotState::Idle;
        let session_inactive = !client_a.is_active.load(Relaxed)
            && !client_a
                .telepathy
                .inner
                .session_states
                .read()
                .await
                .contains_key(&peer_b);
        if terminal_observed && slot_idle && session_inactive
            || tokio::time::Instant::now() >= deadline
        {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    let caller_states = call_state_snapshot(&call_states_a);
    let caller_slot = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("final caller slot snapshot should succeed");
    let caller_session_present = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .contains_key(&peer_b);
    let caller_session_active = client_a.is_active.load(Relaxed);
    client_a.telepathy.shutdown().await;

    let expected_message = "Bob Callee did not accept the call because the session was stopped";
    let matching_terminal_count = caller_states
        .iter()
        .filter(|state| {
            matches!(state, CallState::CallEnded(message, true) if message == expected_message)
        })
        .count();
    assert_eq!(
        matching_terminal_count, 1,
        "remote graceful shutdown must emit exactly one session-stopped CallEnded; \
         states={caller_states:?}, slot={caller_slot:?}, \
         session_present={caller_session_present}, session_active={caller_session_active}"
    );
    assert_eq!(caller_slot.state, CallSlotState::Idle);
    assert_eq!(caller_slot.direct_peer, None);
    assert!(!caller_session_present);
    assert!(!caller_session_active);
}

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

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_start_call_releases_only_acquisition_time_ownership_not_post_acquisition_state()
{
    init_test_tracing();

    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    let peer = SecretKey::generate().public();
    let call_slot = client.telepathy.inner.core_state.call_slot.clone();

    // `start_call_with_operation`'s acquisition path: claim the pending outgoing
    // slot and capture its ownership snapshot under a single lock hold.
    let (result, owner) = call_slot
        .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer)
        .expect("acquisition on an idle slot should succeed");
    assert_eq!(result, CallSlotAcquireResult::Acquired);
    let acquisition_snapshot = owner.expect("Acquired result must carry an ownership snapshot");
    assert_eq!(acquisition_snapshot.state, CallSlotState::PendingOutgoing);
    assert_eq!(acquisition_snapshot.direct_peer, Some(peer));
    let acquisition_generation = acquisition_snapshot.generation;

    // Reproduce the post-acquisition transition `call_handshake` performs once
    // the peer accepts. `transition_pending_to_active_for_peer` preserves the
    // generation, so a non-atomic snapshot captured after this transition would
    // share the acquisition generation but report `ActiveDirect` — the exact
    // stale-owner race the atomic acquisition eliminates.
    assert!(
        call_slot
            .transition_pending_to_active_for_peer(peer)
            .unwrap(),
        "pending slot must transition to ActiveDirect for the acquiring peer"
    );

    let after_transition = call_slot
        .snapshot()
        .expect("snapshot after transition should succeed");
    assert_eq!(after_transition.state, CallSlotState::ActiveDirect);
    assert_eq!(after_transition.direct_peer, Some(peer));
    assert_eq!(
        after_transition.generation, acquisition_generation,
        "transition_pending_to_active_for_peer must preserve the acquisition generation"
    );

    // The cancellation path in `start_call_with_operation` releases only against
    // the acquisition-time snapshot. The state mismatch (PendingOutgoing vs
    // ActiveDirect) is what the atomic snapshot guarantees it always observes.
    let released = call_slot
        .release_if_match(acquisition_snapshot)
        .expect("release_if_match should not error");
    assert!(
        !released,
        "cancelling the original operation must not release the active slot a \
         concurrent handshake transitioned to after acquisition; the atomic \
         acquisition-time snapshot must not match the post-transition state"
    );

    let final_snapshot = call_slot.snapshot().expect("final snapshot should succeed");
    assert_eq!(final_snapshot.state, CallSlotState::ActiveDirect);
    assert_eq!(final_snapshot.direct_peer, Some(peer));
    assert_eq!(final_snapshot.generation, acquisition_generation);

    call_slot.release().expect("cleanup release should succeed");
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_start_call_releases_only_acquisition_time_ownership_not_replacement_generation()
{
    init_test_tracing();

    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    let peer_a = SecretKey::generate().public();
    let peer_b = SecretKey::generate().public();
    let call_slot = client.telepathy.inner.core_state.call_slot.clone();

    let (result, owner_a) = call_slot
        .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer_a)
        .expect("acquisition for peer_a on an idle slot should succeed");
    assert_eq!(result, CallSlotAcquireResult::Acquired);
    let acquisition_snapshot = owner_a.expect("Acquired result must carry an ownership snapshot");
    let acquisition_generation = acquisition_snapshot.generation;

    // Release peer_a and re-acquire for peer_b, mirroring the replacement path
    // `start_call_with_operation`'s cancellation must not clobber: the new owner
    // gets a fresh generation.
    call_slot.release().expect("release should succeed");
    let (result, owner_b) = call_slot
        .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, peer_b)
        .expect("re-acquisition for peer_b on an idle slot should succeed");
    assert_eq!(result, CallSlotAcquireResult::Acquired);
    let replacement_snapshot =
        owner_b.expect("re-Acquired result must carry an ownership snapshot");
    assert_ne!(
        replacement_snapshot.generation, acquisition_generation,
        "replacement acquisition must bump the generation"
    );

    let released = call_slot
        .release_if_match(acquisition_snapshot)
        .expect("release_if_match should not error");
    assert!(
        !released,
        "cancelling the original operation must not release a replacement \
         acquisition's slot (different generation)"
    );

    let final_snapshot = call_slot.snapshot().expect("final snapshot should succeed");
    assert_eq!(final_snapshot.state, CallSlotState::PendingOutgoing);
    assert_eq!(final_snapshot.direct_peer, Some(peer_b));
    assert_eq!(final_snapshot.generation, replacement_snapshot.generation);

    call_slot.release().expect("cleanup release should succeed");
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn parked_connected_callback_does_not_block_end_call() {
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
    let connected_gate_a = ConnectedCallbackGate::new();

    let client_a = build_client_with_connected_gate(
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
        connected_gate_a.clone(),
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
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    // Block until the direct-call controller parks on the Connected callback.
    // The controller is now in `deliver_callback_against_teardown`, holding the
    // ActiveDirect slot for peer_b without entering its event loop.
    connected_gate_a.wait_for_connected().await;

    let active_snapshot = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("active slot snapshot should succeed while Connected is parked");
    assert_eq!(
        active_snapshot.state,
        CallSlotState::ActiveDirect,
        "slot should be ActiveDirect while Connected is parked; got {active_snapshot:?}"
    );
    assert_eq!(active_snapshot.direct_peer, Some(peer_id_b));

    // The deadlock regression: without `deliver_callback_against_teardown`
    // racing teardown against the parked callback, `end_call` would never
    // observe its `end_call` notification and `wait_for_release` would hang
    // forever. Bound the wait to catch the regression deterministically.
    tokio::time::timeout(Duration::from_secs(15), client_a.telepathy.end_call())
        .await
        .expect("end_call must return promptly even when the direct Connected callback is parked");

    // `end_call` returning is not sufficient: it must have returned BECAUSE the
    // slot was released, not because the bounded fallback fired. The slot must
    // be Idle here — a regression that returned early without releasing would
    // leave it ActiveDirect.
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;
    assert_call_slot_idle(
        &client_a,
        "slot must be Idle once end_call returns; a stuck controller would leave it ActiveDirect",
    );

    // Release the parked callback so any remaining controller bookkeeping can
    // drain before shutdown.
    connected_gate_a.release();

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn parked_call_ended_callback_does_not_wedge_slot_ownership() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("wedge-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("wedge-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_id_b = contact_b.get_peer_id();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let call_ended_park_a = CallEndedPark::new();

    let client_a = build_client_with_call_ended_park(
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
        call_ended_park_a.clone(),
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

    let _shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("client_a should start the outgoing call");
    wait_for_connected(&call_states_a, "parked_call_ended_wedge_test").await;

    let active_snapshot = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("active slot snapshot should succeed once Connected is observed");
    assert_eq!(
        active_snapshot.state,
        CallSlotState::ActiveDirect,
        "slot should be ActiveDirect while the call is up; got {active_snapshot:?}"
    );
    assert_eq!(active_snapshot.direct_peer, Some(peer_id_b));

    // client_b hangs up. Its controller sends a Goodbye, which causes
    // client_a's controller to break with a terminal Notify outcome. With the
    // old ordering, client_a would deliver `CallEnded` BEFORE releasing the
    // slot, so the parked callback would wedge the slot at ActiveDirect.
    // The fix delivers `CallEnded` AFTER slot release, so the slot must
    // transition to Idle even while the callback remains parked.
    tokio::time::timeout(Duration::from_secs(15), client_b.telepathy.end_call())
        .await
        .expect("client_b end_call must return promptly");

    // Confirm the CallEnded observation actually arrived on client_a and is
    // parked. Without this guarantee the test would not exercise the wedge
    // path.
    tokio::time::timeout(
        Duration::from_secs(15),
        call_ended_park_a.wait_for_call_ended(),
    )
    .await
    .expect("client_a must observe CallEnded from client_b's hangup");

    // The decisive assertion: the slot is Idle even though the CallEnded
    // callback is still parked. The old ordering would leave it ActiveDirect.
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;
    assert_call_slot_idle(
        &client_a,
        "client_a's slot must be Idle while the CallEnded callback is parked; \
         a regression would leave it ActiveDirect and wedge a fresh call",
    );

    // A fresh call must be able to start on the now-Idle slot, proving the
    // previous parked callback does not hold backend ownership.
    tokio::time::timeout(
        Duration::from_secs(15),
        client_a.telepathy.start_call(&contact_b),
    )
    .await
    .expect("client_a should start a fresh call after the previous slot was released")
    .expect("client_a fresh start_call should succeed");

    // Drain the fresh call so the test can shut down cleanly. Release the
    // parked callback from the FIRST call first so any bookkeeping finishes.
    call_ended_park_a.release();
    tokio::time::timeout(Duration::from_secs(15), client_a.telepathy.end_call())
        .await
        .expect("client_a fresh-call end_call must return promptly");
    wait_for_slot_idle(&client_a, &peer_id_b.to_string()).await;
}
