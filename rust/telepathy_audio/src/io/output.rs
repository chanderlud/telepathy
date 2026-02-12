//! Audio output API.
//!
//! This module provides a high-level API for playing back processed audio.
//! It handles device selection, resampling, codec decoding, and volume control.
//!
//! # Example
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//! use bytes::Bytes;
//!
//! let host = AudioHost::new();
//! let output = AudioOutputBuilder::new()
//!     .sample_rate(48000)
//!     .volume(1.0)
//!     .build(&host)
//!     .unwrap();
//!
//! // Get the sender to feed audio data
//! let sender = output.sender();
//!
//! // Send audio data
//! // sender.send(audio_data).await;
//! ```

use crate::devices::AudioHost;
#[cfg(not(target_family = "wasm"))]
use crate::devices::{DeviceError, get_output_device};
use crate::error::AudioError;
use crate::internal::NETWORK_FRAME;
use crate::internal::processor::output_processor;
use crate::internal::state::OutputProcessorState;
use crate::internal::thread::{self, JoinHandle};
use crate::internal::traits::AudioOutput;
#[cfg(not(target_family = "wasm"))]
use crate::internal::traits::CHANNEL_SIZE;
#[cfg(not(target_family = "wasm"))]
use crate::internal::traits::RingBufferOutput;
use crate::sea::codec::file::SeaFileHeader;
use crate::sea::decoder::SeaDecoder;
use atomic_float::AtomicF32;
use bytes::Bytes;
#[cfg(not(target_family = "wasm"))]
use cpal::Sample;
#[cfg(not(target_family = "wasm"))]
use cpal::SampleFormat;
#[cfg(not(target_family = "wasm"))]
use cpal::traits::{DeviceTrait, StreamTrait};
use kanal::{Sender, unbounded};
use log::{debug, error};
use nnnoiseless::FRAME_SIZE;
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
pub struct AudioOutputBuilder {
    config: AudioOutputConfig,
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
    network_sender: Sender<Bytes>,
    processor_handle: JoinHandle<()>,
}

impl AudioOutputBuilder {
    /// Creates a new audio output builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: AudioOutputConfig::default(),
            shared_output_volume: None,
            shared_deafened: None,
            shared_rms: None,
            shared_loss: None,
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
    ) -> Result<OutputBuildContext, AudioError> {
        // Create shared atomic state (use provided shared atomics or create new ones)
        let output_volume = self
            .shared_output_volume
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicF32::new(self.config.volume)));
        let deafened = self.shared_deafened.clone().unwrap_or_default();
        let rms_sender = self.shared_rms.clone().unwrap_or_default();
        let loss_sender = self.shared_loss.clone().unwrap_or_default();

        let (network_sender, network_receiver) = unbounded();

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
            if let Err(e) =
                output_processor(network_receiver, processor_output, ratio, state, decoder)
            {
                error!("Output processor error: {}", e);
            }
            debug!("Output processor thread ended");
        })?;

        Ok(OutputBuildContext {
            output_volume,
            deafened,
            loss_sender,
            network_sender,
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
    #[cfg(not(target_family = "wasm"))]
    pub fn build(self, host: &AudioHost) -> Result<AudioOutputHandle, AudioError> {
        use rtrb::RingBuffer;

        // Get the output device
        let device_handle =
            get_output_device(host, self.config.device_id.as_deref()).map_err(|e| match e {
                DeviceError::NoDefaultDevice => AudioError::Device("No output device".to_string()),
                DeviceError::DeviceNotFound(id) => {
                    AudioError::Device(format!("Device not found: {}", id))
                }
                DeviceError::EnumerationFailed(msg) => AudioError::Device(msg),
                DeviceError::InvalidDeviceId(id) => {
                    AudioError::Device(format!("Invalid device ID: {}", id))
                }
            })?;

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
            _ => return Err(AudioError::Config("Unsupported sample format".to_string())),
        };
        stream.play()?;

        Ok(AudioOutputHandle {
            _stream: stream,
            processor_handle: Some(context.processor_handle),
            network_sender: context.network_sender,
            output_volume: context.output_volume,
            deafened: context.deafened,
            loss_sender: context.loss_sender,
        })
    }

    /// Builds and starts the audio output stream (WASM version).
    ///
    /// On WASM, this creates a WebOutput-based audio output that writes to a
    /// shared buffer. Unlike audio input, output uses synchronous `build` on WASM.
    ///
    /// # WASM-Specific Behavior
    ///
    /// - Creates a shared `Arc<Mutex<Vec<f32>>>` buffer for audio samples
    /// - Processed audio is written to this buffer by the processor thread
    /// - The caller is responsible for consuming this buffer from a Web Audio
    ///   API AudioWorklet or ScriptProcessorNode
    /// - Assumes 48kHz output sample rate for the web audio context
    /// - The `AudioOutputHandle` holds an `Arc` to the buffer (`_web_buffer`)
    ///
    /// # Note
    ///
    /// The web buffer is bounded by `CHANNEL_SIZE` (2,400 samples). When full,
    /// additional samples are dropped to prevent unbounded memory growth.
    #[cfg(target_family = "wasm")]
    pub fn build(self, _host: &AudioHost) -> Result<AudioOutputHandle, AudioError> {
        use crate::internal::traits::WebOutput;
        use std::sync::Arc;
        use wasm_sync::Mutex;

        // Create shared buffer for web output
        let web_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let processor_output = WebOutput::new(web_buffer.clone());

        // Fixed 48kHz output for web audio context
        let output_sample_rate = 48000_u32;

        // Build common components (channels, threads, state)
        let context = self.build_common(processor_output, output_sample_rate)?;

        Ok(AudioOutputHandle {
            _web_buffer: Some(web_buffer),
            processor_handle: Some(context.processor_handle),
            network_sender: context.network_sender,
            output_volume: context.output_volume,
            deafened: context.deafened,
            loss_sender: context.loss_sender,
        })
    }
}

impl Default for AudioOutputBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to a running audio output stream.
///
/// This handle allows controlling the audio output (deafen/undeafen, volume)
/// and provides a sender for feeding audio data. Resources are automatically
/// cleaned up when dropped.
///
/// ## Lifecycle
///
/// - **Creation**: Created by [`AudioOutputBuilder::build`]
/// - **Running**: Audio is received via sender, processed, and played
/// - **Cleanup**: When dropped, the handle:
///   1. Closes the network sender to signal threads to stop
///   2. Waits for decoder (if enabled) and processor threads to join
///   3. Stops the underlying audio stream
///
/// ## Thread Safety
///
/// All control methods (`deafen`, `undeafen`, `set_volume`, etc.) are thread-safe
/// and can be called from any thread. They use atomic operations internally.
/// The sender returned by `sender()` can be cloned and used from multiple threads.
///
/// ## Platform Differences
///
/// - **Native**: Uses cpal stream for audio output
/// - **WASM**: Uses a shared buffer (`Arc<Mutex<Vec<f32>>>`) that should be
///   consumed by a Web Audio API AudioWorklet or ScriptProcessorNode
///
/// ## Drop vs stop()
///
/// Calling `drop` (implicit when handle goes out of scope) and `stop()` have
/// the same effect. Use `stop()` when you need to explicitly wait for cleanup
/// completion in a specific code location.
pub struct AudioOutputHandle {
    #[cfg(not(target_family = "wasm"))]
    _stream: cpal::Stream,
    #[cfg(target_family = "wasm")]
    _web_buffer: Option<std::sync::Arc<wasm_sync::Mutex<Vec<f32>>>>,
    processor_handle: Option<JoinHandle<()>>,
    network_sender: Sender<Bytes>,
    output_volume: Arc<AtomicF32>,
    deafened: Arc<AtomicBool>,
    loss_sender: Arc<AtomicUsize>,
}

impl AudioOutputHandle {
    /// Returns a sender for feeding audio data to the output.
    ///
    /// Use this sender to send `ProcessorMessage` frames to be played.
    pub fn sender(&self) -> Sender<Bytes> {
        self.network_sender.clone()
    }

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

    /// Stops the audio output and waits for all threads to finish.
    ///
    /// This is called automatically when the handle is dropped, but can
    /// be called explicitly if you need to wait for completion.
    pub fn stop(mut self) {
        // Close the sender to signal threads to stop (mirrors Drop implementation)
        _ = self.network_sender.close();

        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
    }

    /// Gets the loss receiver
    pub fn loss_receiver(&self) -> Arc<AtomicUsize> {
        self.loss_sender.clone()
    }
}

impl Drop for AudioOutputHandle {
    fn drop(&mut self) {
        // Close the sender to signal threads to stop - must happen BEFORE joining threads
        // to ensure the receiver loops can exit and threads can join cleanly
        _ = self.network_sender.close();

        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
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
#[cfg(not(target_family = "wasm"))]
fn build_output_stream_with_format<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut output_consumer: rtrb::Consumer<f32>,
    output_channels: usize,
    error_notify: Option<Arc<Notify>>,
) -> Result<cpal::Stream, AudioError>
where
    T: Sample + cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    use cpal::traits::DeviceTrait;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &_| {
            // Number of mono samples needed (one per frame/channel-group)
            let num_frames = data.len() / output_channels;
            match output_consumer.read_chunk(num_frames) {
                Ok(chunk) => {
                    for (sample_f32, frame) in
                        chunk.into_iter().zip(data.chunks_mut(output_channels))
                    {
                        let sample_t = T::from_sample(sample_f32);
                        frame.fill(sample_t);
                    }
                }
                Err(_) => {
                    // Not enough samples available; fill with silence
                    let silence = T::from_sample(0.0f32);
                    data.fill(silence);
                }
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

    Ok(stream)
}
