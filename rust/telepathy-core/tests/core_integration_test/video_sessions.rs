use super::common::{
    DEFAULT_SAMPLE_RATE, ManagerLifecycle, ProcessBoundaryProbe, TwoClientShutdownGuard,
    build_client_with_options, init_test_tracing, shared_relay_map, wait_for_connected,
    wait_for_sessions,
};
use bytes::BytesMut;
use futures_util::stream;
use std::io;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
use telepathy_core::internal::state::CallSlotState;
use telepathy_core::internal::video::platform::{forward_capture_chunks, forward_playback_frames};
use telepathy_core::types::{
    CallState, CodecConfig, Contact, VideoSource, VideoStartOutcome, VideoUnavailable,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[path = "video_sessions/lifecycle.rs"]
mod lifecycle;
#[path = "video_sessions/protocol.rs"]
mod protocol;

#[tokio::test]
async fn capture_preserves_512_byte_chunk_boundaries_in_length_frames() {
    let source = [7_u8; 513];
    let (mut source_writer, mut source_reader) = tokio::io::duplex(1024);
    source_writer
        .write_all(&source)
        .await
        .expect("write source");
    drop(source_writer);

    let (frame_writer, frame_reader) = tokio::io::duplex(2048);
    let mut transport = FramedWrite::new(frame_writer, LengthDelimitedCodec::new());
    forward_capture_chunks(&mut source_reader, &mut transport).await;
    drop(transport);

    let mut frames = FramedRead::new(frame_reader, LengthDelimitedCodec::new());
    let first = futures_util::StreamExt::next(&mut frames)
        .await
        .expect("first frame")
        .expect("first frame decode");
    let second = futures_util::StreamExt::next(&mut frames)
        .await
        .expect("second frame")
        .expect("second frame decode");
    assert_eq!(first, &source[..512]);
    assert_eq!(second, &source[512..]);
}

struct PartialWriter(Vec<u8>);

impl AsyncWrite for PartialWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        source: &[u8],
    ) -> Poll<io::Result<usize>> {
        let written = source.len().min(2);
        self.0.extend_from_slice(&source[..written]);
        Poll::Ready(Ok(written))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn receiver_writes_each_framed_payload_completely() {
    let payload = BytesMut::from(&b"complete frame payload"[..]);
    let mut frames = stream::iter([Ok::<_, io::Error>(payload.clone())]);
    let mut writer = PartialWriter(Vec::new());

    forward_playback_frames(&mut frames, &mut writer).await;

    assert_eq!(writer.0, payload);
}

#[tokio::test]
async fn process_probe_spawns_pipes_exits_and_reaps_once_without_ffmpeg() {
    let probe = ProcessBoundaryProbe::default();

    let observation = timeout(Duration::from_secs(5), probe.spawn_pipe_exit_and_reap())
        .await
        .expect("current test executable must finish its help process promptly")
        .expect("current test executable must spawn with a piped stdout");

    assert!(observation.status.success());
    assert!(!observation.stdout.is_empty());
    assert_eq!(probe.started(), 1);
    assert_eq!(probe.reaped(), 1);
}

#[tokio::test]
async fn process_spawn_failure_is_reported_without_waiting_for_ffmpeg() {
    let missing_program = std::env::temp_dir().join(format!(
        "telepathy-missing-screenshare-process-{}",
        std::process::id()
    ));

    let error = tokio::process::Command::new(missing_program)
        .spawn()
        .expect_err("a missing process must fail during spawn");

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[tokio::test]
async fn playback_stops_on_stream_reset_without_writing_a_partial_frame() {
    let mut frames = stream::iter([Err::<BytesMut, io::Error>(io::Error::new(
        io::ErrorKind::ConnectionReset,
        "simulated media stream reset",
    ))]);
    let mut writer = PartialWriter(Vec::new());

    forward_playback_frames(&mut frames, &mut writer).await;

    assert!(writer.0.is_empty());
}

#[tokio::test]
async fn blocked_capture_stops_when_current_record_select_observes_stop() {
    let (mut source_writer, mut source_reader) = tokio::io::duplex(128);
    let source = tokio::spawn(async move {
        let _ = source_writer.write_all(&vec![9_u8; 64 * 1024]).await;
    });

    let (frame_writer, _frame_reader) = tokio::io::duplex(128);
    let mut transport = FramedWrite::new(frame_writer, LengthDelimitedCodec::new());
    let stop = Arc::new(Notify::new());
    let transfer_stop = Arc::clone(&stop);
    let transfer = tokio::spawn(async move {
        tokio::select! {
            _ = forward_capture_chunks(&mut source_reader, &mut transport) => false,
            _ = transfer_stop.notified() => true,
        }
    });
    let mut transfer = Box::pin(transfer);

    timeout(Duration::from_millis(100), &mut transfer)
        .await
        .expect_err("capture must remain blocked while its framed receiver is not reading");
    stop.notify_waiters();
    assert!(
        timeout(Duration::from_secs(1), transfer)
            .await
            .expect("blocked capture must observe stop promptly")
            .expect("capture task must not panic")
    );
    timeout(Duration::from_secs(1), source)
        .await
        .expect("capture source must close after stop")
        .expect("capture source task must not panic");
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_session_rejects_video_before_active_call_uses_existing_behavior() {
    init_test_tracing();
    let relay_map = shared_relay_map();
    let codec_config = CodecConfig::new(true, true, 5.0);
    let key_a = iroh::SecretKey::generate();
    let key_b = iroh::SecretKey::generate();
    let contact_a = Contact::new("video-client-a".to_string(), key_a.public().to_string())
        .expect("contact a must be valid");
    let contact_b = Contact::new("video-client-b".to_string(), key_b.public().to_string())
        .expect("contact b must be valid");
    let call_states_a = Arc::new(Mutex::new(Vec::new()));
    let call_states_b = Arc::new(Mutex::new(Vec::new()));
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
        Arc::clone(&call_states_a),
        None,
        ManagerLifecycle::Restartable,
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
        Arc::clone(&call_states_b),
        None,
        ManagerLifecycle::Restartable,
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

    for _ in 0..2 {
        assert_eq!(
            client_a
                .telepathy
                .request_video_source(&contact_b, VideoSource::Display)
                .await,
            VideoStartOutcome::NoSession
        );
    }
    assert_eq!(
        client_a.telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle
    );

    client_a
        .telepathy
        .start_call(&contact_b)
        .await
        .expect("caller must start the real two-peer call");
    wait_for_connected(&call_states_a, "video sender").await;
    wait_for_connected(&call_states_b, "video receiver").await;

    let outcome = client_a
        .telepathy
        .request_video_source(&contact_b, VideoSource::Display)
        .await;

    assert_eq!(
        outcome,
        VideoStartOutcome::Unavailable(VideoUnavailable::ConfigurationUnavailable)
    );

    assert!(
        !call_states_a
            .lock()
            .unwrap()
            .iter()
            .any(|state| matches!(state, CallState::CallEnded(_, _)))
    );

    timeout(Duration::from_secs(15), client_a.telepathy.end_call())
        .await
        .expect("call teardown must not hang after a no-config screenshare request");
    client_a.stop_session_and_wait_for_runtime(&contact_b).await;
    client_b.stop_session_and_wait_for_runtime(&contact_a).await;
    assert_eq!(
        client_a.telepathy.inner.core_state.call_slot.current(),
        CallSlotState::Idle
    );

    shutdown_guard.disarm();
    drop(shutdown_guard);
    client_a.telepathy.shutdown().await;
    client_b.telepathy.shutdown().await;
}
