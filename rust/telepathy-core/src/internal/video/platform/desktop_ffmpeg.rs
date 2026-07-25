// allow: SIZE_OK - target-specific FFmpeg configuration and process adapter must compile together.
#[cfg(feature = "integration-testing")]
use super::CommandDescription;
use super::{
    CapabilityProbe, Result, VideoAvailability, VideoCapabilities, VideoSourceCapability,
    VideoUnavailable,
};
use crate::internal::video::{VideoCodec, VideoMediaDescriptor, VideoMediaFormat, VideoSource};
use crate::types::{Capabilities, RecordingConfig};
use bytes::Bytes;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use regex::Regex;
use speedy::{Readable, Writable};
use std::fmt::Display;
#[cfg(not(target_family = "wasm"))]
use std::process::Stdio;
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
use std::process::{ExitStatus, Output};
use std::str::FromStr;
#[cfg(not(target_family = "wasm"))]
#[cfg(not(target_family = "wasm"))]
use tokio::process::Command;
#[cfg(not(target_family = "wasm"))]
use tokio::select;
#[cfg(not(target_family = "wasm"))]
use tokio_util::sync::CancellationToken;
#[cfg(not(target_family = "wasm"))]
use tracing::{info, instrument};

#[cfg(not(target_family = "wasm"))]
use crate::internal::error::ErrorKind;

#[cfg(not(target_family = "wasm"))]
const BUFFER_SIZE: usize = 512;
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

pub(crate) const fn initial_video_capabilities() -> VideoCapabilities {
    VideoCapabilities::unavailable(VideoUnavailable::RuntimeUnavailable)
}

fn video_capabilities(
    encoders: Option<&[Encoder]>,
    decoders: Option<&[Decoder]>,
    devices: &[Device],
) -> VideoCapabilities {
    let send = match encoders {
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
                vec![VideoSourceCapability::new(VideoSource::Display, formats)]
            };
            VideoAvailability::Available(sources)
        }
        None => VideoAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable),
    };
    let receive = match decoders {
        Some(decoders) => {
            let mut formats = Vec::new();
            for decoder in decoders {
                let format = VideoMediaFormat::MpegTs(decoder.codec());
                if !formats.contains(&format) {
                    formats.push(format);
                }
            }
            VideoAvailability::Available(formats)
        }
        None => VideoAvailability::Unavailable(VideoUnavailable::RuntimeUnavailable),
    };
    VideoCapabilities::new(send, receive)
}

#[derive(Clone, Debug, PartialEq, Eq, Readable, Writable)]
pub(crate) enum Device {
    DirectShow,
    GdiGrab,
    DesktopDuplication,
    AVFoundation(Vec<String>),
    X11Grab,
}

impl Device {
    #[cfg(target_os = "windows")]
    fn devices() -> Vec<Self> {
        vec![Self::DesktopDuplication, Self::GdiGrab, Self::DirectShow]
    }

    #[cfg(target_os = "macos")]
    fn devices() -> Vec<Self> {
        // let devices_output = Command::new("ffmpeg")
        //     .arg("-hide_banner")
        //     .arg("-f")
        //     .arg("avfoundation")
        //     .arg("-list_devices")
        //     .arg("true")
        //     .arg("-i")
        //     .arg("\"\"")
        //     .output()
        //     .await;

        // TODO parse the output and use it for devices

        vec![Self::AVFoundation(vec![])]
    }

    #[cfg(target_os = "linux")]
    fn devices() -> Vec<Self> {
        vec![Self::X11Grab]
    }

    #[cfg(not(target_family = "wasm"))]
    fn to_args(&self, encoder: Encoder) -> Vec<&str> {
        // TODO figure out a way to only add the video size for encoders if needed
        match self {
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
            _ => todo!(),
        }
    }
}

impl Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectShow => write!(f, "DirectShow"),
            Self::GdiGrab => write!(f, "GDI Grab"),
            Self::DesktopDuplication => write!(f, "Desktop Duplication"),
            Self::AVFoundation(devices) => write!(f, "AVFoundation: {:?}", devices),
            Self::X11Grab => write!(f, "X11 Grab"),
        }
    }
}

impl FromStr for Device {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "DirectShow" => Self::DirectShow,
            "GDI Grab" => Self::GdiGrab,
            "Desktop Duplication" => Self::DesktopDuplication,
            "X11 Grab" => Self::X11Grab,
            _ => Self::AVFoundation(Vec::new()), // TODO handle the devices
        })
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

impl From<Encoder> for &'static str {
    fn from(val: Encoder) -> Self {
        match val {
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Into::<&'static str>::into(*self))
    }
}

impl FromStr for Encoder {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn encoder_from_str(value: &str) -> std::result::Result<Encoder, ()> {
    Encoder::from_str(value)
}

#[cfg(not(target_family = "wasm"))]
impl Encoder {
    /// returns the valid decoders for this encoder in preferred order
    fn decoders(&self) -> Vec<Decoder> {
        match self {
            Self::Libx264 | Self::H264Nvenc | Self::H264Amf | Self::H264Qsv | Self::H264Vaapi => {
                vec![Decoder::H264Cuvid, Decoder::H264Qsv, Decoder::H264]
            }
            Self::Libx265 | Self::HevcNvenc | Self::HevcAmf | Self::HevcQsv | Self::HevcVaapi => {
                vec![Decoder::HevcCuvid, Decoder::HevcQsv, Decoder::Hevc]
            }
            Self::Av1Nvenc | Self::Av1Amf | Self::Av1Qsv | Self::Av1Vaapi => {
                vec![Decoder::Av1Cuvid, Decoder::Av1Qsv]
            }
        }
    }

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

impl From<Decoder> for &'static str {
    fn from(val: Decoder) -> Self {
        match val {
            Decoder::H264 => "h264",
            Decoder::H264Cuvid => "h264_cuvid",
            Decoder::Hevc => "hevc",
            Decoder::HevcCuvid => "hevc_cuvid",
            Decoder::H264Qsv => "h264_qsv",
            Decoder::HevcQsv => "hevc_qsv",
            Decoder::Av1Cuvid => "av1_cuvid",
            Decoder::Av1Qsv => "av1_qsv",
        }
    }
}

impl FromStr for Decoder {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
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

impl Decoder {
    const fn codec(self) -> VideoCodec {
        match self {
            Self::H264 | Self::H264Cuvid | Self::H264Qsv => VideoCodec::H264,
            Self::Hevc | Self::HevcCuvid | Self::HevcQsv => VideoCodec::Hevc,
            Self::Av1Cuvid | Self::Av1Qsv => VideoCodec::Av1,
        }
    }
}
impl RecordingConfig {
    #[cfg(not(target_family = "wasm"))]
    fn make_command(&self, test: bool) -> Command {
        let mut command = Command::new("ffmpeg");
        command.args(self.device.to_args(self.encoder));

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

        command
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub(crate) async fn test_config(&self) -> Result<ExitStatus> {
        let mut command = self.make_command(true);
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

#[cfg(feature = "integration-testing")]
impl CommandDescription {
    pub(super) fn from_command(command: Command) -> Self {
        Self {
            program: command
                .as_std()
                .get_program()
                .to_string_lossy()
                .into_owned(),
            arguments: command
                .as_std()
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
        }
    }
}

#[cfg(feature = "integration-testing")]
pub fn recording_command_for_test(
    encoder: &str,
    device: &str,
    bitrate: u32,
    framerate: u32,
    height: Option<u32>,
) -> Option<CommandDescription> {
    let Ok(encoder) = Encoder::from_str(encoder) else {
        return None;
    };
    let Ok(device) = Device::from_str(device) else {
        return None;
    };
    let config = RecordingConfig {
        encoder,
        device,
        bitrate,
        framerate,
        height,
    };
    Some(CommandDescription::from_command(config.make_command(false)))
}

#[cfg(not(target_family = "wasm"))]
struct PlaybackConfig {
    decoder: Decoder,
}

#[cfg(not(target_family = "wasm"))]
impl PlaybackConfig {
    fn make_command(&self) -> Command {
        let mut command = Command::new("ffplay");

        command.args(["-vcodec", self.decoder.into(), "-f", "mpegts", "-i", "-"]);

        command
    }
}

#[cfg(feature = "integration-testing")]
pub fn playback_command_for_test(
    encoder: &str,
    width: u32,
    height: u32,
) -> Option<CommandDescription> {
    let Ok(encoder) = Encoder::from_str(encoder) else {
        return None;
    };
    let mut command = PlaybackConfig {
        decoder: encoder.decoders()[0],
    }
    .make_command();
    command.args([
        "-x",
        &width.to_string(),
        "-y",
        &height.to_string(),
        "-flags",
        "low_delay",
        "-analyzeduration",
        "1",
        "-window_title",
        "Telepathy Screenshare",
    ]);
    Some(CommandDescription::from_command(command))
}

#[instrument(name = "screenshare.record", skip_all)]
pub(crate) async fn run_sender<S>(
    transport: &mut S,
    stop: &CancellationToken,
    config: RecordingConfig,
) -> Result<()>
where
    S: futures_util::Sink<Bytes> + Unpin,
    S::Error: std::fmt::Display,
{
    info!(event = "screenshare_record_start", ?config);

    let mut command = config.make_command(false);

    command.stdout(Stdio::piped()).stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATION_FLAGS);
    }

    let mut child = command.spawn()?;

    let Some(mut stdout) = child.stdout.take() else {
        return Err(ErrorKind::PlatformUnavailable.into());
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
) -> Result<()>
where
    S: futures_util::Stream<Item = std::result::Result<bytes::BytesMut, E>> + Unpin,
{
    info!("Starting screen playback");
    let encoder = match descriptor.codec() {
        VideoCodec::H264 => Encoder::Libx264,
        VideoCodec::Hevc => Encoder::Libx265,
        VideoCodec::Av1 => Encoder::Av1Nvenc,
    };
    let decoders = encoder.decoders();

    // TODO intelligently chose a decoder instead of using the first one
    let config = PlaybackConfig {
        decoder: decoders
            .into_iter()
            .next()
            .ok_or(ErrorKind::NoEncoderAvailable)?,
    };

    let mut command = config.make_command();

    command
        .args([
            "-x",
            &descriptor.dimensions().0.to_string(),
            "-y",
            &descriptor.dimensions().1.to_string(),
            "-flags",
            "low_delay",
            "-analyzeduration",
            "1",
            // TODO -framedrop
            "-window_title",
            "Telepathy Screenshare",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        command.creation_flags(CREATION_FLAGS);
    }

    let mut child = command.spawn()?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err(ErrorKind::PlatformUnavailable.into());
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
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
    use super::{Device, Encoder, prepare_sender_from_capabilities};
    use crate::internal::video::platform::VideoUnavailable;
    use crate::types::{Capabilities, RecordingConfig};

    fn recording_config() -> RecordingConfig {
        RecordingConfig {
            encoder: Encoder::H264Nvenc,
            device: Device::X11Grab,
            bitrate: 4_000_000,
            framerate: 60,
            height: Some(720),
        }
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
        assert_eq!(generic.receive(), Err(VideoUnavailable::RuntimeUnavailable));
    }
}
