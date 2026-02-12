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
//!         println!("Received {} bytes", data.as_ref().len());
//!     })
//!     .build(&host)
//!     .unwrap();
//! ```

use crate::devices::AudioHost;
#[cfg(not(target_family = "wasm"))]
use crate::devices::{DeviceError, get_input_device};
use crate::error::AudioError;
use crate::internal::buffer_pool::{DEFAULT_POOL_CAPACITY, PooledBuffer};
use crate::internal::processor::input_processor;
use crate::internal::state::InputProcessorState;
use crate::internal::traits::AudioInput;
#[cfg(not(target_family = "wasm"))]
use crate::internal::traits::{CHANNEL_SIZE, RingBufferInput};
#[cfg(target_family = "wasm")]
use crate::platform::web_audio::WebAudioWrapper;
use crate::sea::encoder::{EncoderSettings, SeaEncoder};
use atomic_float::AtomicF32;
#[cfg(not(target_family = "wasm"))]
use cpal::Sample;
#[cfg(not(target_family = "wasm"))]
use cpal::SampleFormat;
#[cfg(not(target_family = "wasm"))]
use cpal::traits::{DeviceTrait, StreamTrait};
use kanal::{AsyncSender, unbounded};
use log::{debug, error};
use nnnoiseless::{DenoiseState, RnnModel};
#[cfg(not(target_family = "wasm"))]
use rtrb::RingBuffer;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Condvar};
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
    /// Residual bits for codec quality (typically 2.0-8.0).
    ///
    /// Higher values provide better quality but larger encoded size.
    pub codec_residual_bits: f32,
    /// Optional notify handle for stream errors.
    ///
    /// When set, the notify is triggered via `notify_one()` whenever a
    /// stream error occurs, in addition to logging the error. Useful for
    /// async error handling and reconnection logic.
    pub error_notify: Option<Arc<Notify>>,
    /// Optional output sample rate override (only used when denoise is disabled).
    ///
    /// When set and `denoise_enabled` is `false`, the processor will resample
    /// to this rate instead of passing through at the device's native rate.
    /// This is useful for matching network requirements (e.g., 48kHz) without
    /// the CPU overhead of noise suppression.
    ///
    /// When `denoise_enabled` is `true`, this field is ignored and output is
    /// always 48kHz (RNNoise requirement).
    pub output_sample_rate: Option<u32>,
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
            error_notify: None,
            output_sample_rate: None,
        }
    }
}

/// Builder for configuring and creating audio input streams.
///
/// Use this builder to configure audio input processing options before
/// starting the stream.
pub struct AudioInputBuilder<F>
where
    F: Fn(PooledBuffer) + Send + 'static,
{
    config: AudioInputConfig,
    callback: Option<F>,
    channel: Option<AsyncSender<PooledBuffer>>,
    /// Optional shared atomic for input volume (enables real-time synchronization)
    shared_input_volume: Option<Arc<AtomicF32>>,
    /// Optional shared atomic for RMS threshold (enables real-time synchronization)
    shared_rms_threshold: Option<Arc<AtomicF32>>,
    /// Optional shared atomic for muted state (enables real-time synchronization)
    shared_muted: Option<Arc<AtomicBool>>,
    /// Optional shared atomic for rms state (enables real-time synchronization)
    shared_rms: Option<Arc<AtomicF32>>,
}

/// Internal context for building audio input, shared between native and WASM
struct InputBuildContext {
    input_volume: Arc<AtomicF32>,
    rms_threshold: Arc<AtomicF32>,
    muted: Arc<AtomicBool>,
    processor_handle: JoinHandle<()>,
    callback_handle: Option<JoinHandle<()>>,
}

impl AudioInputBuilder<fn(PooledBuffer)> {
    /// Creates a new audio input builder with default configuration.
    pub fn new() -> AudioInputBuilder<fn(PooledBuffer)> {
        AudioInputBuilder {
            config: AudioInputConfig::default(),
            channel: None,
            callback: None,
            shared_input_volume: None,
            shared_rms_threshold: None,
            shared_muted: None,
            shared_rms: None,
        }
    }
}

impl Default for AudioInputBuilder<fn(PooledBuffer)> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F> AudioInputBuilder<F>
where
    F: Fn(PooledBuffer) + Send + 'static,
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
        self.config.output_sample_rate = None;
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

    pub fn rms_shared(mut self, shared: &Arc<AtomicF32>) -> Self {
        self.shared_rms = Some(shared.clone());
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

    /// Sets a notify handle to be triggered on stream errors.
    ///
    /// When set, the notify will be triggered via `notify_one()` whenever
    /// a stream error occurs, in addition to logging the error.
    pub fn on_error(mut self, notify: &Arc<Notify>) -> Self {
        self.config.error_notify = Some(notify.clone());
        self
    }

    /// Sets the output sample rate when denoising is disabled.
    ///
    /// This allows forcing a specific output sample rate (e.g., 48kHz for network
    /// compatibility) without enabling noise suppression. When denoise is enabled,
    /// this setting is ignored as RNNoise requires 48kHz.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - The desired output sample rate in Hz (e.g., 48000)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use telepathy_audio::AudioInputBuilder;
    ///
    /// let builder = AudioInputBuilder::new()
    ///     .denoise(false, None)      // Disable denoising first
    ///     .output_sample_rate(48000); // Then set custom output rate
    /// ```
    pub fn output_sample_rate(mut self, sample_rate: u32) -> Self {
        if !self.config.denoise_enabled {
            self.config.output_sample_rate = Some(sample_rate);
        }
        self
    }

    /// Sets the callback for receiving processed audio data.
    ///
    /// The callback receives processed audio frames as `PooledBuffer`.
    pub fn callback<G>(self, callback: G) -> AudioInputBuilder<G>
    where
        G: Fn(PooledBuffer) + Send + 'static,
    {
        AudioInputBuilder {
            config: self.config,
            channel: None,
            callback: Some(callback),
            shared_input_volume: self.shared_input_volume,
            shared_rms_threshold: self.shared_rms_threshold,
            shared_muted: self.shared_muted,
            shared_rms: self.shared_rms,
        }
    }

    /// Sets the channel for receiving processed audio data.
    ///
    /// The channel receives processed audio frames as `PooledBuffer`.
    pub fn channel(mut self, channel: AsyncSender<PooledBuffer>) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Common initialization logic shared between native and WASM builds.
    ///
    /// This method handles all shared setup steps:
    /// - Creates shared atomic state (input_volume, rms_threshold, muted, rms_sender)
    /// - Creates unbounded channels for inter-thread communication
    /// - Calculates output sample rate and resampling ratio with the following precedence:
    ///   1. **Denoise enabled**: Always 48kHz (RNNoise requirement)
    ///   2. **Custom output_sample_rate**: Uses the specified rate
    ///   3. **Neither**: Uses device's native sample rate (pass-through)
    /// - Creates denoiser if needed (DenoiseState with optional custom model)
    /// - Creates InputProcessorState for atomic state management
    /// - Creates encoder if codec is enabled (sample rate matches processor output)
    /// - Spawns processor thread (calls `input_processor` with calculated ratio)
    /// - Spawns callback thread if callback is set
    ///
    /// # Type Parameters
    ///
    /// * `I` - Type implementing `AudioInput` trait (e.g., `RingBufferInput`, `WebAudioInput`)
    ///
    /// # Arguments
    ///
    /// * `processor_input` - The audio input source for the processor
    /// * `input_sample_rate` - Device's native sample rate in Hz
    ///
    /// # Returns
    ///
    /// Returns `Result<InputBuildContext, AudioError>` containing handles and state for the created threads,
    /// or an error if codec initialization fails.
    fn build_common<I: AudioInput + Send + 'static>(
        self,
        processor_input: I,
        input_sample_rate: u32,
    ) -> Result<InputBuildContext, AudioError> {
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
        let rms_sender = self.shared_rms.clone().unwrap_or_default();

        let (processor_sender, output_receiver) = if let Some(sender) = self.channel {
            (sender.to_sync(), None)
        } else {
            let (processor_sender, processor_receiver) = unbounded();
            (processor_sender, Some(processor_receiver))
        };

        // create denoiser if needed
        let denoiser = if self.config.denoise_enabled {
            Some(DenoiseState::from_model(
                self.config.denoise_model.clone().unwrap_or_default(),
            ))
        } else {
            None
        };
        // build input processor state
        let state = InputProcessorState::new(
            &input_volume,
            &rms_threshold,
            &muted,
            rms_sender,
            DEFAULT_POOL_CAPACITY,
        );
        // determine the output sample rate
        let sample_rate = if self.config.denoise_enabled {
            48_000
        } else {
            self.config.output_sample_rate.unwrap_or(input_sample_rate)
        };
        // calculate the required ratio
        let ratio = sample_rate as f64 / input_sample_rate as f64;
        // create the encoder if needed
        let encoder = if self.config.codec_enabled {
            Some(SeaEncoder::new(
                1,
                sample_rate,
                EncoderSettings {
                    residual_bits: self.config.codec_residual_bits,
                    vbr: self.config.codec_vbr,
                    ..Default::default()
                },
            )?)
        } else {
            None
        };

        // spawn processor thread
        let processor_handle = thread::spawn(move || {
            if let Err(e) = input_processor(
                processor_input,
                processor_sender,
                ratio,
                denoiser,
                state,
                encoder,
            ) {
                error!("Input processor error: {}", e);
            }
            debug!("Input processor thread ended");
        });

        // spawn callback thread if callback is set
        let callback_handle = self.callback.and_then(|callback| {
            let receiver = output_receiver?;
            Some(thread::spawn(move || {
                while let Ok(buffer) = receiver.recv() {
                    callback(buffer)
                }
                debug!("Callback thread ended");
            }))
        });

        Ok(InputBuildContext {
            input_volume,
            rms_threshold,
            muted,
            processor_handle,
            callback_handle,
        })
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
        if self.callback.is_none() && self.channel.is_none() {
            return Err(AudioError::Config(
                "either callback or channel must be set".to_string(),
            ));
        }

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

        let input_channels = config.channels() as usize;
        let device_sample_rate = config.sample_rate();
        let sample_format = config.sample_format();

        // Create ring buffer for cpal stream to processor
        let (input_producer, input_consumer) = RingBuffer::<f32>::new(CHANNEL_SIZE);
        // Create condvar to wake input processor when the ring buffer changes
        let input_notify = Arc::new(Condvar::new());
        // Create sender for input stream to input processor
        let input_sender = RingBufferSender {
            producer: input_producer,
            notify: input_notify.clone(),
        };
        // Create processor input and extract error_notify before consuming self
        let processor_input = RingBufferInput::new(input_consumer, input_notify);
        let error_notify = self.config.error_notify.clone();
        // Build common components (channels, threads, state)
        let context = self.build_common(processor_input, device_sample_rate)?;

        // Build the audio stream with the appropriate sample format
        let stream = match sample_format {
            SampleFormat::I8 => build_input_stream_with_format::<i8>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::I16 => build_input_stream_with_format::<i16>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::I32 => build_input_stream_with_format::<i32>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::I64 => build_input_stream_with_format_64::<i64>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::U8 => build_input_stream_with_format::<u8>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::U16 => build_input_stream_with_format::<u16>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::U32 => build_input_stream_with_format::<u32>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::U64 => build_input_stream_with_format_64::<u64>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::F32 => build_input_stream_with_format::<f32>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            SampleFormat::F64 => build_input_stream_with_format_64::<f64>(
                device,
                &config.into(),
                input_sender,
                input_channels,
                error_notify,
            )?,
            _ => return Err(AudioError::Config("Unsupported sample format".to_string())),
        };

        stream.play()?;

        Ok(AudioInputHandle {
            _stream: Some(stream),
            processor_handle: Some(context.processor_handle),
            callback_handle: context.callback_handle,
            input_volume: context.input_volume,
            rms_threshold: context.rms_threshold,
            muted: context.muted,
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
    /// # Parameters
    ///
    /// * `web_audio_wrapper` - An optional pre-initialized `WebAudioWrapper`. When provided,
    ///   this wrapper will be used instead of creating a new one. This is important for
    ///   satisfying Web Audio API threading requirements, as the wrapper must be initialized
    ///   on the correct thread (typically the main thread during user interaction).
    ///   When `None`, a new wrapper will be created (useful for standalone usage).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # #[cfg(target_family = "wasm")]
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// use telepathy_audio::{AudioHost, AudioInputBuilder};
    ///
    /// let host = AudioHost::new();
    /// // Without pre-initialized wrapper (creates new one)
    /// let input = AudioInputBuilder::new()
    ///     .callback(|data| { /* process audio */ })
    ///     .build_async(&host, None)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(target_family = "wasm")]
    pub async fn build_async(
        self,
        _host: &AudioHost,
        web_audio_wrapper: Option<Arc<WebAudioWrapper>>,
    ) -> Result<AudioInputHandle, AudioError> {
        use crate::platform::web_audio::WebAudioInput;

        if self.callback.is_none() && self.channel.is_none() {
            return Err(AudioError::Config(
                "either callback or channel must be set".to_string(),
            ));
        }

        // Use provided WebAudioWrapper or initialize a new one (requests microphone permission)
        let web_audio = match web_audio_wrapper {
            Some(wrapper) => wrapper,
            None => {
                let wrapper = WebAudioWrapper::new()
                    .await
                    .map_err(|e| AudioError::Platform(format!("Web Audio init failed: {:?}", e)))?;
                Arc::new(wrapper)
            }
        };

        let input_sample_rate = web_audio.sample_rate;
        let processor_input = WebAudioInput::from(&*web_audio);

        // Build common components (channels, threads, state)
        let context = self.build_common(processor_input, input_sample_rate as u32)?;

        // Resume the audio context
        web_audio.resume();

        Ok(AudioInputHandle {
            _web_audio: Some(web_audio),
            processor_handle: Some(context.processor_handle),
            callback_handle: context.callback_handle,
            input_sender: None, // No sender for WASM - controlled via web_audio
            input_volume: context.input_volume,
            rms_threshold: context.rms_threshold,
            muted: context.muted,
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
/// - **Running**: Audio is captured, processed (with optional encoding), and delivered via callback
/// - **Cleanup**: When dropped, the handle:
///   1. Closes the input channel to signal threads to stop
///   2. Waits for processor and callback (if enabled) threads to join
///   3. Stops the underlying audio stream
///
/// ## Thread Safety
///
/// All control methods (`mute`, `unmute`, `set_volume`, etc.) are thread-safe
/// and can be called from any thread. They use atomic operations internally.
///
/// ## Platform Differences
///
/// - **Native**: Uses cpal stream and rtrb ring buffer for input communication
/// - **WASM**: Uses WebAudioWrapper and Web Audio API; requires `build_async`
///
/// ## Drop vs stop()
///
/// Calling `drop` (implicit when handle goes out of scope) and `stop()` have
/// the same effect. Use `stop()` when you need to explicitly wait for cleanup
/// completion in a specific code location.
pub struct AudioInputHandle {
    #[cfg(not(target_family = "wasm"))]
    _stream: Option<cpal::Stream>,
    #[cfg(target_family = "wasm")]
    _web_audio: Option<std::sync::Arc<crate::platform::web_audio::WebAudioWrapper>>,
    processor_handle: Option<JoinHandle<()>>,
    callback_handle: Option<JoinHandle<()>>,
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
    #[cfg(not(target_family = "wasm"))]
    pub fn stop(mut self) {
        // Drop the stream so its callback stops before joining threads
        drop(self._stream.take());

        // Wait for threads to finish
        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.callback_handle.take() {
            let _ = handle.join();
        }
    }

    /// Stops the audio input and waits for all threads to finish.
    ///
    /// This is called automatically when the handle is dropped, but can
    /// be called explicitly if you need to wait for completion.
    #[cfg(target_family = "wasm")]
    pub fn stop(mut self) {
        // Drop the WebAudioWrapper to set finished flag before joining threads
        self._web_audio.take();

        // Wait for threads to finish
        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.callback_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AudioInputHandle {
    fn drop(&mut self) {
        // Drop the underlying source so its callback stops
        #[cfg(not(target_family = "wasm"))]
        {
            self._stream.take();
        }
        #[cfg(target_family = "wasm")]
        {
            self._web_audio.take();
        }

        // Wait for threads to finish
        if let Some(handle) = self.processor_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.callback_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Lock free sender for native targets
///
/// Crucially, when the sender is dropped, the input processor is woken up
#[cfg(not(target_family = "wasm"))]
struct RingBufferSender {
    producer: rtrb::Producer<f32>,
    notify: Arc<Condvar>,
}

impl Drop for RingBufferSender {
    fn drop(&mut self) {
        self.notify.notify_one();
    }
}

/// Builds an input stream with automatic sample format conversion.
///
/// This helper function creates a cpal input stream that converts the device's
/// native sample format T to f32 for the processing pipeline using cpal's
/// `to_float_sample()` method.
///
/// # Type Parameters
///
/// * `T` - The device's native sample format (e.g., i16, f32, u16)
///
/// # Arguments
///
/// * `device` - The cpal device to build the stream on
/// * `config` - Stream configuration (sample rate, channels, etc.)
/// * `input_producer` - Ring buffer producer for f32 samples to the processor
/// * `input_channels` - Number of input channels
/// * `error_notify` - Optional notify handle for stream errors
#[cfg(not(target_family = "wasm"))]
fn build_input_stream_with_format<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut input_sender: RingBufferSender,
    input_channels: usize,
    error_notify: Option<Arc<Notify>>,
) -> Result<cpal::Stream, AudioError>
where
    T: Sample<Float = f32> + cpal::SizedSample + Send + 'static,
{
    use cpal::traits::DeviceTrait;

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            let Ok(chunk) = input_sender
                .producer
                .write_chunk_uninit(data.len() / input_channels)
            else {
                return;
            };

            chunk.fill_from_iter(data.chunks(input_channels).map(|frame| {
                // Convert device format T to f32
                frame[0].to_float_sample()
            }));
            input_sender.notify.notify_one();
        },
        move |err| {
            log::error!("Input stream error: {}", err);
            if let Some(ref notify) = error_notify {
                notify.notify_one();
            }
        },
        None,
    )?;

    Ok(stream)
}

/// Builds an input stream for 64-bit sample formats (i64, u64, f64).
///
/// These types use f64 as their intermediate float type, so we need a separate
/// helper that converts f64 to f32.
#[cfg(not(target_family = "wasm"))]
fn build_input_stream_with_format_64<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut input_sender: RingBufferSender,
    input_channels: usize,
    error_notify: Option<Arc<Notify>>,
) -> Result<cpal::Stream, AudioError>
where
    T: Sample<Float = f64> + cpal::SizedSample + Send + 'static,
{
    use cpal::traits::DeviceTrait;

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            let Ok(chunk) = input_sender
                .producer
                .write_chunk_uninit(data.len() / input_channels)
            else {
                return;
            };

            chunk.fill_from_iter(data.chunks(input_channels).map(|frame| {
                // Convert device format T to f64, then to f32
                let sample_f64 = frame[0].to_float_sample();
                sample_f64 as f32
            }));
            input_sender.notify.notify_one();
        },
        move |err| {
            log::error!("Input stream error: {}", err);
            if let Some(ref notify) = error_notify {
                notify.notify_one();
            }
        },
        None,
    )?;

    Ok(stream)
}
