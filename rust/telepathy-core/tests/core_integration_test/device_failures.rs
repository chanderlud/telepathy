use super::common::{
    CallbackCapturingAudioHost, DEFAULT_SAMPLE_RATE, DeviceSelectionOperation,
    DeviceSelectionProbe, InputSampleRateGate, MOCK_DEVICE_ID, ManagerLifecycle,
    PendingAcceptProbe, STALE_INPUT_DEVICE_ID, STALE_OUTPUT_DEVICE_ID, StreamErrorProbe,
    assert_call_slot_idle, assert_no_call_ended_contains, build_client,
    build_client_with_accept_probe, build_client_with_options, call_state_snapshot,
    init_test_tracing, shared_relay_map, sorted_room_members, wait_for_call_ended_contains,
    wait_for_connected, wait_for_sessions, wait_for_slot_idle,
};

use iroh::SecretKey;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::types::Contact;
use telepathy_core::types::{CallState, CodecConfig};
use tokio::time::sleep;

#[tokio::test(flavor = "multi_thread")]
async fn setup_output_synchronous_failure_emits_call_ended() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "setup-output-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "setup-output-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");

    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let accept_probe_b = PendingAcceptProbe::default();

    let dialer_host =
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe.clone());
    dialer_host.fail_output_synchronously.store(true, Relaxed);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        dialer_host,
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

    wait_for_call_ended_contains(&call_states_a, "Audio device error", false, "alice").await;

    let states_a = call_state_snapshot(&call_states_a);
    let ended_count = states_a
        .iter()
        .filter(|state| matches!(state, CallState::CallEnded(_, _)))
        .count();
    assert_eq!(
        ended_count, 1,
        "expected exactly one CallEnded on the dialer; states were {states_a:?}"
    );
    assert!(
        states_a
            .iter()
            .all(|state| !matches!(state, CallState::Connected)),
        "dialer must never reach Connected when setup_output fails synchronously; states were {states_a:?}"
    );
    assert_no_call_ended_contains(&call_states_a, "no output device", "alice");
    assert_call_slot_idle(
        &client_a,
        "dialer should leave the call slot idle after synchronous setup_output failure",
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_output_synchronous_flag_disabled_still_connects() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "setup-output-happy-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "setup-output-happy-client-b".to_string(),
        key_b.public().to_string(),
    )
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
        .expect("bob should accept the call");

    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_output_setup_synchronous_failure_clears_state_and_releases_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "room-output-setup-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "room-output-setup-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let output_failure_host =
        CallbackCapturingAudioHost::new(input_error_probe, output_error_probe.clone());
    output_failure_host
        .fail_output_synchronously
        .store(true, Relaxed);
    let room_members = sorted_room_members(&contact_a, &contact_b);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        output_failure_host,
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
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should start joining room");
    client_b
        .telepathy
        .join_room(room_members)
        .await
        .expect("client b should join room and send peer join");
    output_error_probe.wait_setup_attempted().await;

    wait_for_call_ended_contains(
        &call_states_a,
        "Audio device error",
        false,
        "room output setup failure",
    )
    .await;
    wait_for_slot_idle(&client_a, &contact_a.get_peer_id().to_string()).await;
    assert_eq!(
        client_a.telepathy.inner.current_room_generation().await,
        None,
        "synchronous output setup failure after peer join must clear RoomState"
    );
    assert_call_slot_idle(
        &client_a,
        "synchronous room output setup failure must release the call slot",
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

/// Synchronous input setup failure must remove the `RoomState` installed by
/// public `join_room` and release its `RoomCall` slot before any room processing
/// starts.
#[tokio::test(flavor = "multi_thread")]
async fn room_input_setup_synchronous_failure_clears_state_and_releases_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "room-input-setup-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "room-input-setup-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));

    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();
    let input_failure_host =
        CallbackCapturingAudioHost::new(input_error_probe.clone(), output_error_probe);
    input_failure_host
        .fail_input_synchronously
        .store(true, Relaxed);

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        input_failure_host,
        call_states_a.clone(),
    )
    .await;

    client_a
        .telepathy
        .join_room(sorted_room_members(&contact_a, &contact_b))
        .await
        .expect("join_room should return after spawning its controller");
    input_error_probe.wait_setup_attempted().await;

    wait_for_call_ended_contains(
        &call_states_a,
        "Audio device error",
        false,
        "room input setup failure",
    )
    .await;
    wait_for_slot_idle(&client_a, &contact_a.get_peer_id().to_string()).await;
    assert_eq!(
        client_a.telepathy.inner.current_room_generation().await,
        None,
        "synchronous input setup failure must clear RoomState"
    );
    assert_call_slot_idle(
        &client_a,
        "synchronous room input setup failure must release the call slot",
    );

    client_a.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_input_device_ends_room_before_member_notification() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "room-input-setup-fail-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "room-input-setup-fail-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let device_probe = DeviceSelectionProbe::default();
    let accept_probe_b = PendingAcceptProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
            .with_device_selection_probe(device_probe.clone()),
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

    client_a
        .telepathy
        .set_input_device(Some(STALE_INPUT_DEVICE_ID.to_string()))
        .await;
    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    let error = client_a
        .telepathy
        .join_room(sorted_room_members(&contact_a, &contact_b))
        .await
        .expect_err("stale room input must fail before publishing room state");
    assert!(
        error.to_string().contains(STALE_INPUT_DEVICE_ID),
        "room input failure should preserve stale-device error; error={error}"
    );

    wait_for_call_ended_contains(
        &call_states_a,
        "Audio device error",
        false,
        "room input setup failure",
    )
    .await;
    wait_for_slot_idle(&client_a, &contact_a.get_peer_id().to_string()).await;
    assert_eq!(
        client_a.telepathy.inner.current_room_generation().await,
        None,
        "synchronous input setup failure must clear RoomState"
    );
    assert_call_slot_idle(
        &client_a,
        "stale room input failure must release the call slot",
    );
    assert_eq!(
        device_probe.snapshot(),
        vec![(
            (DeviceSelectionOperation::InputSampleRate),
            Some(STALE_INPUT_DEVICE_ID.to_string())
        )],
        "stale room input must fail before input/output stream setup"
    );
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::InputSampleRate);
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenInput);
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenOutput);
    sleep(Duration::from_millis(750)).await;
    assert_eq!(
        accept_probe_b.opened.load(Relaxed),
        0,
        "stale room input must not send Hello or open an accept prompt"
    );
    assert!(
        call_state_snapshot(&call_states_a)
            .iter()
            .all(|state| !matches!(
                state,
                CallState::Waiting | CallState::Connected | CallState::RoomJoin(_)
            )),
        "stale room input must not publish room-call state"
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_output_device_ends_direct_call_and_allows_retry() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "stale-output-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "stale-output-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let device_probe = DeviceSelectionProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
            .with_device_selection_probe(device_probe.clone()),
        call_states_a.clone(),
    )
    .await;
    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::<MockAudioInput, MockAudioOutput>::default(),
        call_states_b.clone(),
    )
    .await;

    client_a
        .telepathy
        .set_output_device(Some(STALE_OUTPUT_DEVICE_ID.to_string()))
        .await;
    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("stale output attempt should enter call negotiation");
    wait_for_call_ended_contains(&call_states_a, "Audio device error", false, "stale output").await;
    wait_for_slot_idle(&client_a, &contact_b.get_peer_id().to_string()).await;
    wait_for_slot_idle(&client_b, &contact_a.get_peer_id().to_string()).await;
    assert!(
        call_state_snapshot(&call_states_a)
            .iter()
            .all(|state| !matches!(state, CallState::Connected)),
        "stale output must fail stream setup before connection"
    );
    device_probe
        .wait_for(
            DeviceSelectionOperation::OpenOutput,
            STALE_OUTPUT_DEVICE_ID,
            1,
        )
        .await;
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenOutput);

    client_a
        .telepathy
        .set_output_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("valid output retry should acquire released direct-call ownership");
    wait_for_connected(&call_states_a, "stale output retry client a").await;
    wait_for_connected(&call_states_b, "stale output retry client b").await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_output_device_ends_room_call_and_allows_retry() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
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
    let peer_b = contact_b.get_peer_id().to_string();
    let room_members = sorted_room_members(&contact_a, &contact_b);
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let device_probe = DeviceSelectionProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
            .with_device_selection_probe(device_probe.clone()),
        call_states_a.clone(),
    )
    .await;
    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::<MockAudioInput, MockAudioOutput>::default(),
        Default::default(),
    )
    .await;

    client_a
        .telepathy
        .set_output_device(Some(STALE_OUTPUT_DEVICE_ID.to_string()))
        .await;
    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;
    client_a
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client a should begin stale-output room attempt");
    client_b
        .telepathy
        .join_room(room_members.clone())
        .await
        .expect("client b should join stale-output room attempt");

    wait_for_call_ended_contains(
        &call_states_a,
        "Audio device error",
        false,
        "stale room output",
    )
    .await;
    wait_for_slot_idle(&client_a, &peer_b).await;
    assert_eq!(
        client_a.telepathy.inner.current_room_generation().await,
        None,
        "stale output must clear failed room ownership"
    );
    device_probe
        .wait_for(
            DeviceSelectionOperation::OpenOutput,
            STALE_OUTPUT_DEVICE_ID,
            1,
        )
        .await;
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenOutput);

    client_a
        .telepathy
        .set_output_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;
    let (retry, ()) = tokio::join!(client_a.telepathy.audio_test(), async {
        device_probe
            .wait_for(DeviceSelectionOperation::OpenOutput, MOCK_DEVICE_ID, 1)
            .await;
        client_a.telepathy.end_call().await;
    });
    retry.expect("valid audio test should acquire ownership released by failed room call");
    assert_call_slot_idle(
        &client_a,
        "valid retry should clean up ownership after failed room call",
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_input_device_ends_direct_call_before_connection_and_allows_retry() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "stale-input-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "stale-input-client-b".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let device_probe = DeviceSelectionProbe::default();
    let input_error_probe = StreamErrorProbe::new();
    let output_error_probe = StreamErrorProbe::new();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(input_error_probe, output_error_probe)
            .with_device_selection_probe(device_probe.clone()),
        call_states_a.clone(),
    )
    .await;
    let client_b = build_client(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        MockAudioHost::<MockAudioInput, MockAudioOutput>::default(),
        call_states_b.clone(),
    )
    .await;

    client_a
        .telepathy
        .set_input_device(Some(STALE_INPUT_DEVICE_ID.to_string()))
        .await;
    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("stale input attempt should enter call negotiation");
    wait_for_call_ended_contains(&call_states_a, "Audio device error", false, "stale input").await;
    wait_for_slot_idle(&client_a, &contact_b.get_peer_id().to_string()).await;

    let failed_states = call_state_snapshot(&call_states_a);
    assert!(
        failed_states
            .iter()
            .all(|state| !matches!(state, CallState::Connected)),
        "stale input must end the call before connection; states={failed_states:?}"
    );
    assert_eq!(
        device_probe.snapshot(),
        vec![(
            DeviceSelectionOperation::InputSampleRate,
            Some(STALE_INPUT_DEVICE_ID.to_string())
        )],
        "stale input must fail during sample-rate negotiation before any stream opens"
    );
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::InputSampleRate);
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenInput);
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenOutput);

    client_a
        .telepathy
        .set_input_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("valid input retry should acquire released direct-call ownership");
    wait_for_connected(&call_states_a, "stale input retry client a").await;
    wait_for_connected(&call_states_b, "stale input retry client b").await;

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn accepting_callee_stale_input_device_notifies_both_sides_and_releases_slot() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "incoming-stale-input-caller".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "incoming-stale-input-callee".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let accept_probe_b = PendingAcceptProbe::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::<MockAudioInput, MockAudioOutput>::default(),
        call_states_a.clone(),
    )
    .await;
    let client_b = build_client_with_accept_probe(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new()),
        call_states_b.clone(),
        accept_probe_b.clone(),
    )
    .await;

    client_b
        .telepathy
        .set_input_device(Some(STALE_INPUT_DEVICE_ID.to_string()))
        .await;
    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("caller should start outgoing call");
    accept_probe_b.wait_opened().await;
    client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("callee should accept incoming call");

    wait_for_call_ended_contains(
        &call_states_b,
        "Audio device error",
        false,
        "accepting callee",
    )
    .await;
    wait_for_call_ended_contains(
        &call_states_a,
        "audio device problem",
        true,
        "caller receiving audio-device goodbye",
    )
    .await;
    wait_for_slot_idle(&client_b, &contact_a.get_peer_id().to_string()).await;
    assert_call_slot_idle(
        &client_b,
        "accepting callee must release pending direct-call ownership",
    );

    let callee_states = call_state_snapshot(&call_states_b);
    assert_eq!(
        callee_states
            .iter()
            .filter(|state| matches!(state, CallState::CallEnded(_, _)))
            .count(),
        1,
        "accepting callee must emit exactly one CallEnded; states={callee_states:?}"
    );

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn accepting_callee_stale_input_device_releases_slot_when_control_stream_is_closed() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "closed-stream-stale-input-caller".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "closed-stream-stale-input-callee".to_string(),
        key_b.public().to_string(),
    )
    .expect("contact b invalid");
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
    let accept_probe_b = PendingAcceptProbe::default();
    let device_probe_b = DeviceSelectionProbe::default();
    let setup_gate = InputSampleRateGate::default();

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::<MockAudioInput, MockAudioOutput>::default(),
        Default::default(),
    )
    .await;
    let client_b = build_client_with_accept_probe(
        relay_map,
        key_b,
        vec![contact_a.clone()],
        &codec_config,
        CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
            .with_device_selection_probe(device_probe_b.clone())
            .with_input_sample_rate_gate(setup_gate.clone()),
        call_states_b.clone(),
        accept_probe_b.clone(),
    )
    .await;

    client_b
        .telepathy
        .set_input_device(Some(STALE_INPUT_DEVICE_ID.to_string()))
        .await;
    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("caller should start outgoing call");
    accept_probe_b.wait_opened().await;
    client_b
        .telepathy
        .start_call(&contact_a)
        .await
        .expect("callee should accept incoming call");
    device_probe_b
        .wait_for(
            DeviceSelectionOperation::InputSampleRate,
            STALE_INPUT_DEVICE_ID,
            1,
        )
        .await;

    client_a.telepathy.shutdown().await;
    setup_gate.release();

    wait_for_call_ended_contains(
        &call_states_b,
        "Audio device error",
        false,
        "accepting callee after closed control stream",
    )
    .await;
    wait_for_slot_idle(&client_b, &contact_a.get_peer_id().to_string()).await;
    assert_call_slot_idle(
        &client_b,
        "closed control stream must not retain pending direct-call ownership",
    );

    let callee_states = call_state_snapshot(&call_states_b);
    assert_eq!(
        callee_states
            .iter()
            .filter(|state| matches!(state, CallState::CallEnded(_, _)))
            .count(),
        1,
        "accepting callee must emit exactly one CallEnded; states={callee_states:?}"
    );

    client_b.telepathy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_output_device_fails_audio_test_and_allows_retry() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(false, false, 5.0);
    let key = SecretKey::generate();
    let contact = Contact::new(
        "stale-output-audio-test".to_string(),
        key.public().to_string(),
    )
    .expect("contact invalid");
    let device_probe = DeviceSelectionProbe::default();
    let client = build_client_with_options(
        relay_map,
        key,
        vec![contact],
        &codec_config,
        CallbackCapturingAudioHost::new(StreamErrorProbe::new(), StreamErrorProbe::new())
            .with_device_selection_probe(device_probe.clone()),
        Arc::new(Mutex::new(Vec::new())),
        None,
        ManagerLifecycle::Restartable,
    )
    .await;

    client
        .telepathy
        .set_output_device(Some(STALE_OUTPUT_DEVICE_ID.to_string()))
        .await;
    let error = client
        .telepathy
        .audio_test()
        .await
        .expect_err("stale output must fail audio-test stream setup");
    assert!(
        error.to_string().contains(STALE_OUTPUT_DEVICE_ID),
        "audio test should preserve stale output lookup error; error={error}"
    );
    assert_call_slot_idle(
        &client,
        "stale output audio test must release audio-test ownership",
    );
    device_probe.assert_no_default_attempt(DeviceSelectionOperation::OpenOutput);

    client
        .telepathy
        .set_output_device(Some(MOCK_DEVICE_ID.to_string()))
        .await;
    let (retry, ()) = tokio::join!(client.telepathy.audio_test(), async {
        device_probe
            .wait_for(DeviceSelectionOperation::OpenOutput, MOCK_DEVICE_ID, 1)
            .await;
        client.telepathy.end_call().await;
    });
    retry.expect("valid output audio-test retry should acquire released ownership");
    assert_call_slot_idle(&client, "valid audio-test retry should clean up ownership");

    client.telepathy.shutdown().await;
}
