//! Error types for audio operations.
//!
//! [`Error`] is the crate-level error boundary. Variants hold structured
//! sub-errors so callers can match on the actual failure rather than parsing
//! display text. Inner errors preserve their original source via
//! [`std::error::Error::source`].
//!
//! ## Hierarchy
//!
//! ```text
//! Error
//! ├── Device(DeviceError)
//! ├── Stream(StreamError)
//! ├── Processing(ProcessingError)
//! ├── Channel(ChannelError)
//! ├── Config(ConfigError)
//! ├── Task(TaskError)
//! ├── AudioFile(AudioFileError)
//! └── Wasm(WasmError)              // target_family = "wasm" only
//! ```
//!
//! [`From`] conversions preserve the original error from external libraries
//! without stringification. The crate does not depend on `thiserror`; all
//! human-facing messages are produced by [`Display`] implementations only.

use crate::devices::DeviceError;
use crate::sea::codec::common::SeaError;
use cpal::{SampleFormat, SupportedStreamConfig};

/// Comprehensive error type for audio operations.
#[derive(Debug)]
pub enum Error {
    /// Device enumeration, selection, default-config, or stream-build failures.
    Device(DeviceError),
    /// Audio stream lifecycle failures (build/play) outside of `DeviceError`.
    Stream(StreamError),
    /// Resampling, codec, or in-memory buffer processing failures.
    Processing(ProcessingError),
    /// Inter-thread ring buffer / channel operation failures.
    Channel(ChannelError),
    /// Invalid builder configuration or missing required component.
    Config(ConfigError),
    /// Spawned task lifecycle or join failures.
    Task(TaskError),
    /// Audio file parsing or interpretation errors (WAV/SEA file headers, samples).
    AudioFile(AudioFileError),
    /// WASM-only errors around threading or browser-side failures.
    #[cfg(target_family = "wasm")]
    Wasm(WasmError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Device(inner) => write!(f, "device error: {}", inner),
            Error::Stream(inner) => write!(f, "stream error: {}", inner),
            Error::Processing(inner) => write!(f, "processing error: {}", inner),
            Error::Channel(inner) => write!(f, "channel error: {}", inner),
            Error::Config(inner) => write!(f, "configuration error: {}", inner),
            Error::Task(inner) => write!(f, "task error: {}", inner),
            Error::AudioFile(inner) => write!(f, "audio file error: {}", inner),
            #[cfg(target_family = "wasm")]
            Error::Wasm(inner) => write!(f, "wasm error: {}", inner),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Device(inner) => Some(inner),
            Error::Stream(inner) => Some(inner),
            Error::Processing(inner) => Some(inner),
            Error::Channel(inner) => Some(inner),
            Error::Config(inner) => Some(inner),
            Error::Task(inner) => Some(inner),
            Error::AudioFile(inner) => Some(inner),
            #[cfg(target_family = "wasm")]
            Error::Wasm(inner) => Some(inner),
        }
    }
}

// ---- Stream ----

/// Failures raised by audio stream lifecycle operations outside of
/// [`DeviceError`] (such as the standalone playback path in `player.rs`).
#[derive(Debug)]
pub enum StreamError {
    /// CPAL's `build_output_stream` returned an error during playback setup.
    BuildOutputStream {
        /// The output stream config that was attempted.
        config: Option<SupportedStreamConfig>,
        /// The CPAL error.
        source: cpal::Error,
    },
    /// `Stream::play()` returned an error.
    Play {
        /// Whether the failing stream was an input or output stream.
        direction: StreamDirection,
        /// The CPAL error.
        source: cpal::Error,
    },
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::BuildOutputStream { source, .. } => {
                write!(f, "failed to build output stream: {}", source)
            }
            StreamError::Play { direction, source } => {
                write!(
                    f,
                    "failed to start {} stream playback: {}",
                    direction, source
                )
            }
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StreamError::BuildOutputStream { source, .. } | StreamError::Play { source, .. } => {
                Some(source)
            }
        }
    }
}

/// Whether a stream failure refers to an input or output stream.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StreamDirection {
    /// Input (microphone capture) stream.
    Input,
    /// Output (playback) stream.
    Output,
}

impl std::fmt::Display for StreamDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamDirection::Input => f.write_str("input"),
            StreamDirection::Output => f.write_str("output"),
        }
    }
}

// ---- Processing ----

/// Failures raised by audio processing (resampling, decoding, conversion).
#[derive(Debug)]
pub enum ProcessingError {
    /// `rubato::Fft::new` rejected the supplied parameters.
    ResamplerConstruction(rubato::ResamplerConstructionError),
    /// `rubato::Resampler` failed during frame processing.
    Resample(rubato::ResampleError),
    /// `audioadapter_buffers` rejected the supplied slice dimensions.
    Buffer(audioadapter_buffers::SizeError),
    /// A slice could not be converted into a fixed-size array.
    Slice(std::array::TryFromSliceError),
    /// The SEA codec returned an error during streaming encode/decode.
    Codec(SeaError),
    /// Resampler factory rejected zero channels.
    ResamplerZeroChannels,
    /// An internal frame source was constructed with zero channels.
    ZeroChannelFrameSource,
}

impl std::fmt::Display for ProcessingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessingError::ResamplerConstruction(err) => {
                write!(f, "resampler construction error: {}", err)
            }
            ProcessingError::Resample(err) => write!(f, "resample error: {}", err),
            ProcessingError::Buffer(err) => write!(f, "audio buffer error: {:?}", err),
            ProcessingError::Slice(err) => write!(f, "slice conversion error: {}", err),
            ProcessingError::Codec(err) => write!(f, "codec error: {}", err),
            ProcessingError::ResamplerZeroChannels => {
                write!(f, "resampler requires > 0 channels")
            }
            ProcessingError::ZeroChannelFrameSource => {
                write!(f, "audio frame source has zero channels")
            }
        }
    }
}

impl std::error::Error for ProcessingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProcessingError::ResamplerConstruction(err) => Some(err),
            ProcessingError::Resample(err) => Some(err),
            ProcessingError::Slice(err) => Some(err),
            ProcessingError::Codec(err) => Some(err),
            ProcessingError::Buffer(_)
            | ProcessingError::ResamplerZeroChannels
            | ProcessingError::ZeroChannelFrameSource => None,
        }
    }
}

// ---- Channel ----

/// Failures raised by inter-thread channel/ring-buffer operations.
#[derive(Debug)]
pub enum ChannelError {
    /// `rtrb::chunks::ChunkError` from a producer or consumer operation.
    Chunk(rtrb::chunks::ChunkError),
    /// Blocking ring-buffer write canceled via the cancellation flag.
    BlockingWriteCanceled,
    /// The consumer side of a ring buffer was dropped before the producer finished.
    ConsumerAbandoned,
    /// A data source returned an `io::Error` from `recv`.
    DataSourceFailed(std::io::Error),
    /// A data sink returned an `io::Error` from `send`.
    DataSinkFailed(std::io::Error),
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelError::Chunk(err) => write!(f, "chunk error: {}", err),
            ChannelError::BlockingWriteCanceled => write!(f, "blocking ring-buffer write canceled"),
            ChannelError::ConsumerAbandoned => write!(f, "ring-buffer consumer abandoned"),
            ChannelError::DataSourceFailed(err) => {
                write!(f, "audio data source failed: {}", err)
            }
            ChannelError::DataSinkFailed(err) => write!(f, "audio data sink failed: {}", err),
        }
    }
}

impl std::error::Error for ChannelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ChannelError::Chunk(err) => Some(err),
            ChannelError::DataSourceFailed(err) | ChannelError::DataSinkFailed(err) => Some(err),
            ChannelError::BlockingWriteCanceled | ChannelError::ConsumerAbandoned => None,
        }
    }
}

// ---- Config ----

/// Failures raised by invalid builder configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// An input builder has no data sink set via `callback()` or `sink()`.
    MissingDataSink,
    /// An output builder has no data source set via `source()`.
    MissingDataSource,
    /// A WASM input builder has no `WebAudioWrapper` set.
    MissingWebAudioWrapper,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::MissingDataSink => {
                write!(
                    f,
                    "a data sink must be set via callback() or sink() before build()"
                )
            }
            ConfigError::MissingDataSource => {
                write!(f, "a data source must be set via source() before build()")
            }
            ConfigError::MissingWebAudioWrapper => write!(
                f,
                "WebAudioWrapper must be set via web_audio_wrapper() before calling build() on WASM targets"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

// ---- Task ----

/// Failures raised by spawned task lifecycles (join/init/teardown).
#[derive(Debug)]
pub enum TaskError {
    /// `tokio::task::JoinError` returned when awaiting a spawned task.
    Join(tokio::task::JoinError),
    /// A playback task dropped its initialization oneshot without sending a result.
    PlaybackInitChannelClosed,
    /// A WASM oneshot receive failed.
    #[cfg(target_family = "wasm")]
    OneshotReceive(tokio::sync::oneshot::error::RecvError),
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskError::Join(err) => write!(f, "task join error: {}", err),
            TaskError::PlaybackInitChannelClosed => {
                write!(
                    f,
                    "playback task terminated unexpectedly before initialization"
                )
            }
            #[cfg(target_family = "wasm")]
            TaskError::OneshotReceive(err) => {
                write!(f, "wasm blocking task channel closed: {}", err)
            }
        }
    }
}

impl std::error::Error for TaskError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TaskError::Join(err) => Some(err),
            #[cfg(target_family = "wasm")]
            TaskError::OneshotReceive(err) => Some(err),
            TaskError::PlaybackInitChannelClosed => None,
        }
    }
}

// ---- AudioFile ----

/// Failures raised when parsing a WAV or SEA file.
#[derive(Debug)]
pub enum AudioFileError {
    /// The byte buffer is shorter than the minimum required for a valid header.
    TooShort {
        /// Actual byte length of the input buffer.
        actual: usize,
        /// Minimum number of bytes required for a valid header.
        required: usize,
    },
    /// The WAV signature (RIFF/WAVE) is missing or invalid.
    InvalidSignature,
    /// The WAV `(audio_format, bits_per_sample)` pair is not supported.
    UnsupportedSampleFormat {
        /// WAV `audio_format` value (1 = PCM, 3 = IEEE float).
        audio_format: u16,
        /// Bits per sample from the WAV header.
        bits_per_sample: u16,
    },
    /// The WAV header advertises zero channels.
    ZeroChannels,
    /// The WAV header advertises zero sample rate.
    ZeroSampleRate,
    /// A WAV sample is shorter than the bytes-per-sample for its declared format.
    InvalidSampleBytes {
        /// The declared WAV sample format.
        sample_format: SampleFormat,
        /// Expected number of bytes per sample.
        bytes_per_sample: usize,
        /// Actual byte slice length.
        actual: usize,
    },
    /// The declared WAV sample format cannot be unpacked.
    UnknownSampleFormat(SampleFormat),
    /// The SEA codec returned an error during file/header parsing.
    Codec(SeaError),
}

impl std::fmt::Display for AudioFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioFileError::TooShort { actual, required } => write!(
                f,
                "audio file is too short: got {} bytes, need at least {}",
                actual, required
            ),
            AudioFileError::InvalidSignature => {
                write!(f, "missing or invalid RIFF/WAVE signature")
            }
            AudioFileError::UnsupportedSampleFormat {
                audio_format,
                bits_per_sample,
            } => write!(
                f,
                "unsupported WAV sample format: audio_format={}, bits_per_sample={}",
                audio_format, bits_per_sample
            ),
            AudioFileError::ZeroChannels => write!(f, "WAV header reports zero channels"),
            AudioFileError::ZeroSampleRate => write!(f, "WAV header reports zero sample rate"),
            AudioFileError::InvalidSampleBytes {
                sample_format,
                bytes_per_sample,
                actual,
            } => write!(
                f,
                "invalid {} sample: expected {} bytes, got {}",
                sample_format, bytes_per_sample, actual
            ),
            AudioFileError::UnknownSampleFormat(sample_format) => {
                write!(f, "unsupported sample format: {:?}", sample_format)
            }
            AudioFileError::Codec(err) => write!(f, "SEA codec error: {}", err),
        }
    }
}

impl std::error::Error for AudioFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AudioFileError::Codec(err) => Some(err),
            _ => None,
        }
    }
}

// ---- Wasm ----

/// WASM-only failures around threading, browser interop, or async tasks.
#[cfg(target_family = "wasm")]
#[derive(Debug)]
pub enum WasmError {
    /// A JS-side error bubbled up via `wasm_bindgen`.
    JavaScript(String),
    /// `thread::spawn` panicked (typically because `SharedArrayBuffer` is unavailable).
    ThreadSpawnPanic {
        /// Classified reason for the panic (e.g. missing COOP/COEP headers).
        reason: SpawnFailureReason,
        /// The string payload of the panic, when one was extractable.
        message: Option<String>,
    },
}

#[cfg(target_family = "wasm")]
impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WasmError::JavaScript(value) => write!(f, "JavaScript error: {:?}", value),
            WasmError::ThreadSpawnPanic { reason, message } => {
                if let Some(msg) = message {
                    write!(f, "wasm thread spawn panicked ({}): {}", reason, msg)
                } else {
                    write!(f, "wasm thread spawn panicked: {}", reason)
                }
            }
        }
    }
}

#[cfg(target_family = "wasm")]
impl std::error::Error for WasmError {}

/// Classifies why a `thread::spawn` call panicked on WASM.
#[cfg(target_family = "wasm")]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SpawnFailureReason {
    /// No COOP/COEP headers; `SharedArrayBuffer` is unavailable.
    SharedArrayBufferUnavailable,
    /// `spawn` panicked for some other reason.
    Other,
}

#[cfg(target_family = "wasm")]
impl std::fmt::Display for SpawnFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnFailureReason::SharedArrayBufferUnavailable => write!(
                f,
                "threading unavailable (missing SharedArrayBuffer / COOP-COEP headers)"
            ),
            SpawnFailureReason::Other => write!(f, "spawn panicked for an unknown reason"),
        }
    }
}

// ---- Conversions from external errors ----

impl From<DeviceError> for Error {
    fn from(e: DeviceError) -> Self {
        Error::Device(e)
    }
}

impl From<rubato::ResamplerConstructionError> for Error {
    fn from(err: rubato::ResamplerConstructionError) -> Self {
        Error::Processing(ProcessingError::ResamplerConstruction(err))
    }
}

impl From<rubato::ResampleError> for Error {
    fn from(err: rubato::ResampleError) -> Self {
        Error::Processing(ProcessingError::Resample(err))
    }
}

impl From<audioadapter_buffers::SizeError> for Error {
    fn from(err: audioadapter_buffers::SizeError) -> Self {
        Error::Processing(ProcessingError::Buffer(err))
    }
}

impl From<std::array::TryFromSliceError> for Error {
    fn from(err: std::array::TryFromSliceError) -> Self {
        Error::Processing(ProcessingError::Slice(err))
    }
}

impl From<SeaError> for Error {
    fn from(err: SeaError) -> Self {
        Error::Processing(ProcessingError::Codec(err))
    }
}

impl From<rtrb::chunks::ChunkError> for Error {
    fn from(err: rtrb::chunks::ChunkError) -> Self {
        Error::Channel(ChannelError::Chunk(err))
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(err: tokio::task::JoinError) -> Self {
        Error::Task(TaskError::Join(err))
    }
}

#[cfg(target_family = "wasm")]
impl From<tokio::sync::oneshot::error::RecvError> for Error {
    fn from(err: tokio::sync::oneshot::error::RecvError) -> Self {
        Error::Task(TaskError::OneshotReceive(err))
    }
}

#[cfg(target_family = "wasm")]
impl From<WasmError> for Error {
    fn from(err: WasmError) -> Self {
        Error::Wasm(err)
    }
}

#[cfg(target_family = "wasm")]
impl From<wasm_bindgen::JsValue> for Error {
    fn from(err: wasm_bindgen::JsValue) -> Self {
        Error::Wasm(WasmError::JavaScript(format!("{:?}", err)))
    }
}

/// Inspects the panic payload from a `thread::spawn` call and produces a typed
/// [`WasmError`]. Returns `Some` when the failure is WASM-specific; native
/// callers should treat `None` as "use the normal error path".
#[cfg(target_family = "wasm")]
pub(crate) fn classify_panic_payload(panic_info: Box<dyn std::any::Any + Send>) -> WasmError {
    if let Some(s) = panic_info.downcast_ref::<&'static str>() {
        if s.contains("SharedArrayBuffer") || s.contains("COOP") || s.contains("COEP") {
            return WasmError::ThreadSpawnPanic {
                reason: SpawnFailureReason::SharedArrayBufferUnavailable,
                message: Some((*s).to_string()),
            };
        }
        return WasmError::ThreadSpawnPanic {
            reason: SpawnFailureReason::Other,
            message: Some((*s).to_string()),
        };
    }
    if let Some(s) = panic_info.downcast_ref::<String>() {
        if s.contains("SharedArrayBuffer") || s.contains("COOP") || s.contains("COEP") {
            return WasmError::ThreadSpawnPanic {
                reason: SpawnFailureReason::SharedArrayBufferUnavailable,
                message: Some(s.clone()),
            };
        }
        return WasmError::ThreadSpawnPanic {
            reason: SpawnFailureReason::Other,
            message: Some(s.clone()),
        };
    }
    WasmError::ThreadSpawnPanic {
        reason: SpawnFailureReason::Other,
        message: None,
    }
}
