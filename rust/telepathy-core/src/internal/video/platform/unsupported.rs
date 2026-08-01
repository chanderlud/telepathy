use super::CapabilityProbe;
use super::Result;
use crate::internal::error::ErrorKind;
use crate::internal::video::VideoMediaDescriptor;
use crate::types::{
    Capabilities, RecordingConfig, VideoCapabilities, VideoCapabilityAvailability, VideoUnavailable,
};
use bytes::Bytes;
use std::fmt::Display;
use tokio_util::sync::CancellationToken;

pub(crate) async fn probe_capabilities() -> CapabilityProbe {
    CapabilityProbe::new(
        Capabilities::default(),
        VideoCapabilities {
            send: VideoCapabilityAvailability::Unavailable(VideoUnavailable::PlatformUnsupported),
            receive: VideoCapabilityAvailability::Unavailable(
                VideoUnavailable::PlatformUnsupported,
            ),
            send_sources: Vec::new(),
            receive_formats: Vec::new(),
        },
    )
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
