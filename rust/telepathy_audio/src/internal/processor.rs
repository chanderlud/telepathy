//! Core audio processor functions.
//!
//! This module contains the main audio processing functions for input
//! and output streams, including resampling, noise suppression, volume
//! control, and silence detection.
//!
//! ## Threading Model
//!
//! Both [`input_processor`] and [`output_processor`] are designed to run in
//! dedicated threads. They perform blocking operations on ring buffers/channels
//! and should not be called from async contexts without spawning a blocking task.
//!
//! ## Channel Closure Behavior
//!
//! When the input channel closes (sender dropped), both processors return
//! `Ok(())` gracefully. This is the normal shutdown mechanism triggered when
//! the audio handle is dropped.
//!
//! ## Typical Usage Pattern
//!
//! ```rust,no_run
//! use std::thread;
//! // Spawn processor in dedicated thread
//! // let handle = thread::spawn(move || {
//! //     input_processor(input, output, sample_rate, denoiser, codec, state)
//! // });
//! // ...
//! // handle.join().unwrap();
//! ```

use crate::constants::{MINIMUM_SILENCE_LENGTH, TRANSITION_LENGTH};
use crate::error::Error;
use crate::internal::NETWORK_FRAME;
use crate::internal::buffer_pool::BufferPool;
use crate::internal::processing::*;
use crate::internal::state::{InputProcessorState, OutputProcessorState};
use crate::internal::traits::{AudioInput, AudioOutput};
use crate::internal::utils::{make_transition_down, make_transition_up, resampler_factory};
use crate::io::traits::{AudioDataSink, AudioDataSource, ClosedOrFailed};
use crate::sea::decoder::SeaDecoder;
use crate::sea::encoder::SeaEncoder;
use log::{debug, warn};
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use rubato::Resampler;
use std::sync::Arc;

/// Processes audio input and sends it to the output channel.
///
/// This function handles the complete input processing pipeline:
/// - Reading from the audio input source
/// - Resampling to 48kHz if noise suppression is enabled
/// - Applying input volume adjustment
/// - Noise suppression (if enabled)
/// - RMS calculation and threshold detection
/// - Silence transition handling
/// - Converting to i16 samples for network transmission
///
/// ## Sample Rate Behavior
///
/// The processor's output sample rate is determined by the `ratio` parameter,
/// which is calculated by the caller (typically in `build_common`). The ratio
/// determines the resampling behavior:
///
/// - **ratio = 1.0**: No resampling (pass-through)
/// - **ratio > 1.0**: Upsampling (e.g., 44.1kHz → 48kHz when denoise enabled)
/// - **ratio < 1.0**: Downsampling
///
/// The caller determines the appropriate ratio based on:
/// - **Denoise enabled**: ratio = 48000 / input_rate (always upsample to 48kHz)
/// - **Custom output_sample_rate**: ratio = output_rate / input_rate
/// - **Neither**: ratio = 1.0 (pass-through at device rate)
///
/// This design moves ratio calculation to the caller for improved testability
/// and flexibility, allowing custom output rates when denoising is disabled.
///
/// ## Threading
///
/// This function is designed to run in a dedicated thread. It performs blocking
/// reads from the input source and blocking sends to the output channel. The
/// function returns when the input source signals end-of-stream (read returns 0).
///
/// ## Channel Communication
///
/// - Sends `Bytes` (i16 samples as bytes) to the output channel
/// - Channel closure causes function to return `Ok(())`
///
/// # Arguments
///
/// * `input` - The audio input source implementing `AudioInput`
/// * `output` - Channel for sending processed audio frames (as `Bytes`)
/// * `ratio` - Resampling ratio (output_rate / input_rate). Calculated by caller
///   based on denoise setting and optional custom output sample rate.
/// * `denoiser` - Optional noise suppression state (requires 48kHz input)
/// * `state` - Shared state for volume, mute, and statistics
/// * `encoder_option` - Optional SEA encoder for codec encoding
///
/// # Returns
///
/// Returns `Ok(())` when the input stream ends or channel closes, or an error
/// if processing fails.
pub fn input_processor<I: AudioInput>(
    mut input: I,
    sink: impl AudioDataSink,
    ratio: f64,
    mut denoiser: Option<Box<DenoiseState>>,
    state: InputProcessorState,
    mut encoder_option: Option<SeaEncoder>,
) -> Result<(), Error> {
    // the maximum value for i16 as f32
    let max_i16_f32 = i16::MAX as f32;

    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 + 10_f64) as usize;
    let in_len = (FRAME_SIZE as f64 / ratio).ceil() as usize;

    // resampler is Some if resampling is needed
    let mut resampler = resampler_factory(ratio, 1, in_len)?;
    // the input for the resampler
    let mut pre_vec = Vec::with_capacity(in_len);
    pre_vec.resize(in_len, 0_f32);
    let mut pre_buf = [pre_vec];
    // the output for the resampler
    let mut post_vec = Vec::with_capacity(post_len);
    post_vec.resize(post_len, 0_f32);
    let mut post_buf = [post_vec];
    // the output for rnnoise
    let mut out_buf = [0_f32; FRAME_SIZE];
    // output for 16 bit samples
    let mut int_buffer = [0; FRAME_SIZE];

    // the position in pre_buf
    let mut position = 0;
    // a counter for short silence detection
    let mut silence_length = 0_u8;
    // switches to false when the sinc closes
    let mut sink_open = true;

    while sink_open {
        let read = input.read_into(&mut pre_buf[0][position..in_len])?;
        if read == 0 {
            debug!("Input processor ended (EOF)");
            break;
        }
        position += read;

        if state.is_muted() {
            position = 0;
            continue;
        } else if position < in_len {
            continue;
        } else {
            position = 0;
        }

        let (target_buffer, len) = if let Some(resampler) = &mut resampler {
            // resample the data
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;
            (&mut post_buf[0], processed.1)
        } else {
            (&mut pre_buf[0], FRAME_SIZE)
        };

        // the first frame may be smaller than FRAME_SIZE
        if len != FRAME_SIZE {
            warn!("input_processor: len != FRAME_SIZE: {}", len);
            continue;
        }

        // apply the input volume & scale the samples to -32768.0 to 32767.0
        wide_float_scaler(
            &mut target_buffer[..len],
            max_i16_f32 * state.input_volume(),
        );

        if let Some(ref mut denoiser) = denoiser {
            // denoise the frame
            denoiser.process_frame(&mut out_buf, &target_buffer[..len]);
        } else {
            out_buf = target_buffer[..len].try_into()?;
        };

        // calculate the rms
        let rms = calculate_rms(&out_buf);
        // send the rms to the statistics collector
        state.send_rms(rms);

        // check if the frame is below the rms threshold
        if rms < state.rms_threshold() {
            if silence_length < MINIMUM_SILENCE_LENGTH {
                silence_length += 1; // short silences are ignored
            } else if silence_length == MINIMUM_SILENCE_LENGTH {
                let last_sample = out_buf[0] as i16;
                if last_sample > 0 {
                    // insert frame to cleanly transition down to silence
                    send_frame(
                        make_transition_down(TRANSITION_LENGTH, last_sample),
                        &sink,
                        state.buffer_pool(),
                        &mut encoder_option,
                        &mut sink_open,
                    )?;
                }
                // don't transition down again
                silence_length += 1;
                continue;
            } else {
                continue;
            }
        } else {
            let first_sample = out_buf[0] as i16;
            if silence_length > 0 && first_sample > 0 {
                // insert frame to transition up from silence to the audio
                send_frame(
                    make_transition_up(TRANSITION_LENGTH, first_sample),
                    &sink,
                    state.buffer_pool(),
                    &mut encoder_option,
                    &mut sink_open,
                )?;
            }

            silence_length = 0;
        }

        // Use SIMD-accelerated f32 to i16 conversion
        wide_f32_to_i16(&out_buf, &mut int_buffer);
        // send the frame to the next stage, either codec or network
        send_frame(
            int_buffer,
            &sink,
            state.buffer_pool(),
            &mut encoder_option,
            &mut sink_open,
        )?;
    }

    debug!("Input processor ended");
    Ok(())
}

/// Processes audio output from the network and sends it to the output device.
///
/// This function handles the complete output processing pipeline:
/// - Receiving audio frames from the network
/// - Converting from i16 to f32 samples
/// - Applying output volume adjustment
/// - RMS calculation for statistics
/// - Resampling to the output device sample rate
/// - Handling deafen state and buffer overflow
///
/// ## Threading
///
/// This function is designed to run in a dedicated thread. It performs blocking
/// receives from the input channel and writes to the output sink. The function
/// returns when the input channel closes (sender dropped).
///
/// ## Channel Communication
///
/// - Receives `Bytes` (i16 samples as bytes) from the input channel
/// - Drops frames when output is full (tracks loss via state)
/// - Ignores frames when deafened
///
/// # Arguments
///
/// * `input` - Channel for receiving audio frames (as `Bytes`)
/// * `output` - The audio output destination implementing `AudioOutput`
/// * `ratio` - Resampling ratio (output_rate / input_rate)
/// * `state` - Shared state for volume, deafen, and statistics
///
/// # Returns
///
/// Returns `Ok(())` when the input channel closes, or an error if processing fails.
pub fn output_processor<O: AudioOutput>(
    source: impl AudioDataSource,
    mut output: O,
    ratio: f64,
    state: OutputProcessorState,
    mut decoder_option: Option<SeaDecoder>,
) -> Result<(), Error> {
    // base scale to convert i16 to f32
    let scale = 1_f32 / i16::MAX as f32;
    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 * ratio + 10_f64) as usize;

    let mut decoded_buf = [0_i16; FRAME_SIZE];
    // resampler is Some if resampling is needed
    let mut resampler_option = resampler_factory(ratio, 1, FRAME_SIZE)?;
    // the input for the resampler
    let pre_buf = [&mut [0_f32; FRAME_SIZE]];
    // the output for the resampler
    let mut post_vec = Vec::with_capacity(post_len);
    post_vec.resize(post_len, 0_f32);
    let mut post_buf = [post_vec];

    loop {
        let buffer = match source.recv() {
            Ok(b) => b,
            Err(ClosedOrFailed::Closed) => break,
            Err(ClosedOrFailed::Failed(error)) => Err(error)?,
        };

        let int_samples = if state.is_deafened() {
            continue;
        } else if output.is_full() {
            state.send_loss(FRAME_SIZE);
            continue; // ignore frames while output is full
        } else if let Some(decoder) = &mut decoder_option {
            decoder.decode_frame(&buffer, &mut decoded_buf)?;
            &decoded_buf
        } else if buffer.len() != NETWORK_FRAME {
            warn!("output frame != FRAME_SIZE: {}", buffer.len());
            continue;
        } else {
            unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const i16, FRAME_SIZE) }
        };

        // convert the i16 samples to f32 & apply the output volume
        wide_i16_to_f32(int_samples, pre_buf[0], scale * state.output_volume());
        // send the rms to the statistics collector
        state.send_rms(calculate_rms(pre_buf[0]));
        // get finalized samples
        let float_samples = if let Some(resampler) = &mut resampler_option {
            // resample the data
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;
            // send the resampled data to the output stream
            &post_buf[0][..processed.1]
        } else {
            // if no resampling is needed, send the data to the output stream
            &*pre_buf[0]
        };

        let lost = output.write_samples(float_samples)?;
        if lost > 0 {
            state.send_loss(lost);
        }
    }

    debug!("Output processor ended");
    Ok(())
}

/// Sends an audio frame to the output channel using a pooled buffer.
///
/// This helper function acquires a buffer from the pool, writes the i16 sample
/// array into it, and sends it through the output channel.
/// Using pooled buffers significantly reduces allocation pressure compared
/// to creating new `Bytes` buffers for each frame (~100 frames/second at 48kHz).
///
/// When the receiver drops the `PooledBuffer`, it will attempt to return the
/// underlying buffer to the pool (if not shared/cloned).
///
/// # Arguments
///
/// * `frame` - The audio frame to send (480 i16 samples)
/// * `output` - Sender for the output channel
/// * `pool` - Buffer pool for acquiring reusable buffers
///
/// # Returns
///
/// Returns `Ok(())` if the frame was sent successfully, or an error if the
/// channel send operation fails.
///
/// # Safety
///
/// Uses `unsafe` to reinterpret the i16 array as a byte slice. This is safe
/// because i16 has a well-defined memory layout and the slice lifetime is
/// constrained to the function scope.
#[inline]
fn send_frame(
    frame: [i16; FRAME_SIZE],
    sink: &impl AudioDataSink,
    pool: &Arc<BufferPool>,
    encoder_option: &mut Option<SeaEncoder>,
    sink_open: &mut bool,
) -> Result<(), Error> {
    // Acquire a buffer from the pool
    let mut pooled = BufferPool::acquire(pool);
    if let Some(encoder) = encoder_option {
        // Encode frame into the pooled buffer
        encoder.encode_frame(frame, pooled.inner_mut())?;
    } else {
        // Copy frame data into the pooled buffer
        pooled.inner_mut().copy_from_slice(unsafe {
            std::slice::from_raw_parts(frame.as_ptr() as *const u8, NETWORK_FRAME)
        });
    }
    // Send buffer to encoder or network
    match sink.send(pooled) {
        Ok(()) => Ok(()),
        Err(ClosedOrFailed::Closed) => {
            // break the processor loop on close
            *sink_open = false;
            Ok(())
        }
        // propagate failures
        Err(ClosedOrFailed::Failed(error)) => Err(error.into()),
    }
}
