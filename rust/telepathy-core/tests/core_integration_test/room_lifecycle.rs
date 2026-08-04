use super::common::{
    CallEndedPark, CallbackCapturingAudioHost, ConnectedCallbackGate, DEFAULT_SAMPLE_RATE,
    DeviceSelectionOperation, DeviceSelectionProbe, InputSampleRateGate, MOCK_DEVICE_ID,
    ManagerLifecycle, OutputOpenGate, PendingAcceptProbe, RoomEventKind, StreamErrorProbe,
    TwoClientShutdownGuard, WaitingCallbackGate, assert_call_slot_idle, assert_room_event_sequence,
    assert_slot_remains_outside_direct_call_states, build_client, build_client_with_accept_probe,
    build_client_with_call_ended_park, build_client_with_connected_gate,
    build_client_with_lookup_contacts, build_client_with_options,
    build_client_with_options_and_initial_contacts, build_client_with_waiting_gate,
    call_state_snapshot, init_test_tracing, log_lines_containing, room_join_count,
    room_leave_count, shared_relay_map, sorted_room_members, wait_for_call_ended_contains,
    wait_for_connected, wait_for_log_line, wait_for_log_line_count, wait_for_no_extra_room_leave,
    wait_for_room_join_count, wait_for_room_leave_count, wait_for_sessions, wait_for_slot_idle,
    wait_for_slot_room_call, wait_for_stable_session_pair,
};

use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::types::{CallState, CodecConfig, Contact, SessionStatus};
use tokio::sync::Notify;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
async fn stale_incoming_prompt_transport_stop_releases_slot_before_room_join() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("timeout-caller-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("timeout-callee-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
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
        Arc::new(Mutex::new(Vec::new())),
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
        Arc::new(Mutex::new(Vec::new())),
        accept_probe_b.clone(),
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
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("caller should start direct call");
    accept_probe_b.wait_opened().await;

    let removed = client_b
        .telepathy
        .inner
        .session_states
        .write()
        .await
        .remove(&peer_a);
    assert!(
        removed.is_some(),
        "callee session should still be registered"
    );
    client_a.telepathy.shutdown().await;
    accept_probe_b.wait_cancelled().await;

    client_b
        .telepathy
        .join_room(vec![])
        .await
        .expect("callee should immediately enter a room after stale incoming call stops");
    wait_for_slot_room_call(&client_b, "callee after stale incoming stop").await;

    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_b, &peer_a.to_string()).await;
    shutdown_guard.disarm();
    client_b.telepathy.shutdown().await;
}

/// Recreates the exact handler sequence behind the intermittent wedge in
/// `concurrent_room_end_immediately_rejoins_without_direct_negotiation`: a
/// teardown `Goodbye` from the previous room generation lands in a fresh
/// outgoing room negotiation. On the broken code the negotiation ended
/// silently on that goodbye, so the caller dropped the leg while the peer
/// completed it, leaving an asymmetric mesh. The negotiation must instead
/// hold the goodbye in grace and complete when the peer's affirmative
/// message arrives.
///
/// Deterministic ordering:
/// 1. Bob's first room controller parks inside its `Connected` delivery
///    before admitting Bob's session handshake, so Bob's session task waits
///    on admission without reading its stream.
/// 2. Alice ends the room; her teardown goodbye is buffered unread at Bob.
/// 3. Alice rejoins; her outgoing negotiation sends its Hello and parks on
///    the response read. The sleep bounds task-scheduling latency for her
///    negotiation to start; the log assertion below fails loudly if Bob's
///    goodbye ever misses her negotiation.
/// 4. Bob's end_call abandons the parked callback and tears his session
///    handshake down, writing the previous generation's goodbye into
///    Alice's parked negotiation — the exact message order from the flake.
/// 5. Bob rejoins; his fresh Hello reaches Alice's negotiation behind the
///    stale goodbye and the leg completes on both sides.
#[tokio::test(flavor = "multi_thread")]
async fn stale_room_goodbye_during_rejoin_negotiation_survives_until_peer_affirms() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "stale-goodbye-alice".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new("stale-goodbye-bob".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let members = sorted_room_members(&contact_a, &contact_b);

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let connected_gate_b = ConnectedCallbackGate::new();

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
    let client_b = build_client_with_connected_gate(
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
        connected_gate_b.clone(),
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

    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(members.clone()),
        client_b.telepathy.join_room(members.clone()),
    );
    join_a.expect("alice should join the initial room");
    join_b.expect("bob should join the initial room");
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    connected_gate_b.wait_for_connected().await;

    // Alice's session must fully exit her gen-1 handshake before the rejoin:
    // the handshake post-loop drains a pending start_call permit, so notifying
    // before it finishes would consume her gen-2 negotiation trigger. Her next
    // idle-loop wait proves the post-loop completed.
    let alice_idle_markers = &["session_waiting_for_event", &peer_b];
    let alice_idle_count = log_lines_containing(alice_idle_markers).len();
    client_a.telepathy.end_call().await;
    wait_for_log_line_count(
        alice_idle_markers,
        alice_idle_count + 1,
        "alice's session must return to idle after her gen-1 handshake teardown",
    )
    .await;

    client_a
        .telepathy
        .join_room(members.clone())
        .await
        .expect("alice should immediately rejoin after ending the room");
    sleep(Duration::from_millis(50)).await;

    client_b.telepathy.end_call().await;
    connected_gate_b.release();
    client_b
        .telepathy
        .join_room(members)
        .await
        .expect("bob should immediately rejoin after ending the room");

    wait_for_log_line(
        &["room_goodbye_during_negotiation_grace", &peer_b],
        "alice's outgoing negotiation must hold bob's stale teardown goodbye in grace",
    )
    .await;
    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;
    // bob's gen-1 admission was parked and torn down with the controller, so
    // his only RoomJoin for alice comes from the completed gen-2 leg.
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;

    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.end_call().await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_room_join_before_acquisition_leaves_slot_idle() {
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

    client
        .telepathy
        .join_room_with_operation(vec![], &cancelled)
        .await
        .expect("a cancelled room operation is a successful no-op");

    assert_call_slot_idle(
        &client,
        "cancelling before room acquisition must leave the slot idle",
    );
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_room_join_after_acquisition_does_not_publish_or_promote_room() {
    init_test_tracing();
    let call_states = Arc::new(Mutex::new(Vec::new()));
    let device_probe = DeviceSelectionProbe::default();
    let setup_gate = InputSampleRateGate::default();
    let host = CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
        .with_device_selection_probe(device_probe.clone())
        .with_input_sample_rate_gate(setup_gate.clone());
    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        host,
        call_states.clone(),
    )
    .await;
    client
        .telepathy
        .set_input_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;

    let operation = CancellationToken::new();
    let cancel_after_setup_starts = {
        let device_probe = device_probe.clone();
        let operation = operation.clone();
        let setup_gate = setup_gate.clone();
        tokio::spawn(async move {
            device_probe
                .wait_for(DeviceSelectionOperation::InputSampleRate, MOCK_DEVICE_ID, 1)
                .await;
            operation.cancel();
            setup_gate.release();
        })
    };

    client
        .telepathy
        .join_room_with_operation(vec![], &operation)
        .await
        .expect("cancelled room join should complete without error");
    cancel_after_setup_starts
        .await
        .expect("setup cancellation task should finish");

    let states = call_state_snapshot(&call_states);
    let published_room = client
        .telepathy
        .inner
        .current_room_generation()
        .await
        .is_some();
    let cancelled_slot = client
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("cancelled room slot snapshot should succeed");
    let promoted = states
        .iter()
        .any(|state| matches!(state, CallState::Waiting | CallState::Connected));
    if published_room
        || cancelled_slot.state != telepathy_core::internal::state::CallSlotState::Idle
        || promoted
    {
        client.telepathy.end_call().await;
        client.telepathy.shutdown().await;
    }
    assert!(
        !promoted,
        "cancelled room join must not publish Waiting or Connected; states={states:?}"
    );
    assert!(
        !published_room,
        "cancelled room join must leave room_state unpublished"
    );
    assert_eq!(
        cancelled_slot.state,
        telepathy_core::internal::state::CallSlotState::Idle,
        "cancelled room join after setup must release RoomCall ownership; slot={cancelled_slot:?}"
    );

    client
        .telepathy
        .join_room(vec![])
        .await
        .expect("fresh room join should acquire ownership after cancellation cleanup");
    wait_for_slot_room_call(&client, "fresh room join after cancelled setup").await;
    assert!(
        client
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_some(),
        "fresh room join should publish a new room_state"
    );

    client.telepathy.end_call().await;
    assert_call_slot_idle(
        &client,
        "fresh room retry should release ownership on end_call",
    );
    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_room_join_while_queuing_members_resolves_and_tears_down() {
    init_test_tracing();
    let call_states = Arc::new(Mutex::new(Vec::new()));
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
        call_states.clone(),
    )
    .await;

    // Hold the session_states write lock so the member-queue loop's
    // `session_states.read()` (and any subsequent `start_session.send`) inside
    // `join_room_with_operation` blocks deterministically. The room controller
    // never touches session_states before it receives a Join, so this neither
    // stalls its startup nor its cleanup.
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let release_lock = Arc::new(Notify::new());
    let session_states = client.telepathy.inner.session_states.clone();
    let release_clone = release_lock.clone();
    let hold_task = tokio::spawn(async move {
        let _guard = session_states.write().await;
        let _ = held_tx.send(());
        release_clone.notified().await;
    });
    held_rx
        .await
        .expect("session_states write lock should be acquired");

    // A member with no live session forces the member-queue loop onto the
    // start_session path; the read() that gates that path blocks on the held lock.
    let member = SecretKey::generate().public().to_string();
    let operation = CancellationToken::new();
    let join_future = client
        .telepathy
        .join_room_with_operation(vec![member], &operation);
    tokio::pin!(join_future);

    // Wait until join_room has published the RoomState (it sits immediately before
    // the member-queue loop), then cancel while its session_states.read() is pending.
    let wait_for_publish = async {
        loop {
            if client
                .telepathy
                .inner
                .current_room_generation()
                .await
                .is_some()
            {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::select! {
        _ = &mut join_future => {
            panic!("join_room must block on the held session_states lock until cancelled");
        }
        _ = wait_for_publish => {
            operation.cancel();
        }
    }
    join_future
        .await
        .expect("cancelled room join while queuing members must resolve cleanly");

    release_lock.notify_one();
    hold_task
        .await
        .expect("session_states lock-hold task should finish");

    let states_before_window = call_state_snapshot(&call_states);
    assert!(
        !states_before_window
            .iter()
            .any(|state| matches!(state, CallState::Connected)),
        "cancelled room join must never reach Connected; states={states_before_window:?}"
    );

    // Cancellation must have torn this generation down: room_state cleared, slot idle.
    assert!(
        client
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none(),
        "cancelled room join must clear room_state"
    );
    assert_call_slot_idle(
        &client,
        "cancelled room join while queuing members must release the slot",
    );

    // Stability window: no late Waiting/Connected arrives after teardown.
    sleep(Duration::from_millis(500)).await;
    let states_after_window = call_state_snapshot(&call_states);
    let promotions = |states: &[CallState]| {
        states
            .iter()
            .filter(|state| matches!(state, CallState::Waiting | CallState::Connected))
            .count()
    };
    assert_eq!(
        promotions(&states_after_window),
        promotions(&states_before_window),
        "no late Waiting/Connected expected after cancelled room join; \
         before={states_before_window:?} after={states_after_window:?}"
    );

    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_room_interrupts_gated_peer_output_setup() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-output-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-output-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);

    // Gate A's `setup_output` at the host's `open_output` call (the production
    // path through `AudioOutputBuilder::build`). `CoreState::output_device` is
    // `pub(crate)`, so the test must drive the gate via host behavior.
    let device_probe = DeviceSelectionProbe::default();
    let output_gate = OutputOpenGate::default();
    let host_a = CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
        .with_device_selection_probe(device_probe.clone())
        .with_output_open_gate(output_gate.clone());

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        host_a,
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
        .set_output_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;

    let operation_a = CancellationToken::new();
    // Cancel A's room operation once its per-peer `setup_output` reaches the
    // host (recorded as `OpenOutput`), then release the gate so the cancelled
    // call can return. Without the cancellation race A's room controller
    // strands on the gated `open_output`; with the race, cancelling interrupts
    // it and tears this generation down.
    let cancel_after_open_output = {
        let device_probe = device_probe.clone();
        let operation = operation_a.clone();
        let output_gate = output_gate.clone();
        tokio::spawn(async move {
            device_probe
                .wait_for(DeviceSelectionOperation::OpenOutput, MOCK_DEVICE_ID, 1)
                .await;
            operation.cancel();
            output_gate.release();
        })
    };

    client_a
        .telepathy
        .join_room_with_operation(room_members.clone(), &operation_a)
        .await
        .expect("client a should join room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room");

    // B observing A's RoomJoin means the room handshake completed both ways; A
    // has therefore received B's Join and its `setup_output` is blocked inside
    // the gated `open_output`.
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;

    operation_a.cancel();
    output_gate.release();
    cancel_after_open_output
        .await
        .expect("output gate cancellation task should finish");

    wait_for_slot_idle(&client_a, &peer_a).await;
    assert!(
        client_a
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none(),
        "A's room_state must be cleared after cancelling gated peer output setup"
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_input_task_panic_clears_ownership_and_notifies_once() {
    init_test_tracing();
    let call_states = Arc::new(Mutex::new(Vec::new()));
    let host = CallbackCapturingAudioHost::new(
        super::common::StreamErrorProbe::new(),
        super::common::StreamErrorProbe::new(),
    );
    let client = build_client(
        shared_relay_map(),
        SecretKey::generate(),
        vec![],
        &CodecConfig::new(true, true, 5.0),
        host.clone(),
        call_states.clone(),
    )
    .await;

    client
        .telepathy
        .join_room(vec![])
        .await
        .expect("room should start before its input task panics");
    host.panic_input.store(true, Relaxed);

    wait_for_call_ended_contains(
        &call_states,
        "The call ended unexpectedly",
        false,
        "room input task panic",
    )
    .await;
    wait_for_slot_idle(&client, "room input task panic should release its slot").await;
    assert!(
        client
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none()
    );
    assert_call_slot_idle(&client, "room input task panic should leave slot idle");
    assert_eq!(
        call_state_snapshot(&call_states)
            .iter()
            .filter(|state| matches!(state, CallState::CallEnded(_, _)))
            .count(),
        1,
        "room input task panic must emit exactly one terminal callback"
    );

    client.telepathy.shutdown().await;
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
    assert_room_event_sequence(&states_a, &peer_b, [RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a, [RoomEventKind::Join]);

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
    assert_room_event_sequence(&states_a, &peer_b, [RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a, [RoomEventKind::Join]);

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
        [RoomEventKind::Join, RoomEventKind::Leave],
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

    let client_b = build_client_with_options(
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
        None,
        ManagerLifecycle::RevisionCycles(2),
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
        .join_room(room_members.clone())
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    client_b.is_active.store(false, Relaxed);
    client_b.stop_session_and_wait_for_runtime(&contact_a).await;
    wait_for_room_leave_count(&call_states_a, &peer_b, 1).await;
    client_b.telepathy.start_session(&contact_a).await;

    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should rejoin room");
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
        [
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

    let client_b = build_client_with_options(
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
        None,
        ManagerLifecycle::RevisionCycles(3),
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
        .join_room(room_members.clone())
        .await
        .expect("client b should join room");

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    client_b.is_active.store(false, Relaxed);
    client_b.stop_session_and_wait_for_runtime(&contact_a).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    client_b
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client b should rejoin room");
    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;

    client_b.is_active.store(false, Relaxed);
    client_b.stop_session_and_wait_for_runtime(&contact_a).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should rejoin room");
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
        [
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

    let client_b = build_client_with_options(
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
        None,
        ManagerLifecycle::RevisionCycles(2),
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
        client_b
            .telepathy
            .join_room(room_members.clone())
            .await
            .is_ok(),
        "client b should join room"
    );

    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;

    // Simulate a transport drop and reconnect while the room call stays active.
    client_b.is_active.store(false, Relaxed);
    client_b.stop_session_and_wait_for_runtime(&contact_a).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_b, &contact_a, &client_a, &contact_b).await;
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should rejoin room");

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
        [
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );
}

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
    assert_room_event_sequence(&states_a, &peer_b, [RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a, [RoomEventKind::Join]);

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

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
        [RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_b,
        &peer_a,
        [RoomEventKind::Join, RoomEventKind::Join],
    );

    shutdown_guard.disarm();
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_room_end_immediately_rejoins_without_direct_negotiation() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let key_c = SecretKey::generate();
    let contact_a = Contact::new("rejoin-race-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("rejoin-race-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let contact_c = Contact::new("rejoin-race-c".to_string(), key_c.public().to_string())
        .expect("contact c invalid");
    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let peer_c = contact_c.get_peer_id().to_string();
    let mut room_members = vec![peer_a.clone(), peer_b.clone(), peer_c.clone()];
    room_members.sort();

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let call_states_c = Arc::new(Mutex::new(Vec::new()));
    let accept_probe_a = PendingAcceptProbe::default();
    let accept_probe_b = PendingAcceptProbe::default();
    let accept_probe_c = PendingAcceptProbe::default();
    let client_a = build_client_with_accept_probe(
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
        accept_probe_a.clone(),
    )
    .await;
    let client_b = build_client_with_accept_probe(
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
        accept_probe_b.clone(),
    )
    .await;
    let client_c = build_client_with_accept_probe(
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
        accept_probe_c.clone(),
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

    let (join_a, join_b, join_c) = tokio::join!(
        client_a.telepathy.join_room(room_members.clone()),
        client_b.telepathy.join_room(room_members.clone()),
        client_c.telepathy.join_room(room_members.clone()),
    );
    join_a.expect("client a should establish the initial room mesh");
    join_b.expect("client b should establish the initial room mesh");
    join_c.expect("client c should establish the initial room mesh");
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_a, &peer_c, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_c, 1).await;
    wait_for_room_join_count(&call_states_c, &peer_a, 1).await;
    wait_for_room_join_count(&call_states_c, &peer_b, 1).await;

    // Each controller is tearing down while its peers' room handshakes leave
    // and request reconciliation. Rejoin as soon as every end_call confirms
    // release: a stale old-generation reconcile would have latched a direct
    // start_call permit and make one of these joins fail with CallAlreadyActive.
    tokio::join!(
        client_a.telepathy.end_call(),
        client_b.telepathy.end_call(),
        client_c.telepathy.end_call(),
    );
    let (rejoin_a, rejoin_b, rejoin_c) = tokio::join!(
        client_a.telepathy.join_room(room_members.clone()),
        client_b.telepathy.join_room(room_members.clone()),
        client_c.telepathy.join_room(room_members),
    );
    rejoin_a.expect("client a should immediately rejoin after concurrent room teardown");
    rejoin_b.expect("client b should immediately rejoin after concurrent room teardown");
    rejoin_c.expect("client c should immediately rejoin after concurrent room teardown");

    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;
    wait_for_room_join_count(&call_states_a, &peer_c, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_c, 2).await;
    wait_for_room_join_count(&call_states_c, &peer_a, 2).await;
    wait_for_room_join_count(&call_states_c, &peer_b, 2).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_sessions(&client_a, &contact_c, &client_c, &contact_a).await;
    wait_for_sessions(&client_b, &contact_c, &client_c, &contact_b).await;
    wait_for_slot_room_call(&client_a, "client a after immediate room rejoin").await;
    wait_for_slot_room_call(&client_b, "client b after immediate room rejoin").await;
    wait_for_slot_room_call(&client_c, "client c after immediate room rejoin").await;

    assert_eq!(
        accept_probe_a.opened.load(Relaxed),
        0,
        "client a must not receive a ghost direct-call prompt"
    );
    assert_eq!(
        accept_probe_b.opened.load(Relaxed),
        0,
        "client b must not receive a ghost direct-call prompt"
    );
    assert_eq!(
        accept_probe_c.opened.load(Relaxed),
        0,
        "client c must not receive a ghost direct-call prompt"
    );

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    let states_c = call_state_snapshot(&call_states_c);
    for (label, states) in [
        ("client a", &states_a),
        ("client b", &states_b),
        ("client c", &states_c),
    ] {
        assert!(
            !states
                .iter()
                .any(|state| matches!(state, CallState::CallEnded(_, _))),
            "{label} must not observe a direct-call timeout or terminal state; states={states:?}"
        );
    }
    assert_room_event_sequence(
        &states_a,
        &peer_b,
        [RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_a,
        &peer_c,
        [RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_b,
        &peer_a,
        [RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_b,
        &peer_c,
        [RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_c,
        &peer_a,
        [RoomEventKind::Join, RoomEventKind::Join],
    );
    assert_room_event_sequence(
        &states_c,
        &peer_b,
        [RoomEventKind::Join, RoomEventKind::Join],
    );

    tokio::join!(
        client_a.telepathy.end_call(),
        client_b.telepathy.end_call(),
        client_c.telepathy.end_call(),
    );
    wait_for_slot_idle(&client_a, &peer_a).await;
    wait_for_slot_idle(&client_b, &peer_b).await;
    wait_for_slot_idle(&client_c, &peer_c).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_c.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_peer_rejection_does_not_end_active_room() {
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
    let mut room_members = vec![peer_a.clone(), peer_b.clone(), peer_c];
    room_members.sort();

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
        call_states_c,
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

    // C has live sessions but never enters the room. It rejects A and B's room
    // Hello messages while their matching room negotiations complete.
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
    wait_for_slot_room_call(&client_a, "client a after peer rejection").await;
    wait_for_slot_room_call(&client_b, "client b after peer rejection").await;

    // Exceed the original 10-second HelloAck timeout. Before this fix, each
    // rejected negotiation waited for that timeout and then emitted CallEnded.
    sleep(Duration::from_secs(11)).await;
    wait_for_slot_room_call(&client_a, "client a after rejection timeout window").await;
    wait_for_slot_room_call(&client_b, "client b after rejection timeout window").await;
    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert!(
        !states_a
            .iter()
            .any(|state| matches!(state, CallState::CallEnded(_, _))),
        "client a emitted global CallEnded after C rejected its room negotiation; states={states_a:?}"
    );
    assert!(
        !states_b
            .iter()
            .any(|state| matches!(state, CallState::CallEnded(_, _))),
        "client b emitted global CallEnded after C rejected its room negotiation; states={states_b:?}"
    );

    // Explicit room termination remains the operation that releases its slot.
    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a).await;
    wait_for_slot_idle(&client_b, &peer_b).await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_c.telepathy.shutdown().await;
}

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

    let client_c = build_client_with_options(
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
        None,
        ManagerLifecycle::RevisionCycles(2),
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

    // The manager revision refreshes every transport, giving the new `Join` a
    // different `connection_id` from the old `Leave` on both sides.
    client_c.is_active.store(false, Relaxed);
    client_c.stop_session_and_wait_for_runtime(&contact_a).await;
    client_c.telepathy.start_session(&contact_a).await;
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
        [
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );
    assert_room_event_sequence(
        &states_b,
        &peer_c,
        [
            RoomEventKind::Join,
            RoomEventKind::Leave,
            RoomEventKind::Join,
        ],
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_c.telepathy.shutdown().await;
}

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

#[tokio::test(flavor = "multi_thread")]
async fn room_end_call_during_parked_waiting_callback_tears_down() {
    init_test_tracing();
    let call_states = Arc::new(Mutex::new(Vec::new()));
    let waiting_gate = WaitingCallbackGate::new();

    let client = build_client_with_waiting_gate(
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
        call_states.clone(),
        waiting_gate.clone(),
    )
    .await;

    // Drive join_room through publication. The controller reaches the Waiting
    // observation asynchronously after publication_sender is acknowledged.
    client
        .telepathy
        .join_room(vec![])
        .await
        .expect("join_room should publish RoomState and return");

    // Block the test until the controller actually invokes the Waiting
    // callback. By this point the standalone select! would have latched its
    // end_call.notified() branch.
    waiting_gate.wait_for_waiting().await;

    // Sanity: RoomState is published and the slot is held while Waiting parks.
    let generation = client
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("RoomState should be installed while Waiting is parked");
    assert!(
        generation > 0,
        "Waiting must be parked against a real RoomState generation; got {generation}"
    );
    wait_for_slot_room_call(&client, "slot held while Waiting parked").await;

    // Reproduce the local hangup race: end_call fires while the controller is
    // still mid-Waiting. With the unfixed standalone select!, end_call's
    // notify was consumed here but the controller never reached cleanup, so
    // wait_for_release would hang forever and this timeout would fire.
    tokio::time::timeout(Duration::from_secs(15), client.telepathy.end_call())
        .await
        .expect("end_call must return when Waiting is parked; a stale controller would block wait_for_release forever");

    // Teardown proof #1: room_state was cleared by cleanup_room_controller.
    assert!(
        client
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none(),
        "room_state must be cleared after end_call tore down the parked controller"
    );

    // Teardown proof #2: the call slot released back to Idle.
    assert_call_slot_idle(
        &client,
        "end_call during parked Waiting must release the call slot",
    );

    // The Waiting observation was abandoned mid-flight: deliver_room_observation
    // returned false and dropped the parked callback future before it pushed
    // Waiting. The local user must never observe Waiting after teardown won.
    let states = call_state_snapshot(&call_states);
    assert!(
        !states
            .iter()
            .any(|state| matches!(state, CallState::Waiting)),
        "Waiting must not be delivered after teardown won the race; states={states:?}"
    );

    // Release the gate so any lingering waits resolve before shutdown joins
    // the controller task. The controller already exited via cleanup.
    waiting_gate.release();

    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn parked_call_ended_callback_does_not_wedge_room_slot_ownership() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "wedge-room-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "wedge-room-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let room_members = sorted_room_members(&contact_a, &contact_b);
    let call_ended_park_a = CallEndedPark::new();

    let host_a = CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new());

    let client_a = build_client_with_call_ended_park(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        host_a.clone(),
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

    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

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
        .join_room(room_members.clone())
        .await
        .expect("client b should join room");
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    let first_generation_a = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_a should have RoomState after first join");

    let slot_in_room = client_a
        .telepathy
        .inner
        .core_state
        .call_slot
        .snapshot()
        .expect("room slot snapshot should succeed while room is active");
    assert_eq!(
        slot_in_room.state,
        telepathy_core::internal::state::CallSlotState::RoomCall,
        "slot should be RoomCall while the room is up; got {slot_in_room:?}"
    );

    // Trigger a terminal Notify outcome on client_a by panicking its input
    // task. The controller breaks, runs cleanup_room_controller (releasing
    // room_state and the slot), then delivers CallEnded from the outer task.
    // With the old ordering the inline deliver_callback_against_teardown
    // would have parked here and blocked cleanup, wedging the slot.
    host_a.panic_input.store(true, Relaxed);

    tokio::time::timeout(
        Duration::from_secs(15),
        call_ended_park_a.wait_for_call_ended(),
    )
    .await
    .expect("client_a must observe CallEnded from its input task panic");

    // The decisive assertion: room_state is cleared and the call slot is Idle
    // on client_a, even though the CallEnded callback is still parked. The old
    // ordering would have kept room_state installed and the slot at RoomCall.
    let room_generation_after_panic = tokio::time::timeout(
        Duration::from_secs(15),
        client_a.telepathy.inner.current_room_generation(),
    )
    .await
    .expect("current_room_generation must resolve within 15s");
    assert!(
        room_generation_after_panic.is_none(),
        "client_a room_state must be cleared after the controller tore down; \
         a regression would leave it installed and wedge a fresh join"
    );
    wait_for_slot_idle(&client_a, &peer_a).await;
    assert_call_slot_idle(
        &client_a,
        "client_a slot must be Idle while the CallEnded callback is parked; \
         a regression would leave it RoomCall",
    );

    // A fresh join_room must succeed on the now-Idle slot, proving the parked
    // callback from the previous room does not hold backend ownership. client_b
    // must first leave its still-active room so the mesh re-establishes cleanly.
    call_ended_park_a.release();
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_b, &peer_b).await;
    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client_a should re-join room after the previous slot was released");
    client_b
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client_b should re-join room");
    wait_for_room_join_count(&call_states_a, &peer_b, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 2).await;
    let second_generation_a = client_a
        .telepathy
        .inner
        .current_room_generation()
        .await
        .expect("client_a should have RoomState after re-join");
    assert!(
        second_generation_a > first_generation_a,
        "re-join should bump the room generation; first={first_generation_a}, second={second_generation_a}"
    );

    shutdown_guard.disarm();
    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a).await;
    wait_for_slot_idle(&client_b, &peer_b).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_room_join_admits_member_before_room_state_publication() {
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
    let contact_a = Contact::new("cold-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("cold-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    // Gate B's `setup_call` -> `input_sample_rate` (helpers.rs:447). With the
    // gate held, B's `join_room_with_operation` stalls in `setup_call`
    // (internal.rs:527) BEFORE it publishes its `RoomState` (internal.rs:632).
    // During that window B's `is_in_room(peer)` returns false because
    // `room_state` is still None, so B's `session_manager` (core.rs:338-343)
    // closes any authorized no-contact room peer that dials in.
    let device_probe_b = DeviceSelectionProbe::default();
    let input_gate_b = InputSampleRateGate::default();
    let host_b = CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
        .with_device_selection_probe(device_probe_b.clone())
        .with_input_sample_rate_gate(input_gate_b.clone());

    // Both clients are built with EMPTY contact vectors. A and B know each
    // other only through the shared sorted two-peer member list, so an inbound
    // dial from A exercises the no-contact room-admission path.
    let client_a = build_client(
        relay_map,
        key_a,
        vec![],
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
        vec![],
        &codec_config,
        host_b,
        call_states_b.clone(),
    )
    .await;

    // B's input device must resolve to MOCK_DEVICE_ID so the device-selection
    // probe records the matching id when `input_sample_rate` is reached.
    client_b
        .telepathy
        .set_input_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;

    let core_b = client_b.telepathy.inner.clone();
    let contact_lookup_b = client_b.contact_lookup_probe.clone();

    // Start B's room join in a spawned task. B acquires the RoomCall slot
    // (internal.rs:474), enters `setup_call`, and parks on the gated
    // `input_sample_rate`. It cannot publish its `RoomState` until the gate
    // is released. The InputSampleRateGate blocks on a sync Condvar, so B's
    // join must run on a dedicated worker thread.
    let b_join_handle = {
        let members = room_members.clone();
        tokio::spawn(async move {
            let result = client_b.telepathy.join_room(members).await;
            (client_b, result)
        })
    };

    // Deterministically confirm B is parked inside `setup_call` before A
    // joins: the probe records InputSampleRate before the gate blocks.
    device_probe_b
        .wait_for(DeviceSelectionOperation::InputSampleRate, MOCK_DEVICE_ID, 1)
        .await;

    // A joins the room. A publishes its `RoomState` quickly (MockAudioHost
    // returns immediately), then iterates the member list and dials B
    // (internal.rs:659-689). A's dial arrives at B's session_manager while B
    // is still gated -- the cold room-join publication race window.
    client_a
        .telepathy
        .join_room(room_members)
        .await
        .expect("client a should join room");

    contact_lookup_b.wait_for(&peer_a.to_vec(), 2).await;
    assert!(
        core_b.current_room_generation().await.is_none(),
        "B must admit A before publishing RoomState"
    );

    // Release the input-sample-rate gate so B's `setup_call` returns and B's
    // `join_room` can publish its `RoomState` and iterate its own member list.
    // Await B's join before asserting so the blocking gate/task can never be
    // stranded by an earlier assertion failure.
    input_gate_b.release();
    let (client_b, b_join_result) = b_join_handle
        .await
        .expect("b join task should complete after gate release");
    b_join_result.expect("client b should join room after gate release");

    let shutdown_guard = TwoClientShutdownGuard {
        a: &client_a,
        b: &client_b,
        dropped: AtomicBool::new(false),
    };

    // --- Post-release behavior (fixed path): both clients reach RoomCall and
    // observe each other's RoomJoin with no spurious leaves.
    wait_for_slot_room_call(&client_a, "client_a after cold room join").await;
    wait_for_slot_room_call(&client_b, "client_b after cold room join").await;
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a_str, 0, Duration::from_secs(1)).await;

    let states_a = call_state_snapshot(&call_states_a);
    let states_b = call_state_snapshot(&call_states_b);
    assert_room_event_sequence(&states_a, &peer_b_str, [RoomEventKind::Join]);
    assert_room_event_sequence(&states_b, &peer_a_str, [RoomEventKind::Join]);

    // --- Orderly teardown.
    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;

    shutdown_guard.disarm();
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_session_collision_handoff_keeps_membership_until_replacement_is_admitted() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    // An incoming replacement wins only when the local peer sorts after the
    // remote peer. Pick identities deterministically so the replacement below
    // exercises `session_collision_kept_new` on client_a.
    let (key_a, key_b) = loop {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        if key_a.public() > key_b.public() {
            break (key_a, key_b);
        }
    };
    let cancelled_replacement_key = key_b.clone();
    let admitted_replacement_key = key_b.clone();
    let contact_a = Contact::new(
        "handoff-room-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "handoff-room-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);

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
        call_states_b,
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
        .join_room(room_members.clone())
        .await
        .expect("client b should join room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;

    let old_session_id = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_b)
        .map(|state| state.id())
        .expect("client a should have the admitted room session");

    // A same-identity client creates a collision-winning incoming replacement
    // but never starts room negotiation. Shutting it down must restore the
    // admitted old session rather than leaving the map owned by a dead session.
    let cancelled_replacement = build_client_with_lookup_contacts(
        relay_map,
        cancelled_replacement_key,
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
    cancelled_replacement
        .telepathy
        .start_session(&contact_a)
        .await;

    wait_for_stable_session_pair(
        &client_a,
        &peer_b,
        &cancelled_replacement,
        &peer_a,
        Some(old_session_id),
    )
    .await;
    assert_eq!(
        room_leave_count(&call_state_snapshot(&call_states_a), &peer_b_str),
        0,
        "the old room connection must remain effective while the replacement is unadmitted"
    );
    cancelled_replacement.telepathy.shutdown().await;

    wait_for_stable_session_pair(&client_a, &peer_b, &client_b, &peer_a, None).await;
    assert_eq!(
        client_a
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .get(&peer_b)
            .map(|state| state.id()),
        Some(old_session_id),
        "a cancelled replacement must restore the admitted old room session"
    );
    wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_secs(1)).await;

    // A second same-identity client joins the same room. Its incoming session
    // wins the collision at A; the old transport must remain effective until
    // this replacement is admitted, producing a second Join but no Leave.
    let admitted_replacement = build_client(
        relay_map,
        admitted_replacement_key,
        vec![],
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
    admitted_replacement
        .telepathy
        .join_room(room_members)
        .await
        .expect("replacement should join room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 2).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_secs(1)).await;
    assert_room_event_sequence(
        &call_state_snapshot(&call_states_a),
        &peer_b_str,
        [RoomEventKind::Join, RoomEventKind::Join],
    );

    admitted_replacement.telepathy.end_call().await;
    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    wait_for_slot_idle(&admitted_replacement, &peer_b_str).await;
    admitted_replacement.telepathy.shutdown().await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn active_room_retains_same_identity_candidate_until_predecessor_finishes() {
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
        "stale-room-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "stale-room-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);
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
    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(room_members.clone()),
        old_client_b.telepathy.join_room(room_members.clone()),
    );
    join_a.expect("client_a should join the room");
    join_b.expect("old client_b should join the room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;

    let predecessor_id = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_b)
        .map(|state| state.id())
        .expect("client_a should register the admitted predecessor");

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
        call_states_replacement_b,
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
        "the active-room candidate must wait while the admitted predecessor remains live"
    );

    old_client_b.telepathy.shutdown().await;

    // Poll the session map directly: the connection-level Connected status fires
    // when the replacement's dial connects, which can precede the deferred
    // candidate's promotion by an unbounded wait, so it cannot gate this assert.
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
                return;
            }
            sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(60), promoted)
        .await
        .unwrap_or_else(|_| {
            panic!("the active-room candidate should replace the dead predecessor")
        });

    replacement_client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("the replacement should rejoin through the promoted session");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 2).await;
    wait_for_slot_room_call(&client_a, "client_a after stale room handoff").await;

    replacement_client_b.telepathy.end_call().await;
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&replacement_client_b, &peer_b_str).await;
    replacement_client_b.telepathy.shutdown().await;
    client_a.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_replacement_map_mismatch_tears_down_deferred_predecessor() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let (key_a, key_b) = loop {
        let key_a = SecretKey::generate();
        let key_b = SecretKey::generate();
        if key_a.public() > key_b.public() {
            break (key_a, key_b);
        }
    };
    let replacement_key = key_b.clone();
    let contact_a = Contact::new(
        "predecessor-teardown-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "predecessor-teardown-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);
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
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(room_members.clone()),
        client_b.telepathy.join_room(room_members),
    );
    join_a.expect("client a should join the room");
    join_b.expect("client b should join the room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;

    let predecessor = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_b)
        .cloned()
        .expect("client a should have the admitted predecessor session");
    let predecessor_id = predecessor.id();

    // Given: a real incoming replacement wins the collision while the predecessor is admitted.
    let replacement_client = build_client_with_lookup_contacts(
        relay_map,
        replacement_key,
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
    replacement_client.telepathy.start_session(&contact_a).await;
    wait_for_stable_session_pair(
        &client_a,
        &peer_b,
        &replacement_client,
        &peer_a,
        Some(predecessor_id),
    )
    .await;

    let replacement = client_a
        .telepathy
        .inner
        .session_states
        .read()
        .await
        .get(&peer_b)
        .cloned()
        .expect("client a should install the collision-winning replacement");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if Arc::strong_count(&predecessor) >= 4 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("replacement should defer the admitted predecessor");

    let predecessor_weak = Arc::downgrade(&predecessor);
    let replacement_weak = Arc::downgrade(&replacement);

    // When: a third session state takes the map entry before the replacement exits.
    let displaced = client_a
        .telepathy
        .inner
        .session_states
        .write()
        .await
        .insert(
            peer_b,
            Arc::new(telepathy_core::internal::state::SessionState::new_for_test()),
        )
        .expect("the replacement should still own the map entry");
    assert_eq!(displaced.id(), replacement.id());
    drop(displaced);
    drop(replacement);
    drop(predecessor);

    replacement_client.telepathy.shutdown().await;

    // Then: restore cannot run, so teardown must release both session tasks and the connection.
    tokio::time::timeout(Duration::from_secs(10), async {
        while replacement_weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the aborted replacement session should finish");
    tokio::time::timeout(Duration::from_secs(10), async {
        while predecessor_weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the deferred predecessor should be torn down rather than orphaned");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !client_b
                .telepathy
                .inner
                .session_states
                .read()
                .await
                .contains_key(&peer_a)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the predecessor connection should close on the remote client");

    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reciprocal_room_joins_use_one_canonical_session_without_churn() {
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
    let contact_a = Contact::new("canonical-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("canonical-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let members = sorted_room_members(&contact_a, &contact_b);
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let client_a = build_client(
        relay_map,
        key_a,
        vec![],
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
        vec![],
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

    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(members.clone()),
        client_b.telepathy.join_room(members),
    );
    join_a.expect("client a should join room");
    join_b.expect("client b should join room");

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a_str, 0, Duration::from_secs(1)).await;
    assert_eq!(
        room_join_count(&call_state_snapshot(&call_states_a), &peer_b_str),
        1
    );
    assert_eq!(
        room_join_count(&call_state_snapshot(&call_states_b), &peer_a_str),
        1
    );

    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_reconciles_a_missing_session_without_another_join() {
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
    let contact_a = Contact::new("retry-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("retry-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let members = sorted_room_members(&contact_a, &contact_b);
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let client_a = build_client(
        relay_map,
        key_a,
        vec![],
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
        vec![],
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

    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(members.clone()),
        client_b.telepathy.join_room(members),
    );
    join_a.expect("client a should join room");
    join_b.expect("client b should join room");
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 1).await;

    client_a.telepathy.stop_session(&contact_b).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_room_join_count(&call_states_a, &peer_b_str, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 2).await;

    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_teardown_cancels_a_pending_canonical_reconnect() {
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
    let contact_a = Contact::new("teardown-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("teardown-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let members = sorted_room_members(&contact_a, &contact_b);
    let client_a = build_client(
        relay_map,
        key_a,
        vec![],
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

    client_a
        .telepathy
        .join_room(members.clone())
        .await
        .expect("client a should join the room before peer b starts");
    // Wait until A's first canonical dial to B has been attempted and failed.
    // This is the deterministic precondition: end_call must arrive while a
    // reconnect dial for B is pending in the scheduler.
    client_a
        .session_status_probe
        .wait_for(peer_b.as_bytes(), SessionStatus::Inactive)
        .await;
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![],
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
    client_b
        .telepathy
        .join_room(members)
        .await
        .expect("client b should join after client a leaves");

    // A regressed late reconcile tick cannot slip past a 2.5s negative window
    // (ROOM_DIAL_RECONCILE_INTERVAL is 1s, so this spans more than two ticks).
    let stability_deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
    loop {
        assert!(
            client_a
                .telepathy
                .inner
                .session_states
                .read()
                .await
                .get(&peer_b)
                .is_none(),
            "client a must not reconnect after room teardown"
        );
        assert!(
            client_b
                .telepathy
                .inner
                .session_states
                .read()
                .await
                .get(&peer_a)
                .is_none(),
            "client b must not receive a late room reconnection"
        );
        if tokio::time::Instant::now() >= stability_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_start_terminalizes_while_room_dial_is_in_flight() {
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
    let contact_a = Contact::new("coalesced-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("coalesced-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id();
    let members = sorted_room_members(&contact_a, &contact_b);
    let client_a = build_client_with_options_and_initial_contacts(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        vec![],
        &codec_config,
        MockAudioHost::new(
            MockAudioInput::default(),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Single,
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

    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    client_b.telepathy.shutdown().await;
    client_a
        .session_status_probe
        .wait_for(peer_b.as_bytes(), SessionStatus::Inactive)
        .await;
    client_a.session_status_probe.park_connecting();
    client_a
        .telepathy
        .join_room(members)
        .await
        .expect("client a should join while client b is offline");
    client_a
        .session_status_probe
        .wait_for(peer_b.as_bytes(), SessionStatus::Connecting)
        .await;

    client_a.telepathy.start_session(&contact_b).await;
    let direct_availability = tokio::time::timeout(
        Duration::from_secs(5),
        client_a.telepathy.start_call(&contact_b),
    )
    .await;

    client_a.session_status_probe.release_connecting();
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a).await;
    client_a.telepathy.shutdown().await;

    let direct_result = direct_availability
        .expect("room-coalesced direct availability must terminalize through session_manager");
    assert!(
        direct_result.is_err(),
        "an offline room peer must not produce a callable session"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn room_retries_when_the_canonical_peer_starts_late() {
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
    let contact_a = Contact::new("late-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("late-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let members = sorted_room_members(&contact_a, &contact_b);
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let client_a = build_client(
        relay_map,
        key_a,
        vec![],
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

    client_a
        .telepathy
        .join_room(members.clone())
        .await
        .expect("client a should join before peer b starts");
    sleep(Duration::from_millis(300)).await;

    let client_b = build_client(
        relay_map,
        key_b,
        vec![],
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
    client_b
        .telepathy
        .join_room(members)
        .await
        .expect("client b should join once it starts");

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 1).await;

    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_goodbye_rearms_a_persistent_session_for_peer_rejoin() {
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
    let contact_a = Contact::new("goodbye-room-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("goodbye-room-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let members = sorted_room_members(&contact_a, &contact_b);
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
    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(members.clone()),
        client_b.telepathy.join_room(members.clone()),
    );
    join_a.expect("client a should join room");
    join_b.expect("client b should join room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 1).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 0, Duration::from_millis(500)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a_str, 0, Duration::from_millis(500)).await;
    assert_eq!(
        room_join_count(&call_state_snapshot(&call_states_a), &peer_b_str),
        1
    );
    assert_eq!(
        room_join_count(&call_state_snapshot(&call_states_b), &peer_a_str),
        1
    );

    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    wait_for_room_leave_count(&call_states_a, &peer_b_str, 1).await;
    assert!(
        client_a
            .telepathy
            .inner
            .session_states
            .read()
            .await
            .contains_key(&peer_b),
        "client a should retain the direct session after room goodbye"
    );

    client_b
        .telepathy
        .join_room(members)
        .await
        .expect("client b should rejoin the room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 2).await;
    wait_for_no_extra_room_leave(&call_states_a, &peer_b_str, 1, Duration::from_millis(500)).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_a_str, 0, Duration::from_millis(500)).await;
    assert_eq!(
        room_join_count(&call_state_snapshot(&call_states_a), &peer_b_str),
        2
    );
    assert_eq!(
        room_join_count(&call_state_snapshot(&call_states_b), &peer_a_str),
        2
    );

    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_reconcile_discards_stale_generation_after_teardown() {
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
    let contact_a = Contact::new(
        "reconcile-teardown-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "reconcile-teardown-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let peer_a = contact_a.get_peer_id();
    let peer_b = contact_b.get_peer_id();
    let peer_a_str = peer_a.to_string();
    let peer_b_str = peer_b.to_string();
    let members = sorted_room_members(&contact_a, &contact_b);
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
    let (join_a, join_b) = tokio::join!(
        client_a.telepathy.join_room(members.clone()),
        client_b.telepathy.join_room(members.clone()),
    );
    join_a.expect("client a should join the initial room");
    join_b.expect("client b should join the initial room");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 1).await;

    let session_states = client_a.telepathy.inner.session_states.clone();
    let session_guard = session_states.write().await;
    assert!(
        session_guard.contains_key(&peer_b),
        "client a should retain its session for client b"
    );

    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    wait_for_room_leave_count(&call_states_a, &peer_b_str, 1).await;

    // `room_handshake` requests reconciliation before delivering RoomLeave. The
    // held writer therefore parks the manager at reconcile's session-state read;
    // yielding covers the remaining scheduler handoff because the harness has no
    // probe between the preceding room-state snapshot and that read acquisition.
    tokio::task::yield_now().await;
    client_a.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    assert!(
        client_a
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none(),
        "client a room must be torn down while reconcile is gated"
    );

    drop(session_guard);
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    let (rejoin_a, rejoin_b) = tokio::join!(
        client_a.telepathy.join_room(members.clone()),
        client_b.telepathy.join_room(members),
    );
    rejoin_a.expect("client a should immediately rejoin after the gated reconcile");
    rejoin_b.expect("client b should immediately rejoin after the gated reconcile");
    wait_for_room_join_count(&call_states_a, &peer_b_str, 2).await;
    wait_for_room_join_count(&call_states_b, &peer_a_str, 2).await;
    wait_for_slot_room_call(&client_a, "client a after gated reconcile rejoin").await;
    wait_for_slot_room_call(&client_b, "client b after gated reconcile rejoin").await;

    assert_eq!(
        accept_probe_b.opened.load(Relaxed),
        0,
        "client b must not receive a ghost direct-call prompt"
    );
    for (label, states) in [
        ("client a", call_state_snapshot(&call_states_a)),
        ("client b", call_state_snapshot(&call_states_b)),
    ] {
        assert!(
            !states
                .iter()
                .any(|state| matches!(state, CallState::CallEnded(_, _))),
            "{label} must not observe a terminal direct-call state; states={states:?}"
        );
    }

    client_a.telepathy.end_call().await;
    client_b.telepathy.end_call().await;
    wait_for_slot_idle(&client_a, &peer_a_str).await;
    wait_for_slot_idle(&client_b, &peer_b_str).await;
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn six_peer_room_builds_the_canonical_mesh() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let mut identities: Vec<_> = (0..6).map(|_| SecretKey::generate()).collect();
    identities.sort_by_key(|identity| identity.public());
    let contacts: Vec<_> = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| {
            Contact::new(
                format!("six-peer-room-{index}"),
                identity.public().to_string(),
            )
            .expect("contact invalid")
        })
        .collect();
    let peers: Vec<_> = contacts.iter().map(Contact::get_peer_id).collect();
    let peer_strings: Vec<_> = peers.iter().map(ToString::to_string).collect();
    let members = peer_strings.clone();
    let call_states: Vec<_> = (0..contacts.len())
        .map(|_| Arc::new(Mutex::new(Vec::new())))
        .collect();
    let mut clients = Vec::with_capacity(contacts.len());

    for (identity, states) in identities.into_iter().zip(call_states.iter().cloned()) {
        clients.push(
            build_client(
                relay_map,
                identity,
                vec![],
                &codec_config,
                MockAudioHost::new(
                    MockAudioInput::default(),
                    DEFAULT_SAMPLE_RATE,
                    MockAudioOutput,
                    DEFAULT_SAMPLE_RATE,
                ),
                states,
            )
            .await,
        );
    }

    for client in &clients {
        client
            .telepathy
            .join_room(members.clone())
            .await
            .expect("client should join six-peer room");
    }

    for (index, client) in clients.iter().enumerate() {
        for (other_index, peer) in peer_strings.iter().enumerate() {
            if index != other_index {
                wait_for_room_join_count(&call_states[index], peer, 1).await;
            }
        }
        assert_eq!(
            client.telepathy.inner.session_states.read().await.len(),
            contacts.len() - 1,
            "each six-peer member should retain every other canonical session"
        );
    }
    wait_for_no_extra_room_leave(
        &call_states[0],
        &peer_strings[1],
        0,
        Duration::from_millis(500),
    )
    .await;
    sleep(Duration::from_millis(500)).await;
    for (index, states) in call_states.iter().enumerate() {
        let snapshot = call_state_snapshot(states);
        for (other_index, peer) in peer_strings.iter().enumerate() {
            if index != other_index {
                assert_eq!(room_join_count(&snapshot, peer), 1);
                assert_eq!(room_leave_count(&snapshot, peer), 0);
            }
        }
    }

    for client in &clients {
        client.telepathy.end_call().await;
    }
    for (client, peer) in clients.iter().zip(peer_strings.iter()) {
        wait_for_slot_idle(client, peer).await;
        client.telepathy.shutdown().await;
    }
}
