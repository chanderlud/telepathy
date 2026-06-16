use crate::constants::TRANSITION_LENGTH;
use crate::devices::{AudioDeviceInfo, AudioDeviceList, AudioHost, DeviceError, device_to_info};
#[cfg(not(target_family = "wasm"))]
use crate::internal::traits::{AudioInput, RingBufferInput};
use crate::internal::traits::{AudioOutput, CHANNEL_SIZE, RingBufferOutput};
use crate::internal::utils::{hann_fade_in, hann_fade_out};
#[cfg(not(target_family = "wasm"))]
use crate::io::input::RingBufferSender;
use crate::io::{SendStream, StreamErrorCallback};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, DeviceId, DeviceIdError, FromSample, Sample, SampleFormat, SizedSample, Stream,
    StreamConfig,
};
use rtrb::chunks::ChunkError;
use rtrb::{Consumer, RingBuffer};
use std::fmt;
use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::sync::Condvar;
use tracing::error;

/// CPAL-backed audio host for device management.
///
/// The `CpalAudioHost` wraps the platform-specific audio backend and provides
/// thread-safe access to device enumeration and selection functionality.
///
/// A single `CpalAudioHost` instance should be shared across all components
/// that need audio device access.
///
/// ## Platform Behavior
///
/// - On **WASM**: Attempts to use `cpal::HostId::AudioWorklet` for better performance,
///   falls back to cpal's default host if AudioWorklet is unavailable. The fallback
///   is determined by cpal and may vary by browser.
/// - On **Windows**: Uses WASAPI (Windows Audio Session API)
/// - On **macOS**: Uses CoreAudio
/// - On **Linux**: Uses ALSA
///
/// ## Thread Safety
///
/// `CpalAudioHost` uses `Arc` internally and is safe to clone and share across threads.
/// The underlying cpal host is wrapped in `Arc<cpal::Host>` for efficient sharing.
#[derive(Clone)]
pub struct CpalAudioHost {
    host: Arc<cpal::Host>,
}

impl CpalAudioHost {
    /// Creates a new audio host with platform-appropriate initialization.
    pub fn new() -> Self {
        let host = cpal::default_host();
        Self {
            host: Arc::new(host),
        }
    }

    /// Returns a reference to the underlying cpal host.
    ///
    /// This method is intended for internal use.
    pub(crate) fn inner(&self) -> &cpal::Host {
        &self.host
    }

    /// Returns an atomic reference to the underlying cpal host.
    ///
    /// This method is intended for internal use.
    pub(crate) fn clone_inner(&self) -> Arc<cpal::Host> {
        Arc::clone(&self.host)
    }
}

impl Default for CpalAudioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Arc<cpal::Host>> for CpalAudioHost {
    fn from(host: Arc<cpal::Host>) -> Self {
        Self { host }
    }
}

impl fmt::Debug for CpalAudioHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpalAudioHost").finish()
    }
}

impl AudioHost for CpalAudioHost {
    type InputStream = Stream;
    type OutputStream = SendStream;

    fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        let devices = self
            .inner()
            .input_devices()
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

        Ok(devices.filter_map(|d| device_to_info(&d)).collect())
    }

    fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        let devices = self
            .inner()
            .output_devices()
            .map_err(|e| DeviceError::EnumerationFailed(e.to_string()))?;

        Ok(devices.filter_map(|d| device_to_info(&d)).collect())
    }

    fn list_all_devices(&self) -> Result<AudioDeviceList, DeviceError> {
        let input_devices = self.list_input_devices()?;
        let output_devices = self.list_output_devices()?;

        Ok(AudioDeviceList {
            input_devices,
            output_devices,
        })
    }

    fn input_sample_rate(&self, device_id: Option<&str>) -> Result<u32, DeviceError> {
        let input_device = self.get_input_device(device_id)?;
        let config = input_device.default_input_config()?;
        Ok(config.sample_rate())
    }

    fn output_sample_rate(&self, device_id: Option<&str>) -> Result<u32, DeviceError> {
        let output_device = self.get_output_device(device_id)?;
        let config = output_device.default_output_config()?;
        Ok(config.sample_rate())
    }

    #[cfg(not(target_family = "wasm"))]
    fn open_input(
        &self,
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioInput + Send + 'static, u32, Self::InputStream), DeviceError> {
        let input_device = self.get_input_device(device_id)?;
        let config = input_device.default_input_config()?;

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
        let processor_input = RingBufferInput::new(input_consumer, input_notify);

        // Build the audio stream with the appropriate sample format
        let stream = match sample_format {
            SampleFormat::I8 => build_input_stream_with_format::<i8>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::I16 => build_input_stream_with_format::<i16>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::I32 => build_input_stream_with_format::<i32>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::I64 => build_input_stream_with_format_64::<i64>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::U8 => build_input_stream_with_format::<u8>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::U16 => build_input_stream_with_format::<u16>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::U32 => build_input_stream_with_format::<u32>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::U64 => build_input_stream_with_format_64::<u64>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::F32 => build_input_stream_with_format::<f32>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            SampleFormat::F64 => build_input_stream_with_format_64::<f64>(
                &input_device,
                &config.into(),
                input_sender,
                input_channels,
                error_callback,
            )?,
            _ => {
                return Err(DeviceError::UnsupportedConfig(
                    "Unsupported sample format".to_string(),
                ));
            }
        };

        // Start playback
        stream.play()?;
        Ok((processor_input, device_sample_rate, stream))
    }

    fn open_output(
        &self,
        device_id: Option<&str>,
        error_callback: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioOutput + Send + 'static, u32, Self::OutputStream), DeviceError> {
        let output_device = self.get_output_device(device_id)?;
        let output_config = output_device.default_output_config()?;

        // Create ring buffer for lock-free producer/consumer communication
        let (output_producer, output_consumer) = RingBuffer::<f32>::new(CHANNEL_SIZE * 4);

        let output_channels = output_config.channels() as usize;
        let device_sample_rate = output_config.sample_rate();
        let sample_format = output_config.sample_format();

        let output_device = self.get_output_device(device_id)?;

        // Build the audio stream with the appropriate sample format
        let stream = match sample_format {
            SampleFormat::I8 => build_output_stream_with_format::<i8>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::I16 => build_output_stream_with_format::<i16>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::I32 => build_output_stream_with_format::<i32>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::I64 => build_output_stream_with_format::<i64>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::U8 => build_output_stream_with_format::<u8>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::U16 => build_output_stream_with_format::<u16>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::U32 => build_output_stream_with_format::<u32>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::U64 => build_output_stream_with_format::<u64>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::F32 => build_output_stream_with_format::<f32>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            SampleFormat::F64 => build_output_stream_with_format::<f64>(
                &output_device,
                &output_config.into(),
                output_consumer,
                output_channels,
                error_callback,
            )?,
            _ => {
                return Err(DeviceError::UnsupportedConfig(
                    "Unsupported sample format".to_string(),
                ));
            }
        };

        // Start playback
        stream.0.play()?;
        Ok((
            RingBufferOutput::new(output_producer),
            device_sample_rate,
            stream,
        ))
    }
}

impl CpalAudioHost {
    fn get_input_device(&self, device_id: Option<&str>) -> Result<Device, DeviceError> {
        if let Some(id) = device_id {
            let parsed: DeviceId = id
                .parse()
                .map_err(|e: DeviceIdError| DeviceError::InvalidDeviceId(e.to_string()))?;

            // Try to find the device by ID
            if let Some(device) = self.inner().device_by_id(&parsed) {
                return Ok(device);
            }

            // Fall back to default device
            tracing::warn!(device.id = id, "input_device_not_found_fallback_to_default");
        }

        // Get default device
        let device = self
            .inner()
            .default_input_device()
            .ok_or(DeviceError::NoDefaultDevice)?;

        Ok(device)
    }

    fn get_output_device(&self, device_id: Option<&str>) -> Result<Device, DeviceError> {
        if let Some(id) = device_id {
            let parsed: DeviceId = id
                .parse()
                .map_err(|e: DeviceIdError| DeviceError::InvalidDeviceId(e.to_string()))?;

            // Try to find the device by ID
            if let Some(device) = self.inner().device_by_id(&parsed) {
                return Ok(device);
            }

            // Fall back to default device
            tracing::warn!(
                device.id = id,
                "output_device_not_found_fallback_to_default"
            );
        }

        // Get default device
        let device = self
            .inner()
            .default_output_device()
            .ok_or(DeviceError::NoDefaultDevice)?;

        Ok(device)
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
/// * `error_callback` - Optional callback for stream errors
#[cfg(not(target_family = "wasm"))]
fn build_input_stream_with_format<T>(
    device: &Device,
    config: &StreamConfig,
    mut input_sender: RingBufferSender,
    input_channels: usize,
    mut error_callback: Option<StreamErrorCallback>,
) -> Result<Stream, DeviceError>
where
    T: Sample<Float = f32> + SizedSample + Send + 'static,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            input_stream_helper(
                &mut input_sender,
                input_channels,
                data.len(),
                data.chunks(input_channels)
                    .map(|frame| frame[0].to_float_sample()),
            );
        },
        move |err| {
            if let Some(callback) = error_callback.as_mut() {
                callback(err);
            } else {
                error!(error = %err, "input_stream_error");
            }
        },
        None,
    )?;

    Ok(stream)
}

/// Builds an input stream for the 64-bit sample formats whose `Float` is `f64`.
///
/// A separate helper is required because cpal's `Sample` trait maps those types
/// through an f64 intermediate; the standard helper funnels through f32.
#[cfg(not(target_family = "wasm"))]
fn build_input_stream_with_format_64<T>(
    device: &Device,
    config: &StreamConfig,
    mut input_sender: RingBufferSender,
    input_channels: usize,
    mut error_callback: Option<StreamErrorCallback>,
) -> Result<Stream, DeviceError>
where
    T: Sample<Float = f64> + SizedSample + Send + 'static,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            input_stream_helper(
                &mut input_sender,
                input_channels,
                data.len(),
                data.chunks(input_channels).map(|frame| {
                    let sample_f64 = frame[0].to_float_sample();
                    sample_f64 as f32
                }),
            );
        },
        move |err| {
            if let Some(callback) = error_callback.as_mut() {
                callback(err);
            } else {
                error!(error = %err, "input_stream_error");
            }
        },
        None,
    )?;

    Ok(stream)
}

#[cfg(not(target_family = "wasm"))]
fn input_stream_helper(
    input_sender: &mut RingBufferSender,
    input_channels: usize,
    data_len: usize,
    sample_iter: impl Iterator<Item = f32>,
) {
    let Ok(chunk) = input_sender
        .producer
        .write_chunk_uninit(data_len / input_channels)
    else {
        return;
    };

    chunk.fill_from_iter(sample_iter);
    input_sender.notify.notify_one();
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
/// * `error_callback` - Optional callback for stream errors
fn build_output_stream_with_format<T>(
    device: &Device,
    config: &StreamConfig,
    mut output_consumer: Consumer<f32>,
    output_channels: usize,
    mut error_callback: Option<StreamErrorCallback>,
) -> Result<SendStream, DeviceError>
where
    T: Sample + SizedSample + FromSample<f32> + Send + 'static,
{
    let mut last_sample = 0_f32;
    let mut was_underrun = true;
    let mut was_missing = false;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &_| {
            debug_assert!(output_channels > 0);

            let total_frames = data.len() / output_channels;
            let available_frames = total_frames.min(output_consumer.slots());

            if was_missing && available_frames == 0 {
                // shortcut for when the fade already occurred & we are playing silence
                data.fill(T::from_sample(0_f32));
                return;
            }

            let mut frames = data.chunks_mut(output_channels);

            let mut pulled = 0;
            if available_frames > 0 {
                match output_consumer.read_chunk(available_frames) {
                    Ok(chunk) => {
                        let mut samples = chunk.into_iter();

                        // Fade-in on recovery: a Hann ramp over the first samples
                        // smooths the discontinuity after an underrun.
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
                            was_underrun = false;
                            was_missing = false;
                        }

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

                for frame in frames {
                    frame.fill(T::from_sample(0.0));
                }

                last_sample = 0.0;
            }
        },
        move |err| {
            if let Some(callback) = error_callback.as_mut() {
                callback(err);
            } else {
                error!(error = %err, "output_stream_error");
            }
        },
        None,
    )?;

    Ok(SendStream(stream))
}
