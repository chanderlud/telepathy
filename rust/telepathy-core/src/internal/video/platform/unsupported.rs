use super::{CapabilityProbe, Result, VideoCapabilities, VideoUnavailable};
use crate::internal::error::ErrorKind;
use crate::internal::video::VideoMediaDescriptor;
use crate::types::{Capabilities, RecordingConfig};
use bytes::Bytes;
use speedy::{Readable, Writable};
use std::fmt::Display;
use std::str::FromStr;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Readable, Writable)]
pub(crate) enum Device {
    DirectShow,
    GdiGrab,
    DesktopDuplication,
    AVFoundation(Vec<String>),
    X11Grab,
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

#[derive(Clone, Copy, Debug, Readable, Writable)]
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

impl Encoder {
    const fn name(self) -> &'static str {
        match self {
            Self::Libx264 => "libx264",
            Self::H264Nvenc => "h264_nvenc",
            Self::H264Amf => "h264_amf",
            Self::H264Qsv => "h264_qsv",
            Self::H264Vaapi => "h264_vaapi",
            Self::Libx265 => "libx265",
            Self::HevcNvenc => "hevc_nvenc",
            Self::HevcAmf => "hevc_amf",
            Self::HevcQsv => "hevc_qsv",
            Self::HevcVaapi => "hevc_vaapi",
            Self::Av1Nvenc => "av1_nvenc",
            Self::Av1Amf => "av1_amf",
            Self::Av1Qsv => "av1_qsv",
            Self::Av1Vaapi => "av1_vaapi",
        }
    }
}

impl Display for Encoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

impl From<Encoder> for &'static str {
    fn from(encoder: Encoder) -> Self {
        encoder.name()
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

#[derive(Clone, Copy, Debug)]
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

pub(crate) fn encoder_from_str(value: &str) -> std::result::Result<Encoder, ()> {
    Encoder::from_str(value)
}
pub(crate) async fn probe_capabilities() -> CapabilityProbe {
    CapabilityProbe::new(Capabilities::default(), initial_video_capabilities())
}
pub(crate) const fn initial_video_capabilities() -> VideoCapabilities {
    VideoCapabilities::unavailable(VideoUnavailable::PlatformUnsupported)
}
pub(crate) fn prepare_sender(
    _: &RecordingConfig,
    _: u32,
    _: u32,
    _: &Capabilities,
    _: &VideoCapabilities,
) -> std::result::Result<VideoMediaDescriptor, VideoUnavailable> {
    Err(VideoUnavailable::PlatformUnsupported)
}
pub(crate) async fn run_sender<S>(
    _: &mut S,
    _: &CancellationToken,
    _: RecordingConfig,
) -> Result<()>
where
    S: futures_util::Sink<Bytes> + Unpin,
    S::Error: Display,
{
    Err(ErrorKind::PlatformUnavailable.into())
}
pub(crate) async fn run_receiver<S, E>(
    _: &mut S,
    _: &CancellationToken,
    _: VideoMediaDescriptor,
) -> Result<()>
where
    S: futures_util::Stream<Item = std::result::Result<bytes::BytesMut, E>> + Unpin,
{
    Err(ErrorKind::PlatformUnavailable.into())
}

#[cfg(feature = "integration-testing")]
pub fn recording_command_for_test(
    _: &str,
    _: &str,
    _: u32,
    _: u32,
    _: Option<u32>,
) -> Option<super::CommandDescription> {
    None
}
#[cfg(feature = "integration-testing")]
pub fn playback_command_for_test(_: &str, _: u32, _: u32) -> Option<super::CommandDescription> {
    None
}
