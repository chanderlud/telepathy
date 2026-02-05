//! Core audio processor functions.
//!
//! This module contains the main audio processing functions for input
//! and output streams, including resampling, noise suppression, volume
//! control, and silence detection.
//!
//! ## Threading Model
//!
//! Both [`input_processor`] and [`output_processor`] are designed to run in
//! dedicated threads. They perform blocking operations on channels and should
//! not be called from async contexts without spawning a blocking task.
//!
//! ```text
//! ┌─────────────┐    ┌──────────────────┐    ┌─────────────┐    ┌──────────┐
//! │ Audio       │───▶│ input_processor  │───▶│ encoder     │───▶│ Callback │
//! │ Stream      │    │ (thread)         │    │ (thread)    │    │ (thread) │
//! └─────────────┘    └──────────────────┘    └─────────────┘    └──────────┘
//!
//! ┌─────────────┐    ┌──────────────────┐    ┌──────────────────┐    ┌────────┐
//! │ Network     │───▶│ decoder          │───▶│ output_processor │───▶│ Output │
//! │ Receiver    │    │ (thread)         │    │ (thread)         │    │ Stream │
//! └─────────────┘    └──────────────────┘    └──────────────────┘    └────────┘
//! ```
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
use crate::error::AudioError;
use crate::internal::processing::*;
use crate::internal::state::{InputProcessorState, OutputProcessorState};
use crate::internal::traits::{AudioInput, AudioOutput};
use crate::internal::utils::{make_transition_down, make_transition_up, resampler_factory};
use bytes::Bytes;
use kanal::{Receiver, Sender};
use log::{debug, warn};
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use rubato::Resampler;

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
/// When denoise is enabled, the processor upsamples to 48kHz for rnnoise
/// processing and outputs 48kHz frames (no downsample back to device rate).
/// When denoise is disabled, the processor passes through at the device rate.
/// The encoder sample rate must match the processor's output rate accordingly.
///
/// ## Threading
///
/// This function is designed to run in a dedicated thread. It performs blocking
/// reads from the input source and blocking sends to the output channel. The
/// function returns when the input source signals end-of-stream (read returns 0).
///
/// ## Channel Communication
///
/// - Sends [`ProcessorMessage::samples`] when codec is enabled
/// - Sends [`ProcessorMessage::slice`] when codec is disabled
/// - Channel closure causes function to return `Ok(())`
///
/// # Arguments
///
/// * `input` - The audio input source implementing `AudioInput`
/// * `output` - Channel sender for processed audio frames
/// * `sample_rate` - Input sample rate in Hz (device's native rate)
/// * `denoiser` - Optional noise suppression state (requires 48kHz input)
/// * `codec_enabled` - Whether the codec is enabled (affects output format)
/// * `state` - Shared state for volume, mute, and statistics
///
/// # Returns
///
/// Returns `Ok(())` when the input stream ends or channel closes, or an error
/// if processing fails.
pub fn input_processor<I: AudioInput>(
    mut input: I,
    codec_output: Option<Sender<[i16; FRAME_SIZE]>>,
    network_output: Option<Sender<Bytes>>,
    sample_rate: f64,
    mut denoiser: Option<Box<DenoiseState>>,
    state: InputProcessorState,
) -> Result<(), AudioError> {
    // the maximum value for i16 as f32
    let max_i16_f32 = i16::MAX as f32;

    let ratio = if denoiser.is_some() {
        // rnnoise requires a 48kHz sample rate
        48_000_f64 / sample_rate
    } else {
        // do not resample if not using rnnoise
        1_f64
    };

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

    // output for 16 bit samples. the compiler does not recognize that it is used
    #[allow(unused_assignments)]
    let mut int_buffer = [0; FRAME_SIZE];

    // the position in pre_buf
    let mut position = 0;
    // a counter for short silence detection
    let mut silence_length = 0_u8;

    loop {
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
                        &codec_output,
                        &network_output,
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
                    &codec_output,
                    &network_output,
                )?;
            }

            silence_length = 0;
        }

        // cast the f32 samples to i16
        int_buffer = out_buf.map(|x| x as i16);
        // send the frame to the next stage, either codec or network
        send_frame(int_buffer, &codec_output, &network_output)?;
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
/// - Receives [`ProcessorMessage`] containing either raw bytes or i16 samples
/// - Drops frames when output is full (tracks loss via state)
/// - Ignores frames when deafened
///
/// # Arguments
///
/// * `input` - Channel receiver for incoming audio frames
/// * `output` - The audio output destination implementing `AudioOutput`
/// * `ratio` - Resampling ratio (output_rate / input_rate)
/// * `state` - Shared state for volume, deafen, and statistics
///
/// # Returns
///
/// Returns `Ok(())` when the input channel closes, or an error if processing fails.
pub fn output_processor<O: AudioOutput>(
    codec_input: Option<Receiver<[i16; FRAME_SIZE]>>,
    network_input: Option<Receiver<Bytes>>,
    mut output: O,
    ratio: f64,
    state: OutputProcessorState,
) -> Result<(), AudioError> {
    // base scale to convert i16 to f32
    let scale = 1_f32 / i16::MAX as f32;
    // size of i16 in bytes
    let i16_size = size_of::<i16>();
    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 * ratio + 10_f64) as usize;

    // resampler is Some if resampling is needed
    let mut resampler_option = resampler_factory(ratio, 1, FRAME_SIZE)?;
    // the input for the resampler
    let pre_buf = [&mut [0_f32; FRAME_SIZE]];
    // the output for the resampler
    let mut post_vec = Vec::with_capacity(post_len);
    post_vec.resize(post_len, 0_f32);
    let mut post_buf = [post_vec];

    loop {
        let mut network_message = None;
        let mut codec_message = None;
        if let Some(ref receiver) = codec_input {
            if let Ok(message) = receiver.recv() {
                codec_message = Some(message);
            } else {
                break;
            }
        } else if let Some(ref receiver) = network_input {
            if let Ok(message) = receiver.recv() {
                network_message = Some(message);
            } else {
                break;
            }
        }

        if state.is_deafened() {
            continue;
        } else if output.is_full() {
            state.send_loss(FRAME_SIZE);
            continue; // ignore frames while output is full
        }

        let int_samples: &[i16] = match (&network_message, &codec_message) {
            (Some(bytes), _) => {
                // convert the bytes to 16-bit integers
                unsafe {
                    std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / i16_size)
                }
            }
            (_, Some(samples)) => samples,
            (None, None) => {
                warn!("invalid output processor config");
                break;
            }
        };

        if int_samples.len() != FRAME_SIZE {
            warn!("output frame != FRAME_SIZE: {}", int_samples.len());
            continue;
        }

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

fn send_frame(
    frame: [i16; FRAME_SIZE],
    codec_output: &Option<Sender<[i16; FRAME_SIZE]>>,
    network_output: &Option<Sender<Bytes>>,
) -> Result<(), AudioError> {
    if let Some(sender) = codec_output {
        sender.send(frame)?;
    } else if let Some(sender) = network_output {
        let bytes = unsafe {
            std::slice::from_raw_parts(frame.as_ptr() as *const u8, FRAME_SIZE * size_of::<i16>())
        };
        sender.send(Bytes::from(bytes))?;
    }

    Ok(())
}
