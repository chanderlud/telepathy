use super::{CapabilityProbe, Encoder, Result, VideoCapabilities, VideoUnavailable};
use crate::internal::error::ErrorKind;
use crate::internal::video::VideoMediaDescriptor;
use crate::types::{Capabilities, RecordingConfig};
use bytes::Bytes;
use std::fmt::Display;
use std::str::FromStr;
use tokio_util::sync::CancellationToken;

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
    startup: tokio::sync::oneshot::Sender<crate::internal::video::VideoWorkerStartup>,
) -> Result<()>
where
    S: futures_util::Sink<Bytes> + Unpin,
    S::Error: Display,
{
    let _ = startup.send(crate::internal::video::VideoWorkerStartup::Failed);
    Err(ErrorKind::PlatformUnavailable.into())
}
pub(crate) async fn run_receiver<S, E>(
    _: &mut S,
    _: &CancellationToken,
    _: VideoMediaDescriptor,
    startup: tokio::sync::oneshot::Sender<crate::internal::video::VideoWorkerStartup>,
) -> Result<()>
where
    S: futures_util::Stream<Item = std::result::Result<bytes::BytesMut, E>> + Unpin,
{
    let _ = startup.send(crate::internal::video::VideoWorkerStartup::Failed);
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
