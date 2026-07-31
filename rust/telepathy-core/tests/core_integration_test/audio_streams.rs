use super::common::{
    CallbackCapturingAudioHost, DEFAULT_SAMPLE_RATE, ManagerLifecycle, PendingAcceptProbe,
    RoomEventKind, StreamErrorProbe, TwoClientShutdownGuard, assert_call_slot_idle,
    assert_no_call_ended_contains, assert_room_event_sequence, build_client,
    build_client_with_accept_probe, build_client_with_options, call_state_snapshot,
    init_test_tracing, shared_relay_map, simulated_stream_error, sorted_room_members,
    stream_error_scenario, wait_for_call_ended_contains, wait_for_connected,
    wait_for_no_extra_room_leave, wait_for_room_join_count, wait_for_room_leave_count,
    wait_for_sessions, wait_for_slot_idle, wait_for_slot_room_call,
};

use iroh::SecretKey;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{
    AudioFrameIndexCapture, MockAudioHost, MockAudioInput, MockAudioOutput, RecordingAudioOutput,
    SequencedAudioInput,
};
use telepathy_core::types::Contact;
use telepathy_core::types::{CallState, CodecConfig};

#[tokio::test(flavor = "multi_thread")]
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

    let playback_capture = AudioFrameIndexCapture::new(512);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            SequencedAudioInput::new(DEFAULT_SAMPLE_RATE),
            DEFAULT_SAMPLE_RATE,
            MockAudioOutput,
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
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
            RecordingAudioOutput::new(playback_capture.clone()),
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;

    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a.telepathy.inner.core_state.set_input_volume(0.0);

    let b_session = client_a
        .telepathy
        .inner
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

    let log = playback_capture.drain();
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

#[tokio::test(flavor = "multi_thread")]
async fn audio_test_input_stream_error_propagates_and_clears_state() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(false, false, 5.0);

    let key = SecretKey::generate();
    let contact = Contact::new("audio-test-client".to_string(), key.public().to_string())
        .expect("contact invalid");

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let client = build_client_with_options(
        relay_map,
        key,
        vec![contact],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Restartable,
    )
    .await;

    let (result, ()) = tokio::join!(async { client.telepathy.audio_test().await }, async {
        input_error_probe.wait_captured().await;
        input_error_probe.trigger(simulated_stream_error(
            "simulated input device disconnected",
        ));
    });

    let error = result.expect_err("audio_test should fail when input stream errors");
    assert!(
        error
            .to_string()
            .contains("simulated input device disconnected"),
        "expected propagated disconnect error, got {error}"
    );
    assert_call_slot_idle(
        &client,
        "audio_test should leave the call slot idle after cleanup",
    );

    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_test_output_stream_error_propagates_and_clears_state() {
    init_test_tracing();
    let relay_map = shared_relay_map();

    let codec_config = CodecConfig::new(false, false, 5.0);

    let key = SecretKey::generate();
    let contact = Contact::new("audio-test-client".to_string(), key.public().to_string())
        .expect("contact invalid");

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let client = build_client_with_options(
        relay_map,
        key,
        vec![contact],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe, output_error_probe.clone()),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Restartable,
    )
    .await;

    let (result, ()) = tokio::join!(async { client.telepathy.audio_test().await }, async {
        output_error_probe.wait_captured().await;
        output_error_probe.trigger(simulated_stream_error(
            "simulated output device disconnected",
        ));
    });

    let error = result.expect_err("audio_test should fail when output stream errors");
    assert!(
        error
            .to_string()
            .contains("simulated output device disconnected"),
        "expected propagated disconnect error, got {error}"
    );
    assert_call_slot_idle(
        &client,
        "audio_test should leave the call slot idle after cleanup",
    );

    client.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_call_input_stream_error_surfaces_local_message() {
    normal_call_stream_error_surfaces_local_message(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_call_output_stream_error_surfaces_local_message() {
    normal_call_stream_error_surfaces_local_message(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_input_stream_error_surfaces_local_message() {
    room_stream_error_surfaces_local_message(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_output_stream_error_surfaces_local_message() {
    room_stream_error_surfaces_local_message(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_input_stream_error_removes_only_failing_peer() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let key_c = SecretKey::generate();
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
    let contact_c = Contact::new(
        "room-stream-error-client-c".to_string(),
        key_c.public().to_string(),
    )
    .expect("contact c invalid");

    let peer_a = contact_a.get_peer_id().to_string();
    let peer_b = contact_b.get_peer_id().to_string();
    let peer_c = contact_c.get_peer_id().to_string();
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let call_states_c = Arc::new(Mutex::new(Vec::new()));
    let mut room_members = vec![peer_a.clone(), peer_b.clone(), peer_c.clone()];
    room_members.sort();
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone(), contact_c.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe),
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
        .join_room(room_members)
        .await
        .expect("client c should join room");
    wait_for_room_join_count(&call_states_a, &peer_b, 1).await;
    wait_for_room_join_count(&call_states_a, &peer_c, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_a, 1).await;
    wait_for_room_join_count(&call_states_b, &peer_c, 1).await;
    wait_for_room_join_count(&call_states_c, &peer_a, 1).await;
    wait_for_room_join_count(&call_states_c, &peer_b, 1).await;

    input_error_probe.wait_captured().await;
    input_error_probe.trigger(simulated_stream_error(
        "simulated input device disconnected",
    ));

    wait_for_call_ended_contains(&call_states_a, "Microphone error", false, "room client a").await;
    wait_for_room_leave_count(&call_states_b, &peer_a, 1).await;
    wait_for_room_leave_count(&call_states_c, &peer_a, 1).await;
    wait_for_no_extra_room_leave(&call_states_b, &peer_c, 0, Duration::from_secs(1)).await;
    wait_for_no_extra_room_leave(&call_states_c, &peer_b, 0, Duration::from_secs(1)).await;
    wait_for_slot_idle(&client_a, &peer_b).await;
    wait_for_slot_room_call(&client_b, "room client b after peer audio error").await;
    wait_for_slot_room_call(&client_c, "room client c after peer audio error").await;

    for (label, states) in [
        ("room client b", call_state_snapshot(&call_states_b)),
        ("room client c", call_state_snapshot(&call_states_c)),
    ] {
        assert!(
            states
                .iter()
                .all(|state| !matches!(state, CallState::CallEnded(_, _))),
            "{label} should receive RoomLeave without ending its room call; states were {states:?}"
        );
    }

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
    client_c.telepathy.shutdown().await;
}

async fn room_stream_error_surfaces_local_message(trigger_input: bool) {
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

async fn normal_call_stream_error_surfaces_local_message(trigger_input: bool) {
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
