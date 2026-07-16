use super::common::{
    CallbackCapturingAudioHost, DEFAULT_SAMPLE_RATE, ManagerLifecycle, RecordingOutput,
    SequencedInput, StreamErrorProbe, assert_call_slot_idle, build_client,
    build_client_with_options, init_test_tracing, normal_call_stream_error_surfaces_local_message,
    room_stream_error_sends_audio_error_goodbye_on_control_stream,
    room_stream_error_surfaces_local_message, shared_relay_map, simulated_stream_error,
    wait_for_sessions,
};

use iroh::SecretKey;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::types::CodecConfig;
use telepathy_core::types::Contact;

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

    let playback_log = Arc::new(Mutex::new(Vec::new()));

    let client_a = build_client(
        relay_map,
        key_a,
        vec![contact_b.clone()],
        &codec_config,
        MockAudioHost::new(
            SequencedInput::new(DEFAULT_SAMPLE_RATE),
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
            RecordingOutput::new(playback_log.clone()),
            DEFAULT_SAMPLE_RATE,
        ),
        Default::default(),
    )
    .await;

    client_a
        .telepathy
        .inner
        .start_session
        .as_ref()
        .unwrap()
        .send(contact_b.get_peer_id())
        .await
        .unwrap();

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
async fn room_input_stream_error_sends_audio_error_goodbye_on_control_stream() {
    room_stream_error_sends_audio_error_goodbye_on_control_stream(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn room_output_stream_error_sends_audio_error_goodbye_on_control_stream() {
    room_stream_error_sends_audio_error_goodbye_on_control_stream(false).await;
}
