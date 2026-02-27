//! Audio output API.
//!
//! This module provides a high-level API for playing back processed audio.
//! It handles device selection, resampling, codec decoding, and volume control.
//!
//! # Example
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
//! use bytes::Bytes;
//! use std::sync::mpsc;
//!
//! let host = AudioHost::new();
//! let (tx, rx) = mpsc::channel::<Bytes>();
//!
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .volume(1.0)
//!     .source(MpscSource::new(rx))
//!     .build(&host)
//!     .unwrap();
//!
//! // Feed audio frames via your chosen channel (here: std::sync::mpsc::Sender)
//! // tx.send(Bytes::from_static(&[0u8; 960])).unwrap();
//! let _ = tx;
//! ```

use crate::constants::TRANSITION_LENGTH;
use crate::devices::{AudioHost, get_output_device};
use crate::error::Error;
use crate::internal::NETWORK_FRAME;
use crate::internal::processor::output_processor;
use crate::internal::state::OutputProcessorState;
use crate::internal::thread::{self, JoinHandle};
use crate::internal::traits::AudioOutput;
use crate::internal::traits::CHANNEL_SIZE;
use crate::internal::traits::RingBufferOutput;
use crate::internal::utils::{hann_fade_in, hann_fade_out};
use crate::io::SendStream;
use crate::io::traits::AudioDataSource;
use crate::sea::codec::file::SeaFileHeader;
use crate::sea::decoder::SeaDecoder;
use atomic_float::AtomicF32;
use cpal::Sample;
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, StreamTrait};
use log::{debug, error};
use nnnoiseless::FRAME_SIZE;
use rtrb::chunks::ChunkError;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use tokio::sync::Notify;

/// Configuration for audio output processing.
///
/// This struct holds all configuration options for an audio output stream.
/// Use [`AudioOutputBuilder`] for a more ergonomic way to construct these options.
pub struct AudioOutputConfig {
    /// Device ID for output device selection.
    ///
    /// When `None`, uses the system's default output device.
    /// When `Some(id)`, attempts to find the device with that ID,
    /// falling back to default if not found.
    pub device_id: Option<String>,
    /// Remote (source) sample rate in Hz.
    ///
    /// This is the sample rate of incoming audio data. The output processor
    /// automatically resamples to the output device's native rate if needed.
    pub sample_rate: u32,
    /// Output volume multiplier (1.0 = unity gain).
    ///
    /// Values less than 1.0 reduce volume, greater than 1.0 amplify.
    pub volume: f32,
    /// Set to true when codec is enabled
    pub codec_enabled: bool,
    /// Optional notify handle for stream errors.
    ///
    /// When set, the notify is triggered via `notify_one()` whenever a
    /// stream error occurs, in addition to logging the error. Useful for
    /// async error handling and reconnection logic.
    pub error_notify: Option<Arc<Notify>>,
}

impl Default for AudioOutputConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            sample_rate: 48_000,
            volume: 1.0,
            codec_enabled: false,
            error_notify: None,
        }
    }
}

/// Builder for configuring and creating audio output streams.
///
/// Use this builder to configure audio output processing options before
/// starting the stream.
pub struct AudioOutputBuilder<R>
where
    R: AudioDataSource,
{
    config: AudioOutputConfig,
    source: Option<R>,
    /// Optional shared atomic for output volume (enables real-time synchronization)
    shared_output_volume: Option<Arc<AtomicF32>>,
    /// Optional shared atomic for deafened state (enables real-time synchronization)
    shared_deafened: Option<Arc<AtomicBool>>,
    shared_rms: Option<Arc<AtomicF32>>,
    shared_loss: Option<Arc<AtomicUsize>>,
}

/// Internal context for building audio output, shared between native and WASM
struct OutputBuildContext {
    output_volume: Arc<AtomicF32>,
    deafened: Arc<AtomicBool>,
    loss_sender: Arc<AtomicUsize>,
    processor_handle: JoinHandle<()>,
}

impl AudioOutputBuilder<Box<dyn AudioDataSource>> {
    /// Creates a new audio output builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: AudioOutputConfig::default(),
            source: None,
            shared_output_volume: None,
            shared_deafened: None,
            shared_rms: None,
            shared_loss: None,
        }
    }
}

impl<R> AudioOutputBuilder<R>
where
    R: AudioDataSource,
{
    /// Sets a custom source for receiving audio data.
    pub fn source<T: AudioDataSource>(self, source: T) -> AudioOutputBuilder<T> {
        AudioOutputBuilder {
            config: self.config,
            source: Some(source),
            shared_output_volume: self.shared_output_volume,
            shared_deafened: self.shared_deafened,
            shared_rms: self.shared_rms,
            shared_loss: self.shared_loss,
        }
    }

    /// Sets the output device by ID.
    ///
    /// If not set or set to None, the default output device will be used.
    pub fn device(mut self, device_id: Option<String>) -> Self {
        self.config.device_id = device_id;
        self
    }

    /// Sets the remote (source) sample rate in Hz.
    ///
    /// This is the sample rate of the incoming audio data. The audio will
    /// be resampled to the output device's native sample rate if needed.
    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.config.sample_rate = rate;
        self
    }

    /// Sets the output volume multiplier.
    ///
    /// * 1.0 = unity gain (default)
    /// * 0.5 = half volume
    /// * 2.0 = double volume
    pub fn volume(mut self, volume: f32) -> Self {
        self.config.volume = volume;
        self
    }

    /// Sets a shared atomic for output volume, enabling real-time synchronization.
    ///
    /// When provided, the builder will use this shared atomic instead of creating
    /// a new one. This allows external code to modify the volume in real-time
    /// and have the changes immediately affect audio playback.
    ///
    /// Use this when you need to share volume control with other components,
    /// such as a core state manager. For simple cases where you only need to
    /// set the initial volume, use [`volume`](Self::volume) instead.
    pub fn output_volume_shared(mut self, volume: &Arc<AtomicF32>) -> Self {
        self.shared_output_volume = Some(volume.clone());
        self
    }

    /// Sets a shared atomic for deafened state, enabling real-time synchronization.
    ///
    /// When provided, the builder will use this shared atomic instead of creating
    /// a new one. This allows external code to modify the deafened state in real-time
    /// and have the changes immediately affect audio playback.
    ///
    /// Use this when you need to share deafen control with other components,
    /// such as a core state manager. The deafened state can still be controlled
    /// via the handle's `deafen()` and `undeafen()` methods after building.
    pub fn deafened_shared(mut self, deafened: &Arc<AtomicBool>) -> Self {
        self.shared_deafened = Some(deafened.clone());
        self
    }

    pub fn rms_shared(mut self, rms: &Arc<AtomicF32>) -> Self {
        self.shared_rms = Some(rms.clone());
        self
    }

    pub fn loss_shared(mut self, loss: &Arc<AtomicUsize>) -> Self {
        self.shared_loss = Some(loss.clone());
        self
    }

    /// Enables codec decoding
    pub fn codec(mut self, enabled: bool) -> Self {
        self.config.codec_enabled = enabled;
        self
    }

    /// Sets a notify handle to be triggered on stream errors.
    ///
    /// When set, the notify will be triggered via `notify_one()` whenever
    /// a stream error occurs, in addition to logging the error.
    pub fn on_error(mut self, notify: Arc<Notify>) -> Self {
        self.config.error_notify = Some(notify);
        self
    }

    /// Common initialization logic shared between native and WASM builds.
    ///
    /// This method handles all shared setup steps:
    /// - Creates shared atomic state (output_volume, deafened, rms_sender, loss_sender)
    /// - Creates unbounded channel for network → processor communication
    /// - Calculates resampling ratio: `output_sample_rate / config.sample_rate`
    /// - Creates OutputProcessorState for atomic state management
    /// - Auto-constructs SEA codec header when codec is enabled and no header provided:
    ///   - version: 1
    ///   - channels: 1 (mono)
    ///   - chunk_size: 960
    ///   - frames_per_chunk: 480
    ///   - sample_rate: from config
    /// - Creates decoder if codec is enabled and passes it to the processor thread
    /// - Spawns processor thread (calls `output_processor` function with optional decoder)
    ///
    /// # Type Parameters
    ///
    /// * `O` - Type implementing `AudioOutput` trait (e.g., `RingBufferOutput`, `WebOutput`)
    ///
    /// # Arguments
    ///
    /// * `processor_output` - The audio output destination for the processor
    /// * `output_sample_rate` - Device's native sample rate in Hz
    ///
    /// # Returns
    ///
    /// Returns `Result<OutputBuildContext, AudioError>` containing handles and state for the created threads,
    /// or an error if codec initialization fails.
    fn build_common<O: AudioOutput + Send + 'static>(
        self,
        processor_output: O,
        output_sample_rate: u32,
    ) -> Result<OutputBuildContext, Error> {
        // Create shared atomic state (use provided shared atomics or create new ones)
        let output_volume = self
            .shared_output_volume
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicF32::new(self.config.volume)));
        let deafened = self.shared_deafened.clone().unwrap_or_default();
        let rms_sender = self.shared_rms.clone().unwrap_or_default();
        let loss_sender = self.shared_loss.clone().unwrap_or_default();
        let source = self
            .source
            .ok_or_else(|| Error::Config("a data source must be set via source()".to_string()))?;

        // Calculate resampling ratio
        let ratio = output_sample_rate as f64 / self.config.sample_rate as f64;

        let state =
            OutputProcessorState::new(&output_volume, rms_sender, &deafened, loss_sender.clone());

        let decoder = if self.config.codec_enabled {
            Some(SeaDecoder::new(SeaFileHeader {
                version: 1,
                channels: 1,
                chunk_size: NETWORK_FRAME as u16,
                frames_per_chunk: FRAME_SIZE as u16,
                sample_rate: self.config.sample_rate,
            })?)
        } else {
            None
        };

        // Spawn processor thread (safe_spawn catches panics on WASM when threading is unavailable)
        let processor_handle = thread::safe_spawn(move || {
            if let Err(e) = output_processor(source, processor_output, ratio, state, decoder) {
                error!("Output processor error: {}", e);
            }
            debug!("Output processor thread ended");
        })?;

        Ok(OutputBuildContext {
            output_volume,
            deafened,
            loss_sender,
            processor_handle,
        })
    }

    /// Builds and starts the audio output stream.
    ///
    /// This method creates and configures all necessary processing threads
    /// and returns an `AudioOutputHandle` for controlling the stream.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The device cannot be found
    /// - The stream cannot be created
    /// - The device uses an unsupported sample format
    pub fn build(self, host: &AudioHost) -> Result<AudioOutputHandle, Error> {
        use rtrb::RingBuffer;
        if self.source.is_none() {
            return Err(Error::Config(
                "a data source must be set via source()".to_string(),
            ));
        }

        // Get the output device
        let device_handle = get_output_device(host, self.config.device_id.as_deref())?;

        let device = device_handle.device();
        let config = device.default_output_config()?;

        let output_channels = config.channels() as usize;
        let output_sample_rate = config.sample_rate();
        let sample_format = config.sample_format();

        // Create ring buffer for lock-free producer/consumer communication
        let (output_producer, output_consumer) = RingBuffer::<f32>::new(CHANNEL_SIZE * 4);

        // Create processor output and build common components
        let processor_output = RingBufferOutput::new(output_producer);
        let error_notify = self.config.error_notify.clone();
        let context = self.build_common(processor_output, output_sample_rate)?;

        // Build the audio stream with the appropriate sample format
        let stream = match sample_format {
            SampleFormat::I8 => build_output_stream_with_format::<i8>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::I16 => build_output_stream_with_format::<i16>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::I32 => build_output_stream_with_format::<i32>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::I64 => build_output_stream_with_format::<i64>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::U8 => build_output_stream_with_format::<u8>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::U16 => build_output_stream_with_format::<u16>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::U32 => build_output_stream_with_format::<u32>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::U64 => build_output_stream_with_format::<u64>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::F32 => build_output_stream_with_format::<f32>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            SampleFormat::F64 => build_output_stream_with_format::<f64>(
                device,
                &config.into(),
                output_consumer,
                output_channels,
                error_notify,
            )?,
            _ => return Err(Error::Config("Unsupported sample format".to_string())),
        };
        // Start playback
        stream.0.play()?;

        Ok(AudioOutputHandle {
            _stream: stream,
            _processor_handle: Some(context.processor_handle),
            output_volume: context.output_volume,
            deafened: context.deafened,
            loss_sender: context.loss_sender,
        })
    }
}

impl Default for AudioOutputBuilder<Box<dyn AudioDataSource>> {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to a running audio output stream.
///
/// This handle allows controlling the audio output (deafen/undeafen, volume)
/// while audio data is received from the user-provided source. Resources are automatically
/// cleaned up when dropped.
///
/// ## Thread Safety
///
/// All control methods (`deafen`, `undeafen`, `set_volume`, etc.) are thread-safe
/// and can be called from any thread. They use atomic operations internally.
pub struct AudioOutputHandle {
    _stream: SendStream,
    _processor_handle: Option<JoinHandle<()>>,
    output_volume: Arc<AtomicF32>,
    deafened: Arc<AtomicBool>,
    loss_sender: Arc<AtomicUsize>,
}

impl AudioOutputHandle {
    /// Deafens the audio output.
    ///
    /// When deafened, incoming audio data will be discarded and no sound
    /// will be played.
    pub fn deafen(&self) {
        self.deafened.store(true, Relaxed);
    }

    /// Undeafens the audio output.
    pub fn undeafen(&self) {
        self.deafened.store(false, Relaxed);
    }

    /// Returns whether the output is currently deafened.
    pub fn is_deafened(&self) -> bool {
        self.deafened.load(Relaxed)
    }

    /// Sets the output volume multiplier.
    pub fn set_volume(&self, volume: f32) {
        self.output_volume.store(volume, Relaxed);
    }

    /// Gets the current output volume multiplier.
    pub fn volume(&self) -> f32 {
        self.output_volume.load(Relaxed)
    }

    /// Gets the loss receiver
    pub fn loss_receiver(&self) -> Arc<AtomicUsize> {
        self.loss_sender.clone()
    }
}

/// Builds an output stream with automatic sample format conversion.
///
/// This helper function creates a cpal output stream that converts f32 samples
/// from the processing pipeline to the device's native sample format T.
///
/// Uses `rtrb::Consumer` with the chunks API for efficient bulk reads from the
/// lock-free ring buffer, matching the producer-side `RingBufferOutput` pattern.
///
/// # Type Parameters
///
/// * `T` - The device's native sample format (e.g., i16, f32, u16)
///
/// # Arguments
///
/// * `device` - The cpal device to build the stream on
/// * `config` - Stream configuration (sample rate, channels, etc.)
/// * `output_consumer` - Ring buffer consumer for f32 samples from the processor
/// * `output_channels` - Number of output channels
/// * `error_notify` - Optional notify handle for stream errors
fn build_output_stream_with_format<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut output_consumer: rtrb::Consumer<f32>,
    output_channels: usize,
    error_notify: Option<Arc<Notify>>,
) -> Result<SendStream, Error>
where
    T: Sample + cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    use cpal::traits::DeviceTrait;

    let mut last_sample = 0_f32;
    let mut was_underrun = true;
    let mut was_missing = false;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &_| {
            debug_assert!(output_channels > 0);

            let total_frames = data.len() / output_channels;
            // How many real frames we can pull right now
            let available_frames = total_frames.min(output_consumer.slots());

            if was_missing && available_frames == 0 {
                // shortcut for when the fade already occurred & we are playing silence
                data.fill(T::from_sample(0_f32));
                return;
            }

            let mut frames = data.chunks_mut(output_channels);

            // Read as many as possible (maybe 0)
            let mut pulled = 0;
            if available_frames > 0 {
                match output_consumer.read_chunk(available_frames) {
                    Ok(chunk) => {
                        let mut samples = chunk.into_iter();

                        // Fade-in on recovery: apply ramp to the incoming samples
                        let ramp_in_len = if was_underrun {
                            TRANSITION_LENGTH.min(available_frames)
                        } else {
                            0
                        };

                        for i in 0..ramp_in_len {
                            let Some(frame) = frames.next() else {
                                break;
                            };
                            let Some(sample_f32) = samples.next() else {
                                break;
                            };

                            let g = hann_fade_in(i, ramp_in_len);
                            let out = sample_f32 * g;

                            last_sample = sample_f32;
                            frame.fill(T::from_sample(out));
                            pulled += 1;
                        }

                        if ramp_in_len > 0 {
                            // Reset state after playing real samples
                            was_underrun = false;
                            was_missing = false;
                        }

                        // Remaining real samples (no gain)
                        while let (Some(frame), Some(sample_f32)) = (frames.next(), samples.next())
                        {
                            last_sample = sample_f32;
                            frame.fill(T::from_sample(sample_f32));
                            pulled += 1;
                        }
                    }
                    Err(ChunkError::TooFewSlots(_)) => {
                        // Shouldn't happen since we min() with slots(), but if it does:
                        pulled = 0;
                    }
                }
            }

            // If we couldn't fill the buffer, fade out then silence
            let missing = total_frames.saturating_sub(pulled);
            if missing > 0 {
                was_underrun = true;
                was_missing = true;

                let fade_len = TRANSITION_LENGTH.min(missing);

                // Smooth fade from last real sample -> 0
                for i in 0..fade_len {
                    let Some(frame) = frames.next() else {
                        break;
                    };
                    let g = hann_fade_out(i, fade_len);
                    frame.fill(T::from_sample(last_sample * g));
                }

                // Remaining frames: hard silence
                for frame in frames {
                    frame.fill(T::from_sample(0.0));
                }

                last_sample = 0.0;
            }
        },
        move |err| {
            error!("Output stream error: {}", err);
            if let Some(ref notify) = error_notify {
                notify.notify_one();
            }
        },
        None,
    )?;

    Ok(SendStream(stream))
}
