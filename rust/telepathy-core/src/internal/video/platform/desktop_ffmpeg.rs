// allow: SIZE_OK - target-specific FFmpeg configuration and process adapter must compile together.
use super::{CapabilityProbe, Decoder, Device, Encoder, Result};
use crate::internal::video::{
    VideoMediaDescriptor, VideoMediaFormat, VideoSource, VideoWorkerStartup,
};
use crate::types::{
    Capabilities, RecordingConfig, VideoCapabilities, VideoCapabilityAvailability,
    VideoSourceCapability, VideoUnavailable,
};
use bytes::Bytes;
use regex::Regex;
use std::process::Stdio;
use std::process::{ExitStatus, Output};
use std::str::FromStr;
use tokio::process::Command;
use tokio::select;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument};

use crate::internal::error::{Error, ErrorKind};

#[cfg(target_os = "windows")]
const CREATION_FLAGS: u32 = 0x08000000;

pub(crate) async fn probe_capabilities() -> CapabilityProbe {
    let codec_regex = Regex::new("V....[D.] ([^= ]+)\\s+(.+)").unwrap();

    let mut command = Command::new("ffmpeg");
    command.arg("-hide_banner").arg("-encoders");

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATION_FLAGS);
    }

    let encoders_result = command.output().await;

    let mut command = Command::new("ffplay");
    command.arg("-hide_banner").arg("-decoders");

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATION_FLAGS);
    }

    let decoders_result = command.output().await;

    let encoders = encoders_result.ok().map(|output| {
        parse_codecs(output, &codec_regex)
            .into_iter()
            .filter_map(|codec| Encoder::from_str(&codec).ok())
            .collect::<Vec<_>>()
    });
    let decoders = decoders_result.ok().map(|output| {
        parse_codecs(output, &codec_regex)
            .into_iter()
            .filter_map(|codec| Decoder::from_str(&codec).ok())
            .collect::<Vec<_>>()
    });
    let devices = Device::devices();
    let video = video_capabilities(encoders.as_deref(), decoders.as_deref(), devices.as_slice());
    let compatibility = Capabilities {
        _available: encoders.is_some() && decoders.is_some(),
        encoders: encoders.unwrap_or_default(),
        _decoders: decoders.unwrap_or_default(),
        devices,
    };
    CapabilityProbe::new(compatibility, video)
}

fn video_capabilities(
    encoders: Option<&[Encoder]>,
    decoders: Option<&[Decoder]>,
    devices: &[Device],
) -> VideoCapabilities {
    let (send, send_sources) = match encoders {
        Some(encoders) => {
            let mut formats = Vec::new();
            for encoder in encoders {
                let format = VideoMediaFormat::MpegTs(encoder.codec());
                if !formats.contains(&format) {
                    formats.push(format);
                }
            }
            let sources = if devices.is_empty() || formats.is_empty() {
                Vec::new()
            } else {
                vec![VideoSourceCapability {
                    source: VideoSource::Display,
                    formats,
                }]
            };
            (VideoCapabilityAvailability::Available, sources)
        }
        None => (
            VideoCapabilityAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable),
            Vec::new(),
        ),
    };
    let (receive, receive_formats) = match decoders {
        Some(decoders) => {
            let mut formats = Vec::new();
            for decoder in decoders {
                let format = VideoMediaFormat::MpegTs(decoder.codec());
                if !formats.contains(&format) {
                    formats.push(format);
                }
            }
            (VideoCapabilityAvailability::Available, formats)
        }
        None => (
            VideoCapabilityAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable),
            Vec::new(),
        ),
    };
    VideoCapabilities {
        send,
        receive,
        send_sources,
        receive_formats,
    }
}

impl Device {
    fn to_args(&self, encoder: Encoder) -> std::result::Result<Vec<&str>, ErrorKind> {
        let arguments = match self {
            Self::DesktopDuplication => match encoder {
                Encoder::H264Nvenc | Encoder::H264Qsv => vec![
                    "-init_hw_device",
                    "d3d11va",
                    "-filter_complex",
                    "ddagrab=video_size=1920x1080",
                ],
                Encoder::HevcNvenc | Encoder::Av1Nvenc => {
                    vec!["-init_hw_device", "d3d11va", "-filter_complex", "ddagrab=0"]
                }
                _ => vec![
                    "-init_hw_device",
                    "d3d11va",
                    "-filter_complex",
                    "ddagrab=0,hwdownload,format=bgra",
                ],
            },
            Self::GdiGrab => match encoder {
                Encoder::H264Nvenc | Encoder::H264Qsv => vec![
                    "-f",
                    "gdigrab",
                    "-framerate",
                    "30",
                    "-video_size",
                    "1920x1080",
                    "-i",
                    "desktop",
                ],
                _ => vec!["-f", "gdigrab", "-framerate", "30", "-i", "desktop"],
            },
            _ => return Err(ErrorKind::PlatformUnavailable),
        };
        Ok(arguments)
    }
}

pub(crate) fn encoder_from_str(value: &str) -> std::result::Result<Encoder, ()> {
    Encoder::from_str(value)
}

pub(crate) fn prepare_sender(
    config: &RecordingConfig,
    width: u32,
    height: u32,
    capabilities: &Capabilities,
    video_capabilities: &VideoCapabilities,
) -> std::result::Result<VideoMediaDescriptor, VideoUnavailable> {
    prepare_sender_from_capabilities(config, width, height, capabilities, video_capabilities)
}

fn prepare_sender_from_capabilities(
    config: &RecordingConfig,
    width: u32,
    height: u32,
    capabilities: &Capabilities,
    generic: &VideoCapabilities,
) -> std::result::Result<VideoMediaDescriptor, VideoUnavailable> {
    let available = generic.formats(VideoSource::Display)?;
    if !capabilities.encoders.contains(&config.encoder)
        || !capabilities.devices.contains(&config.device)
    {
        return Err(VideoUnavailable::ConfigurationUnavailable);
    }

    let descriptor = VideoMediaDescriptor::display(config.encoder.codec(), width, height);
    let format = VideoMediaFormat::MpegTs(config.encoder.codec());
    if available.contains(&format) {
        Ok(descriptor)
    } else {
        Err(VideoUnavailable::FormatUnavailable(format))
    }
}

impl RecordingConfig {
    fn make_command(&self, test: bool) -> Result<Command> {
        let mut command = Command::new("ffmpeg");
        command.args(self.device.to_args(self.encoder)?);

        // sets the video size if specified
        if let Some(height) = self.height {
            command.arg("-vf");
            command.arg(format!("trunc(oh*a/2)*2:{}", height));
        }

        if test {
            command.arg("-frames:v");
            command.arg("1");
        }

        command.args([
            "-c:v",
            self.encoder.into(),
            "-delay",
            "0",
            "-b:v",
            self.bitrate.to_string().as_str(),
            "-bufsize",
            "1M",
            "-f",
            "mpegts",
            "-",
        ]);

        Ok(command)
    }

    pub(crate) async fn test_config(&self) -> Result<ExitStatus> {
        let mut command = self.make_command(true)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(target_os = "windows")]
        {
            command.creation_flags(CREATION_FLAGS);
        }

        let mut child = command.spawn()?;
        child.wait().await.map_err(Into::into)
    }
}

struct PlaybackConfig {
    decoder: Decoder,
}

impl PlaybackConfig {
    fn make_command(&self) -> Command {
        let mut command = Command::new("ffplay");

        command.args(["-vcodec", self.decoder.into(), "-f", "mpegts", "-i", "-"]);

        command
    }
}

fn make_playback_command(
    descriptor: VideoMediaDescriptor,
    decoders: &[Decoder],
) -> std::result::Result<Command, ErrorKind> {
    let config = PlaybackConfig {
        decoder: select_decoder(decoders, descriptor)?,
    };
    let mut command = config.make_command();
    command.args([
        "-x",
        &descriptor.dimensions().0.to_string(),
        "-y",
        &descriptor.dimensions().1.to_string(),
        "-flags",
        "low_delay",
        "-analyzeduration",
        "1",
        "-window_title",
        "Telepathy Screenshare",
    ]);
    Ok(command)
}

fn select_decoder(
    decoders: &[Decoder],
    descriptor: VideoMediaDescriptor,
) -> std::result::Result<Decoder, ErrorKind> {
    decoders
        .iter()
        .copied()
        .find(|decoder| decoder.codec() == descriptor.codec())
        .ok_or(ErrorKind::NoEncoderAvailable)
}

#[instrument(name = "screenshare.record", skip_all)]
pub(crate) async fn run_sender<S>(
    transport: &mut S,
    stop: &CancellationToken,
    config: RecordingConfig,
    startup: tokio::sync::oneshot::Sender<VideoWorkerStartup>,
) -> Result<()>
where
    S: futures_util::Sink<Bytes> + Unpin,
    S::Error: std::fmt::Display,
{
    info!(event = "screenshare_record_start", ?config);

    let startup_result: Result<_> = (|| {
        let mut command = config.make_command(false)?;
        command.stdout(Stdio::piped()).stderr(Stdio::null());

        #[cfg(target_os = "windows")]
        command.creation_flags(CREATION_FLAGS);

        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::from(ErrorKind::PlatformUnavailable))?;
        Ok((child, stdout))
    })();
    let (mut child, mut stdout) = match startup_result {
        Ok(state) => {
            let _ = startup.send(VideoWorkerStartup::Ready);
            state
        }
        Err(error) => {
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Err(error);
        }
    };

    let future = super::forward_capture_chunks(&mut stdout, transport);

    select! {
        _ = future => {
            info!("Recording finished");
        }
        _ = stop.cancelled() => {
            info!("Recording stopped");
        }
    }

    drop(stdout);
    terminate_and_reap(&mut child).await;
    Ok(())
}

#[instrument(name = "screenshare.playback", skip_all)]
pub(crate) async fn run_receiver<S, E>(
    transport: &mut S,
    stop: &CancellationToken,
    descriptor: VideoMediaDescriptor,
    startup: tokio::sync::oneshot::Sender<VideoWorkerStartup>,
) -> Result<()>
where
    S: futures_util::Stream<Item = std::result::Result<bytes::BytesMut, E>> + Unpin,
{
    info!("Starting screen playback");
    let (capabilities, _) = probe_capabilities().await.into_parts();
    let startup_result: Result<_> = (|| {
        let mut command = make_playback_command(descriptor, &capabilities._decoders)?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        command.creation_flags(CREATION_FLAGS);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::from(ErrorKind::PlatformUnavailable))?;
        Ok((child, stdin))
    })();
    let (mut child, mut stdin) = match startup_result {
        Ok(state) => {
            let _ = startup.send(VideoWorkerStartup::Ready);
            state
        }
        Err(error) => {
            let _ = startup.send(VideoWorkerStartup::Failed);
            return Err(error);
        }
    };

    let future = super::forward_playback_frames(transport, &mut stdin);

    select! {
        _ = future => {
            info!("Playback finished");
        }
        _ = stop.cancelled() => {
            info!("Playback stopped");
        }
    }

    drop(stdin);
    terminate_and_reap(&mut child).await;
    Ok(())
}

async fn terminate_and_reap(child: &mut tokio::process::Child) {
    if tokio::time::timeout(std::time::Duration::from_secs(1), child.wait())
        .await
        .is_ok()
    {
        return;
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn parse_codecs(output: Output, regex: &Regex) -> Vec<String> {
    let output_str = String::from_utf8_lossy(&output.stdout);

    regex
        .captures_iter(&output_str)
        .filter_map(|cap| cap.get(1))
        .map(|cap| cap.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Device, Encoder, make_playback_command, prepare_sender_from_capabilities, select_decoder,
    };
    use crate::internal::error::{Error, ErrorKind};
    use crate::internal::video::platform::Decoder;
    use crate::internal::video::{VideoCodec, VideoMediaDescriptor};
    use crate::types::{
        Capabilities, RecordingConfig, VideoCapabilityAvailability, VideoUnavailable,
    };

    fn recording_config() -> RecordingConfig {
        RecordingConfig {
            encoder: Encoder::H264Nvenc,
            device: Device::X11Grab,
            bitrate: 4_000_000,
            framerate: 60,
            height: Some(720),
        }
    }

    fn command_parts(command: &tokio::process::Command) -> (String, Vec<String>) {
        let command = command.as_std();
        (
            command.get_program().to_string_lossy().into_owned(),
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn sender_command_preserves_current_ffmpeg_arguments() {
        let config = RecordingConfig {
            encoder: Encoder::H264Nvenc,
            device: Device::GdiGrab,
            bitrate: 4_000_000,
            framerate: 60,
            height: Some(720),
        };
        let command = config.make_command(false).unwrap();
        let (program, arguments) = command_parts(&command);

        assert_eq!(program, "ffmpeg");
        assert_eq!(
            arguments,
            [
                "-f",
                "gdigrab",
                "-framerate",
                "30",
                "-video_size",
                "1920x1080",
                "-i",
                "desktop",
                "-vf",
                "trunc(oh*a/2)*2:720",
                "-c:v",
                "h264_nvenc",
                "-delay",
                "0",
                "-b:v",
                "4000000",
                "-bufsize",
                "1M",
                "-f",
                "mpegts",
                "-",
            ]
        );
    }

    #[test]
    fn receiver_command_preserves_current_ffplay_arguments() {
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1280, 720);
        let command = make_playback_command(
            descriptor,
            &[Decoder::H264Cuvid, Decoder::H264Qsv, Decoder::H264],
        )
        .unwrap();
        let (program, arguments) = command_parts(&command);

        assert_eq!(program, "ffplay");
        assert_eq!(
            arguments,
            [
                "-vcodec",
                "h264_cuvid",
                "-f",
                "mpegts",
                "-i",
                "-",
                "-x",
                "1280",
                "-y",
                "720",
                "-flags",
                "low_delay",
                "-analyzeduration",
                "1",
                "-window_title",
                "Telepathy Screenshare",
            ]
        );
    }

    #[test]
    fn implemented_devices_preserve_command_arguments() {
        assert_eq!(
            Device::DesktopDuplication
                .to_args(Encoder::H264Nvenc)
                .unwrap(),
            [
                "-init_hw_device",
                "d3d11va",
                "-filter_complex",
                "ddagrab=video_size=1920x1080",
            ]
        );
        assert_eq!(
            Device::GdiGrab.to_args(Encoder::H264Nvenc).unwrap(),
            [
                "-f",
                "gdigrab",
                "-framerate",
                "30",
                "-video_size",
                "1920x1080",
                "-i",
                "desktop",
            ]
        );
        assert!(
            Device::devices()
                .iter()
                .all(|device| device.to_args(Encoder::Libx264).is_ok())
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            Device::devices(),
            [Device::DesktopDuplication, Device::GdiGrab]
        );
        #[cfg(not(target_os = "windows"))]
        assert!(Device::devices().is_empty());
    }

    #[test]
    fn unimplemented_device_returns_typed_error_without_panicking() {
        assert!(matches!(
            recording_config().make_command(false),
            Err(Error {
                kind: ErrorKind::PlatformUnavailable
            })
        ));
    }

    #[test]
    fn decoder_selection_uses_first_compatible_local_decoder() {
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1_280, 720);
        let decoders = [Decoder::Hevc, Decoder::H264Qsv, Decoder::H264];

        assert!(matches!(
            select_decoder(&decoders, descriptor),
            Ok(Decoder::H264Qsv)
        ));
    }

    #[test]
    fn decoder_selection_fails_when_local_probe_has_no_compatible_decoder() {
        let descriptor = VideoMediaDescriptor::display(VideoCodec::Av1, 1_280, 720);

        assert!(matches!(
            select_decoder(&[Decoder::H264, Decoder::Hevc], descriptor),
            Err(ErrorKind::NoEncoderAvailable)
        ));
    }

    #[test]
    fn sender_start_rejects_encoder_removed_after_preflight() {
        let capabilities = Capabilities {
            _available: true,
            encoders: vec![Encoder::Libx264],
            _decoders: Vec::new(),
            devices: vec![Device::X11Grab],
        };

        let generic = super::video_capabilities(
            Some(&capabilities.encoders),
            Some(&capabilities._decoders),
            &capabilities.devices,
        );
        let result = prepare_sender_from_capabilities(
            &recording_config(),
            1_280,
            720,
            &capabilities,
            &generic,
        );

        assert_eq!(result, Err(VideoUnavailable::ConfigurationUnavailable));
    }

    #[test]
    fn sender_start_rejects_device_removed_after_preflight() {
        let capabilities = Capabilities {
            _available: true,
            encoders: vec![Encoder::H264Nvenc],
            _decoders: Vec::new(),
            devices: vec![Device::GdiGrab],
        };

        let generic = super::video_capabilities(
            Some(&capabilities.encoders),
            Some(&capabilities._decoders),
            &capabilities.devices,
        );
        let result = prepare_sender_from_capabilities(
            &recording_config(),
            1_280,
            720,
            &capabilities,
            &generic,
        );

        assert_eq!(result, Err(VideoUnavailable::ConfigurationUnavailable));
    }

    #[test]
    fn sender_start_uses_directional_capability_when_receiver_is_unavailable() {
        let capabilities = Capabilities {
            _available: false,
            encoders: vec![Encoder::H264Nvenc],
            _decoders: Vec::new(),
            devices: vec![Device::X11Grab],
        };
        let generic =
            super::video_capabilities(Some(&capabilities.encoders), None, &capabilities.devices);

        let result = prepare_sender_from_capabilities(
            &recording_config(),
            1_280,
            720,
            &capabilities,
            &generic,
        );

        assert!(result.is_ok());
        assert_eq!(
            generic.receive,
            VideoCapabilityAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable)
        );
    }
}
