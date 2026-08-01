use crate::internal::error::{Error as CoreError, ErrorKind as CoreErrorKind};
use crate::internal::video::platform;
use crate::internal::video::{
    VIDEO_MEDIA_MAX_FRAME_LENGTH, VIDEO_NEGOTIATION_TIMEOUT, VIDEO_PREAMBLE_MAX_LENGTH,
    VideoPreamble, VideoProtocolError, VideoWorkerStartup, decode_preamble, encode_preamble,
};
use crate::types::RecordingConfig;
use iroh::endpoint::{Connection, VarInt};
use std::io::{Error, ErrorKind, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;

fn protocol_error(error: VideoProtocolError) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!("invalid video transport data: {error:?}"),
    )
}

pub async fn write_preamble<W>(writer: &mut W, preamble: VideoPreamble) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let bytes = encode_preamble(&preamble).map_err(protocol_error)?;
    let length = u16::try_from(bytes.len())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "video preamble exceeds u16 length"))?;
    writer.write_u16(length).await?;
    writer.write_all(&bytes).await
}

pub async fn read_preamble<R>(reader: &mut R) -> Result<VideoPreamble>
where
    R: AsyncRead + Unpin,
{
    let length = usize::from(reader.read_u16().await?);
    if length > VIDEO_PREAMBLE_MAX_LENGTH {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "video preamble exceeds limit",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    decode_preamble(&bytes).map_err(protocol_error)
}

pub(crate) async fn read_preamble_until_cancelled<R>(
    reader: &mut R,
    cancellation: &CancellationToken,
) -> Result<VideoPreamble>
where
    R: AsyncRead + Unpin,
{
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(Error::new(ErrorKind::Interrupted, "video transport cancelled")),
        preamble = read_preamble(reader) => preamble,
    }
}

pub(crate) async fn run_sender(
    connection: &Connection,
    preamble: VideoPreamble,
    config: RecordingConfig,
    cancellation: &CancellationToken,
    startup: oneshot::Sender<VideoWorkerStartup>,
) -> std::result::Result<(), CoreError> {
    let mut stream = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Ok(());
        },
        _ = tokio::time::sleep(VIDEO_NEGOTIATION_TIMEOUT) => {
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Err(CoreErrorKind::TransportSend.into());
        },
        stream = connection.open_uni() => match stream {
            Ok(stream) => stream,
            Err(_) => {
                let _ = startup.send(VideoWorkerStartup::Failed);
                return Err(CoreErrorKind::TransportSend.into());
            }
        },
    };
    let preamble_result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            let _ = stream.reset(VarInt::from_u32(1));
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Ok(());
        },
        result = write_preamble(&mut stream, preamble) => result,
    };
    if let Err(error) = preamble_result {
        let _ = stream.reset(VarInt::from_u32(1));
        let _ = startup.send(VideoWorkerStartup::Failed);
        return Err(error.into());
    }
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(VIDEO_MEDIA_MAX_FRAME_LENGTH)
        .new_codec();
    let mut framed = FramedWrite::new(stream, codec);
    let result = platform::run_sender(&mut framed, cancellation, config, startup).await;
    let mut stream = framed.into_inner();
    if result.is_ok() || cancellation.is_cancelled() {
        let _ = stream.finish();
    } else {
        let _ = stream.reset(VarInt::from_u32(1));
    }
    result
}

pub(crate) async fn run_receiver(
    connection: &Connection,
    expected: VideoPreamble,
    cancellation: &CancellationToken,
    startup: oneshot::Sender<VideoWorkerStartup>,
) -> std::result::Result<(), CoreError> {
    let mut stream = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Ok(());
        },
        _ = tokio::time::sleep(VIDEO_NEGOTIATION_TIMEOUT) => {
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Err(CoreErrorKind::TransportRecv.into());
        },
        stream = connection.accept_uni() => match stream {
            Ok(stream) => stream,
            Err(_) => {
                let _ = startup.send(VideoWorkerStartup::Failed);
                return Err(CoreErrorKind::TransportRecv.into());
            }
        },
    };
    let preamble = match read_preamble_until_cancelled(&mut stream, cancellation).await {
        Ok(preamble) => preamble,
        Err(error) => {
            let _ = stream.stop(VarInt::from_u32(1));
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Err(error.into());
        }
    };
    if preamble != expected {
        let _ = stream.stop(VarInt::from_u32(1));
        let _ = startup.send(VideoWorkerStartup::Failed);
        return Err(CoreErrorKind::TransportRecv.into());
    }
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(VIDEO_MEDIA_MAX_FRAME_LENGTH)
        .new_codec();
    let mut framed = FramedRead::new(stream, codec);
    let result =
        platform::run_receiver(&mut framed, cancellation, expected.descriptor, startup).await;
    let mut stream = framed.into_inner();
    if result.is_err() && !cancellation.is_cancelled() {
        let _ = stream.stop(VarInt::from_u32(1));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{read_preamble, read_preamble_until_cancelled, run_receiver, write_preamble};
    use crate::internal::ALPN;
    use crate::internal::video::{
        VIDEO_MEDIA_MAX_FRAME_LENGTH, VideoCodec, VideoControl, VideoMediaDescriptor, VideoPhase,
        VideoPreamble, VideoSessionId, VideoSlot, VideoTerminalReason, VideoWorkerStartup,
    };
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};
    use iroh::endpoint::Connection;
    use tokio::io::duplex;
    use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
    use tokio_util::sync::CancellationToken;

    async fn iroh_pair() -> (iroh::Endpoint, iroh::Endpoint, Connection, Connection) {
        use iroh::endpoint::presets;

        let server = iroh::Endpoint::builder(presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .expect("server endpoint binds");
        let client = iroh::Endpoint::builder(presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint binds");
        let server_addr = server.addr();
        let (outbound, inbound) = tokio::join!(client.connect(server_addr, ALPN), async {
            server
                .accept()
                .await
                .expect("server receives connection")
                .await
        });
        (
            client,
            server,
            outbound.expect("client connects"),
            inbound.expect("server accepts"),
        )
    }

    #[tokio::test]
    async fn preamble_round_trips_before_media() {
        let (mut sender, mut receiver) = duplex(1024);
        let preamble = VideoPreamble::new(
            VideoSessionId::new(),
            VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720),
        );

        let send = tokio::spawn(async move { write_preamble(&mut sender, preamble).await });
        let received = read_preamble(&mut receiver).await;

        assert_eq!(received.expect("preamble is received"), preamble);
        assert!(send.await.expect("writer joins").is_ok());
    }

    #[tokio::test]
    async fn cancelled_preamble_read_returns_without_waiting_for_peer() {
        let (_sender, mut receiver) = duplex(1024);
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        let result = read_preamble_until_cancelled(&mut receiver, &cancel).await;

        assert_eq!(
            result.expect_err("cancelled read fails").kind(),
            std::io::ErrorKind::Interrupted
        );
    }

    #[tokio::test]
    async fn partial_preamble_returns_unexpected_eof() {
        let (mut sender, mut receiver) = duplex(1024);
        tokio::io::AsyncWriteExt::write_all(&mut sender, &[0, 4, 1])
            .await
            .expect("prefix writes");
        drop(sender);

        let result = read_preamble(&mut receiver).await;

        assert_eq!(
            result.expect_err("partial preamble fails").kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_iroh_peers_exchange_preamble_and_bounded_media_frame() {
        let (client, server, outbound, inbound) = iroh_pair().await;
        let preamble = VideoPreamble::new(
            VideoSessionId::new(),
            VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720),
        );
        let payload = vec![0x5A; VIDEO_MEDIA_MAX_FRAME_LENGTH];
        let expected = payload.clone();

        let sender_connection = outbound.clone();
        let sender = tokio::spawn(async move {
            let mut stream = sender_connection
                .open_uni()
                .await
                .expect("uni stream opens");
            write_preamble(&mut stream, preamble)
                .await
                .expect("preamble writes");
            let codec = LengthDelimitedCodec::builder()
                .max_frame_length(VIDEO_MEDIA_MAX_FRAME_LENGTH)
                .new_codec();
            let mut framed = FramedWrite::new(stream, codec);
            framed
                .send(Bytes::from(payload))
                .await
                .expect("media frame writes");
            framed.into_inner().finish().expect("stream finishes");
        });
        let mut stream = inbound.accept_uni().await.expect("uni stream accepted");
        assert_eq!(
            read_preamble(&mut stream).await.expect("preamble reads"),
            preamble
        );
        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(VIDEO_MEDIA_MAX_FRAME_LENGTH)
            .new_codec();
        let mut framed = FramedRead::new(stream, codec);
        assert_eq!(
            framed
                .next()
                .await
                .expect("media frame arrives")
                .expect("media frame reads")
                .as_ref(),
            expected.as_slice()
        );
        sender.await.expect("sender joins");
        client.close().await;
        server.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn receiver_reports_failed_startup_when_preamble_does_not_match() {
        let (client, server, outbound, inbound) = iroh_pair().await;
        let expected = VideoPreamble::new(
            VideoSessionId::new(),
            VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720),
        );
        let mismatched = VideoPreamble::new(
            VideoSessionId::new(),
            VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720),
        );
        let cancellation = CancellationToken::new();
        let (startup_sender, startup_receiver) = tokio::sync::oneshot::channel();
        let receiver = tokio::spawn(async move {
            run_receiver(&inbound, expected, &cancellation, startup_sender).await
        });
        let mut stream = outbound.open_uni().await.expect("uni stream opens");
        write_preamble(&mut stream, mismatched)
            .await
            .expect("mismatched preamble writes");

        assert_eq!(
            startup_receiver.await.expect("startup result is reported"),
            VideoWorkerStartup::Failed
        );
        assert!(receiver.await.expect("receiver worker joins").is_err());
        client.close().await;
        server.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn real_iroh_accept_wait_is_cancelled_and_joined() {
        let (client, server, _outbound, inbound) = iroh_pair().await;
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = worker_cancellation.cancelled() => true,
                _ = inbound.accept_uni() => false,
            }
        });

        cancellation.cancel();

        assert!(
            worker
                .await
                .expect("accept worker joins after cancellation")
        );
        client.close().await;
        server.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn slot_becomes_idle_only_after_real_iroh_accept_worker_joins() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (client, server, _outbound, inbound) = iroh_pair().await;
        let slot = Arc::new(VideoSlot::default());
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720);
        let session_id = VideoSessionId::new();
        let launch = slot
            .receive(VideoControl::offer(session_id, descriptor), true)
            .await
            .launch()
            .expect("accepted offer arms receiver");
        let joined = Arc::new(AtomicBool::new(false));
        let worker_joined = Arc::clone(&joined);
        let cancellation = launch.cancellation().clone();
        let worker = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {}
                _ = inbound.accept_uni() => {}
            }
            worker_joined.store(true, Ordering::Relaxed);
        });
        assert!(slot.install(&launch, worker).await);

        slot.cancel_and_join(launch.attempt(), VideoTerminalReason::Teardown)
            .await;

        assert!(joined.load(Ordering::Relaxed));
        assert!(
            slot.current_event("peer".to_string(), VideoPhase::Terminal, None,)
                .await
                .is_none()
        );
        client.close().await;
        server.close().await;
    }
}
