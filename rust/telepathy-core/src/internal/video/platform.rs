use crate::internal::error::Error;
use crate::types::{Capabilities, VideoMediaFormat, VideoSource, VideoUnavailable};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[path = "platform/desktop_ffmpeg.rs"]
mod selected;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[path = "platform/unsupported.rs"]
mod selected;
#[cfg(test)]
mod unsupported_contract {
    include!("platform/unsupported.rs");
}

pub(crate) use selected::{
    Decoder, Device, Encoder, initial_video_capabilities, prepare_sender, probe_capabilities,
    run_receiver, run_sender,
};
#[cfg(feature = "integration-testing")]
pub use selected::{playback_command_for_test, recording_command_for_test};

type Result<T> = std::result::Result<T, Error>;
const BUFFER_SIZE: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VideoAvailability<T> {
    Available(T),
    Unavailable(VideoUnavailable),
}

impl<T> VideoAvailability<T> {
    const fn as_result(&self) -> std::result::Result<&T, VideoUnavailable> {
        match self {
            Self::Available(value) => Ok(value),
            Self::Unavailable(reason) => Err(*reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoSourceCapability {
    source: VideoSource,
    formats: Vec<VideoMediaFormat>,
}

impl VideoSourceCapability {
    pub(crate) const fn new(source: VideoSource, formats: Vec<VideoMediaFormat>) -> Self {
        Self { source, formats }
    }

    pub(crate) fn into_parts(self) -> (VideoSource, Vec<VideoMediaFormat>) {
        (self.source, self.formats)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoCapabilities {
    send: VideoAvailability<Vec<VideoSourceCapability>>,
    receive: VideoAvailability<Vec<VideoMediaFormat>>,
}

pub(crate) struct CapabilityProbe {
    compatibility: Capabilities,
    video: VideoCapabilities,
}

impl CapabilityProbe {
    pub(crate) const fn new(compatibility: Capabilities, video: VideoCapabilities) -> Self {
        Self {
            compatibility,
            video,
        }
    }

    pub(crate) fn into_parts(self) -> (Capabilities, VideoCapabilities) {
        (self.compatibility, self.video)
    }
}

impl VideoCapabilities {
    pub(crate) const fn new(
        send: VideoAvailability<Vec<VideoSourceCapability>>,
        receive: VideoAvailability<Vec<VideoMediaFormat>>,
    ) -> Self {
        Self { send, receive }
    }

    pub(crate) const fn unavailable(reason: VideoUnavailable) -> Self {
        Self {
            send: VideoAvailability::Unavailable(reason),
            receive: VideoAvailability::Unavailable(reason),
        }
    }

    pub(crate) fn send(&self) -> std::result::Result<&[VideoSourceCapability], VideoUnavailable> {
        self.send.as_result().map(Vec::as_slice)
    }

    pub(crate) fn receive(&self) -> std::result::Result<&[VideoMediaFormat], VideoUnavailable> {
        self.receive.as_result().map(Vec::as_slice)
    }

    pub(crate) fn into_availability(
        self,
    ) -> (
        VideoAvailability<Vec<VideoSourceCapability>>,
        VideoAvailability<Vec<VideoMediaFormat>>,
    ) {
        (self.send, self.receive)
    }

    pub(crate) fn formats(
        &self,
        source: VideoSource,
    ) -> std::result::Result<&[VideoMediaFormat], VideoUnavailable> {
        self.send()?
            .iter()
            .find(|capability| capability.source == source)
            .map(|capability| capability.formats.as_slice())
            .ok_or(VideoUnavailable::SourceUnavailable(source))
    }
}

pub(crate) fn encoder_from_str(value: &str) -> std::result::Result<Encoder, ()> {
    selected::encoder_from_str(value)
}

#[cfg(feature = "integration-testing")]
#[derive(Debug, PartialEq, Eq)]
pub struct CommandDescription {
    pub program: String,
    pub arguments: Vec<String>,
}

pub async fn forward_capture_chunks<R, S>(stdout: &mut R, transport: &mut S)
where
    R: AsyncRead + Unpin,
    S: futures_util::Sink<Bytes> + Unpin,
    S::Error: std::fmt::Display,
{
    let mut frame = [0_u8; BUFFER_SIZE];
    while let Ok(read) = stdout.read(&mut frame).await {
        if read == 0 {
            break;
        }
        if transport
            .send(Bytes::copy_from_slice(&frame[..read]))
            .await
            .is_err()
        {
            break;
        }
    }
}

pub async fn forward_playback_frames<S, W, E>(transport: &mut S, stdin: &mut W)
where
    S: futures_util::Stream<Item = std::result::Result<bytes::BytesMut, E>> + Unpin,
    W: AsyncWrite + Unpin,
{
    while let Some(Ok(message)) = transport.next().await {
        if stdin.write_all(&message).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Device, Encoder, VideoAvailability, VideoCapabilities, VideoSourceCapability,
        VideoUnavailable,
    };
    use crate::internal::video::{VideoCodec, VideoMediaFormat, VideoSource};
    use crate::types::RecordingConfig;

    #[test]
    fn capabilities_represent_send_only_source_formats_without_boolean_ambiguity() {
        let format = VideoMediaFormat::MpegTs(VideoCodec::H264);
        let capabilities = VideoCapabilities::new(
            VideoAvailability::Available(vec![VideoSourceCapability::new(
                VideoSource::Display,
                vec![format],
            )]),
            VideoAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable),
        );

        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Ok(&[format][..])
        );
        assert_eq!(
            capabilities.receive(),
            Err(VideoUnavailable::RuntimeUnavailable)
        );
    }

    #[test]
    fn unsupported_capabilities_use_the_same_typed_shape() {
        let capabilities = VideoCapabilities::unavailable(VideoUnavailable::PlatformUnsupported);

        assert_eq!(
            capabilities.send(),
            Err(VideoUnavailable::PlatformUnsupported)
        );
        assert_eq!(
            capabilities.receive(),
            Err(VideoUnavailable::PlatformUnsupported)
        );
        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Err(VideoUnavailable::PlatformUnsupported)
        );
    }

    #[test]
    fn available_empty_capabilities_remain_distinct_from_unavailable() {
        let capabilities = VideoCapabilities::new(
            VideoAvailability::Available(Vec::new()),
            VideoAvailability::Available(Vec::new()),
        );

        assert_eq!(capabilities.send(), Ok(&[][..]));
        assert_eq!(capabilities.receive(), Ok(&[][..]));
        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Err(VideoUnavailable::SourceUnavailable(VideoSource::Display))
        );
    }

    #[tokio::test]
    async fn unsupported_adapter_query_and_start_report_typed_unavailable() {
        let (compatibility, capabilities) = super::unsupported_contract::probe_capabilities()
            .await
            .into_parts();
        let config = RecordingConfig {
            encoder: Encoder::H264Nvenc,
            device: Device::X11Grab,
            bitrate: 4_000_000,
            framerate: 60,
            height: Some(720),
        };

        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Err(VideoUnavailable::PlatformUnsupported)
        );
        assert_eq!(
            super::unsupported_contract::prepare_sender(
                &config,
                1_280,
                720,
                &compatibility,
                &capabilities,
            ),
            Err(VideoUnavailable::PlatformUnsupported)
        );
    }
}
