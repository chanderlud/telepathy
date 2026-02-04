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

use crate::codec::decoder;
use crate::devices::AudioHost;
#[cfg(not(target_family = "wasm"))]
use crate::devices::{DeviceError, get_output_device};
use crate::error::AudioError;
use crate::processor::output_processor;
use crate::state::OutputProcessorState;
#[cfg(not(target_family = "wasm"))]
use crate::traits::CHANNEL_SIZE;
#[cfg(not(target_family = "wasm"))]
use crate::traits::ChannelOutput;
use atomic_float::AtomicF32;
#[cfg(not(target_family = "wasm"))]
use cpal::SampleFormat;
#[cfg(not(target_family = "wasm"))]
use cpal::traits::{DeviceTrait, StreamTrait};
#[cfg(not(target_family = "wasm"))]
use kanal::bounded;
use kanal::{Sender, unbounded_async};
use log::{debug, error};
use sea_codec::ProcessorMessage;
use sea_codec::codec::file::SeaFileHeader;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::thread::{self, JoinHandle};
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
    /// Whether codec decoding is enabled.
    ///
    /// When enabled, incoming data is decoded using the SEA codec before playback.
    pub codec_enabled: bool,
    /// Pre-defined SEA file header for room calls.
    ///
    /// When `Some(header)`, uses the provided header for decoding.
    /// When `None` and `codec_enabled` is `true`, a default header is
    /// auto-constructed with: version=1, channels=1, chunk_size=960,
    /// frames_per_chunk=480, and the configured sample_rate.
    pub codec_header: Option<SeaFileHeader>,
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
            sample_rate: 48000,
            volume: 1.0,
            codec_enabled: false,
            codec_header: None,
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
}

impl AudioOutputBuilder {
    /// Creates a new audio output builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: AudioOutputConfig::default(),
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

    /// Configures codec decoding.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable codec decoding
    /// * `header` - Optional SEA file header for rooms
    pub fn codec(mut self, enabled: bool, header: Option<SeaFileHeader>) -> Self {
        self.config.codec_enabled = enabled;
        self.config.codec_header = header;
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

        if config.sample_format() != SampleFormat::F32 {
            return Err(AudioError::Config("Unsupported sample format".to_string()));
        }

        let output_channels = config.channels() as usize;
        let output_sample_rate = config.sample_rate();

        // Create shared atomic state
        let output_volume = Arc::new(AtomicF32::new(self.config.volume));
        let deafened = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let loss_sender = Arc::new(AtomicUsize::new(0));

        // Create channels
        let (network_sender, network_receiver) = unbounded_async::<ProcessorMessage>();
        let (decoded_sender, decoded_receiver) = unbounded_async::<ProcessorMessage>();
        let (output_sender, output_receiver) = bounded::<f32>(CHANNEL_SIZE * 4);

        // Calculate resampling ratio
        let ratio = output_sample_rate as f64 / self.config.sample_rate as f64;

        let state =
            OutputProcessorState::new(&output_volume, rms_sender, &deafened, loss_sender.clone());

        let codec_enabled = self.config.codec_enabled;
        // Auto-construct room SEA header when codec is enabled and no header provided
        let codec_header = if codec_enabled && self.config.codec_header.is_none() {
            Some(SeaFileHeader {
                version: 1,
                channels: 1,
                chunk_size: 960,
                frames_per_chunk: 480,
                sample_rate: self.config.sample_rate,
            })
        } else {
            self.config.codec_header.clone()
        };

        // Spawn decoder thread if codec enabled, and select appropriate receiver for processor
        let (decoder_handle, processor_receiver_sync) = if codec_enabled {
            let network_receiver_sync = network_receiver.to_sync();
            let decoded_sender_sync = decoded_sender.to_sync();
            let handle = thread::spawn(move || {
                if let Err(e) = decoder(network_receiver_sync, decoded_sender_sync, codec_header) {
                    error!("Decoder error: {}", e);
                }
                debug!("Decoder thread ended");
            });
            (Some(handle), decoded_receiver.to_sync())
        } else {
            (None, network_receiver.to_sync())
        };

        // Spawn processor thread
        let processor_output = ChannelOutput::from(output_sender);
        let processor_handle = thread::spawn(move || {
            if let Err(e) =
                output_processor(processor_receiver_sync, processor_output, ratio, state)
            {
                error!("Output processor error: {}", e);
            }
            debug!("Output processor thread ended");
        });

        // Build the audio stream
        let error_notify = self.config.error_notify.clone();
        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &_| {
                for frame in data.chunks_mut(output_channels) {
                    let sample = output_receiver.recv().unwrap_or(0.0);
                    // Write the same sample to all channels (mono to stereo)
                    frame.fill(sample);
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

        stream.play()?;

        Ok(AudioOutputHandle {
            _stream: stream,
            processor_handle: Some(processor_handle),
            decoder_handle,
            network_sender: network_sender.to_sync(),
            output_volume,
            deafened,
            loss_sender,
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
        use crate::traits::WebOutput;
        use std::sync::Arc;
        use wasm_sync::Mutex;

        // Create shared buffer for web output
        let web_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let processor_output = WebOutput::new(web_buffer.clone());

        // Create shared atomic state
        let output_volume = Arc::new(AtomicF32::new(self.config.volume));
        let deafened = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let loss_sender = Arc::new(AtomicUsize::new(0));

        // Create channels
        let (network_sender, network_receiver) = unbounded_async::<ProcessorMessage>();
        let (decoded_sender, decoded_receiver) = unbounded_async::<ProcessorMessage>();

        // Calculate resampling ratio (assume 48kHz output for web)
        let output_sample_rate = 48000_u32;
        let ratio = output_sample_rate as f64 / self.config.sample_rate as f64;

        let state =
            OutputProcessorState::new(&output_volume, rms_sender, &deafened, loss_sender.clone());

        let codec_enabled = self.config.codec_enabled;
        // Auto-construct room SEA header when codec is enabled and no header provided
        let codec_header = if codec_enabled && self.config.codec_header.is_none() {
            Some(SeaFileHeader {
                version: 1,
                channels: 1,
                chunk_size: 960,
                frames_per_chunk: 480,
                sample_rate: self.config.sample_rate,
            })
        } else {
            self.config.codec_header.clone()
        };

        // Spawn decoder thread if codec enabled, and select appropriate receiver for processor
        let (decoder_handle, processor_receiver_sync) = if codec_enabled {
            let network_receiver_sync = network_receiver.to_sync();
            let decoded_sender_sync = decoded_sender.to_sync();
            let handle = thread::spawn(move || {
                if let Err(e) = decoder(network_receiver_sync, decoded_sender_sync, codec_header) {
                    error!("Decoder error: {}", e);
                }
                debug!("Decoder thread ended");
            });
            (Some(handle), decoded_receiver.to_sync())
        } else {
            (None, network_receiver.to_sync())
        };

        // Spawn processor thread
        let processor_handle = thread::spawn(move || {
            if let Err(e) =
                output_processor(processor_receiver_sync, processor_output, ratio, state)
            {
                error!("Output processor error: {}", e);
            }
            debug!("Output processor thread ended");
        });

        Ok(AudioOutputHandle {
            _web_buffer: Some(web_buffer),
            processor_handle: Some(processor_handle),
            decoder_handle,
            network_sender: network_sender.to_sync(),
            output_volume,
            deafened,
            loss_sender,
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
    decoder_handle: Option<JoinHandle<()>>,
    network_sender: Sender<ProcessorMessage>,
    output_volume: Arc<AtomicF32>,
    deafened: Arc<AtomicBool>,
    loss_sender: Arc<AtomicUsize>,
}

impl AudioOutputHandle {
    /// Returns a sender for feeding audio data to the output.
    ///
    /// Use this sender to send `ProcessorMessage` frames to be played.
    pub fn sender(&self) -> Sender<ProcessorMessage> {
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
        // Drop the sender to signal threads to stop
        drop(std::mem::replace(
            &mut self.network_sender,
            kanal::unbounded().0,
        ));

        // Wait for threads to finish
        if let Some(handle) = self.decoder_handle.take() {
            let _ = handle.join();
        }
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

        // Wait for threads to finish
        if let Some(handle) = self.decoder_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
    }
}
