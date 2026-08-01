use super::lifecycle::IrohPair;
use bytes::Bytes;
use futures_util::SinkExt;
use telepathy_core::internal::video::transport::read_preamble;
use telepathy_core::internal::video::{VIDEO_MEDIA_MAX_FRAME_LENGTH, VIDEO_PREAMBLE_MAX_LENGTH};
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
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
    let oversized_stream = pair.inbound.accept_uni().await.expect("stream accepted");
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(VIDEO_MEDIA_MAX_FRAME_LENGTH)
        .new_codec();
    let mut framed = FramedRead::new(oversized_stream, codec);
    assert!(
        futures_util::StreamExt::next(&mut framed)
            .await
            .expect("oversized frame is observed")
            .is_err()
    );
    oversized.await.expect("oversized sender joins");
    pair.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_real_iroh_receiver_applies_bounded_backpressure_and_stop_joins_sender() {
    let pair = IrohPair::connect().await;
    let cancellation = CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let sender_connection = pair.outbound.clone();
    let sender = tokio::spawn(async move {
        let stream = sender_connection.open_uni().await.expect("stream opens");
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(VIDEO_MEDIA_MAX_FRAME_LENGTH)
            .new_codec();
        let mut framed = FramedWrite::new(stream, codec);
        let frame = vec![0x5A; VIDEO_MEDIA_MAX_FRAME_LENGTH];
        let mut sent = 0_usize;
        loop {
            tokio::select! {
                biased;
                _ = worker_cancellation.cancelled() => {
                    let _ = framed
                        .get_mut()
                        .reset(iroh::endpoint::VarInt::from_u32(1));
                    return sent;
                }
                result = framed.send(Bytes::copy_from_slice(&frame)) => {
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
