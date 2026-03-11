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

use std::sync::atomic::Ordering::Relaxed;
use crate::constants::{MINIMUM_SILENCE_LENGTH, VAD_CLOSE_RATE};
use crate::error::Error;
use crate::internal::NETWORK_FRAME;
use crate::internal::buffer_pool::BufferPool;
use crate::internal::processing::*;
use crate::internal::state::{
    InputProcessorState, InputState, OutputProcessorState, SpeechParams, SpeechState,
    VadProcessorState,
};
use crate::internal::traits::{AudioInput, AudioOutput};
use crate::internal::utils::resampler_factory;
use crate::io::traits::{AudioDataSink, AudioDataSource, ClosedOrFailed};
use crate::sea::decoder::SeaDecoder;
use crate::sea::encoder::SeaEncoder;
use log::{debug, warn};
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use rubato::Resampler;
use ten_vad_rs::{TARGET_SAMPLE_RATE, TenVad};

const VAD_FRAME: usize = 256;
const MODEL_BYTES: &[u8] = include_bytes!("../../../../assets/models/ten-vad.onnx");

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
    input: I,
    sink: impl AudioDataSink,
    ratio: f64,
    mut denoiser: Option<Box<DenoiseState>>,
    state: InputProcessorState,
    mut encoder_option: Option<SeaEncoder>,
) -> Result<(), Error> {
    // the maximum value for i16 as f32
    let max_i16_f32 = i16::MAX as f32;

    // the output for rnnoise
    let mut out_buf = [0_f32; FRAME_SIZE];
    // output for 16 bit samples
    let mut int_buffer = [0; FRAME_SIZE];

    // main processor logic
    let callback = |target_buffer: &mut Vec<f32>, len: usize, state: &mut InputProcessorState| {
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
            if state.silence_length < MINIMUM_SILENCE_LENGTH {
                state.silence_length += 1; // short silences are ignored
            } else {
                return Ok(true); // long ones are dropped
            }
        } else {
            state.silence_length = 0;
        }

        // use SIMD-accelerated f32 to i16 conversion
        wide_f32_to_i16(&out_buf, &mut int_buffer);

        // acquire a buffer from the pool
        let mut pooled = BufferPool::acquire(state.buffer_pool());
        if let Some(encoder) = &mut encoder_option {
            // encode frame into the pooled buffer
            encoder.encode_frame(int_buffer, pooled.inner_mut())?;
        } else {
            // copy frame data into the pooled buffer
            pooled.inner_mut().copy_from_slice(unsafe {
                std::slice::from_raw_parts(int_buffer.as_ptr() as *const u8, NETWORK_FRAME)
            });
        }

        // send buffer to network
        match sink.send(pooled) {
            Ok(()) => Ok(true),
            Err(ClosedOrFailed::Closed) => Ok(false),
            // propagate failures
            Err(ClosedOrFailed::Failed(error)) => Err(error)?,
        }
    };

    // run resample and process loop to completion
    base_resampler(input, ratio, state, FRAME_SIZE, callback)?;
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

pub fn vad_processor<I: AudioInput>(
    input: I,
    ratio: f64,
    state: VadProcessorState,
) -> Result<(), Error> {
    // the maximum value for i16 as f32
    let max_i16_f32 = i16::MAX as f32;

    let mut vad = TenVad::new_from_bytes(MODEL_BYTES, TARGET_SAMPLE_RATE)?;
    // input for vad
    let mut vad_input = [0; VAD_FRAME];
    let speech_params = SpeechParams::new(TARGET_SAMPLE_RATE as usize, VAD_FRAME);
    let mut speech_state = SpeechState::default();
    let mut current_threshold = 0_f32;

    // set up vad processing logic
    let callback = |target_buffer: &mut Vec<f32>, len: usize, state: &mut VadProcessorState| {
        // scale the samples to -32768.0 to 32767.0
        wide_float_scaler(
            &mut target_buffer[..len],
            max_i16_f32,
        );
        // use SIMD-accelerated f32 to i16 conversion
        wide_f32_to_i16(&target_buffer[..len], &mut vad_input);

        let score = vad.process_frame(&vad_input)?;
        let gate_open = speech_state.update(&speech_params, score);

        if gate_open {
            current_threshold = 0_f32;
        } else {
            current_threshold += (state.silence_ceiling() - current_threshold) * VAD_CLOSE_RATE;
        }
        state.rms_threshold.store(current_threshold, Relaxed);

        Ok(true)
    };

    // run resample and process loop to completion
    base_resampler(input, ratio, state, VAD_FRAME, callback)?;
    debug!("VAD processor ended");
    Ok(())
}

fn base_resampler<I: AudioInput, S: InputState>(
    mut input: I,
    ratio: f64,
    mut state: S,
    frame_size: usize,
    mut callback: impl FnMut(&mut Vec<f32>, usize, &mut S) -> Result<bool, Error>,
) -> Result<(), Error> {
    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (frame_size as f64 + 10_f64) as usize;
    let in_len = (frame_size as f64 / ratio).ceil() as usize;

    // resampler is Some if resampling is needed
    let mut resampler = resampler_factory(ratio, 1, in_len)?;
    // the input for the resampler
    let mut pre_buf = [vec![0_f32; in_len]];
    // the output for the resampler
    let mut post_buf = [vec![0_f32; post_len]];

    // the position in pre_buf
    let mut position = 0;

    loop {
        let read = input.read_into(&mut pre_buf[0][position..in_len])?;
        if read == 0 {
            debug!("Resampler loop ended (EOF)");
            break;
        }
        position += read;

        if state.is_muted() {
            position = 0;
            state.reset_silence();
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
            (&mut pre_buf[0], frame_size)
        };

        // the first frame may be smaller than frame_size
        if len != frame_size {
            warn!("base_resampler: len != frame_size: {}", len);
            continue;
        }

        // callback returns false to break the loop
        if callback(target_buffer, len, &mut state)? {
            continue;
        } else {
            break;
        }
    }

    Ok(())
}
