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

use crate::SeaFileHeader;
use crate::error::AudioError;
use crate::internal::processing::wide_mul;
use crate::internal::utils::{db_to_multiplier, resampler_factory};
use crate::sea::decoder::SeaDecoder;
use crate::sea::encoder::{EncoderSettings, SeaEncoder};
use atomic_float::AtomicF32;
use bytes::BytesMut;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, Host, SampleFormat};
#[cfg(not(target_family = "wasm"))]
use kanal::{Sender, bounded};
use log::{debug, error, info};
use nnnoiseless::FRAME_SIZE;
use rubato::Resampler;
use std::mem;
use std::sync::Arc;
#[cfg(target_family = "wasm")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;
use tokio::select;
use tokio::sync::{Mutex, Notify};
use tokio::task::spawn_blocking;
#[cfg(target_family = "wasm")]
use wasm_sync::{Condvar, Mutex as WasmMutex};

/// Number of frames to fade out when canceling playback.
/// This prevents audio pops/clicks when stopping playback abruptly.
const FADE_FRAMES: usize = 60;

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

/// Audio buffer for WASM platform with synchronization primitives.
#[cfg(target_family = "wasm")]
#[derive(Default)]
struct AudioBuffer {
    /// The audio sample buffer.
    buffer: WasmMutex<Vec<f32>>,
    /// Flag indicating playback has been canceled.
    canceled: AtomicBool,
    /// Condition variable for buffer synchronization.
    condvar: Condvar,
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
    host: Arc<Host>,
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
        cfg_if::cfg_if! {
            if #[cfg(target_family = "wasm")] {
                // Attempt to use the AudioWorklet host on WASM for better performance
                let host = cpal::host_from_id(cpal::HostId::AudioWorklet).unwrap_or(cpal::default_host());
            } else {
                let host = cpal::default_host();
            }
        }

        Self {
            output_volume: Arc::new(AtomicF32::new(db_to_multiplier(output_volume_db))),
            output_device: Default::default(),
            host: Arc::new(host),
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
        let output_device = get_output_device(&self.output_device, &self.host).await?;
        let output_config = output_device
            .default_output_config()
            .map_err(|e| AudioError::Device(format!("Failed to get output config: {}", e)))?;

        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();
        let output_volume = self.output_volume.clone();

        // Use a oneshot channel to receive initialization result from the spawned task
        // This allows us to return errors from stream creation before the task continues
        let (init_tx, init_rx) = tokio::sync::oneshot::channel::<Result<(), AudioError>>();

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

        // Wait for initialization result from the spawned task
        // This returns as soon as the stream is built and playing, or on error
        match init_rx.await {
            Ok(Ok(())) => Ok(SoundHandle {
                cancel,
                _handle: handle,
            }),
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
        self.host.clone()
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

/// Gets the output device based on the configured device ID.
async fn get_output_device(
    output_device: &Arc<Mutex<Option<DeviceId>>>,
    host: &Arc<Host>,
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
    init_tx: tokio::sync::oneshot::Sender<Result<(), AudioError>>,
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
            let chunk_size = header.chunk_size as usize;
            spec.channels = header.channels as u32;
            spec.sample_rate = header.sample_rate;
            let mut decoder = SeaDecoder::new(header)?;

            let handle = spawn_blocking(move || {
                let mut decoded = Vec::new();
                let mut buffer = [0_i16; FRAME_SIZE];

                for chunk in local_bytes[14..].chunks(chunk_size) {
                    decoder.decode_frame(chunk, &mut buffer)?;
                    decoded.push(buffer);
                }

                Ok::<Vec<[i16; FRAME_SIZE]>, AudioError>(decoded)
            });

            samples = Some(handle.await??);
            info!("decoding sound took {:?}", now.elapsed());
        }

        // Validate that we have valid audio parameters before proceeding
        // This prevents divide-by-zero and buffer panics from malformed input
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(AudioError::InvalidWav);
        }

        // The resampling ratio used by the processor
        let ratio = output_config.sample_rate() as f64 / spec.sample_rate as f64;

        // Sends samples from the processor to the output stream
        #[cfg(not(target_family = "wasm"))]
        let (processed_sender, processed_receiver) = bounded::<Vec<f32>>(1_000);

        // Handles synchronization between the processor and output stream
        #[cfg(target_family = "wasm")]
        let processed_sender: Arc<AudioBuffer> = Default::default();
        #[cfg(target_family = "wasm")]
        let audio_buffer = processed_sender.clone();

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

        let output_stream = output_device.build_output_stream(
            &output_config.into(),
            move |output: &mut [f32], _| {
                #[cfg(target_family = "wasm")]
                let mut data = {
                    audio_buffer.condvar.notify_one();
                    audio_buffer.buffer.lock().unwrap()
                };

                for frame in output.chunks_mut(output_channels) {
                    #[cfg(not(target_family = "wasm"))]
                    let samples_result = processed_receiver.recv();
                    #[cfg(not(target_family = "wasm"))]
                    let canceled = samples_result.is_err();

                    #[cfg(target_family = "wasm")]
                    let canceled = {
                        let mut canceled = audio_buffer.canceled.load(Relaxed);

                        if !canceled && data.is_empty() {
                            audio_buffer.canceled.store(true, Relaxed);
                            canceled = true;
                        }

                        canceled
                    };

                    if canceled {
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
                        // This unwrap is safe as the result was already checked for is_err
                        #[cfg(not(target_family = "wasm"))]
                        let samples = samples_result.unwrap();
                        #[cfg(target_family = "wasm")]
                        let samples: Vec<f32> = data.drain(..output_channels).collect();

                        // Play the samples
                        frame.copy_from_slice(&samples);
                        last_samples = samples;
                    }
                }
            },
            move |err| {
                error!("Error in player stream: {}", err);
            },
            None,
        )?;

        // The sender used by the processor
        let sender = processed_sender.clone();

        let processor_future = spawn_blocking(move || {
            processor(
                (samples.is_none().then_some(bytes), samples),
                spec.sample_format,
                spec,
                output_volume,
                sender,
                output_channels,
                ratio,
            )
        });

        output_stream.play()?; // Play the stream

        Ok((
            output_stream,
            output_finished,
            processed_sender,
            processor_future,
        ))
    }
    .await;

    // Signal initialization result to the caller
    let (output_stream, output_finished, processed_sender, mut processor_future) = match init_result
    {
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

    // Keep the stream alive
    let _output_stream = output_stream;

    let mut processor_join: Option<_> = None;

    select! {
        _ = cancel.notified() => {
            debug!("reached cancel sound branch");
            // This causes the stream to begin fading out
            cfg_if::cfg_if! {
                if #[cfg(target_family = "wasm")] {
                    processed_sender.canceled.store(true, Relaxed);
                } else {
                    processed_sender.close()?;
                }
            }
        }
        result = &mut processor_future => {
            debug!("processor finished: {:?}", result);
            // Keep track of the return value
            processor_join = Some(result);
        }
    }

    // Wait for playback to complete before tearing down
    output_finished.notified().await;
    debug!("starting to tear down player stack");
    // Join the processor task
    match processor_join {
        Some(result) => {
            result.map_err(|e| AudioError::Processing(format!("Processor join error: {}", e)))??
        }
        None => processor_future
            .await
            .map_err(|e| AudioError::Processing(format!("Processor join error: {}", e)))??,
    };
    debug!("finished tearing down player stack");
    Ok(())
}

/// Processes audio data (WAV or decoded SEA).
fn processor(
    input: (Option<Vec<u8>>, Option<Vec<[i16; FRAME_SIZE]>>),
    sample_format: SampleFormat,
    spec: AudioHeader,
    output_volume: Arc<AtomicF32>,
    #[cfg(not(target_family = "wasm"))] processed_sender: Sender<Vec<f32>>,
    #[cfg(target_family = "wasm")] audio_buffer: Arc<AudioBuffer>,
    output_channels: usize,
    ratio: f64,
) -> Result<(), AudioError> {
    let (bytes, samples) = input;
    let sample_size = sample_format.sample_size();
    let channels_usize = spec.channels as usize;

    // The number of samples in the file
    let sample_count = bytes
        .as_ref()
        .map(|b| (b.len() - 44) / sample_size / channels_usize)
        .or_else(|| Some(samples.as_ref()?.len() * FRAME_SIZE / channels_usize))
        .unwrap_or_default();
    // The number of audio samples which will be played
    let audio_len = (sample_count as f64 * ratio) as f32;
    let mut position = 0_f32; // The playback position

    // Constants used for fading in and out
    let fade_basis = sample_count as f32 / 100_f32;
    let fade_out = fade_basis;
    let fade_in = audio_len - fade_basis;

    // Rubato requires 10 extra bytes in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 / spec.channels as f64 * ratio + 10.0) as usize;

    // The output for the resampler
    let mut post_buf = vec![vec![0_f32; post_len]; channels_usize];
    // The input for the resampler
    let mut pre_buf = vec![vec![0_f32; FRAME_SIZE / spec.channels as usize]; channels_usize];
    // Groups of samples ready to be sent to the output
    let mut out_buf = Vec::with_capacity(output_channels);

    let mut resampler =
        resampler_factory(ratio, channels_usize, FRAME_SIZE / spec.channels as usize)?;
    let output_volume = output_volume.load(Relaxed);

    let mut byte_chunks = bytes
        .as_ref()
        .map(|bytes| bytes[44..].chunks(FRAME_SIZE * sample_size));

    let mut sample_chunks = samples.as_ref().map(|samples| samples.iter());

    'outer: loop {
        match (byte_chunks.as_mut(), sample_chunks.as_mut()) {
            (None, Some(samples)) => {
                let samples = if let Some(samples) = samples.next() {
                    samples
                } else {
                    break 'outer;
                };

                let scale = 1_f32 / i16::MAX as f32;

                for (i, sample) in samples.chunks(channels_usize).enumerate() {
                    for (j, channel) in sample.iter().enumerate() {
                        let sample = *channel as f32 * scale;
                        pre_buf[j][i] = sample;
                    }
                }
            }
            (Some(chunks), None) => {
                let chunk = if let Some(chunk) = chunks.next() {
                    chunk
                } else {
                    break 'outer;
                };

                match sample_format {
                    SampleFormat::U8 => {
                        let scale = 1_f32 / u8::MAX as f32;

                        for (i, sample) in chunk.chunks(channels_usize).enumerate() {
                            for (j, channel) in sample.iter().enumerate() {
                                let sample = *channel as f32 * scale;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::I16 => {
                        let scale = 1_f32 / i16::MAX as f32;

                        for (i, sample) in chunk.chunks(2 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(2).enumerate() {
                                let sample = i16::from_le_bytes(channel.try_into()?) as f32 * scale;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::I32 => {
                        let scale = 1_f32 / i32::MAX as f32;

                        for (i, sample) in chunk.chunks(4 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(4).enumerate() {
                                let sample = i32::from_le_bytes(channel.try_into()?) as f32 * scale;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::F32 => {
                        for (i, sample) in chunk.chunks(4 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(4).enumerate() {
                                let sample = f32::from_le_bytes(channel.try_into()?);
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::F64 => {
                        for (i, sample) in chunk.chunks(8 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(8).enumerate() {
                                let sample = f64::from_le_bytes(channel.try_into()?) as f32;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    _ => {
                        return Err(AudioError::Processing(format!(
                            "Unknown sample format: {:?}",
                            sample_format
                        )));
                    }
                }
            }
            _ => break 'outer,
        }

        for channel in pre_buf.iter_mut() {
            wide_mul(channel, output_volume);
        }

        let (target_buffer, len) = if let Some(resampler) = &mut resampler {
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;
            (&mut post_buf, processed.1)
        } else {
            (&mut pre_buf, FRAME_SIZE / spec.channels as usize)
        };

        // I'm not sure how to refactor this loop to pass the needless range test
        #[allow(clippy::needless_range_loop)]
        for i in 0..len {
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
                let sample = if j >= channels_usize {
                    target_buffer[0][i]
                } else {
                    target_buffer[j][i]
                };

                out_buf.push(sample * multiplier);
            }

            // Send samples for each channel to the output
            #[cfg(not(target_family = "wasm"))]
            {
                let buffer = mem::take(&mut out_buf);
                if processed_sender.send(buffer).is_err() {
                    break 'outer;
                }
            }

            #[cfg(target_family = "wasm")]
            {
                if audio_buffer.canceled.load(Relaxed) {
                    break 'outer;
                }

                // Enforce bounding on the buffer
                if let Ok(data) = audio_buffer.buffer.lock() {
                    drop(
                        audio_buffer
                            .condvar
                            .wait_while(data, |d| d.len() > (10_000 * channels_usize)),
                    );
                }

                if let Ok(mut data) = audio_buffer.buffer.lock() {
                    let mut buffer = mem::take(&mut out_buf);
                    data.append(&mut buffer);
                } else {
                    error!("failed to lock audio buffer");
                    break 'outer;
                }
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
    let _ = processed_sender.close();
    Ok(())
}

/// Converts WAV bytes to SEA codec bytes.
///
/// This function encodes WAV audio data into the SEA codec format,
/// which provides efficient compression for audio transmission.
///
/// # Arguments
///
/// * `bytes` - The WAV file bytes (must include 44-byte header).
/// * `residual_bits` - Quality parameter for encoding (higher = better quality).
///
/// # Returns
///
/// The encoded SEA file bytes.
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

    let handle = spawn_blocking(move || {
        let settings = EncoderSettings {
            frames_per_chunk: FRAME_SIZE as u16 / channels as u16,
            vbr: true,
            residual_bits,
            ..Default::default()
        };

        let mut encoder = SeaEncoder::new(channels as u8, sample_rate, settings)?;

        let mut samples = [0; FRAME_SIZE];
        let mut buffer = BytesMut::new();
        let mut data: Vec<u8> = Vec::new();

        for chunk in bytes[44..].chunks(FRAME_SIZE * sample_size) {
            let written = match spec.sample_format {
                SampleFormat::U8 => {
                    for (j, sample) in chunk.iter().enumerate() {
                        samples[j] = ((*sample as i16) - 128) << 8;
                    }
                    chunk.len()
                }
                SampleFormat::I16 => {
                    for (i, sample_bytes) in chunk.chunks_exact(2).enumerate() {
                        samples[i] = i16::from_le_bytes([sample_bytes[0], sample_bytes[1]]);
                    }
                    chunk.len() / 2
                }
                _ => break,
            };

            if written < FRAME_SIZE {
                samples[written..].fill(0);
            }

            encoder.encode_frame(samples, &mut buffer)?;
            data.extend_from_slice(bytes.as_ref());
        }

        Ok::<Vec<u8>, AudioError>(data)
    });

    handle.await?
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
}
