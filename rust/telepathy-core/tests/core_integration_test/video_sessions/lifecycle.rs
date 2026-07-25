use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use iroh::endpoint::{Connection, presets};
use telepathy_core::internal::video::transport::{
    read_media_frame, read_preamble, write_media_frame, write_preamble,
};
use telepathy_core::internal::video::{
    VideoControl, VideoMediaDescriptor, VideoPreamble, VideoSlot, VideoSlotEffect,
};
use telepathy_core::types::{VideoCodec, VideoPhase, VideoRole, VideoTerminalReason};
use tokio::time::timeout;

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
            write_media_frame(&mut stream, b"bounded-video-frame")
                .await
                .expect("media frame writes");
            sender_cancel.cancelled().await;
            let _ = stream.finish();
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
            assert_eq!(
                read_media_frame(&mut stream)
                    .await
                    .expect("media frame reads")
                    .as_ref(),
                b"bounded-video-frame"
            );
            receiver_cancel.cancelled().await;
            receiver_done.store(true, Ordering::Relaxed);
        });
        assert!(sender_slot.install(&sender_launch, sender_worker).await);
        assert!(
            receiver_slot
                .install(&receiver_launch, receiver_worker)
                .await
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
        assert!(!sender_slot.is_reserved().await);
        assert!(!receiver_slot.is_reserved().await);
    }

    pair.close().await;
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
        let replacement = match slot
            .receive(VideoControl::offer(remote_id, descriptor), false)
            .await
        {
            VideoSlotEffect::SendAndLaunch(_, launch) => launch,
            other => panic!("canonical remote offer must replace local, got {other:?}"),
        };
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
        timeout(
            Duration::from_secs(2),
            slot.cancel_and_join(replacement.attempt(), VideoTerminalReason::Stopped),
        )
        .await
        .expect("replacement cleanup is bounded");
        assert!(!slot.is_reserved().await);
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
    assert!(slot.is_reserved().await);
    assert!(!cleanup.is_finished());
    release.notify_one();
    assert_eq!(
        timeout(Duration::from_secs(2), cleanup)
            .await
            .expect("cleanup is bounded")
            .expect("cleanup task joins"),
        Some(VideoTerminalReason::Teardown)
    );
    assert!(!slot.is_reserved().await);
}
