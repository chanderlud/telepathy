//! Audio output API.
//!
//! This module provides a high-level API for playing back processed audio.
//! It handles device selection, resampling, codec decoding, and volume control.
//!
//! # Example
//!
//! ```rust,no_run
//! use telepathy_audio::devices::CpalAudioHost;
//! use telepathy_audio::io::AudioOutputBuilder;
//! use telepathy_audio::adapters::MpscSource;
//! use bytes::Bytes;
//! use std::sync::mpsc;
//!
//! let host = CpalAudioHost::new();
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

use crate::devices::AudioHost;
use crate::error::Error;
use crate::internal::NETWORK_FRAME;
use crate::internal::processor::output_processor;
use crate::internal::state::OutputProcessorState;
use crate::internal::thread::{self, JoinHandle};
use crate::io::StreamErrorCallback;
use crate::io::traits::AudioDataSource;
use crate::sea::codec::file::SeaFileHeader;
use crate::sea::decoder::SeaDecoder;
use atomic_float::AtomicF32;
use nnnoiseless::FRAME_SIZE;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use tracing::{debug, error};

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
    /// Optional callback for stream errors.
    ///
    /// When set, the callback receives the underlying CPAL stream error.
    /// When unset, stream errors are logged by default.
    pub error_callback: Option<StreamErrorCallback>,
}

impl Default for AudioOutputConfig {
    fn default() -> Self {
        Self {
            device_id: None,
            sample_rate: 48_000,
            volume: 1.0,
            codec_enabled: false,
            error_callback: None,
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

    /// Sets a callback to be triggered on stream errors.
    ///
    /// When set, the callback receives the underlying CPAL stream error.
    pub fn on_error<F>(mut self, callback: F) -> Self
    where
        F: FnMut(cpal::StreamError) + Send + 'static,
    {
        self.config.error_callback = Some(Box::new(callback));
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
    pub fn build<I>(
        mut self,
        host: &impl AudioHost<OutputStream = I>,
    ) -> Result<AudioOutputHandle<I>, Error>
    where
        I: Send + 'static,
    {
        if self.source.is_none() {
            return Err(Error::Config(
                "a data source must be set via source()".to_string(),
            ));
        }

        // Open the output
        let error_callback = self.config.error_callback.take();
        let (processor_output, output_rate, stream) =
            host.open_output(self.config.device_id.as_deref(), error_callback)?;

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
            if let Err(e) = output_processor(
                source,
                processor_output,
                self.config.sample_rate as usize,
                output_rate as usize,
                state,
                decoder,
            ) {
                error!("Output processor error: {}", e);
            }
            debug!("Output processor thread ended");
        })?;

        Ok(AudioOutputHandle {
            _stream: Some(stream),
            _processor_handle: Some(processor_handle),
            output_volume,
            deafened,
            loss_sender,
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
pub struct AudioOutputHandle<S> {
    _stream: Option<S>,
    _processor_handle: Option<JoinHandle<()>>,
    output_volume: Arc<AtomicF32>,
    deafened: Arc<AtomicBool>,
    loss_sender: Arc<AtomicUsize>,
}

impl<S> AudioOutputHandle<S> {
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
