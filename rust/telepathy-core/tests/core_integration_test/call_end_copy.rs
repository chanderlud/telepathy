use super::common::{
    DEFAULT_SAMPLE_RATE, assert_no_call_ended_before_connected, assert_no_call_ended_contains,
    build_client, call_state_snapshot, init_test_tracing, shared_relay_map,
    wait_for_call_ended_contains, wait_for_connected, wait_for_sessions,
};

use iroh::SecretKey;
use std::sync::{Arc, Mutex};
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::state::CallSlotState;
use telepathy_core::types::Contact;
use telepathy_core::types::{CallState, CodecConfig};

#[tokio::test(flavor = "multi_thread")]
async fn outgoing_call_busy_emits_localized_copy() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("busy-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("busy-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

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
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Force Bob's slot into `AudioTest` so his listener rejects Alice's `Hello` with
    // `Busy`. We use the slot API directly (rather than `audio_test()`) because the
    // latter drives a real call loop that would block on `end_call`.
    assert!(
        client_b
            .telepathy
            .inner
            .core_state
            .call_slot
            .try_acquire(CallSlotState::AudioTest, None)
            .expect("slot acquire should succeed"),
        "Bob's slot must be acquirable for the busy test setup"
    );

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    let busy_message = format!("{} is busy", contact_b.nickname());
    wait_for_call_ended_contains(&call_states_a, &busy_message, true, "alice").await;

    assert_no_call_ended_contains(&call_states_a, "Busy", "alice");

    // Release Bob's `AudioTest` slot so `shutdown` (which only touches
    // `PendingDirect*`/`ActiveDirect`/`RoomCall`) can take it cleanly.
    client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .release()
        .expect("slot release should succeed");

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[test]
fn outgoing_call_did_not_respond_emits_localized_copy() {
    use telepathy_core::internal::error::peer_no_response_message;

    assert_eq!(
        peer_no_response_message("Bob"),
        "Bob did not respond to the call"
    );
    // Empty nickname: formatter still produces a user-facing sentence.
    assert_eq!(peer_no_response_message(""), " did not respond to the call");
    // Unicode nickname: contract must round-trip without mangling.
    assert_eq!(
        peer_no_response_message("Élise Müller"),
        "Élise Müller did not respond to the call"
    );
}

#[test]
fn outgoing_call_goodbye_emits_localized_copy() {
    use telepathy_core::internal::error::{GoodbyeReason, peer_goodbye_reason_message};

    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::SessionStopped),
        "Bob did not accept the call because the session was stopped"
    );
    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::AudioDeviceError),
        "Bob did not accept the call because of an audio device problem"
    );
    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::Error),
        "Bob did not accept the call because of an unexpected problem"
    );
    assert_eq!(
        peer_goodbye_reason_message("Bob", GoodbyeReason::None),
        "Bob did not accept the call"
    );

    // Each variant produces a "{nickname} did not accept the call" prefix.
    for reason in [
        GoodbyeReason::SessionStopped,
        GoodbyeReason::AudioDeviceError,
        GoodbyeReason::Error,
        GoodbyeReason::None,
    ] {
        let rendered = peer_goodbye_reason_message("Bob", reason);
        assert!(
            rendered.starts_with("Bob did not accept the call"),
            "goodbye reason {reason:?} did not start with the expected user-facing prefix; got {rendered:?}"
        );
        // The snake-case wire name must never appear in the user-facing copy.
        let wire_name = format!("{reason:?}");
        assert!(
            !rendered.contains(&wire_name),
            "wire-format variant name {wire_name:?} leaked into user-facing copy {rendered:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn normal_hangup_emits_silent_call_ended_for_remote_peer() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new(
        "silent-hangup-client-a".to_string(),
        key_a.public().to_string(),
    )
    .expect("contact a invalid");
    let contact_b = Contact::new(
        "silent-hangup-client-b".to_string(),
        key_b.public().to_string(),
    )
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

    // Drive a connected direct call.
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");
    wait_for_connected(&call_states_a, "alice").await;
    wait_for_connected(&call_states_b, "bob").await;

    // Alice's controller writes `Goodbye { reason: GoodbyeReason::None }` and
    // returns `Silent`; Bob's controller converts to an empty user-facing
    // message via `from_goodbye_reason`.
    client_a.telepathy.end_call().await;

    // Exactly one remote silent `CallEnded` on Bob — frontend dialog guard
    // suppresses and silent hangup tone plays.
    wait_for_call_ended_contains(&call_states_b, "", true, "bob's silent hangup").await;

    let states_b = call_state_snapshot(&call_states_b);
    for state in &states_b {
        if let CallState::CallEnded(message, _) = state {
            assert_ne!(
                message, "The call ended unexpectedly",
                "peer-driven normal hangup must NOT render to the generic failure copy on the receiving peer"
            );
        }
    }

    let silent_end_count = states_b
        .iter()
        .filter(|state| matches!(state, CallState::CallEnded(message, true) if message.is_empty()))
        .count();
    assert_eq!(
        silent_end_count, 1,
        "expected exactly one remote silent CallEnded on bob; got {silent_end_count} in {states_b:?}"
    );

    assert_no_call_ended_before_connected(&states_b, "bob");

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}

#[test]
fn session_stopped_error_emits_localized_copy() {
    use telepathy_core::internal::error::{
        CALL_END_SESSION_STOPPED, CallEndMessage, Error, ErrorKind,
    };

    let error: Error = ErrorKind::SessionStopped.into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_SESSION_STOPPED,
        "SessionStopped must produce the dedicated session-stopped copy"
    );
    assert_eq!(
        rendered, "The session was stopped",
        "exact wording must stay in sync with the user-facing template"
    );
    // Legacy `Display` produces "Session stopped" (no "The" prefix); the helper
    // must NOT pass that raw text through.
    assert_ne!(
        rendered,
        error.to_string(),
        "legacy Display wording leaked through CallEndMessage"
    );
}

#[test]
fn generic_controller_failure_emits_localized_copy() {
    use telepathy_core::internal::error::{CALL_END_GENERIC, CallEndMessage, Error, ErrorKind};

    // Every internal error kind that maps to the generic copy must produce exactly
    // the expected string and MUST NOT leak the raw `Display` wording.
    let error: Error = ErrorKind::MpscSend.into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_GENERIC,
        "MpscSend must produce the generic copy"
    );
    assert_ne!(
        rendered,
        error.to_string(),
        "raw Display wording must not leak through CallEndMessage"
    );
    assert!(
        !rendered.contains("mpsc"),
        "internal acronym leaked into user copy: {rendered}"
    );

    let error: Error = ErrorKind::TransportSend.into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_GENERIC,
        "TransportSend must produce the generic copy"
    );
    assert!(
        !rendered.contains("Transport"),
        "internal wording leaked into user copy: {rendered}"
    );

    let error: Error = ErrorKind::Poison("test lock").into();
    let rendered = CallEndMessage::from_error(&error).into_string();
    assert_eq!(
        rendered, CALL_END_GENERIC,
        "Poison must produce the generic copy"
    );
    assert!(
        !rendered.contains("Poison"),
        "internal wording leaked into user copy: {rendered}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_internal_error_strings_never_reach_call_ended() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);

    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let contact_a = Contact::new("raw-error-client-a".to_string(), key_a.public().to_string())
        .expect("contact a invalid");
    let contact_b = Contact::new("raw-error-client-b".to_string(), key_b.public().to_string())
        .expect("contact b invalid");

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
        Default::default(),
    )
    .await;

    client_a.telepathy.start_session(&contact_b).await;
    client_b.telepathy.start_session(&contact_a).await;
    wait_for_sessions(&client_a, &contact_b, &client_b, &contact_a).await;

    // Drive the busy path: Bob's slot is `AudioTest` so he rejects Alice's `Hello`
    // with `Busy`. Acquire the slot directly rather than driving `audio_test()`
    // (which blocks on a real call loop).
    assert!(
        client_b
            .telepathy
            .inner
            .core_state
            .call_slot
            .try_acquire(CallSlotState::AudioTest, None)
            .expect("slot acquire should succeed"),
        "Bob's slot must be acquirable for the busy test setup"
    );
    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("alice should start the outgoing call");

    wait_for_call_ended_contains(
        &call_states_a,
        &format!("{} is busy", contact_b.nickname()),
        true,
        "alice",
    )
    .await;
    // Release Bob's `AudioTest` slot before the call-state snapshot so
    // the shutdown path doesn't race with our walk.
    client_b
        .telepathy
        .inner
        .core_state
        .call_slot
        .release()
        .expect("slot release should succeed");

    // Walk every captured `CallEnded` and assert no known internal Display
    // string leaked through. Closed set: any new emission violating this contract
    // must add a row here AND be routed through `CallEndMessage`.
    let states = call_state_snapshot(&call_states_a);
    let forbidden_substrings = [
        // `ErrorKind::Poison` wording
        "Poison",
        // `ErrorKind::MpscSend` wording
        "mpsc",
        // `ErrorKind::TransportSend` / `TransportRecv` wording
        "Transport",
        // `ErrorKind::KanalSend` / `KanalReceive` / `KanalClose` wording
        "Kanal",
        // `ErrorKind::InvalidContactFormat` wording
        "Invalid contact format",
        // `ErrorKind::NoIdentityAvailable` / `NoEncoderAvailable` wording
        "No identity",
        "No encoder",
        // `ErrorKind::ManagerRestartDuringCall` wording
        "Cannot restart manager",
        // `ErrorKind::AttachmentsTooLarge` wording
        "Attachments too large",
        // `ErrorKind::AudioError` raw "Audio error: ..." prefix from
        // the legacy `Display` impl
        "Audio error:",
        // `ErrorKind::AudioInputStream` / `AudioOutputStream` raw
        // "Input stream error: ..." / "Output stream error: ..." prefix
        "Input stream error:",
        "Output stream error:",
        // `ErrorKind::DeviceError` raw "Device error: ..." prefix
        "Device error:",
        // `ErrorKind::BindError` wording
        "Bind error",
        // `ErrorKind::KeyParsing` wording
        "Key parsing",
        // `ErrorKind::Connection` wording
        "Connection error",
        // `ErrorKind::Poison` from anywhere via session-error wording
        "poisoned",
        // Wire-level GoodbyeReason strings that must NOT reach the
        // frontend copy (the renderer does its own mapping via
        // `CallEndMessage::from_goodbye_reason`).
        "an error occurred",
        "transport error",
        "session stopped",
        "audio device error",
    ];

    let mut violations: Vec<String> = Vec::new();
    for state in &states {
        if let CallState::CallEnded(message, _) = state {
            for forbidden in &forbidden_substrings {
                if message.contains(forbidden) {
                    violations.push(format!(
                        "CallEnded message {message:?} contains forbidden substring {forbidden:?}"
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "raw internal error strings leaked into CallEnded copy:\n  {}",
        violations.join("\n  ")
    );

    // Every observed `CallEnded` must match one of the closed user-facing copy
    // templates — the dual of the forbidden-substring check.
    for state in &states {
        if let CallState::CallEnded(message, _) = state {
            let known = [
                "A call is already active",
                "Audio device error",
                "The call ended unexpectedly",
                "The session was stopped",
                "The connection timed out",
            ];
            let is_user_facing_template = known.iter().any(|t| message == *t)
                || message.contains(" did not accept the call")
                || message.contains(" did not respond to the call")
                || message.contains(" is busy")
                || message.starts_with("Received an unexpected message from ");
            assert!(
                is_user_facing_template,
                "CallEnded message {message:?} did not match any known user-facing template; \
                 either a new user-facing template was added (extend this assertion) or \
                 internal wording leaked (route through CallEndMessage)"
            );
        }
    }

    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}
