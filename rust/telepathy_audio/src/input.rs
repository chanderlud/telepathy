//! Audio input API.
//!
//! This module provides a high-level API for capturing and processing audio input.
//! It handles device selection, resampling, noise suppression, codec encoding,
//! and delivers processed audio through a callback mechanism.
//!
//! # Example
//!
//! ```rust,no_run
//! use telepathy_audio::{AudioHost, AudioInputBuilder};
//! use std::sync::Arc;
//!
//! let host = AudioHost::new();
//! let input = AudioInputBuilder::new()
//!     .volume(1.0)
//!     .callback(|data| {
//!         // Handle processed audio data
//!         println!("Received {} bytes", data.len());
//!     })
//!     .build(&host)
//!     .unwrap();
//! ```

use crate::codec::encoder;
use crate::devices::AudioHost;
#[cfg(not(target_family = "wasm"))]
use crate::devices::{DeviceError, get_input_device};
use crate::error::AudioError;
use crate::processor::input_processor;
use crate::state::InputProcessorState;
#[cfg(not(target_family = "wasm"))]
use crate::traits::{CHANNEL_SIZE, ChannelInput};
use atomic_float::AtomicF32;
use bytes::Bytes;
#[cfg(not(target_family = "wasm"))]
use cpal::SampleFormat;
#[cfg(not(target_family = "wasm"))]
use cpal::traits::{DeviceTrait, StreamTrait};
#[cfg(not(target_family = "wasm"))]
use kanal::bounded;
use kanal::unbounded_async;
use log::{debug, error};
use nnnoiseless::{DenoiseState, RnnModel};
use sea_codec::ProcessorMessage;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::{self, JoinHandle};
use tokio::sync::Notify;

/// Configuration for audio input processing.
///
/// This struct holds all configuration options for an audio input stream.
/// Use [`AudioInputBuilder`] for a more ergonomic way to construct these options.
///
/// ## Sample Rate Behavior
///
/// When `denoise_enabled` is `true`, the processor upsamples to 48kHz for
/// RNNoise processing and outputs 48kHz frames (no downsample back to device
/// rate). When `denoise_enabled` is `false`, the processor passes through at
/// the device's native sample rate. The encoder sample rate automatically
/// matches the processor's output rate.
#[derive(Clone)]
pub struct AudioInputConfig {
    /// Device ID for input device selection.
    ///
    /// When `None`, uses the system's default input device.
    /// When `Some(id)`, attempts to find the device with that ID,
    /// falling back to default if not found.
    pub device_id: Option<String>,
    /// Whether noise suppression is enabled.
    ///
    /// When enabled, audio is upsampled to 48kHz for RNNoise processing.
    /// This provides significant noise reduction but increases CPU usage.
    pub denoise_enabled: bool,
    /// Custom noise suppression model bytes.
    ///
    /// When `None`, uses the default RNNoise model.
    /// When `Some(bytes)`, loads a custom RNN model from the provided bytes.
    pub denoise_model: Option<RnnModel>,
    /// Input volume multiplier (1.0 = unity gain).
    ///
    /// Values less than 1.0 reduce volume, greater than 1.0 amplify.
    pub volume: f32,
    /// RMS threshold for silence detection.
    ///
    /// Audio frames with RMS below this threshold are treated as silence.
    /// A value of 0.0 disables silence detection.
    pub rms_threshold: f32,
    /// Whether codec encoding is enabled.
    ///
    /// When enabled, audio is encoded using the SEA codec before being
    /// passed to the callback.
    pub codec_enabled: bool,
    /// Variable bit rate encoding.
    ///
    /// When `true`, the encoder uses variable bit rate for potentially
    /// smaller output. When `false`, uses constant bit rate.
    pub codec_vbr: bool,
    /// Residual bits for codec quality (typically 1.0-8.0).
    ///
    /// Higher values provide better quality but larger encoded size.
    pub codec_residual_bits: f32,
    /// Whether this is a room call.
    ///
    /// When `true`, the encoder skips the Start state, allowing immediate
    /// audio transmission without handshake. Used for multi-party calls.
    pub is_room: bool,
    /// Optional notify handle for stream errors.
    ///
    /// When set, the notify is triggered via `notify_one()` whenever a
    /// stream error occurs, in addition to logging the error. Useful for
    /// async error handling and reconnection logic.
    pub error_notify: Option<Arc<Notify>>,
}

impl Default for AudioInputConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            denoise_enabled: false,
            denoise_model: None,
            volume: 1.0,
            rms_threshold: 0.0,
            codec_enabled: false,
            codec_vbr: false,
            codec_residual_bits: 5.0,
            is_room: false,
            error_notify: None,
        }
    }
}

/// Builder for configuring and creating audio input streams.
///
/// Use this builder to configure audio input processing options before
/// starting the stream.
pub struct AudioInputBuilder<F>
where
    F: Fn(Bytes) + Send + 'static,
{
    config: AudioInputConfig,
    callback: Option<F>,
    /// Optional shared atomic for input volume (enables real-time synchronization)
    shared_input_volume: Option<Arc<AtomicF32>>,
    /// Optional shared atomic for RMS threshold (enables real-time synchronization)
    shared_rms_threshold: Option<Arc<AtomicF32>>,
    /// Optional shared atomic for muted state (enables real-time synchronization)
    shared_muted: Option<Arc<AtomicBool>>,
}

impl AudioInputBuilder<fn(Bytes)> {
    /// Creates a new audio input builder with default configuration.
    pub fn new() -> AudioInputBuilder<fn(Bytes)> {
        AudioInputBuilder {
            config: AudioInputConfig::default(),
            callback: None,
            shared_input_volume: None,
            shared_rms_threshold: None,
            shared_muted: None,
        }
    }
}

impl Default for AudioInputBuilder<fn(Bytes)> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> AudioInputBuilder<F>
where
    F: Fn(Bytes) + Send + 'static,
{
    /// Sets the input device by ID.
    ///
    /// If not set or set to None, the default input device will be used.
    pub fn device(mut self, device_id: Option<String>) -> Self {
        self.config.device_id = device_id;
        self
    }

    /// Configures noise suppression.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable noise suppression
    /// * `model` - Optional custom RNNoise model bytes (None for default model)
    pub fn denoise(mut self, enabled: bool, model: Option<RnnModel>) -> Self {
        self.config.denoise_enabled = enabled;
        self.config.denoise_model = model;
        self
    }

    /// Sets the input volume multiplier.
    ///
    /// * 1.0 = unity gain (default)
    /// * 0.5 = half volume
    /// * 2.0 = double volume
    pub fn volume(mut self, volume: f32) -> Self {
        self.config.volume = volume;
        self
    }

    /// Sets the RMS threshold for silence detection.
    ///
    /// Audio below this threshold will be treated as silence.
    /// A value of 0.0 disables silence detection.
    pub fn rms_threshold(mut self, threshold: f32) -> Self {
        self.config.rms_threshold = threshold;
        self
    }

    /// Sets a shared atomic for input volume, enabling real-time synchronization.
    ///
    /// When provided, the builder will use this shared atomic instead of creating
    /// a new one. This allows external code to modify the volume in real-time
    /// and have the changes immediately affect audio processing.
    ///
    /// Use this when you need to share volume control with other components,
    /// such as a core state manager. For simple cases where you only need to
    /// set the initial volume, use [`volume`](Self::volume) instead.
    pub fn input_volume_shared(mut self, volume: &Arc<AtomicF32>) -> Self {
        self.shared_input_volume = Some(volume.clone());
        self
    }

    /// Sets a shared atomic for RMS threshold, enabling real-time synchronization.
    ///
    /// When provided, the builder will use this shared atomic instead of creating
    /// a new one. This allows external code to modify the threshold in real-time
    /// and have the changes immediately affect silence detection.
    ///
    /// Use this when you need to share threshold control with other components.
    /// For simple cases where you only need to set the initial threshold,
    /// use [`rms_threshold`](Self::rms_threshold) instead.
    pub fn rms_threshold_shared(mut self, threshold: &Arc<AtomicF32>) -> Self {
        self.shared_rms_threshold = Some(threshold.clone());
        self
    }

    /// Sets a shared atomic for muted state, enabling real-time synchronization.
    ///
    /// When provided, the builder will use this shared atomic instead of creating
    /// a new one. This allows external code to modify the muted state in real-time
    /// and have the changes immediately affect audio processing.
    ///
    /// Use this when you need to share mute control with other components,
    /// such as a core state manager. The muted state can still be controlled
    /// via the handle's `mute()` and `unmute()` methods after building.
    pub fn muted_shared(mut self, muted: &Arc<AtomicBool>) -> Self {
        self.shared_muted = Some(muted.clone());
        self
    }

    /// Configures codec encoding.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable codec encoding
    /// * `vbr` - Whether to use variable bit rate
    /// * `residual_bits` - Quality setting for residual encoding
    pub fn codec(mut self, enabled: bool, vbr: bool, residual_bits: f32) -> Self {
        self.config.codec_enabled = enabled;
        self.config.codec_vbr = vbr;
        self.config.codec_residual_bits = residual_bits;
        self
    }

    /// Sets whether this is a room call.
    ///
    /// Room calls skip the encoder start state.
    pub fn room(mut self, is_room: bool) -> Self {
        self.config.is_room = is_room;
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

    /// Sets the callback for receiving processed audio data.
    ///
    /// The callback receives processed audio frames as `Bytes`.
    pub fn callback<G>(self, callback: G) -> AudioInputBuilder<G>
    where
        G: Fn(Bytes) + Send + 'static,
    {
        AudioInputBuilder {
            config: self.config,
            callback: Some(callback),
            shared_input_volume: self.shared_input_volume,
            shared_rms_threshold: self.shared_rms_threshold,
            shared_muted: self.shared_muted,
        }
    }

    /// Builds and starts the audio input stream.
    ///
    /// This method creates and configures all necessary processing threads
    /// and returns an `AudioInputHandle` for controlling the stream.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No callback was set
    /// - The device cannot be found
    /// - The stream cannot be created
    /// - The device uses an unsupported sample format
    #[cfg(not(target_family = "wasm"))]
    pub fn build(self, host: &AudioHost) -> Result<AudioInputHandle, AudioError> {
        let callback = self
            .callback
            .ok_or(AudioError::Config("No callback set".to_string()))?;

        // Get the input device
        let device_handle =
            get_input_device(host, self.config.device_id.as_deref()).map_err(|e| match e {
                DeviceError::NoDefaultDevice => AudioError::Device("No input device".to_string()),
                DeviceError::DeviceNotFound(id) => {
                    AudioError::Device(format!("Device not found: {}", id))
                }
                DeviceError::EnumerationFailed(msg) => AudioError::Device(msg),
                DeviceError::InvalidDeviceId(id) => {
                    AudioError::Device(format!("Invalid device ID: {}", id))
                }
            })?;

        let device = device_handle.device();
        let config = device.default_input_config()?;

        if config.sample_format() != SampleFormat::F32 {
            return Err(AudioError::Config("Unsupported sample format".to_string()));
        }

        let input_channels = config.channels() as usize;
        let device_sample_rate = config.sample_rate();

        // Create shared atomic state (use provided shared atomics or create new ones)
        let input_volume = self
            .shared_input_volume
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicF32::new(self.config.volume)));
        let rms_threshold = self
            .shared_rms_threshold
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicF32::new(self.config.rms_threshold)));
        let muted = self
            .shared_muted
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let rms_sender = Arc::new(AtomicF32::new(0.0));

        // Create channels
        let (input_sender, input_receiver) = bounded::<f32>(CHANNEL_SIZE);
        let (processed_sender, processed_receiver) = unbounded_async::<ProcessorMessage>();
        let (encoded_sender, encoded_receiver) = unbounded_async::<ProcessorMessage>();

        // When denoise is enabled, the processor resamples to 48kHz for rnnoise
        // processing and outputs 48kHz frames (no downsample back to device rate).
        // When denoise is disabled, the processor passes through at device rate.
        // The encoder sample rate must match the processor's output rate.
        let encoder_sample_rate = if self.config.denoise_enabled {
            48_000
        } else {
            device_sample_rate
        };

        // Create denoiser if needed
        let denoiser = if self.config.denoise_enabled {
            Some(DenoiseState::from_model(
                self.config.denoise_model.unwrap_or_default(),
            ))
        } else {
            None
        };

        let state = InputProcessorState::new(&input_volume, &rms_threshold, &muted, rms_sender);

        let codec_enabled = self.config.codec_enabled;
        let codec_vbr = self.config.codec_vbr;
        let codec_residual_bits = self.config.codec_residual_bits;
        let is_room = self.config.is_room;

        // Spawn processor thread
        let processor_input = ChannelInput::from(input_receiver);
        let processed_sender_sync = processed_sender.to_sync();
        let processor_handle = thread::spawn(move || {
            if let Err(e) = input_processor(
                processor_input,
                processed_sender_sync,
                device_sample_rate as f64,
                denoiser,
                codec_enabled,
                state,
            ) {
                error!("Input processor error: {}", e);
            }
            debug!("Input processor thread ended");
        });

        // Select the appropriate receiver for the callback and spawn encoder if needed
        let (encoder_handle, output_receiver) = if codec_enabled {
            let processed_receiver_sync = processed_receiver.to_sync();
            let encoded_sender_sync = encoded_sender.to_sync();
            let handle = thread::spawn(move || {
                if let Err(e) = encoder(
                    processed_receiver_sync,
                    encoded_sender_sync,
                    encoder_sample_rate,
                    codec_vbr,
                    codec_residual_bits,
                    is_room,
                ) {
                    error!("Encoder error: {}", e);
                }
                debug!("Encoder thread ended");
            });
            (Some(handle), encoded_receiver.to_sync())
        } else {
            (None, processed_receiver.to_sync())
        };

        // Spawn callback thread
        let callback_handle = thread::spawn(move || {
            while let Ok(message) = output_receiver.recv() {
                let bytes = match message {
                    ProcessorMessage::Data(data) => data,
                    ProcessorMessage::Samples(samples) => {
                        let i16_size = size_of::<i16>();
                        let bytes = unsafe {
                            std::slice::from_raw_parts(
                                samples.as_ptr() as *const u8,
                                samples.len() * i16_size,
                            )
                        };
                        Bytes::copy_from_slice(bytes)
                    }
                };
                callback(bytes);
            }
            debug!("Callback thread ended");
        });

        // Build the audio stream
        let input_sender_clone = input_sender.clone();
        let error_notify = self.config.error_notify.clone();
        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &_| {
                for frame in data.chunks(input_channels) {
                    // Send only the first channel (mono)
                    let _ = input_sender_clone.try_send(frame[0]);
                }
            },
            move |err| {
                error!("Input stream error: {}", err);
                if let Some(ref notify) = error_notify {
                    notify.notify_one();
                }
            },
            None,
        )?;

        stream.play()?;

        Ok(AudioInputHandle {
            _stream: stream,
            processor_handle: Some(processor_handle),
            encoder_handle,
            callback_handle: Some(callback_handle),
            input_sender: Some(input_sender),
            input_volume,
            rms_threshold,
            muted,
        })
    }

    /// Builds and starts the audio input stream (WASM version - sync stub).
    ///
    /// **WASM Note**: This method always returns an error on WASM targets.
    /// Use [`build_async`](Self::build_async) instead, as microphone access
    /// requires async permission handling via the Web Audio API.
    ///
    /// # Errors
    ///
    /// Always returns `AudioError::Platform` on WASM targets.
    #[cfg(target_family = "wasm")]
    pub fn build(self, _host: &AudioHost) -> Result<AudioInputHandle, AudioError> {
        Err(AudioError::Platform("Use build_async for WASM".to_string()))
    }

    /// Builds and starts the audio input stream asynchronously (WASM version).
    ///
    /// This method handles the async initialization required for Web Audio API
    /// access, including requesting microphone permissions from the browser.
    ///
    /// # WASM-Specific Behavior
    ///
    /// - Creates a `WebAudioWrapper` that manages the AudioContext and AudioWorklet
    /// - Requests microphone permission (browser will show permission dialog)
    /// - Sets up an AudioWorklet processor for low-latency audio capture
    /// - The `AudioInputHandle` holds an `Arc<WebAudioWrapper>` to keep it alive
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # #[cfg(target_family = "wasm")]
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use telepathy_audio::{AudioHost, AudioInputBuilder};
    ///
    /// let host = AudioHost::new();
    /// let input = AudioInputBuilder::new()
    ///     .callback(|data| { /* process audio */ })
    ///     .build_async(&host)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(target_family = "wasm")]
    pub async fn build_async(self, _host: &AudioHost) -> Result<AudioInputHandle, AudioError> {
        use crate::web_audio::{WebAudioInput, WebAudioWrapper};
        use std::sync::Arc;

        let callback = self
            .callback
            .ok_or(AudioError::Config("No callback set".to_string()))?;

        // Initialize WebAudioWrapper (requests microphone permission)
        let web_audio = WebAudioWrapper::new()
            .await
            .map_err(|e| AudioError::Platform(format!("Web Audio init failed: {:?}", e)))?;

        let input_sample_rate = web_audio.sample_rate;
        let processor_input = WebAudioInput::from(&web_audio);

        // Store the wrapper to keep it alive
        let web_audio = Arc::new(web_audio);

        // Create shared atomic state (use provided shared atomics or create new ones)
        let input_volume = self
            .shared_input_volume
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicF32::new(self.config.volume)));
        let rms_threshold = self
            .shared_rms_threshold
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicF32::new(self.config.rms_threshold)));
        let muted = self
            .shared_muted
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let rms_sender = Arc::new(AtomicF32::new(0.0));

        // Create channels
        let (processed_sender, processed_receiver) = unbounded_async::<ProcessorMessage>();
        let (encoded_sender, encoded_receiver) = unbounded_async::<ProcessorMessage>();

        // When denoise is enabled, the processor resamples to 48kHz for rnnoise
        // processing and outputs 48kHz frames (no downsample back to device rate).
        // When denoise is disabled, the processor passes through at device rate.
        // The encoder sample rate must match the processor's output rate.
        let encoder_sample_rate = if self.config.denoise_enabled {
            48_000
        } else {
            input_sample_rate
        };

        // Create denoiser if needed
        let denoiser = if self.config.denoise_enabled {
            let model = if let Some(model_bytes) = &self.config.denoise_model {
                RnnModel::from_bytes(model_bytes).unwrap_or_default()
            } else {
                RnnModel::default()
            };
            Some(DenoiseState::from_model(model))
        } else {
            None
        };

        let state = InputProcessorState::new(&input_volume, &rms_threshold, &muted, rms_sender);

        let codec_enabled = self.config.codec_enabled;
        let codec_vbr = self.config.codec_vbr;
        let codec_residual_bits = self.config.codec_residual_bits;
        let is_room = self.config.is_room;

        // Spawn processor thread
        let processed_sender_sync = processed_sender.to_sync();
        let processor_handle = thread::spawn(move || {
            if let Err(e) = input_processor(
                processor_input,
                processed_sender_sync,
                input_sample_rate as f64,
                denoiser,
                codec_enabled,
                state,
            ) {
                error!("Input processor error: {}", e);
            }
            debug!("Input processor thread ended");
        });

        // Select the appropriate receiver for the callback and spawn encoder if needed
        let (encoder_handle, output_receiver) = if codec_enabled {
            let processed_receiver_sync = processed_receiver.to_sync();
            let encoded_sender_sync = encoded_sender.to_sync();
            let handle = thread::spawn(move || {
                if let Err(e) = encoder(
                    processed_receiver_sync,
                    encoded_sender_sync,
                    encoder_sample_rate,
                    codec_vbr,
                    codec_residual_bits,
                    is_room,
                ) {
                    error!("Encoder error: {}", e);
                }
                debug!("Encoder thread ended");
            });
            (Some(handle), encoded_receiver.to_sync())
        } else {
            (None, processed_receiver.to_sync())
        };

        // Spawn callback thread
        let callback_handle = thread::spawn(move || {
            while let Ok(message) = output_receiver.recv() {
                let bytes = match message {
                    ProcessorMessage::Data(data) => data,
                    ProcessorMessage::Samples(samples) => {
                        let i16_size = size_of::<i16>();
                        let bytes = unsafe {
                            std::slice::from_raw_parts(
                                samples.as_ptr() as *const u8,
                                samples.len() * i16_size,
                            )
                        };
                        Bytes::copy_from_slice(bytes)
                    }
                };
                callback(bytes);
            }
            debug!("Callback thread ended");
        });

        // Resume the audio context
        web_audio.resume();

        Ok(AudioInputHandle {
            _web_audio: Some(web_audio),
            processor_handle: Some(processor_handle),
            encoder_handle,
            callback_handle: Some(callback_handle),
            input_sender: None, // No sender for WASM - controlled via web_audio
            input_volume,
            rms_threshold,
            muted,
        })
    }
}

/// Handle to a running audio input stream.
///
/// This handle allows controlling the audio input (mute/unmute, volume)
/// and automatically cleans up resources when dropped.
///
/// ## Lifecycle
///
/// - **Creation**: Created by [`AudioInputBuilder::build`] (native) or
///   [`AudioInputBuilder::build_async`] (WASM)
/// - **Running**: Audio is captured, processed, and delivered via callback
/// - **Cleanup**: When dropped, the handle:
///   1. Closes the input channel to signal threads to stop
///   2. Waits for processor, encoder (if enabled), and callback threads to join
///   3. Stops the underlying audio stream
///
/// ## Thread Safety
///
/// All control methods (`mute`, `unmute`, `set_volume`, etc.) are thread-safe
/// and can be called from any thread. They use atomic operations internally.
///
/// ## Platform Differences
///
/// - **Native**: Uses cpal stream and kanal channels for communication
/// - **WASM**: Uses WebAudioWrapper and Web Audio API; requires `build_async`
///
/// ## Drop vs stop()
///
/// Calling `drop` (implicit when handle goes out of scope) and `stop()` have
/// the same effect. Use `stop()` when you need to explicitly wait for cleanup
/// completion in a specific code location.
pub struct AudioInputHandle {
    #[cfg(not(target_family = "wasm"))]
    _stream: cpal::Stream,
    #[cfg(target_family = "wasm")]
    _web_audio: Option<std::sync::Arc<crate::web_audio::WebAudioWrapper>>,
    processor_handle: Option<JoinHandle<()>>,
    encoder_handle: Option<JoinHandle<()>>,
    callback_handle: Option<JoinHandle<()>>,
    input_sender: Option<kanal::Sender<f32>>,
    input_volume: Arc<AtomicF32>,
    rms_threshold: Arc<AtomicF32>,
    muted: Arc<AtomicBool>,
}

impl AudioInputHandle {
    /// Mutes the audio input.
    ///
    /// When muted, no audio data will be processed or sent to the callback.
    pub fn mute(&self) {
        self.muted.store(true, Relaxed);
    }

    /// Unmutes the audio input.
    pub fn unmute(&self) {
        self.muted.store(false, Relaxed);
    }

    /// Returns whether the input is currently muted.
    pub fn is_muted(&self) -> bool {
        self.muted.load(Relaxed)
    }

    /// Sets the input volume multiplier.
    pub fn set_volume(&self, volume: f32) {
        self.input_volume.store(volume, Relaxed);
    }

    /// Gets the current input volume multiplier.
    pub fn volume(&self) -> f32 {
        self.input_volume.load(Relaxed)
    }

    /// Sets the RMS threshold for silence detection.
    pub fn set_rms_threshold(&self, threshold: f32) {
        self.rms_threshold.store(threshold, Relaxed);
    }

    /// Gets the current RMS threshold.
    pub fn rms_threshold(&self) -> f32 {
        self.rms_threshold.load(Relaxed)
    }

    /// Stops the audio input and waits for all threads to finish.
    ///
    /// This is called automatically when the handle is dropped, but can
    /// be called explicitly if you need to wait for completion.
    pub fn stop(mut self) {
        // Drop the sender to signal threads to stop
        self.input_sender.take();

        // Wait for threads to finish
        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.encoder_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.callback_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AudioInputHandle {
    fn drop(&mut self) {
        // Close the sender to signal threads to stop
        if let Some(sender) = self.input_sender.take() {
            _ = sender.close();
        }

        // Wait for threads to finish
        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.encoder_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.callback_handle.take() {
            let _ = handle.join();
        }
    }
}
