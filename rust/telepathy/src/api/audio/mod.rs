use crate::api::audio::processing::*;
use atomic_float::AtomicF32;
use kanal::{Receiver, Sender};
use log::{debug, warn};
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use sea_codec::ProcessorMessage;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;

/// Parameters used for resampling throughout the application
const RESAMPLER_PARAMETERS: SincInterpolationParameters = SincInterpolationParameters {
    sinc_len: 256,
    f_cutoff: 0.95,
    interpolation: SincInterpolationType::Linear,
    oversampling_factor: 256,
    window: WindowFunction::BlackmanHarris2,
};

/// flutter_rust_bridge:ignore
pub(crate) mod codec;
/// flutter_rust_bridge:ignore
#[cfg(target_os = "ios")]
pub(crate) mod ios;
pub mod player;
pub(crate) mod processing;
/// flutter_rust_bridge:ignore
#[cfg(target_family = "wasm")]
pub(crate) mod web_audio;

use crate::api::error::Error;

/// silences of less than this many frames aren't silence
const MINIMUM_SILENCE_LENGTH: u8 = 40;
const TRANSITION_LENGTH: usize = 96;

/// Processes the audio input and sends it to the sending socket
#[allow(clippy::too_many_arguments)]
pub(crate) fn input_processor(
    #[cfg(not(target_family = "wasm"))] receiver: Receiver<f32>,
    #[cfg(target_family = "wasm")] web_input: crate::api::audio::web_audio::WebInput,
    sender: Sender<ProcessorMessage>,
    sample_rate: f64,
    input_factor: Arc<AtomicF32>,
    rms_threshold: Arc<AtomicF32>,
    muted: Arc<AtomicBool>,
    mut denoiser: Option<Box<DenoiseState>>,
    rms_sender: Arc<AtomicF32>,
    codec_enabled: bool,
) -> Result<(), Error> {
    // the maximum value for i16 as f32
    let max_i16_f32 = i16::MAX as f32;
    let i16_size = size_of::<i16>();

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
    let mut pre_buf = [vec![0_f32; in_len]];
    // the output for the resampler
    let mut post_buf = [vec![0_f32; post_len]];
    // the output for rnnoise
    let mut out_buf = [0_f32; FRAME_SIZE];

    // output for 16 bit samples. the compiler does not recognize that it is used
    #[allow(unused_assignments)]
    let mut int_buffer = [0; FRAME_SIZE];

    // the position in pre_buf
    let mut position = 0;
    // a counter user for short silence detection
    let mut silence_length = 0_u8;

    loop {
        #[cfg(not(target_family = "wasm"))]
        {
            if let Ok(sample) = receiver.recv() {
                pre_buf[0][position] = sample;
                position += 1;
            } else {
                break;
            }
        }

        #[cfg(target_family = "wasm")]
        {
            if web_input.finished.load(Relaxed) {
                break;
            }

            if let Ok(mut data) = web_input.pair.0.lock() {
                if data.is_empty() {
                    let c = |i: &mut Vec<f32>| i.is_empty() && !web_input.finished.load(Relaxed);

                    if let Ok(d) = web_input.pair.1.wait_while(data, c) {
                        data = d;
                    } else {
                        break;
                    }
                }

                let data_len = data.len();
                for sample in data.drain(..(in_len - position).min(data_len)) {
                    pre_buf[0][position] = sample;
                    position += 1;
                }
            } else {
                break;
            }
        }

        if muted.load(Relaxed) {
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
            max_i16_f32 * input_factor.load(Relaxed),
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
        rms_sender.fetch_max(rms, Relaxed);

        // check if the frame is below the rms threshold
        if rms < rms_threshold.load(Relaxed) {
            if silence_length < MINIMUM_SILENCE_LENGTH {
                silence_length += 1; // short silences are ignored
            } else if silence_length == MINIMUM_SILENCE_LENGTH {
                // insert frame to cleanly transition down to silence
                sender.send(ProcessorMessage::samples(make_transition_down(
                    TRANSITION_LENGTH,
                    out_buf[0] as i16,
                )))?;
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
                sender.send(ProcessorMessage::samples(make_transition_up(
                    TRANSITION_LENGTH,
                    first_sample,
                )))?;
            }

            silence_length = 0;
        }

        // cast the f32 samples to i16
        int_buffer = out_buf.map(|x| x as i16);

        if codec_enabled {
            sender.send(ProcessorMessage::samples(int_buffer))?;
        } else {
            // convert the i16 samples to bytes
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    int_buffer.as_ptr() as *const u8,
                    int_buffer.len() * i16_size,
                )
            };

            sender.send(ProcessorMessage::slice(bytes))?;
        }
    }

    debug!("Input processor ended");
    Ok(())
}

/// Processes the audio data and sends it to the output stream
pub(crate) fn output_processor(
    receiver: Receiver<ProcessorMessage>,
    #[cfg(target_family = "wasm")] web_output: Arc<wasm_sync::Mutex<Vec<f32>>>,
    #[cfg(not(target_family = "wasm"))] sender: Sender<f32>,
    ratio: f64,
    output_volume: Arc<AtomicF32>,
    rms_sender: Arc<AtomicF32>,
) -> Result<(), Error> {
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
    let mut post_buf = [vec![0_f32; post_len]];

    while let Ok(message) = receiver.recv() {
        #[cfg(not(target_family = "wasm"))]
        if sender.is_full() {
            continue; // no reason to process the frame
        }

        let int_samples: &[i16] = match &message {
            ProcessorMessage::Data(bytes) => {
                // convert the bytes to 16-bit integers
                unsafe {
                    std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / i16_size)
                }
            }
            ProcessorMessage::Samples(samples) => samples.as_ref(),
        };

        if int_samples.len() != FRAME_SIZE {
            warn!("output frame != FRAME_SIZE: {}", int_samples.len());
            continue;
        }

        // convert the i16 samples to f32 & apply the output volume
        wide_i16_to_f32(int_samples, pre_buf[0], scale * output_volume.load(Relaxed));
        // send the rms to the statistics collector
        rms_sender.fetch_max(calculate_rms(pre_buf[0]), Relaxed);
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

        #[cfg(not(target_family = "wasm"))]
        for sample in float_samples {
            sender.try_send(*sample)?;
        }

        #[cfg(target_family = "wasm")]
        web_output
            .lock()
            .map(|mut data| {
                if data.len() < CHANNEL_SIZE {
                    data.extend(float_samples)
                }
            })
            .unwrap();
    }

    debug!("Output processor ended");
    Ok(())
}

/// Produces a resampler if needed
pub(crate) fn resampler_factory(
    ratio: f64,
    channels: usize,
    size: usize,
) -> Result<Option<SincFixedIn<f32>>, Error> {
    if ratio == 1_f64 {
        Ok(None)
    } else {
        // create the resampler if needed
        Ok(Some(SincFixedIn::<f32>::new(
            ratio,
            2_f64,
            RESAMPLER_PARAMETERS,
            size,
            channels,
        )?))
    }
}

fn make_transition_up(length: usize, sample: i16) -> [i16; 480] {
    assert!(length <= 480, "length must be <= 480");

    let mut buf = [0; 480];

    let start = 480 - length;
    let f = sample as i32 / length as i32;

    for i in 0..length {
        // i goes from 0 to length-1
        // Last value (i = length-1) will be: s * length / length = s
        let value = f * (i as i32 + 1);
        buf[start + i] = value as i16;
    }

    buf
}

fn make_transition_down(length: usize, sample: i16) -> [i16; 480] {
    assert!(length <= 480, "length must be <= 480");

    let mut buf = [0; 480];

    let l = length as i32;
    let f = sample as i32 / l;

    // First length items: linear ramp from `sample` down toward 0
    // Remaining (480 - length) items are left as 0.
    for (i, item) in buf.iter_mut().enumerate().take(length) {
        // i = 0       → value ≈ sample
        // i = length - 1   → value ≈ sample * 1/m
        let value = f * (l - i as i32);
        *item = value as i16;
    }

    buf
}
