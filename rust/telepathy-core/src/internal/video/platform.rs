use crate::internal::error::Error;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use crate::internal::video::VideoCodec;
use crate::types::{Capabilities, VideoCapabilities};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use bytes::Bytes;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use futures_util::{SinkExt, StreamExt};
use speedy::{Readable, Writable};
use std::fmt::Display;
use std::str::FromStr;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[path = "platform/desktop_ffmpeg.rs"]
mod selected;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[path = "platform/unsupported.rs"]
mod selected;
pub(crate) use selected::{prepare_sender, probe_capabilities, run_receiver, run_sender};

type Result<T> = std::result::Result<T, Error>;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
const BUFFER_SIZE: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Readable, Writable)]
pub(crate) enum Device {
    DirectShow,
    GdiGrab,
    DesktopDuplication,
    AVFoundation(Vec<String>),
    X11Grab,
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl Device {
    #[cfg(target_os = "windows")]
    fn devices() -> Vec<Self> {
        vec![Self::DesktopDuplication, Self::GdiGrab]
    }

    #[cfg(not(target_os = "windows"))]
    fn devices() -> Vec<Self> {
        Vec::new()
    }
}

impl Display for Device {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectShow => formatter.write_str("DirectShow"),
            Self::GdiGrab => formatter.write_str("GDI Grab"),
            Self::DesktopDuplication => formatter.write_str("Desktop Duplication"),
            Self::AVFoundation(devices) => write!(formatter, "AVFoundation: {devices:?}"),
            Self::X11Grab => formatter.write_str("X11 Grab"),
        }
    }
}

impl FromStr for Device {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "DirectShow" => Ok(Self::DirectShow),
            "GDI Grab" => Ok(Self::GdiGrab),
            "Desktop Duplication" => Ok(Self::DesktopDuplication),
            "X11 Grab" => Ok(Self::X11Grab),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Readable, Writable)]
pub(crate) enum Encoder {
    Libx264,
    H264Nvenc,
    H264Amf,
    H264Qsv,
    H264Vaapi,
    Libx265,
    HevcNvenc,
    HevcAmf,
    HevcQsv,
    HevcVaapi,
    Av1Nvenc,
    Av1Amf,
    Av1Qsv,
    Av1Vaapi,
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl Encoder {
    const fn codec(self) -> VideoCodec {
        match self {
            Self::Libx264 | Self::H264Nvenc | Self::H264Amf | Self::H264Qsv | Self::H264Vaapi => {
                VideoCodec::H264
            }
            Self::Libx265 | Self::HevcNvenc | Self::HevcAmf | Self::HevcQsv | Self::HevcVaapi => {
                VideoCodec::Hevc
            }
            Self::Av1Nvenc | Self::Av1Amf | Self::Av1Qsv | Self::Av1Vaapi => VideoCodec::Av1,
        }
    }
}

impl From<Encoder> for &'static str {
    fn from(encoder: Encoder) -> Self {
        match encoder {
            Encoder::Libx264 => "libx264",
            Encoder::H264Nvenc => "h264_nvenc",
            Encoder::H264Amf => "h264_amf",
            Encoder::H264Qsv => "h264_qsv",
            Encoder::H264Vaapi => "h264_vaapi",
            Encoder::Libx265 => "libx265",
            Encoder::HevcNvenc => "hevc_nvenc",
            Encoder::HevcAmf => "hevc_amf",
            Encoder::HevcQsv => "hevc_qsv",
            Encoder::HevcVaapi => "hevc_vaapi",
            Encoder::Av1Nvenc => "av1_nvenc",
            Encoder::Av1Amf => "av1_amf",
            Encoder::Av1Qsv => "av1_qsv",
            Encoder::Av1Vaapi => "av1_vaapi",
        }
    }
}

impl Display for Encoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str((*self).into())
    }
}

impl FromStr for Encoder {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "libx264" => Ok(Self::Libx264),
            "h264_nvenc" => Ok(Self::H264Nvenc),
            "h264_amf" => Ok(Self::H264Amf),
            "h264_qsv" => Ok(Self::H264Qsv),
            "h264_vaapi" => Ok(Self::H264Vaapi),
            "libx265" => Ok(Self::Libx265),
            "hevc_nvenc" => Ok(Self::HevcNvenc),
            "hevc_amf" => Ok(Self::HevcAmf),
            "hevc_qsv" => Ok(Self::HevcQsv),
            "hevc_vaapi" => Ok(Self::HevcVaapi),
            "av1_nvenc" => Ok(Self::Av1Nvenc),
            "av1_amf" => Ok(Self::Av1Amf),
            "av1_qsv" => Ok(Self::Av1Qsv),
            "av1_vaapi" => Ok(Self::Av1Vaapi),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decoder {
    H264,
    H264Cuvid,
    H264Qsv,
    Hevc,
    HevcCuvid,
    HevcQsv,
    Av1Cuvid,
    Av1Qsv,
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl Decoder {
    const fn codec(self) -> VideoCodec {
        match self {
            Self::H264 | Self::H264Cuvid | Self::H264Qsv => VideoCodec::H264,
            Self::Hevc | Self::HevcCuvid | Self::HevcQsv => VideoCodec::Hevc,
            Self::Av1Cuvid | Self::Av1Qsv => VideoCodec::Av1,
        }
    }
}

impl From<Decoder> for &'static str {
    fn from(decoder: Decoder) -> Self {
        match decoder {
            Decoder::H264 => "h264",
            Decoder::H264Cuvid => "h264_cuvid",
            Decoder::H264Qsv => "h264_qsv",
            Decoder::Hevc => "hevc",
            Decoder::HevcCuvid => "hevc_cuvid",
            Decoder::HevcQsv => "hevc_qsv",
            Decoder::Av1Cuvid => "av1_cuvid",
            Decoder::Av1Qsv => "av1_qsv",
        }
    }
}

impl FromStr for Decoder {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "h264" => Ok(Self::H264),
            "h264_cuvid" => Ok(Self::H264Cuvid),
            "h264_qsv" => Ok(Self::H264Qsv),
            "hevc" => Ok(Self::Hevc),
            "hevc_cuvid" => Ok(Self::HevcCuvid),
            "hevc_qsv" => Ok(Self::HevcQsv),
            "av1_cuvid" => Ok(Self::Av1Cuvid),
            "av1_qsv" => Ok(Self::Av1Qsv),
            _ => Err(()),
        }
    }
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn encoder_from_str(value: &str) -> std::result::Result<Encoder, ()> {
    selected::encoder_from_str(value)
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    use super::{Device, Encoder};
    use crate::internal::video::{VideoCodec, VideoMediaFormat, VideoSource};
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    use crate::types::RecordingConfig;
    use crate::types::{
        VideoCapabilities, VideoCapabilityAvailability, VideoSourceCapability, VideoUnavailable,
    };

    #[test]
    fn capabilities_represent_send_only_source_formats_without_boolean_ambiguity() {
        let format = VideoMediaFormat::MpegTs(VideoCodec::H264);
        let capabilities = VideoCapabilities {
            send: VideoCapabilityAvailability::Available,
            receive: VideoCapabilityAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable),
            send_sources: vec![VideoSourceCapability {
                source: VideoSource::Display,
                formats: vec![format],
            }],
            receive_formats: Vec::new(),
        };

        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Ok(&[format][..])
        );
        assert_eq!(
            capabilities.receive,
            VideoCapabilityAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable)
        );
    }

    #[test]
    fn unsupported_capabilities_use_the_same_typed_shape() {
        let capabilities = VideoCapabilities {
            send: VideoCapabilityAvailability::Unavailable(VideoUnavailable::PlatformUnsupported),
            receive: VideoCapabilityAvailability::Unavailable(
                VideoUnavailable::PlatformUnsupported,
            ),
            send_sources: Vec::new(),
            receive_formats: Vec::new(),
        };

        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Err(VideoUnavailable::PlatformUnsupported)
        );
        assert_eq!(
            capabilities.receive,
            VideoCapabilityAvailability::Unavailable(VideoUnavailable::PlatformUnsupported)
        );
    }

    #[test]
    fn available_empty_capabilities_remain_distinct_from_unavailable() {
        let capabilities = VideoCapabilities {
            send: VideoCapabilityAvailability::Available,
            receive: VideoCapabilityAvailability::Available,
            send_sources: Vec::new(),
            receive_formats: Vec::new(),
        };

        assert_eq!(
            capabilities.formats(VideoSource::Display),
            Err(VideoUnavailable::SourceUnavailable(VideoSource::Display))
        );
        assert_eq!(capabilities.receive, VideoCapabilityAvailability::Available);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    #[tokio::test]
    async fn unsupported_adapter_query_and_start_report_typed_unavailable() {
        let (compatibility, capabilities) =
            super::selected::probe_capabilities().await.into_parts();
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
            super::selected::prepare_sender(&config, 1_280, 720, &compatibility, &capabilities,),
            Err(VideoUnavailable::PlatformUnsupported)
        );
    }
}
