use super::lifecycle::IrohPair;
use speedy::Writable;
use telepathy_core::internal::video::transport::{read_media_frame, read_preamble};
use telepathy_core::internal::video::{
    VIDEO_CONTROL_MAX_FRAME_LENGTH, VIDEO_MEDIA_MAX_FRAME_LENGTH, VIDEO_PREAMBLE_MAX_LENGTH,
    VideoMediaDescriptor, VideoProtocolError, VideoSlot, decode_video_control,
};
use telepathy_core::types::VideoCodec;
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread")]
async fn malformed_preamble_and_over_limit_frame_fail_on_real_iroh_streams() {
    let pair = IrohPair::connect().await;
    let sender_connection = pair.outbound.clone();
    let malformed = tokio::spawn(async move {
        let mut stream = sender_connection.open_uni().await.expect("stream opens");
        stream
            .write_u16((VIDEO_PREAMBLE_MAX_LENGTH + 1) as u16)
            .await
            .expect("length writes");
        stream.finish().expect("stream finishes");
    });
    let mut malformed_stream = pair.inbound.accept_uni().await.expect("stream accepted");
    assert_eq!(
        read_preamble(&mut malformed_stream)
            .await
            .expect_err("oversized preamble fails")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    malformed.await.expect("malformed sender joins");

    let sender_connection = pair.outbound.clone();
    let oversized = tokio::spawn(async move {
        let mut stream = sender_connection.open_uni().await.expect("stream opens");
        stream
            .write_u32((VIDEO_MEDIA_MAX_FRAME_LENGTH + 1) as u32)
            .await
            .expect("length writes");
        stream.finish().expect("stream finishes");
    });
    let mut oversized_stream = pair.inbound.accept_uni().await.expect("stream accepted");
    assert_eq!(
        read_media_frame(&mut oversized_stream)
            .await
            .expect_err("oversized frame fails")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    oversized.await.expect("oversized sender joins");
    pair.close().await;
}

#[tokio::test]
async fn malformed_and_over_limit_controls_are_rejected_before_negotiation() {
    assert_eq!(
        decode_video_control(&[0xFF]),
        Err(VideoProtocolError::Malformed)
    );
    assert_eq!(
        decode_video_control(&vec![0; VIDEO_CONTROL_MAX_FRAME_LENGTH + 1]),
        Err(VideoProtocolError::FrameTooLarge)
    );
    let invalid_offer = VideoSlot::default()
        .start_local(VideoMediaDescriptor::display(VideoCodec::H264, 0, 720))
        .await
        .expect("invalid descriptor still reaches wire validation");
    assert_eq!(
        decode_video_control(&invalid_offer.write_to_vec().expect("control encodes")),
        Err(VideoProtocolError::InvalidDimensions)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_real_iroh_receiver_applies_bounded_backpressure_and_stop_joins_sender() {
    let pair = IrohPair::connect().await;
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let sender_connection = pair.outbound.clone();
    let sender = tokio::spawn(async move {
        let mut stream = sender_connection.open_uni().await.expect("stream opens");
        let frame = vec![0x5A; VIDEO_MEDIA_MAX_FRAME_LENGTH];
        let mut sent = 0_usize;
        loop {
            tokio::select! {
                biased;
                _ = worker_cancellation.cancelled() => {
                    let _ = stream.reset(iroh::endpoint::VarInt::from_u32(1));
                    return sent;
                }
                result = telepathy_core::internal::video::transport::write_media_frame(
                    &mut stream,
                    &frame,
                ) => {
                    result.expect("frame writes until cancellation");
                    sent += 1;
                    assert!(sent < 2_048, "sender bypassed transport backpressure");
                }
            }
        }
    });
    let _held_stream = pair.inbound.accept_uni().await.expect("stream accepted");

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !sender.is_finished(),
        "slow receiver must backpressure sender"
    );
    cancellation.cancel();
    let sent = timeout(Duration::from_secs(2), sender)
        .await
        .expect("cancelled sender joins promptly")
        .expect("sender task does not panic");
    assert!(sent < 2_048);
    pair.close().await;
}
