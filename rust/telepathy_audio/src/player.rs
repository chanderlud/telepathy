//! Audio player module for playing WAV and SEA codec files.
//!
//! This module provides a framework-agnostic audio player with support for:
//! - WAV file playback with multiple sample formats (U8, I16, I32, F32, F64)
//! - SEA codec file playback with automatic decoding
//! - Automatic resampling to match output device sample rate
//! - Volume control with decibel-based API
//! - Smooth fade in/out to prevent audio clicks
//! - Cancellation support with graceful fade-out
//! - Platform support for both native and WebAssembly targets
//!
//! ## Usage
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioPlayer, SoundHandle};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a player with -6dB volume
//!     let player = AudioPlayer::new(-6.0);
//!
//!     // Play a sound file (returns error if device unavailable or data invalid)
//!     let bytes = std::fs::read("sound.wav").unwrap();
//!     let handle = player.play(bytes).await.unwrap();
//!
//!     // Optionally cancel playback
//!     handle.cancel();
//! }
//! ```

use crate::AudioHost;
use crate::error::AudioError;
use crate::internal::processing::wide_mul;
#[cfg(target_family = "wasm")]
use crate::internal::thread;
use crate::internal::traits::CHANNEL_SIZE;
use crate::internal::utils::{db_to_multiplier, resampler_factory};
use crate::sea::codec::file::SeaFileHeader;
use crate::sea::decoder::SeaDecoder;
use crate::sea::encoder::{EncoderSettings, SeaEncoder};
use atomic_float::AtomicF32;
use bytes::BytesMut;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, Host, SampleFormat};
use log::{debug, error, info};
use nnnoiseless::FRAME_SIZE;
use rtrb::RingBuffer;
use rubato::Resampler;
use std::mem;
use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::sync::Condvar;
#[cfg(not(target_family = "wasm"))]
use std::sync::Mutex as SyncMutex;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
use tokio::select;
use tokio::sync::oneshot;
use tokio::sync::{Mutex, Notify};
#[cfg(target_family = "wasm")]
use wasm_bindgen_futures::spawn_local;
#[cfg(target_family = "wasm")]
use wasm_sync::{Condvar, Mutex as SyncMutex};
#[cfg(target_family = "wasm")]
use wasmtimer::std::Instant;

/// Number of frames to fade out when canceling playback.
/// This prevents audio pops/clicks when stopping playback abruptly.
const FADE_FRAMES: usize = 60;

/// Iterator-like source of planar `f32` audio frames.
///
/// Implementations encapsulate format-specific decoding (e.g., WAV unpacking or
/// de-interleaving) and expose a unified "next frame" interface to the processor.
trait AudioFrameSource {
    /// Fills `output` with the next chunk of planar `f32` samples.
    ///
    /// Returns:
    /// - `Ok(Some(frame_count))` with samples per channel
    /// - `Ok(None)` when no more frames are available
    /// - `Err(_)` on processing errors
    fn next_frame(&mut self, output: &mut [Vec<f32>]) -> Result<Option<usize>, AudioError>;

    /// Total number of sample frames per channel in the source.
    fn total_frame_count(&self) -> usize;

    /// Number of audio channels.
    fn channels(&self) -> usize;
}

impl<T: AudioFrameSource + ?Sized> AudioFrameSource for Box<T> {
    fn next_frame(&mut self, output: &mut [Vec<f32>]) -> Result<Option<usize>, AudioError> {
        (**self).next_frame(output)
    }

    fn total_frame_count(&self) -> usize {
        (**self).total_frame_count()
    }

    fn channels(&self) -> usize {
        (**self).channels()
    }
}

/// Trait for converting audio samples from various formats to a target type.
///
/// This trait provides methods for converting from each WAV sample format
/// (U8, I16, I32, F32, F64) to the target type. It handles the appropriate
/// scaling and normalization for each format.
///
/// # Implementations
///
/// - `i16`: Converts samples to 16-bit signed integers with appropriate scaling.
///   - U8: Bias adjustment and bit shifting
///   - I16: Direct pass-through
///   - I32: Downscaling from 32-bit to 16-bit range
///   - F32/F64: Multiplication by i16::MAX with clamping
///
/// - `f32`: Normalizes samples to the [-1.0, 1.0] range.
///   - U8: Normalizes [0, 255] to [-1.0, 1.0]
///   - I16/I32: Divides by MAX value
///   - F32: Direct pass-through
///   - F64: Conversion to f32
trait SampleConversion: Sized {
    fn from_u8_sample(value: u8) -> Self;
    fn from_i16_sample(value: i16) -> Self;
    fn from_i32_sample(value: i32) -> Self;
    fn from_f32_sample(value: f32) -> Self;
    fn from_f64_sample(value: f64) -> Self;
}

impl SampleConversion for i16 {
    fn from_u8_sample(value: u8) -> Self {
        // Convert U8 [0, 255] to I16 [-32768, 32767] with bias adjustment
        ((value as i16) - 128) << 8
    }

    fn from_i16_sample(value: i16) -> Self {
        value
    }

    fn from_i32_sample(value: i32) -> Self {
        // Downscale from i32::MAX to i16::MAX (shift right by 16 bits)
        (value >> 16) as i16
    }

    fn from_f32_sample(value: f32) -> Self {
        // Multiply by i16::MAX and clamp to valid range
        (value * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }

    fn from_f64_sample(value: f64) -> Self {
        // Convert to f32 first, then apply f32 logic
        Self::from_f32_sample(value as f32)
    }
}

impl SampleConversion for f32 {
    fn from_u8_sample(value: u8) -> Self {
        // Normalize [0, 255] to [-1.0, 1.0]
        (value as f32 / u8::MAX as f32) * 2.0 - 1.0
    }

    fn from_i16_sample(value: i16) -> Self {
        // Normalize to [-1.0, 1.0]
        value as f32 / i16::MAX as f32
    }

    fn from_i32_sample(value: i32) -> Self {
        // Normalize to [-1.0, 1.0]
        value as f32 / i32::MAX as f32
    }

    fn from_f32_sample(value: f32) -> Self {
        value
    }

    fn from_f64_sample(value: f64) -> Self {
        value as f32
    }
}

struct WavFrameSource {
    bytes: Vec<u8>,
    sample_format: SampleFormat,
    channels: usize,
    position: usize, // Current byte offset in the data section
}

impl WavFrameSource {
    fn new(bytes: Vec<u8>, sample_format: SampleFormat, channels: usize) -> Self {
        Self {
            bytes,
            sample_format,
            channels,
            position: 0,
        }
    }
}

impl AudioFrameSource for WavFrameSource {
    fn next_frame(&mut self, output: &mut [Vec<f32>]) -> Result<Option<usize>, AudioError> {
        if self.channels == 0 {
            return Err(AudioError::Processing(
                "WavFrameSource: channels is 0".to_string(),
            ));
        }

        let data_start = 44usize.saturating_add(self.position);
        if data_start >= self.bytes.len() {
            return Ok(None);
        }

        let chunk_size = FRAME_SIZE * self.sample_format.sample_size();
        let data_end = (data_start + chunk_size).min(self.bytes.len());
        let chunk = &self.bytes[data_start..data_end];

        self.position = self.position.saturating_add(chunk_size);

        let frame_count =
            unpack_wav_frame::<f32>(chunk, self.sample_format, self.channels, output)?;
        if frame_count == 0 {
            Ok(None)
        } else {
            Ok(Some(frame_count))
        }
    }

    fn total_frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }

        let data_len = self.bytes.len().saturating_sub(44);
        data_len / self.sample_format.sample_size() / self.channels
    }

    fn channels(&self) -> usize {
        self.channels
    }
}

struct DecodedFrameSource {
    samples: Vec<[i16; FRAME_SIZE]>,
    channels: usize,
    position: usize, // Current frame index
}

impl DecodedFrameSource {
    fn new(samples: Vec<[i16; FRAME_SIZE]>, channels: usize) -> Self {
        Self {
            samples,
            channels,
            position: 0,
        }
    }
}

impl AudioFrameSource for DecodedFrameSource {
    fn next_frame(&mut self, output: &mut [Vec<f32>]) -> Result<Option<usize>, AudioError> {
        if self.channels == 0 {
            return Err(AudioError::Processing(
                "DecodedFrameSource: channels is 0".to_string(),
            ));
        }

        if self.position >= self.samples.len() {
            return Ok(None);
        }

        let samples_frame = &self.samples[self.position];
        self.position += 1;

        for channel_buf in output.iter_mut() {
            channel_buf.clear();
        }

        let scale = 1_f32 / i16::MAX as f32;
        for (i, sample) in samples_frame.iter().enumerate() {
            let channel_idx = i % self.channels;
            if channel_idx >= output.len() {
                break;
            }

            let scaled_sample = (*sample as f32) * scale;
            output[channel_idx].push(scaled_sample);
        }

        Ok(Some(FRAME_SIZE / self.channels))
    }

    fn total_frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }

        self.samples.len() * FRAME_SIZE / self.channels
    }

    fn channels(&self) -> usize {
        self.channels
    }
}

/// Audio file header information parsed from WAV format.
struct AudioHeader {
    /// Number of audio channels (1 for mono, 2 for stereo).
    channels: u32,
    /// Sample rate in Hz (e.g., 44100, 48000).
    sample_rate: u32,
    /// Sample format (U8, I16, I32, F32, F64).
    sample_format: SampleFormat,
}

impl TryFrom<&[u8]> for AudioHeader {
    type Error = AudioError;

    /// Parses a WAV file header from bytes.
    ///
    /// Expects at least 44 bytes (standard WAV header size).
    /// Supports PCM (format 1) and IEEE float (format 3) formats.
    /// Validates WAV signature (RIFF/WAVE) and ensures channels/sample_rate are non-zero.
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() < 44 {
            return Err(AudioError::InvalidWav);
        }

        // Validate WAV signature: must start with "RIFF" and contain "WAVE"
        if &value[0..4] != b"RIFF" || &value[8..12] != b"WAVE" {
            return Err(AudioError::InvalidWav);
        }

        let bits_per_sample = u16::from_le_bytes([value[34], value[35]]);
        let audio_format = u16::from_le_bytes([value[20], value[21]]);

        let sample_format = match (audio_format, bits_per_sample) {
            (1, 8) => SampleFormat::U8,
            (1, 16) => SampleFormat::I16,
            (1, 32) => SampleFormat::I32,
            (3, 32) => SampleFormat::F32,
            (3, 64) => SampleFormat::F64,
            _ => {
                return Err(AudioError::InvalidWav);
            }
        };

        let channels = u16::from_le_bytes([value[22], value[23]]) as u32;
        let sample_rate = u32::from_le_bytes([value[24], value[25], value[26], value[27]]);

        // Validate channels and sample_rate are non-zero to prevent divide-by-zero
        if channels == 0 || sample_rate == 0 {
            return Err(AudioError::InvalidWav);
        }

        Ok(Self {
            channels,
            sample_rate,
            sample_format,
        })
    }
}

/// Framework-agnostic audio player for WAV and SEA codec files.
///
/// The player handles device selection, volume control, resampling,
/// and provides cancellation support through `SoundHandle`.
pub struct AudioPlayer {
    /// Output volume as a linear multiplier (stored atomically for thread safety).
    output_volume: Arc<AtomicF32>,
    /// Selected output device ID (None uses default device).
    output_device: Arc<Mutex<Option<DeviceId>>>,
    /// The cpal audio host for device access.
    host: AudioHost,
}

impl AudioPlayer {
    /// Creates a new audio player with the specified output volume.
    ///
    /// # Arguments
    ///
    /// * `output_volume_db` - Output volume in decibels. 0 dB is unity gain,
    ///   negative values attenuate, positive values amplify.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::AudioPlayer;
    ///
    /// // Create player at half volume (-6 dB)
    /// let player = AudioPlayer::new(-6.0);
    /// ```
    pub fn new(output_volume_db: f32) -> Self {
        Self {
            output_volume: Arc::new(AtomicF32::new(db_to_multiplier(output_volume_db))),
            output_device: Default::default(),
            host: AudioHost::new(),
        }
    }

    /// Plays audio from the provided bytes.
    ///
    /// Supports both WAV files (with standard 44-byte header) and SEA codec files.
    /// The format is auto-detected based on header validation.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The audio file bytes (WAV or SEA format).
    ///
    /// # Returns
    ///
    /// A `SoundHandle` that can be used to cancel playback.
    ///
    /// # Errors
    ///
    /// Returns `AudioError` if:
    /// - The file is too short (< 44 bytes for WAV, < 14 bytes for SEA)
    /// - No output device is available
    /// - Stream configuration cannot be obtained
    /// - Stream creation fails
    pub async fn play(&self, bytes: Vec<u8>) -> Result<SoundHandle, AudioError> {
        // Preflight validation: reject clearly invalid inputs
        // SEA codec files need at least 14 bytes for header
        if bytes.len() < 14 {
            return Err(AudioError::InvalidWav);
        }

        // Get output device and config before spawning to catch device errors early
        let output_device = get_output_device(&self.output_device, self.host.inner()).await?;
        let output_config = output_device
            .default_output_config()
            .map_err(|e| AudioError::Device(format!("Failed to get output config: {}", e)))?;

        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let output_volume = self.output_volume.clone();

        // Use a oneshot channel to receive initialization result from the spawned task
        // This allows us to return errors from stream creation before the task continues
        let (init_tx, init_rx) = oneshot::channel::<Result<(), AudioError>>();

        #[cfg(not(target_family = "wasm"))]
        let handle = tokio::spawn(async move {
            if let Err(e) = play_sound_with_device(
                bytes,
                cancel_clone,
                output_device,
                output_config,
                output_volume,
                init_tx,
            )
            .await
            {
                // Log any errors that occur after initialization
                // (initialization errors are already sent via init_tx)
                debug!("Playback error (after init): {:?}", e);
            }
        });

        #[cfg(target_family = "wasm")]
        spawn_local(async move {
            if let Err(e) = play_sound_with_device(
                bytes,
                cancel_clone,
                output_device,
                output_config,
                output_volume,
                init_tx,
            )
            .await
            {
                // Log any errors that occur after initialization
                // (initialization errors are already sent via init_tx)
                debug!("Playback error (after init): {:?}", e);
            }
        });

        // Wait for initialization result from the spawned task
        // This returns as soon as the stream is built and playing, or on error
        match init_rx.await {
            #[cfg(not(target_family = "wasm"))]
            Ok(Ok(())) => Ok(SoundHandle {
                cancel,
                _handle: handle,
            }),
            #[cfg(target_family = "wasm")]
            Ok(Ok(())) => Ok(SoundHandle { cancel }),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Channel was dropped, which means the task panicked or was cancelled
                Err(AudioError::Processing(
                    "Playback task terminated unexpectedly".to_string(),
                ))
            }
        }
    }

    /// Updates the output volume.
    ///
    /// # Arguments
    ///
    /// * `volume_db` - New volume in decibels.
    pub fn set_volume(&self, volume_db: f32) {
        self.output_volume
            .store(db_to_multiplier(volume_db), Relaxed);
    }

    /// Sets the output device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device ID to use, or `None` for the default device.
    pub async fn set_output_device(&self, device_id: Option<DeviceId>) {
        *self.output_device.lock().await = device_id;
    }

    /// Returns a reference to the audio host.
    ///
    /// This can be used to enumerate devices or access other host functionality.
    pub fn host(&self) -> Arc<Host> {
        self.host.clone_inner()
    }
}

/// Handle for controlling active sound playback.
///
/// The handle allows canceling playback, which triggers a graceful
/// fade-out to prevent audio pops.
pub struct SoundHandle {
    /// Notification channel for cancellation.
    cancel: Arc<Notify>,
    /// Task handle (kept alive to ensure playback continues).
    #[cfg(not(target_family = "wasm"))]
    _handle: tokio::task::JoinHandle<()>,
}

impl SoundHandle {
    /// Cancels the sound playback.
    ///
    /// This triggers a graceful fade-out over `FADE_FRAMES` frames
    /// to prevent audio pops/clicks.
    pub fn cancel(&self) {
        self.cancel.notify_one();
    }
}

/// Blocking ring buffer audio output for native and WASM platforms.
///
/// ## Blocking Behavior
///
/// `write_samples_blocking` waits on a [`Condvar`] until the ring buffer has
/// enough free slots to accept the full write. This prevents dropping samples
/// at the cost of applying backpressure to the processor.
///
/// ## Cancellation
///
/// The shared `canceled` flag is checked before and after parking on the condvar
/// so that the producer cannot remain blocked when playback is canceled and the
/// stream callback stops draining the consumer during fade-out.
struct BlockingRingBufferOutput {
    /// The producer end of the ring buffer for audio samples.
    producer: rtrb::Producer<f32>,

    /// Condvar for signaling when space becomes available.
    notify: Arc<Condvar>,

    /// Mutex required for condvar wait protocol.
    ///
    /// This is a dummy mutex (contains no data) used solely to satisfy the
    /// condvar API requirements. The actual synchronization is handled by the
    /// lock-free ring buffer.
    mutex: SyncMutex<()>,

    /// Shared cancellation flag – when set, `write_samples_blocking` returns
    /// early instead of waiting for space that may never arrive.
    canceled: Arc<AtomicBool>,
}

impl BlockingRingBufferOutput {
    fn new(producer: rtrb::Producer<f32>, notify: Arc<Condvar>, canceled: Arc<AtomicBool>) -> Self {
        Self {
            producer,
            notify,
            mutex: SyncMutex::new(()),
            canceled,
        }
    }

    /// Writes all samples to the ring buffer, blocking until enough space is available.
    ///
    /// Returns early with an error when the cancellation flag is set or the
    /// consumer side has been dropped, ensuring the producer thread can never
    /// remain parked on the condvar after cancellation is signaled.
    fn write_samples_blocking(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        let target = samples.len();
        loop {
            if self.canceled.load(Relaxed) {
                return Err(AudioError::Channel(
                    "BlockingRingBufferOutput: write canceled".to_string(),
                ));
            }

            if self.producer.slots() >= target {
                break;
            }

            if self.producer.is_abandoned() {
                return Err(AudioError::Channel(
                    "BlockingRingBufferOutput consumer abandoned".to_string(),
                ));
            }

            let guard = self.mutex.lock().unwrap();

            // Re-check after acquiring the lock to avoid missed makeups.
            if self.canceled.load(Relaxed) {
                drop(guard);
                return Err(AudioError::Channel(
                    "BlockingRingBufferOutput: write canceled".to_string(),
                ));
            }

            if self.producer.slots() >= target {
                drop(guard);
                break;
            }

            drop(self.notify.wait(guard).unwrap());
        }

        let chunk = self.producer.write_chunk_uninit(target)?;
        chunk.fill_from_iter(samples.iter().copied());
        Ok(())
    }
}

/// Converts WAV bytes to SEA codec bytes.
///
/// This function encodes WAV audio data into the SEA codec format,
/// which provides efficient compression for audio transmission.
/// Multichannel audio is automatically downmixed to mono by averaging
/// all channels.
///
/// # Arguments
///
/// * `bytes` - The WAV file bytes (must include 44-byte header).
/// * `residual_bits` - Quality parameter for encoding (higher = better quality).
///
/// # Returns
///
/// The encoded SEA file bytes (mono, regardless of input channel count).
///
/// # Errors
///
/// Returns `AudioError` if the WAV data is invalid or encoding fails.
pub async fn wav_to_sea(bytes: Vec<u8>, residual_bits: f32) -> Result<Vec<u8>, AudioError> {
    if bytes.len() < 44 {
        return Err(AudioError::Processing("WAV data too short".to_string()));
    }

    let spec = AudioHeader::try_from(&bytes[0..44])?;
    let channels = spec.channels;
    let sample_rate = spec.sample_rate;

    let sample_size = match spec.sample_format {
        SampleFormat::U8 => 1,
        SampleFormat::I16 => 2,
        SampleFormat::I32 | SampleFormat::F32 => 4,
        SampleFormat::F64 => 8,
        _ => 1,
    };

    let sample_format = spec.sample_format;

    spawn_cpu_task(move || {
        let settings = EncoderSettings {
            frames_per_chunk: FRAME_SIZE as u16,
            vbr: true,
            residual_bits,
            ..Default::default()
        };

        let mut encoder = SeaEncoder::new(1, sample_rate, settings)?;

        let mut samples = [0_i16; FRAME_SIZE];
        let mut buffer = BytesMut::new();
        let mut data: Vec<u8> = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 0,
            frames_per_chunk: FRAME_SIZE as u16,
            sample_rate,
        }
        .serialize();

        // Create planar buffers for channel-aware unpacking
        let mut planar_buf: Vec<Vec<i16>> = vec![Vec::with_capacity(FRAME_SIZE); channels as usize];

        for chunk in bytes[44..].chunks(FRAME_SIZE * sample_size * channels as usize) {
            // Unpack interleaved bytes to planar i16 buffers
            unpack_wav_frame(chunk, sample_format, channels as usize, &mut planar_buf)?;

            // Downmix all channels to mono by averaging
            for idx in 0..FRAME_SIZE.min(planar_buf.first().map(|b| b.len()).unwrap_or(0)) {
                let mut sum = 0_i32;
                for channel in &planar_buf {
                    if idx < channel.len() {
                        sum += channel[idx] as i32;
                    }
                }
                samples[idx] = (sum / channels as i32) as i16;
            }

            let actual_samples = planar_buf.first().map(|b| b.len()).unwrap_or(0);
            if actual_samples < FRAME_SIZE {
                for sample in samples.iter_mut().take(FRAME_SIZE).skip(actual_samples) {
                    *sample = 0;
                }
            }

            encoder.encode_frame(samples, &mut buffer)?;
            data.extend_from_slice(buffer.as_ref());
        }

        // update the header with the real chunk size
        data[6..=7].copy_from_slice(encoder.chunk_size().to_le_bytes().as_ref());
        Ok::<Vec<u8>, AudioError>(data)
    })
    .await
}

/// Gets the output device based on the configured device ID.
async fn get_output_device(
    output_device: &Arc<Mutex<Option<DeviceId>>>,
    host: &Host,
) -> Result<cpal::Device, AudioError> {
    match *output_device.lock().await {
        Some(ref id) => Ok(host.device_by_id(id).unwrap_or(
            host.default_output_device()
                .ok_or(AudioError::Device("No output device available".to_string()))?,
        )),
        None => host
            .default_output_device()
            .ok_or(AudioError::Device("No output device available".to_string())),
    }
}

/// Internal play sound function with pre-obtained device and config.
///
/// This function is called after preflight validation and device acquisition
/// have been completed by the caller. It signals initialization success or failure
/// via the `init_tx` channel before continuing with playback.
async fn play_sound_with_device(
    mut bytes: Vec<u8>,
    cancel: Arc<Notify>,
    output_device: cpal::Device,
    output_config: cpal::SupportedStreamConfig,
    output_volume: Arc<AtomicF32>,
    init_tx: oneshot::Sender<Result<(), AudioError>>,
) -> Result<(), AudioError> {
    // Perform initialization and signal result to caller
    // This inner async block allows us to use ? while still signaling errors
    let init_result: Result<_, AudioError> = async {
        // Parse the input spec (only attempt WAV parsing if we have enough bytes)
        let spec_result = if bytes.len() >= 44 {
            AudioHeader::try_from(&bytes[0..44])
        } else {
            Err(AudioError::InvalidWav)
        };
        let is_valid_wav = spec_result.is_ok();
        let mut spec = spec_result.unwrap_or(AudioHeader {
            channels: 0,
            sample_rate: 0,
            sample_format: SampleFormat::I16,
        });
        let mut samples = None;

        // If not valid WAV, try to handle as SEA codec file
        if !is_valid_wav {
            let now = Instant::now();
            let local_bytes = mem::take(&mut bytes);
            let header = SeaFileHeader::from_frame(&local_bytes[..14])?;
            info!("loaded header {:?}", header);
            let chunk_size = header.chunk_size as usize;
            spec.channels = header.channels as u32;
            spec.sample_rate = header.sample_rate;
            let mut decoder = SeaDecoder::new(header)?;

            samples = Some(
                spawn_cpu_task(move || {
                    let mut decoded = Vec::new();
                    let mut buffer = [0_i16; FRAME_SIZE];

                    for chunk in local_bytes[14..].chunks(chunk_size) {
                        decoder.decode_frame(chunk, &mut buffer)?;
                        decoded.push(buffer);
                    }

                    Ok::<Vec<[i16; FRAME_SIZE]>, AudioError>(decoded)
                })
                .await?,
            );
            info!("decoding sound took {:?}", now.elapsed());
        }

        // Validate that we have valid audio parameters before proceeding
        // This prevents divide-by-zero and buffer panics from malformed input
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(AudioError::InvalidWav);
        }

        // The resampling ratio used by the processor
        let ratio = output_config.sample_rate() as f64 / spec.sample_rate as f64;

        // Shared cancellation/finish state between control flow, processor, and stream callback
        let processor_canceled = Arc::new(AtomicBool::new(false));
        let processor_finished = Arc::new(AtomicBool::new(false));

        // Create unified ring buffer for processor -> stream communication
        let (processor_producer, stream_consumer) = RingBuffer::<f32>::new(CHANNEL_SIZE * 4);
        let space_available = Arc::new(Condvar::new());
        let processor_output = BlockingRingBufferOutput::new(
            processor_producer,
            space_available.clone(),
            processor_canceled.clone(),
        );

        // Notifies this thread when playback has finished
        let output_finished = Arc::new(Notify::new());
        // Used inside the output stream to notify
        let output_finished_clone = output_finished.clone();
        // Used to chunk the output buffer correctly
        let output_channels = output_config.channels() as usize;
        // Keep track of the last samples played
        let mut last_samples = vec![0_f32; output_channels];
        // A counter used for fading out the last samples when the sound is canceled
        let mut i = 0;
        // Used to provide a fade to 0 when the sound is canceled
        let f32_sample_rate = output_config.sample_rate() as f32;

        let processor_canceled_for_stream = processor_canceled.clone();
        let processor_finished_for_stream = processor_finished.clone();
        let space_available_for_stream = space_available.clone();

        let mut stream_consumer = stream_consumer;
        let output_stream = output_device.build_output_stream(
            &output_config.into(),
            move |output: &mut [f32], _| {
                let mut canceled = processor_canceled_for_stream.load(Relaxed);
                for frame in output.chunks_mut(output_channels) {
                    if canceled {
                        // After full fade fill frames with 0
                        if i == FADE_FRAMES {
                            frame.fill(0_f32);
                            continue;
                        }

                        // Fade each sample
                        for sample in &mut last_samples {
                            *sample *= (1_f32 - i as f32 / f32_sample_rate).max(0_f32);
                        }

                        // Play the samples
                        frame.copy_from_slice(&last_samples);
                        // Advance the counter
                        i += 1;
                        // Notify main thread once after the full fade has occurred
                        if i == FADE_FRAMES {
                            output_finished_clone.notify_one();
                        }
                    } else {
                        match stream_consumer.read_chunk(output_channels) {
                            Ok(chunk) => {
                                for (sample, out) in chunk.into_iter().zip(frame.iter_mut()) {
                                    *out = sample;
                                }
                                last_samples.copy_from_slice(frame);
                                space_available_for_stream.notify_one();
                            }
                            Err(_) => {
                                // If the processor has finished and the buffer is empty, begin fading out.
                                // If the processor is still running, hold the last samples (prevents pops).
                                if processor_finished_for_stream.load(Relaxed) {
                                    processor_canceled_for_stream.store(true, Relaxed);
                                    canceled = true;
                                }
                                frame.copy_from_slice(&last_samples);
                            }
                        }
                    }
                }
            },
            move |err| {
                error!("Error in player stream: {}", err);
            },
            None,
        )?;

        let processor_canceled_for_processor = processor_canceled.clone();
        let processor_finished_for_processor = processor_finished.clone();

        let processor_future = spawn_cpu_task(move || {
            let source: Box<dyn AudioFrameSource> = if let Some(samples) = samples {
                Box::new(DecodedFrameSource::new(samples, spec.channels as usize))
            } else {
                Box::new(WavFrameSource::new(
                    bytes,
                    spec.sample_format,
                    spec.channels as usize,
                ))
            };
            let result = processor(
                source,
                output_volume,
                processor_output,
                output_channels,
                ratio,
                processor_canceled_for_processor,
            );
            processor_finished_for_processor.store(true, Relaxed);
            result
        });

        output_stream.play()?; // Play the stream

        Ok((
            output_stream,
            output_finished,
            processor_canceled,
            processor_future,
            space_available,
        ))
    }
    .await;

    // Signal initialization result to the caller
    let (output_stream, output_finished, processor_canceled, processor_future, space_available) =
        match init_result {
            Ok(state) => {
                let _ = init_tx.send(Ok(()));
                state
            }
            Err(e) => {
                let err_msg = e.to_string();
                let _ = init_tx.send(Err(AudioError::Processing(err_msg)));
                return Err(e);
            }
        };

    tokio::pin!(processor_future);

    let mut processor_join: Option<_> = None;

    select! {
        _ = cancel.notified() => {
            debug!("reached cancel sound branch");
            // This causes the stream to begin fading out and asks the processor to stop.
            // The condvar notification ensures the producer wakes up from any blocking
            // wait inside write_samples_blocking so it can observe the flag and exit.
            processor_canceled.store(true, Relaxed);
            space_available.notify_one();
        }
        result = &mut processor_future => {
            debug!("processor finished: {:?}", result);
            // Keep track of the return value
            processor_join = Some(result);
        }
    }

    // Wait for playback to complete before tearing down
    output_finished.notified().await;
    // we wait one second before dropping the stream.
    // this ensures the effect playback cleanly fades to zero every time
    cfg_if::cfg_if! {
        if #[cfg(target_family = "wasm")] {
            wasmtimer::tokio::sleep(Duration::from_secs(1)).await;
        } else {
             tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    // dropping the output stream ends playback
    drop(output_stream);
    debug!("starting to tear down player stack");
    // Join the processor task
    match processor_join {
        Some(result) => result?,
        None => processor_future.await?,
    };
    debug!("finished tearing down player stack");
    Ok(())
}

/// Processes audio data from either WAV bytes or pre-decoded SEA samples.
///
/// # Arguments
///
/// * `input` - Audio input, either raw WAV bytes or pre-decoded samples
/// * `sample_format` - Sample format for WAV bytes (ignored for decoded samples)
/// * `spec` - Audio specification (channels, sample rate)
fn processor<S: AudioFrameSource>(
    mut source: S,
    output_volume: Arc<AtomicF32>,
    mut output: BlockingRingBufferOutput,
    output_channels: usize,
    ratio: f64,
    processor_canceled: Arc<AtomicBool>,
) -> Result<(), AudioError> {
    let channels_usize = source.channels();
    if channels_usize == 0 {
        return Err(AudioError::Processing(
            "AudioFrameSource: channels is 0".to_string(),
        ));
    }

    // The number of sample frames in the file (per channel)
    let sample_count = source.total_frame_count();
    // The number of audio samples which will be played
    let audio_len = (sample_count as f64 * ratio) as f32;
    let mut position = 0_f32; // The playback position

    // Constants used for fading in and out
    let fade_basis = sample_count as f32 / 100_f32;
    let fade_out = fade_basis;
    let fade_in = audio_len - fade_basis;

    // Rubato requires 10 extra bytes in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 / channels_usize as f64 * ratio + 10.0) as usize;

    // The output for the resampler
    let mut post_buf = vec![vec![0_f32; post_len]; channels_usize];
    // The input for the resampler
    let mut pre_buf = vec![Vec::with_capacity(FRAME_SIZE / channels_usize); channels_usize];
    // Groups of samples ready to be sent to the output
    let mut out_buf = Vec::with_capacity(output_channels);

    let mut resampler = resampler_factory(ratio, channels_usize, FRAME_SIZE / channels_usize)?;
    let output_volume = output_volume.load(Relaxed);

    loop {
        if processor_canceled.load(Relaxed) {
            break;
        }

        let Some(actual_frame_count) = source.next_frame(&mut pre_buf)? else {
            break;
        };

        for channel in pre_buf.iter_mut() {
            wide_mul(channel, output_volume);
        }

        let (target_buffer, len) = if let Some(resampler) = &mut resampler {
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;
            (&mut post_buf, processed.1)
        } else {
            (&mut pre_buf, actual_frame_count)
        };

        for i in 0..len {
            if processor_canceled.load(Relaxed) {
                break;
            }

            let multiplier = if position < audio_len {
                let delta = audio_len - position;

                if delta < fade_out {
                    // Calculate fade out multiplier
                    delta / fade_basis
                } else if delta > fade_in {
                    // Calculate fade in multiplier
                    position / fade_basis
                } else {
                    1_f32 // No fade in or out
                }
            } else {
                0_f32 // The calculated audio_len is too short
            };

            position += 1_f32; // Advance the position

            for j in 0..output_channels {
                // This handles when there are more output channels than input channels
                let src_ch = if j >= channels_usize { 0 } else { j };
                let sample = target_buffer
                    .get(src_ch)
                    .and_then(|ch| ch.get(i))
                    .copied()
                    .unwrap_or(0_f32);

                out_buf.push(sample * multiplier);
            }

            // Write interleaved samples to the unified output
            output.write_samples_blocking(&out_buf)?;
            out_buf.clear();
        }
    }

    Ok(())
}

/// Unpacks WAV audio data from interleaved bytes to planar channel buffers.
///
/// This function properly handles multi-channel audio by:
/// 1. Chunking input bytes based on `bytes_per_sample * channels` to extract complete sample frames
/// 2. Converting each interleaved frame to planar format by distributing samples to channel buffers
/// 3. Using the `SampleConversion` trait to support both i16 and f32 output types
///
/// # Arguments
///
/// * `chunk` - The raw WAV byte data (excluding header)
/// * `sample_format` - The sample format of the input data (U8, I16, I32, F32, F64)
/// * `channels` - Number of audio channels
/// * `output` - Pre-allocated planar buffers, one Vec per channel. Buffers are cleared and filled.
///
/// # Returns
///
/// The number of sample frames successfully processed, or an error for unsupported formats.
///
/// # Example
///
/// ```rust,ignore
/// let mut planar_buf = vec![vec![0_f32; frame_count]; channels];
/// let frames = unpack_wav_frame(&chunk, SampleFormat::I16, 2, &mut planar_buf)?;
/// // planar_buf[0] contains left channel samples
/// // planar_buf[1] contains right channel samples
/// ```
fn unpack_wav_frame<T: SampleConversion>(
    chunk: &[u8],
    sample_format: SampleFormat,
    channels: usize,
    output: &mut [Vec<T>],
) -> Result<usize, AudioError> {
    let bytes_per_sample = sample_format.sample_size();
    let bytes_per_frame = bytes_per_sample * channels;

    // Clear output buffers
    for channel_buf in output.iter_mut() {
        channel_buf.clear();
    }

    let mut frame_count = 0;

    // Process complete frames only
    for frame in chunk.chunks(bytes_per_frame) {
        if frame.len() < bytes_per_frame {
            break; // Skip incomplete frames at the end
        }

        for (ch_idx, sample_bytes) in frame.chunks(bytes_per_sample).enumerate() {
            if ch_idx >= output.len() {
                break;
            }

            let sample =
                match sample_format {
                    SampleFormat::U8 => T::from_u8_sample(sample_bytes[0]),
                    SampleFormat::I16 => {
                        let value = i16::from_le_bytes(sample_bytes.try_into().map_err(|_| {
                            AudioError::Processing("Invalid I16 sample".to_string())
                        })?);
                        T::from_i16_sample(value)
                    }
                    SampleFormat::I32 => {
                        let value = i32::from_le_bytes(sample_bytes.try_into().map_err(|_| {
                            AudioError::Processing("Invalid I32 sample".to_string())
                        })?);
                        T::from_i32_sample(value)
                    }
                    SampleFormat::F32 => {
                        let value = f32::from_le_bytes(sample_bytes.try_into().map_err(|_| {
                            AudioError::Processing("Invalid F32 sample".to_string())
                        })?);
                        T::from_f32_sample(value)
                    }
                    SampleFormat::F64 => {
                        let value = f64::from_le_bytes(sample_bytes.try_into().map_err(|_| {
                            AudioError::Processing("Invalid F64 sample".to_string())
                        })?);
                        T::from_f64_sample(value)
                    }
                    _ => {
                        return Err(AudioError::Processing(format!(
                            "Unsupported sample format: {:?}",
                            sample_format
                        )));
                    }
                };

            output[ch_idx].push(sample);
        }

        frame_count += 1;
    }

    Ok(frame_count)
}

/// Runs a CPU-bound closure on a blocking context and returns its result.
///
/// On native targets uses `tokio::task::spawn_blocking` for optimal performance.
/// On WASM uses the thread abstraction with a oneshot channel to bridge synchronous
/// execution to an awaitable future (no multi-threaded runtime in browser).
#[cfg(not(target_family = "wasm"))]
async fn spawn_cpu_task<F, R>(f: F) -> Result<R, AudioError>
where
    F: FnOnce() -> Result<R, AudioError> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AudioError::JoinError(format!("JoinError: {}", e)))?
}

#[cfg(target_family = "wasm")]
async fn spawn_cpu_task<F, R>(f: F) -> Result<R, AudioError>
where
    F: FnOnce() -> Result<R, AudioError> + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    thread::safe_spawn(move || {
        let result = f();
        let _ = tx.send(result);
    })?;
    rx.await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_to_multiplier() {
        // 0 dB = unity gain
        assert!((db_to_multiplier(0.0) - 1.0).abs() < 0.001);

        // -20 dB = 0.1
        assert!((db_to_multiplier(-20.0) - 0.1).abs() < 0.001);

        // +20 dB = 10.0
        assert!((db_to_multiplier(20.0) - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_audio_header_parsing() {
        // Minimal valid WAV header (44 bytes, 16-bit PCM, stereo, 44100 Hz)
        let mut header = vec![0u8; 44];
        // RIFF header
        header[0..4].copy_from_slice(b"RIFF");
        header[8..12].copy_from_slice(b"WAVE");
        // Format chunk
        header[12..16].copy_from_slice(b"fmt ");
        header[16..20].copy_from_slice(&16u32.to_le_bytes()); // Chunk size
        header[20..22].copy_from_slice(&1u16.to_le_bytes()); // Audio format (PCM)
        header[22..24].copy_from_slice(&2u16.to_le_bytes()); // Channels
        header[24..28].copy_from_slice(&44100u32.to_le_bytes()); // Sample rate
        header[28..32].copy_from_slice(&176400u32.to_le_bytes()); // Byte rate
        header[32..34].copy_from_slice(&4u16.to_le_bytes()); // Block align
        header[34..36].copy_from_slice(&16u16.to_le_bytes()); // Bits per sample

        let parsed = AudioHeader::try_from(&header[..]).unwrap();
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.sample_rate, 44100);
        assert!(matches!(parsed.sample_format, SampleFormat::I16));
    }

    #[test]
    fn test_audio_header_too_short() {
        let short_header = vec![0u8; 20];
        assert!(AudioHeader::try_from(&short_header[..]).is_err());
    }

    // Tests for SampleConversion trait

    #[test]
    fn test_sample_conversion_i16_from_u8() {
        // U8 128 (center) should become I16 0
        assert_eq!(i16::from_u8_sample(128), 0);
        // U8 0 should become I16 -32768
        assert_eq!(i16::from_u8_sample(0), -32768);
        // U8 255 should become I16 32512 (close to max)
        assert_eq!(i16::from_u8_sample(255), 32512);
    }

    #[test]
    fn test_sample_conversion_i16_from_i16() {
        assert_eq!(i16::from_i16_sample(0), 0);
        assert_eq!(i16::from_i16_sample(i16::MAX), i16::MAX);
        assert_eq!(i16::from_i16_sample(i16::MIN), i16::MIN);
    }

    #[test]
    fn test_sample_conversion_i16_from_i32() {
        // i32::MAX should become i16::MAX (approximately)
        assert_eq!(i16::from_i32_sample(i32::MAX), i16::MAX);
        // i32::MIN should become i16::MIN
        assert_eq!(i16::from_i32_sample(i32::MIN), i16::MIN);
        // 0 should remain 0
        assert_eq!(i16::from_i32_sample(0), 0);
    }

    #[test]
    fn test_sample_conversion_i16_from_f32() {
        // 1.0 should clamp to i16::MAX
        assert_eq!(i16::from_f32_sample(1.0), i16::MAX);
        // -1.0 should become -i16::MAX (clamped)
        assert_eq!(i16::from_f32_sample(-1.0), -i16::MAX);
        // 0.0 should remain 0
        assert_eq!(i16::from_f32_sample(0.0), 0);
        // Values beyond range should clamp
        assert_eq!(i16::from_f32_sample(2.0), i16::MAX);
        assert_eq!(i16::from_f32_sample(-2.0), i16::MIN);
    }

    #[test]
    fn test_sample_conversion_f32_from_u8() {
        // U8 0 should become -1.0
        assert!((f32::from_u8_sample(0) - (-1.0)).abs() < 0.01);
        // U8 255 should become 1.0
        assert!((f32::from_u8_sample(255) - 1.0).abs() < 0.01);
        // U8 128 should become approximately 0.0
        assert!(f32::from_u8_sample(128).abs() < 0.01);
    }

    #[test]
    fn test_sample_conversion_f32_from_i16() {
        // i16::MAX should become approximately 1.0
        assert!((f32::from_i16_sample(i16::MAX) - 1.0).abs() < 0.001);
        // i16::MIN should become approximately -1.0
        assert!(f32::from_i16_sample(i16::MIN) < -0.99);
        // 0 should remain 0
        assert!((f32::from_i16_sample(0)).abs() < 0.001);
    }

    #[test]
    fn test_sample_conversion_f32_from_i32() {
        // i32::MAX should become approximately 1.0
        assert!((f32::from_i32_sample(i32::MAX) - 1.0).abs() < 0.001);
        // 0 should remain 0
        assert!((f32::from_i32_sample(0)).abs() < 0.001);
    }

    #[test]
    fn test_sample_conversion_f32_from_f32() {
        assert_eq!(f32::from_f32_sample(0.5), 0.5);
        assert_eq!(f32::from_f32_sample(-0.5), -0.5);
        assert_eq!(f32::from_f32_sample(1.0), 1.0);
    }

    #[test]
    fn test_sample_conversion_f32_from_f64() {
        assert!((f32::from_f64_sample(0.5_f64) - 0.5).abs() < 0.001);
        assert!((f32::from_f64_sample(-0.5_f64) - (-0.5)).abs() < 0.001);
    }

    // Tests for unpack_wav_frame function

    #[test]
    fn test_unpack_wav_frame_u8_mono() {
        // Mono U8 audio: 3 samples [64, 128, 192]
        let chunk = vec![64_u8, 128, 192];
        let mut output: Vec<Vec<f32>> = vec![Vec::new()];

        let frames = unpack_wav_frame(&chunk, SampleFormat::U8, 1, &mut output).unwrap();

        assert_eq!(frames, 3);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].len(), 3);
        // 64/255*2-1 ≈ -0.498, 128/255*2-1 ≈ 0.004, 192/255*2-1 ≈ 0.506
        assert!(output[0][0] < 0.0); // 64 is below center
        assert!(output[0][1].abs() < 0.02); // 128 is center
        assert!(output[0][2] > 0.0); // 192 is above center
    }

    #[test]
    fn test_unpack_wav_frame_u8_stereo() {
        // Stereo U8: [L1, R1, L2, R2] = [64, 192, 128, 255]
        let chunk = vec![64_u8, 192, 128, 255];
        let mut output: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];

        let frames = unpack_wav_frame(&chunk, SampleFormat::U8, 2, &mut output).unwrap();

        assert_eq!(frames, 2);
        assert_eq!(output[0].len(), 2); // Left channel
        assert_eq!(output[1].len(), 2); // Right channel
        // Verify interleaved to planar conversion
        assert!(output[0][0] < 0.0); // L1 = 64
        assert!(output[1][0] > 0.0); // R1 = 192
    }

    #[test]
    fn test_unpack_wav_frame_i16_mono() {
        // Mono I16 audio: 2 samples in little-endian
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&1000_i16.to_le_bytes());
        chunk.extend_from_slice(&(-1000_i16).to_le_bytes());

        let mut output: Vec<Vec<f32>> = vec![Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::I16, 1, &mut output).unwrap();

        assert_eq!(frames, 2);
        assert!(output[0][0] > 0.0); // Positive sample
        assert!(output[0][1] < 0.0); // Negative sample
    }

    #[test]
    fn test_unpack_wav_frame_i16_stereo() {
        // Stereo I16: [L1, R1, L2, R2]
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&10000_i16.to_le_bytes()); // L1
        chunk.extend_from_slice(&(-10000_i16).to_le_bytes()); // R1
        chunk.extend_from_slice(&5000_i16.to_le_bytes()); // L2
        chunk.extend_from_slice(&(-5000_i16).to_le_bytes()); // R2

        let mut output: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::I16, 2, &mut output).unwrap();

        assert_eq!(frames, 2);
        assert_eq!(output[0].len(), 2);
        assert_eq!(output[1].len(), 2);
        // Left channel should be positive, right channel negative
        assert!(output[0][0] > 0.0 && output[0][1] > 0.0);
        assert!(output[1][0] < 0.0 && output[1][1] < 0.0);
    }

    #[test]
    fn test_unpack_wav_frame_i32_mono() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&(i32::MAX / 2).to_le_bytes());

        let mut output: Vec<Vec<f32>> = vec![Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::I32, 1, &mut output).unwrap();

        assert_eq!(frames, 1);
        assert!((output[0][0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_unpack_wav_frame_f32_stereo() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&0.75_f32.to_le_bytes()); // L1
        chunk.extend_from_slice(&(-0.25_f32).to_le_bytes()); // R1

        let mut output: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::F32, 2, &mut output).unwrap();

        assert_eq!(frames, 1);
        assert!((output[0][0] - 0.75).abs() < 0.001);
        assert!((output[1][0] - (-0.25)).abs() < 0.001);
    }

    #[test]
    fn test_unpack_wav_frame_f64_mono() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&0.333_f64.to_le_bytes());

        let mut output: Vec<Vec<f32>> = vec![Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::F64, 1, &mut output).unwrap();

        assert_eq!(frames, 1);
        assert!((output[0][0] - 0.333).abs() < 0.001);
    }

    #[test]
    fn test_unpack_wav_frame_surround_51() {
        // 5.1 surround (6 channels) with I16 samples
        let channels = 6;
        let mut chunk = Vec::new();
        // One frame with 6 channel samples
        for i in 0..channels {
            chunk.extend_from_slice(&((i as i16 + 1) * 1000).to_le_bytes());
        }

        let mut output: Vec<Vec<f32>> = vec![Vec::new(); channels];
        let frames = unpack_wav_frame(&chunk, SampleFormat::I16, channels, &mut output).unwrap();

        assert_eq!(frames, 1);
        for (i, channel) in output.iter().enumerate() {
            assert_eq!(channel.len(), 1);
            // Each channel should have its unique value
            let expected = (i as i16 + 1) * 1000;
            let expected_f32 = expected as f32 / i16::MAX as f32;
            assert!((channel[0] - expected_f32).abs() < 0.001);
        }
    }

    #[test]
    fn test_unpack_wav_frame_empty_chunk() {
        let chunk: Vec<u8> = Vec::new();
        let mut output: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];

        let frames = unpack_wav_frame(&chunk, SampleFormat::I16, 2, &mut output).unwrap();

        assert_eq!(frames, 0);
        assert!(output[0].is_empty());
        assert!(output[1].is_empty());
    }

    #[test]
    fn test_unpack_wav_frame_partial_frame() {
        // Stereo I16 requires 4 bytes per frame, provide only 3
        let chunk = vec![0_u8, 0, 0]; // Incomplete frame

        let mut output: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::I16, 2, &mut output).unwrap();

        // Should skip incomplete frame
        assert_eq!(frames, 0);
    }

    #[test]
    fn test_unpack_wav_frame_to_i16() {
        // Test unpacking to i16 output (used by wav_to_sea)
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&i16::MAX.to_le_bytes()); // L1
        chunk.extend_from_slice(&i16::MIN.to_le_bytes()); // R1

        let mut output: Vec<Vec<i16>> = vec![Vec::new(), Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::I16, 2, &mut output).unwrap();

        assert_eq!(frames, 1);
        assert_eq!(output[0][0], i16::MAX);
        assert_eq!(output[1][0], i16::MIN);
    }

    #[test]
    fn test_unpack_wav_frame_u8_to_i16() {
        // U8 to i16 conversion
        let chunk = vec![0_u8, 128, 255]; // Min, center, max

        let mut output: Vec<Vec<i16>> = vec![Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::U8, 1, &mut output).unwrap();

        assert_eq!(frames, 3);
        assert_eq!(output[0][0], -32768); // 0 -> -32768
        assert_eq!(output[0][1], 0); // 128 -> 0
        assert_eq!(output[0][2], 32512); // 255 -> 32512
    }

    #[test]
    fn test_unpack_wav_frame_f32_to_i16() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&1.0_f32.to_le_bytes());
        chunk.extend_from_slice(&0.0_f32.to_le_bytes());
        chunk.extend_from_slice(&(-1.0_f32).to_le_bytes());

        let mut output: Vec<Vec<i16>> = vec![Vec::new()];
        let frames = unpack_wav_frame(&chunk, SampleFormat::F32, 1, &mut output).unwrap();

        assert_eq!(frames, 3);
        assert_eq!(output[0][0], i16::MAX);
        assert_eq!(output[0][1], 0);
        assert_eq!(output[0][2], -i16::MAX);
    }

    #[test]
    fn test_wav_frame_source_next_frame_and_eos() {
        // 2 frames, stereo I16: [L1, R1, L2, R2]
        let mut bytes = vec![0_u8; 44];
        bytes.extend_from_slice(&1000_i16.to_le_bytes());
        bytes.extend_from_slice(&(-1000_i16).to_le_bytes());
        bytes.extend_from_slice(&2000_i16.to_le_bytes());
        bytes.extend_from_slice(&(-2000_i16).to_le_bytes());

        let mut src = WavFrameSource::new(bytes, SampleFormat::I16, 2);
        let mut out: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];

        assert_eq!(src.channels(), 2);
        assert_eq!(src.total_frame_count(), 2);

        let frames = src.next_frame(&mut out).unwrap().unwrap();
        assert_eq!(frames, 2);
        assert_eq!(out[0].len(), 2);
        assert_eq!(out[1].len(), 2);
        assert!(out[0][0] > 0.0);
        assert!(out[1][0] < 0.0);

        let next = src.next_frame(&mut out).unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn test_decoded_frame_source_next_frame_and_eos() {
        let mut frame = [0_i16; FRAME_SIZE];
        for i in 0..FRAME_SIZE {
            frame[i] = if i % 2 == 0 { i as i16 } else { -(i as i16) };
        }

        let mut src = DecodedFrameSource::new(vec![frame], 2);
        let mut out: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];

        assert_eq!(src.channels(), 2);
        assert_eq!(src.total_frame_count(), FRAME_SIZE / 2);

        let frames = src.next_frame(&mut out).unwrap().unwrap();
        assert_eq!(frames, FRAME_SIZE / 2);
        assert_eq!(out[0].len(), FRAME_SIZE / 2);
        assert_eq!(out[1].len(), FRAME_SIZE / 2);

        // De-interleaving check: channel 0 gets even indices, channel 1 gets odd indices
        let scale = 1_f32 / i16::MAX as f32;
        assert!((out[0][0] - (0_f32 * scale)).abs() < 1e-6);
        assert!((out[1][0] - (-1_f32 * scale)).abs() < 1e-6);
        assert!((out[0][1] - (2_f32 * scale)).abs() < 1e-6);
        assert!((out[1][1] - (-3_f32 * scale)).abs() < 1e-6);

        let next = src.next_frame(&mut out).unwrap();
        assert!(next.is_none());
    }
}
