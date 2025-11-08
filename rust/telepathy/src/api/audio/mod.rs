#[cfg(target_arch = "x86_64")]
use crate::api::utils::i16_to_f32_avx2;
use crate::api::utils::{calculate_rms, i16_to_f32_scalar, resampler_factory, wide_mul};
use atomic_float::AtomicF32;
use kanal::{Receiver, Sender};
use log::{debug, warn};
use nnnoiseless::{DenoiseState, FRAME_SIZE};
use rubato::Resampler;
use sea_codec::ProcessorMessage;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;

/// flutter_rust_bridge:ignore
pub(crate) mod codec;
/// flutter_rust_bridge:ignore
#[cfg(target_os = "ios")]
pub(crate) mod ios;
pub mod player;
/// flutter_rust_bridge:ignore
#[cfg(target_family = "wasm")]
pub(crate) mod web_audio;

use crate::api::error::Error;
use crate::api::telepathy::CHANNEL_SIZE;

/// silences of less than this many frames aren't silence
const MINIMUM_SILENCE_LENGTH: u8 = 40;

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
    rms_sender: Option<Sender<f32>>,
    codec_enabled: bool,
) -> Result<(), Error> {
    // the maximum and minimum values for i16 as f32
    let max_i16_f32 = i16::MAX as f32;
    let min_i16_f32 = i16::MIN as f32;
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

        if position < in_len {
            continue;
        }

        position = 0;

        // sends a silence signal if the input is muted
        if muted.load(Relaxed) {
            sender.send(ProcessorMessage::silence())?;
            continue;
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
        let factor = max_i16_f32 * input_factor.load(Relaxed);

        // rescale the samples to -32768.0 to 32767.0 for rnnoise
        target_buffer.iter_mut().for_each(|x| {
            *x *= factor;
            *x = x.trunc().clamp(min_i16_f32, max_i16_f32);
        });

        if let Some(ref mut denoiser) = denoiser {
            // denoise the frame
            denoiser.process_frame(&mut out_buf, &target_buffer[..len]);
        } else {
            out_buf = target_buffer[..len].try_into()?;
        };

        // calculate the rms
        let rms = calculate_rms(&out_buf);
        // send the rms to the statistics collector
        rms_sender.as_ref().map(|s| s.send(rms));

        // check if the frame is below the rms threshold
        if rms < rms_threshold.load(Relaxed) {
            if silence_length < MINIMUM_SILENCE_LENGTH {
                silence_length += 1; // short silences are ignored
            } else {
                sender.send(ProcessorMessage::silence())?;
                continue;
            }
        } else {
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
    rms_sender: Option<Sender<f32>>,
) -> Result<(), Error> {
    let scale = 1_f32 / i16::MAX as f32;
    let i16_size = size_of::<i16>();

    let mut primary_resampler = resampler_factory(ratio, 1, FRAME_SIZE)?;

    // rubato requires 10 extra spaces in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 * ratio + 10_f64) as usize;

    // the input for the resampler
    let pre_buf = [&mut [0_f32; FRAME_SIZE]];
    // the output for the resampler
    let mut post_buf = [vec![0_f32; post_len]];

    let burst_thresh = CHANNEL_SIZE / 8;

    while let Ok(message) = receiver.recv() {
        // no reason to process the frame
        if sender.is_full() {
            continue;
        }

        let queue_length = sender.len();
        let burst_detected = queue_length > burst_thresh;

        let samples: &[i16] = match &message {
            ProcessorMessage::Silence => {
                if burst_detected {
                    continue; // skip silences during burst
                }

                #[cfg(not(target_family = "wasm"))]
                for _ in 0..FRAME_SIZE {
                    sender.try_send(0_f32)?;
                }

                #[cfg(target_family = "wasm")]
                web_output
                    .lock()
                    .map(|mut data| {
                        if data.len() < CHANNEL_SIZE {
                            data.extend(SILENCE)
                        }
                    })
                    .unwrap();

                continue;
            }
            ProcessorMessage::Data(bytes) => {
                // convert the bytes to 16-bit integers
                unsafe {
                    std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / i16_size)
                }
            }
            ProcessorMessage::Samples(samples) => samples.as_ref(),
        };

        // TODO implement avx512
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("avx2") {
                unsafe { i16_to_f32_avx2(samples, pre_buf[0], scale) }
            } else {
                i16_to_f32_scalar(samples, pre_buf[0], scale)
            }
        }

        #[cfg(not(target_arch = "x86_64"))]
        {
            i16_to_f32_scalar(samples, pre_buf[0], scale)
        }

        // apply the output volume
        wide_mul(pre_buf[0], output_volume.load(Relaxed));

        rms_sender
            .as_ref()
            .map(|s| s.send(calculate_rms(pre_buf[0])));

        if let Some(resampler) = &mut primary_resampler {
            // resample the data
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;

            // send the resampled data to the output stream
            #[cfg(not(target_family = "wasm"))]
            for sample in &post_buf[0][..processed.1] {
                sender.try_send(*sample)?;
            }

            #[cfg(target_family = "wasm")]
            web_output
                .lock()
                .map(|mut data| {
                    if data.len() < CHANNEL_SIZE {
                        data.extend(&post_buf[0][..processed.1])
                    }
                })
                .unwrap();
        } else {
            // if no resampling is needed, send the data to the output stream
            #[cfg(not(target_family = "wasm"))]
            for sample in *pre_buf[0] {
                sender.try_send(sample)?;
            }

            #[cfg(target_family = "wasm")]
            web_output
                .lock()
                .map(|mut data| {
                    if data.len() < CHANNEL_SIZE {
                        data.extend(*pre_buf[0])
                    }
                })
                .unwrap();
        }
    }

    debug!("Output processor ended");
    Ok(())
}
