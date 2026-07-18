use super::common::{
    CallbackCapturingAudioHost, DEFAULT_SAMPLE_RATE, DeviceSelectionOperation,
    DeviceSelectionProbe, InputSampleRateGate, MOCK_DEVICE_ID, RoomCallbackGate, RoomEventKind,
    StreamErrorProbe, TwoClientShutdownGuard, assert_call_slot_idle, assert_room_event_sequence,
    assert_slot_remains_outside_direct_call_states, build_client,
    build_client_with_room_callback_gate, call_state_snapshot, init_test_tracing, room_join_count,
    room_leave_count, shared_relay_map, sorted_room_members, wait_for_call_ended_contains,
    wait_for_connected, wait_for_no_extra_room_leave, wait_for_room_join_count,
    wait_for_room_leave_count, wait_for_sessions, wait_for_slot_idle, wait_for_slot_room_call,
};

use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::types::{CallState, CodecConfig, Contact};
use tokio::sync::Notify;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

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

    // Hold A's output_device lock so A's per-peer setup_output (its first await)
    // blocks. Without the cancellation race this strands A's room controller on
    // the awaited setup_output; with the race, cancelling the operation
    // interrupts it and tears this generation down.
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let release_output = Arc::new(Notify::new());
    let output_device = client_a.telepathy.inner.core_state.output_device.clone();
    let release_clone = release_output.clone();
    let hold_output = tokio::spawn(async move {
        let _guard = output_device.lock().await;
        let _ = held_tx.send(());
        release_clone.notified().await;
    });
    held_rx
        .await
        .expect("output_device lock should be acquired");

    let operation_a = CancellationToken::new();
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
    // has therefore received B's Join and is blocked inside setup_output.
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    operation_a.cancel();

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

    release_output.notify_one();
    hold_output
        .await
        .expect("output_device lock-hold task should finish");

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

/// Authoritative teardown must complete even when the frontend's awaited room
/// callback is parked indefinitely. A gated `Connected` callback simulates a
/// stalled frontend; `end_call` must still drive `cleanup_room_controller` to
/// completion (slot idle, room_state cleared) without releasing the gate.
#[tokio::test(flavor = "multi_thread")]
async fn end_call_tears_down_room_while_gated_callback_is_parked() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("room-callback-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("room-callback-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));

    // Gate A's `Connected` callback: it parks inside the mock and never returns
    // until `release` is invoked. Teardown must proceed without that release.
    let connected_gate = Arc::new(RoomCallbackGate::new(CallState::Connected));

    let client_a = build_client_with_room_callback_gate(
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
        Arc::clone(&connected_gate),
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

    let operation_a = CancellationToken::new();

    // Drive both joins concurrently: each `join_room` publishes its own
    // `room_state` and only resolves once the member-queue loop has finished,
    // so awaiting them serially would deadlock (each waits for the other's
    // publication). The room controller tasks continue running afterwards and
    // deliver `Connected` once the room handshake actually completes.
    let (result_a, result_b) = tokio::join!(
        client_a
            .telepathy
            .join_room_with_operation(room_members.clone(), &operation_a),
        client_b.telepathy.join_room(room_members),
    );
    result_a.expect("client a should join room");
    result_b.expect("client b should join room");

    // A's operation has settled; wait until A's controller reaches the gated
    // `Connected` callback (which the mock parks indefinitely).
    wait_for_slot_room_call(&client_a, "A holds RoomCall after join settles").await;
    connected_gate.wait_until_parked().await;

    // The `Connected` callback is now parked on the gate. Authoritative teardown
    // via `end_call` must not be blocked by that pending observation.
    client_a.telepathy.end_call().await;

    wait_for_slot_idle(&client_a, &peer_a).await;
    assert!(
        client_a
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none(),
        "end_call must clear room_state even with a parked room callback"
    );
    assert_call_slot_idle(
        &client_a,
        "end_call must release the call slot even with a parked room callback",
    );

    // The gate must still be parked (callback future never completed): if it had
    // been released, the `Connected` state would appear in delivery order.
    assert!(
        !connected_gate
            .received()
            .iter()
            .any(|state| matches!(state, CallState::Connected)),
        "the gated Connected callback must not have completed before release"
    );

    // Release the gate so the parked callback future can settle and the spawned
    // controller task can finish without holding test resources.
    connected_gate.release();

    // Give the released callback future a brief moment to settle; it must not
    // promote a fresh call slot or resurrect room_state.
    sleep(Duration::from_millis(200)).await;
    assert_call_slot_idle(
        &client_a,
        "releasing the gate after teardown must not promote a new call",
    );
    assert!(
        client_a
            .telepathy
            .inner
            .current_room_generation()
            .await
            .is_none(),
        "releasing the gate after teardown must not republish room_state"
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}
