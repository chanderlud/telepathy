use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use iroh::endpoint::{Connection, presets};
use telepathy_core::internal::video::transport::{read_preamble, write_preamble};
use telepathy_core::internal::video::{
    VideoControl, VideoMediaDescriptor, VideoPreamble, VideoRejectReason, VideoSlot,
    VideoSlotEffect, VideoWorkerStartup,
};
use telepathy_core::types::{
    VideoCodec, VideoMediaFormat, VideoPhase, VideoRole, VideoTerminalReason,
};
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

pub(super) struct IrohPair {
    pub(super) client: iroh::Endpoint,
    pub(super) server: iroh::Endpoint,
    pub(super) outbound: Connection,
    pub(super) inbound: Connection,
}

impl IrohPair {
    pub(super) async fn connect() -> Self {
        let server = iroh::Endpoint::builder(presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .alpns(vec![b"telepathy/session/1".to_vec()])
            .bind()
            .await
            .expect("server endpoint binds");
        let client = iroh::Endpoint::builder(presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint binds");
        let server_addr = server.addr();
        let (outbound, inbound) =
            tokio::join!(client.connect(server_addr, b"telepathy/session/1"), async {
                server
                    .accept()
                    .await
                    .expect("server receives connection")
                    .await
            });
        Self {
            client,
            server,
            outbound: outbound.expect("client connects"),
            inbound: inbound.expect("server accepts"),
        }
    }

    pub(super) async fn close(self) {
        self.client.close().await;
        self.server.close().await;
    }
}

#[tokio::test]
async fn incoming_offer_admission_rejects_incompatible_format_before_reserving_receiver() {
    let slot = VideoSlot::default();
    let incompatible_session_id = VideoSlot::default()
        .start_local(VideoMediaDescriptor::display(VideoCodec::Hevc, 1280, 720))
        .await
        .expect("test session starts")
        .session_id();
    let incompatible = VideoControl::offer(
        incompatible_session_id,
        VideoMediaDescriptor::display(VideoCodec::Hevc, 1280, 720),
    );
    let VideoControl::Offer(incompatible) = incompatible else {
        unreachable!();
    };

    assert!(matches!(
        slot.receive_offer(
            incompatible,
            true,
            &[VideoMediaFormat::MpegTs(VideoCodec::H264)]
        )
        .await,
        VideoSlotEffect::Send(VideoControl::Reject {
            reason: VideoRejectReason::UnsupportedCodec,
            ..
        })
    ));
    assert!(
        slot.current_event("peer".to_string(), VideoPhase::Terminal, None)
            .await
            .is_none()
    );

    let compatible_session_id = VideoSlot::default()
        .start_local(VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720))
        .await
        .expect("test session starts")
        .session_id();
    let compatible = VideoControl::offer(
        compatible_session_id,
        VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720),
    );
    let VideoControl::Offer(compatible) = compatible else {
        unreachable!();
    };
    assert!(matches!(
        slot.receive_offer(
            compatible,
            true,
            &[VideoMediaFormat::MpegTs(VideoCodec::H264)]
        )
        .await,
        VideoSlotEffect::SendAndLaunch(_, _)
    ));
    assert!(
        slot.current_event("peer".to_string(), VideoPhase::Starting, None)
            .await
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_real_peers_negotiate_activate_stop_and_restart_from_both_sides() {
    let pair = IrohPair::connect().await;
    let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720);
    let slot_a = Arc::new(VideoSlot::default());
    let slot_b = Arc::new(VideoSlot::default());

    for sender_is_a in [true, false] {
        let (sender_slot, receiver_slot, sender_connection, receiver_connection) = if sender_is_a {
            (&slot_a, &slot_b, &pair.outbound, &pair.inbound)
        } else {
            (&slot_b, &slot_a, &pair.inbound, &pair.outbound)
        };
        let offer = sender_slot
            .start_local(descriptor)
            .await
            .expect("idle sender reserves a generation");
        assert!(sender_slot.start_local(descriptor).await.is_none());
        let (ready, receiver_launch) = match receiver_slot.receive(offer, true).await {
            VideoSlotEffect::SendAndLaunch(ready, launch) => (ready, launch),
            other => panic!("receiver must accept the offer, got {other:?}"),
        };
        let sender_launch = match sender_slot.receive(ready, true).await {
            VideoSlotEffect::Launch(launch) => launch,
            other => panic!("sender must launch after ready, got {other:?}"),
        };
        let session_id = offer.session_id();
        let sender_exited = Arc::new(AtomicBool::new(false));
        let receiver_exited = Arc::new(AtomicBool::new(false));
        let sender_cancel = sender_launch.cancellation().clone();
        let sender_connection = sender_connection.clone();
        let sender_done = Arc::clone(&sender_exited);
        let sender_worker = tokio::spawn(async move {
            let mut stream = sender_connection
                .open_uni()
                .await
                .expect("media stream opens");
            write_preamble(&mut stream, VideoPreamble::new(session_id, descriptor))
                .await
                .expect("preamble writes");
            let codec = LengthDelimitedCodec::builder()
                .max_frame_length(telepathy_core::internal::video::VIDEO_MEDIA_MAX_FRAME_LENGTH)
                .new_codec();
            let mut framed = FramedWrite::new(stream, codec);
            framed
                .send(Bytes::from_static(b"bounded-video-frame"))
                .await
                .expect("media frame writes");
            sender_cancel.cancelled().await;
            let _ = framed.into_inner().finish();
            sender_done.store(true, Ordering::Relaxed);
        });
        let receiver_cancel = receiver_launch.cancellation().clone();
        let receiver_connection = receiver_connection.clone();
        let receiver_done = Arc::clone(&receiver_exited);
        let receiver_worker = tokio::spawn(async move {
            let mut stream = receiver_connection
                .accept_uni()
                .await
                .expect("media stream accepted");
            assert_eq!(
                read_preamble(&mut stream).await.expect("preamble reads"),
                VideoPreamble::new(session_id, descriptor)
            );
            let codec = LengthDelimitedCodec::builder()
                .max_frame_length(telepathy_core::internal::video::VIDEO_MEDIA_MAX_FRAME_LENGTH)
                .new_codec();
            let mut framed = FramedRead::new(stream, codec);
            assert_eq!(
                framed
                    .next()
                    .await
                    .expect("media frame arrives")
                    .expect("media frame reads")
                    .as_ref(),
                b"bounded-video-frame"
            );
            receiver_cancel.cancelled().await;
            receiver_done.store(true, Ordering::Relaxed);
        });
        assert!(sender_slot.install(&sender_launch, sender_worker).await);
        assert!(
            sender_slot
                .complete_startup(
                    &sender_launch,
                    VideoWorkerStartup::Ready,
                    "sender".to_string()
                )
                .await
                .is_some()
        );
        assert!(
            receiver_slot
                .install(&receiver_launch, receiver_worker)
                .await
        );
        assert!(
            receiver_slot
                .complete_startup(
                    &receiver_launch,
                    VideoWorkerStartup::Ready,
                    "receiver".to_string()
                )
                .await
                .is_some()
        );
        assert_eq!(
            sender_slot
                .current_event("sender".to_string(), VideoPhase::Active, None)
                .await
                .expect("sender event")
                .role,
            VideoRole::Sender
        );
        assert_eq!(
            receiver_slot
                .current_event("receiver".to_string(), VideoPhase::Active, None)
                .await
                .expect("receiver event")
                .role,
            VideoRole::Receiver
        );

        let stop = VideoControl::stop(session_id, VideoTerminalReason::Stopped);
        let receiver_attempt = match receiver_slot.receive(stop, true).await {
            VideoSlotEffect::Terminal(attempt, VideoTerminalReason::Stopped) => attempt,
            other => panic!("remote stop must terminate receiver, got {other:?}"),
        };
        let (sender_result, receiver_result) = tokio::join!(
            sender_slot.cancel_and_join(sender_launch.attempt(), VideoTerminalReason::Stopped),
            receiver_slot.cancel_and_join(receiver_attempt, VideoTerminalReason::Stopped)
        );
        assert_eq!(sender_result, Some(VideoTerminalReason::Stopped));
        assert_eq!(receiver_result, Some(VideoTerminalReason::Stopped));
        assert!(sender_exited.load(Ordering::Relaxed));
        assert!(receiver_exited.load(Ordering::Relaxed));
        assert!(
            sender_slot
                .current_event("sender".to_string(), VideoPhase::Terminal, None)
                .await
                .is_none()
        );
        assert!(
            receiver_slot
                .current_event("receiver".to_string(), VideoPhase::Terminal, None)
                .await
                .is_none()
        );
    }

    pair.close().await;
}

#[tokio::test]
async fn failed_worker_startup_never_activates_and_still_joins_on_terminal_cleanup() {
    let slot = VideoSlot::default();
    let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720);
    let offer = slot
        .start_local(descriptor)
        .await
        .expect("sender reserves a generation");
    let launch = match slot
        .receive(VideoControl::ready(offer.session_id()), true)
        .await
    {
        VideoSlotEffect::Launch(launch) => launch,
        other => panic!("ready must launch sender, got {other:?}"),
    };
    let cancellation = launch.cancellation().clone();
    let worker = tokio::spawn(async move { cancellation.cancelled().await });

    assert!(slot.install(&launch, worker).await);
    assert!(
        slot.complete_startup(&launch, VideoWorkerStartup::Failed, "sender".to_string())
            .await
            .is_none()
    );
    assert_eq!(
        slot.cancel_and_join(launch.attempt(), VideoTerminalReason::Failed)
            .await,
        Some(VideoTerminalReason::Failed)
    );
    assert!(
        slot.current_event("sender".to_string(), VideoPhase::Terminal, None)
            .await
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn crossed_starts_and_stale_completions_preserve_replacement_generation_under_stress() {
    let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720);
    let slot = VideoSlot::default();

    for _ in 0..20 {
        let local = slot
            .start_local(descriptor)
            .await
            .expect("local generation starts");
        let remote_id = VideoSlot::default()
            .start_local(descriptor)
            .await
            .expect("remote identity is generated")
            .session_id();
        let (displaced, replacement) = match slot
            .receive(VideoControl::offer(remote_id, descriptor), false)
            .await
        {
            VideoSlotEffect::DisplaceAndSendAndLaunch(displaced, _, launch) => (displaced, launch),
            other => panic!("canonical remote offer must replace local, got {other:?}"),
        };
        let terminal = displaced
            .cancel_and_join("peer".to_string(), VideoTerminalReason::Rejected)
            .await;
        assert_eq!(terminal.identity.session_id, local.session_id());
        assert_eq!(terminal.role, VideoRole::Sender);
        assert_eq!(terminal.phase, VideoPhase::Terminal);
        assert_eq!(
            terminal.terminal_reason,
            Some(VideoTerminalReason::Rejected)
        );
        assert!(matches!(
            slot.receive(VideoControl::ready(local.session_id()), false)
                .await,
            VideoSlotEffect::Ignored
        ));
        assert!(matches!(
            slot.receive(
                VideoControl::stop(local.session_id(), VideoTerminalReason::Failed),
                false
            )
            .await,
            VideoSlotEffect::Ignored
        ));
        let worker_cancel = replacement.cancellation().clone();
        let worker = tokio::spawn(async move { worker_cancel.cancelled().await });
        assert!(slot.install(&replacement, worker).await);
        let active = slot
            .complete_startup(&replacement, VideoWorkerStartup::Ready, "peer".to_string())
            .await
            .expect("winning receiver starts");
        assert_eq!(active.identity.session_id, remote_id);
        assert_eq!(active.role, VideoRole::Receiver);
        timeout(
            Duration::from_secs(2),
            slot.cancel_and_join(replacement.attempt(), VideoTerminalReason::Stopped),
        )
        .await
        .expect("replacement cleanup is bounded");
        assert!(
            slot.current_event("peer".to_string(), VideoPhase::Terminal, None)
                .await
                .is_none()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn teardown_does_not_publish_idle_before_blocked_worker_joins() {
    let slot = Arc::new(VideoSlot::default());
    let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720);
    let offer = slot
        .start_local(descriptor)
        .await
        .expect("generation starts");
    let launch = match slot
        .receive(VideoControl::ready(offer.session_id()), true)
        .await
    {
        VideoSlotEffect::Launch(launch) => launch,
        other => panic!("ready must launch sender, got {other:?}"),
    };
    let release = Arc::new(tokio::sync::Notify::new());
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_release = Arc::clone(&release);
    let worker_cancelled = Arc::clone(&cancelled);
    let cancellation = launch.cancellation().clone();
    let worker = tokio::spawn(async move {
        cancellation.cancelled().await;
        worker_cancelled.store(true, Ordering::Relaxed);
        worker_release.notified().await;
    });
    assert!(slot.install(&launch, worker).await);
    let cleanup_slot = Arc::clone(&slot);
    let attempt = launch.attempt();
    let cleanup = tokio::spawn(async move {
        cleanup_slot
            .cancel_and_join(attempt, VideoTerminalReason::Teardown)
            .await
    });
    while !cancelled.load(Ordering::Relaxed) {
        tokio::task::yield_now().await;
    }
    assert!(
        slot.current_event("peer".to_string(), VideoPhase::Stopping, None)
            .await
            .is_some()
    );
    assert!(!cleanup.is_finished());
    release.notify_one();
    assert_eq!(
        timeout(Duration::from_secs(2), cleanup)
            .await
            .expect("cleanup is bounded")
            .expect("cleanup task joins"),
        Some(VideoTerminalReason::Teardown)
    );
    assert!(
        slot.current_event("peer".to_string(), VideoPhase::Terminal, None)
            .await
            .is_none()
    );
}
