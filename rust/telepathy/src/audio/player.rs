use crate::audio::processing::wide_mul;
use crate::audio::resampler_factory;
use crate::error::{DartError, Error, ErrorKind};
use crate::frb_generated::FLUTTER_RUST_BRIDGE_HANDLER;
use crate::telepathy::DeviceName;
use crate::telepathy::utils::{SendStream, db_to_multiplier, get_output_device};
use atomic_float::AtomicF32;
use core::time::Duration;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Host, SampleFormat};
use flutter_rust_bridge::JoinHandle;
use flutter_rust_bridge::spawn;
use flutter_rust_bridge::{frb, spawn_blocking_with};
use kanal::{Receiver, unbounded};
#[cfg(not(target_family = "wasm"))]
use kanal::{Sender, bounded};
use log::error;
use nnnoiseless::FRAME_SIZE;
use rubato::Resampler;
use sea_codec::ProcessorMessage;
use sea_codec::decoder::SeaDecoder;
use sea_codec::encoder::{EncoderSettings, SeaEncoder};
use std::mem;
#[cfg(not(target_family = "wasm"))]
use std::path::Path;
use std::sync::Arc;
#[cfg(target_family = "wasm")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
#[cfg(not(target_family = "wasm"))]
use tokio::fs::File;
#[cfg(not(target_family = "wasm"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::select;
use tokio::sync::Notify;
#[cfg(not(target_family = "wasm"))]
use tokio::time::sleep;
use tokio_util::bytes::Bytes;
#[cfg(target_family = "wasm")]
use wasm_sync::{Condvar, Mutex};
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::sleep;

type DecodedReceiver = (Receiver<ProcessorMessage>, usize);

#[frb(opaque)]
pub struct SoundPlayer {
    /// A multiplier applied to sound effects
    output_volume: Arc<AtomicF32>,

    /// The output device
    output_device: DeviceName,

    /// The cpal host
    host: Arc<Host>,
}

impl SoundPlayer {
    #[frb(sync)]
    pub fn new(output_volume: f32) -> SoundPlayer {
        let host = cpal::default_host();

        Self {
            output_volume: Arc::new(AtomicF32::new(db_to_multiplier(output_volume))),
            output_device: Default::default(),
            host: Arc::new(host),
        }
    }

    #[frb(sync)]
    pub fn host(&self) -> Arc<Host> {
        self.host.clone()
    }

    /// Public play function
    pub async fn play(&mut self, bytes: Vec<u8>) -> SoundHandle {
        let cancel = Arc::new(Notify::new());
        let cancel_clone = cancel.clone();

        let output_volume = self.output_volume.clone();
        let host = self.host.clone();
        let output_device = self.output_device.clone();

        let handle = spawn(async move {
            if let Err(error) =
                play_sound(bytes, cancel_clone, host, output_volume, output_device).await
            {
                error!("Error playing sound: {:?}", error)
            }
        });

        SoundHandle {
            cancel,
            _handle: handle,
        }
    }

    #[frb(sync)]
    pub fn update_output_volume(&self, volume: f32) {
        self.output_volume.store(db_to_multiplier(volume), Relaxed);
    }

    pub async fn update_output_device(&self, name: Option<String>) {
        *self.output_device.lock().await = name;
    }
}

#[frb(opaque)]
pub struct SoundHandle {
    cancel: Arc<Notify>,
    _handle: JoinHandle<()>,
}

impl SoundHandle {
    #[frb(sync)]
    pub fn cancel(&self) {
        self.cancel.notify_one();
    }
}

#[cfg(target_family = "wasm")]
#[derive(Default)]
struct AudioBuffer {
    buffer: Mutex<Vec<f32>>,
    canceled: AtomicBool,
    condvar: Condvar,
}

struct AudioHeader {
    channels: u32,
    sample_rate: u32,
    sample_format: SampleFormat,
}

impl TryFrom<&[u8]> for AudioHeader {
    type Error = ();

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() < 44 {
            return Err(());
        }

        let bits_per_sample = u16::from_le_bytes([value[34], value[35]]);
        let audio_format = u16::from_le_bytes([value[20], value[21]]);

        let sample_format = match (audio_format, bits_per_sample) {
            (1, 8) => SampleFormat::U8,
            (1, 16) => SampleFormat::I16,
            (1, 32) => SampleFormat::I32,
            (3, 32) => SampleFormat::F32,
            (3, 64) => SampleFormat::F64,
            _ => return Err(()),
        };

        Ok(Self {
            channels: u16::from_le_bytes([value[22], value[23]]) as u32,
            sample_rate: u32::from_le_bytes([value[24], value[25], value[26], value[27]]),
            sample_format,
        })
    }
}

/// loads a ringtone into a sea file for future use in the backend
#[cfg(not(target_family = "wasm"))]
pub async fn load_ringtone(path: String) -> Result<(), DartError> {
    let path = Path::new(&path);

    let mut input_file = File::open(path).await.map_err(|error| error.to_string())?;
    let mut output_file = File::create("ringtone.sea")
        .await
        .map_err(|error| error.to_string())?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("wav") => {
            let mut wav_bytes = Vec::new();
            input_file
                .read_to_end(&mut wav_bytes)
                .await
                .map_err(|error| error.to_string())?;
            let sea_bytes = wav_to_sea(&wav_bytes, 5_f32)
                .await
                .map_err(|error| error.to_string())?;
            output_file
                .write_all(&sea_bytes)
                .await
                .map_err(|error| error.to_string())?;
        }
        _ => return Err("Unsupported file type".to_string().into()),
    }

    Ok(())
}

#[cfg(target_family = "wasm")]
pub async fn load_ringtone(path: String) -> Result<(), DartError> {
    Ok(())
}

/// Internal play sound function
async fn play_sound(
    bytes: Vec<u8>,
    cancel: Arc<Notify>,
    host: Arc<Host>,
    output_volume: Arc<AtomicF32>,
    output_device: DeviceName,
) -> Result<(), Error> {
    if bytes.len() < 44 {
        return Err(ErrorKind::InvalidWav.into());
    }

    // get the output device & config
    let output_device = get_output_device(&output_device, &host).await?;
    let output_config = output_device.default_output_config()?;

    // parse the input spec
    let spec_result = AudioHeader::try_from(&bytes[0..44]);
    let is_valid_wav = spec_result.is_ok();
    let mut spec = spec_result.unwrap_or(AudioHeader {
        channels: 0,
        sample_rate: 0,
        sample_format: SampleFormat::I16,
    });
    let mut samples = None;
    let mut decoder_handle = None;
    // if not valid wav, try to handle as SEA codec file
    if !is_valid_wav {
        let (input_sender, input_receiver) = unbounded();
        let (output_sender, output_receiver) = unbounded();
        let input_sender = input_sender.to_async();

        input_sender
            .send(ProcessorMessage::Data(Bytes::copy_from_slice(&bytes[..14])))
            .await?;

        let decoder_future = spawn_blocking_with(
            move || SeaDecoder::new(input_receiver, output_sender, None).unwrap(),
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        );

        let mut decoder = decoder_future.await?;
        let header = decoder.get_header();
        let sample_count =
            (bytes.len() - 14) / header.chunk_size as usize * FRAME_SIZE / header.channels as usize;
        spec.sample_rate = header.sample_rate;
        spec.channels = header.channels as u32;

        decoder_handle = Some(spawn_blocking_with(
            move || while decoder.decode_frame().is_ok() {},
            FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
        ));

        for chunk in bytes[14..].chunks(header.chunk_size as usize) {
            input_sender
                .send(ProcessorMessage::Data(Bytes::copy_from_slice(chunk)))
                .await?;
        }

        samples = Some((output_receiver, sample_count));
    }

    // the resampling ratio used by the processor
    let ratio = output_config.sample_rate().0 as f64 / spec.sample_rate as f64;

    // sends samples from the processor to the output stream
    #[cfg(not(target_family = "wasm"))]
    let (processed_sender, processed_receiver) = bounded::<Vec<f32>>(1_000);

    // handles synchronization between the processor and output stream
    #[cfg(target_family = "wasm")]
    let processed_sender: Arc<AudioBuffer> = Default::default();
    #[cfg(target_family = "wasm")]
    let audio_buffer = processed_sender.clone();

    // on web the output notifies this thread when playback has finished
    #[cfg(target_family = "wasm")]
    let output_finished = Arc::new(Notify::new());
    #[cfg(target_family = "wasm")]
    let output_finished_clone = output_finished.clone();

    // used to chunk the output buffer correctly
    let output_channels = output_config.channels() as usize;
    // keep track of the last samples played
    let mut last_samples = vec![0_f32; output_channels];
    // a counter used for fading out the last samples when the sound is canceled
    let mut i = 0;
    // used to provide a fade to 0 when the sound is canceled
    let f32_sample_rate = output_config.sample_rate().0 as f32;

    let output_stream = SendStream {
        stream: output_device.build_output_stream(
            &output_config.into(),
            move |output: &mut [f32], _: &_| {
                #[cfg(target_family = "wasm")]
                let mut data = {
                    audio_buffer.condvar.notify_one();
                    audio_buffer.buffer.lock().unwrap()
                };

                for frame in output.chunks_mut(output_channels) {
                    #[cfg(not(target_family = "wasm"))]
                    let samples_result = processed_receiver.recv();
                    #[cfg(not(target_family = "wasm"))]
                    let canceled = samples_result.is_err();

                    #[cfg(target_family = "wasm")]
                    let canceled = {
                        let mut canceled = audio_buffer.canceled.load(Relaxed);

                        if !canceled && data.is_empty() {
                            output_finished_clone.notify_one();
                            audio_buffer.canceled.store(true, Relaxed);
                            canceled = true;
                        }

                        canceled
                    };

                    if canceled {
                        // fade each sample
                        for sample in &mut last_samples {
                            *sample *= (1_f32 - i as f32 / f32_sample_rate).max(0_f32);
                        }

                        // play the samples
                        frame.copy_from_slice(&last_samples);
                        i += 1; // advance the counter
                    } else {
                        // this unwrap is safe as the result was already checked for is_err
                        #[cfg(not(target_family = "wasm"))]
                        let samples = samples_result.unwrap();

                        #[cfg(target_family = "wasm")]
                        let samples: Vec<f32> = data.drain(..output_channels).collect();

                        // play the samples
                        frame.copy_from_slice(&samples);
                        last_samples = samples;
                    }
                }
            },
            move |err| {
                error!("Error in player stream: {}", err);
            },
            None,
        )?,
    };

    // the sender used by the processor
    let sender = processed_sender.clone();

    let mut processor_join: Option<_> = None;
    let mut processor_future = spawn_blocking_with(
        move || {
            processor(
                (samples.is_none().then_some(bytes), samples),
                spec.sample_format,
                spec,
                output_volume,
                sender,
                output_channels,
                ratio,
            )
        },
        FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
    );

    output_stream.stream.play()?; // play the stream

    select! {
        _ = cancel.notified() => {
            // this causes the stream to begin fading out
            #[cfg(not(target_family = "wasm"))]
            processed_sender.close()?;
            #[cfg(target_family = "wasm")]
            processed_sender.canceled.store(true, Relaxed);

            // we sleep to prevent the stream from being closed while fading
            sleep(Duration::from_secs(1)).await;
        }
        result = &mut processor_future => {
            // keep track of the return value
            processor_join = Some(result);
            #[cfg(target_family = "wasm")]
            {
                // on web, we need to wait for the output to finish playing before continuing
                output_finished.notified().await;
                // delay allows for fade out
                sleep(Duration::from_secs(1)).await;
            }
        }
    }

    // join the decoder task if there was one
    if let Some(handle) = decoder_handle {
        handle.await?;
    }

    // join the processor task
    let result = match processor_join {
        Some(result) => result,
        None => processor_future.await,
    };

    result??;
    Ok(())
}

/// Processes the WAV data
fn processor(
    input: (Option<Vec<u8>>, Option<DecodedReceiver>),
    sample_format: SampleFormat,
    spec: AudioHeader,
    output_volume: Arc<AtomicF32>,
    #[cfg(not(target_family = "wasm"))] processed_sender: Sender<Vec<f32>>,
    #[cfg(target_family = "wasm")] audio_buffer: Arc<AudioBuffer>,
    output_channels: usize,
    ratio: f64,
) -> Result<(), Error> {
    let (bytes, samples) = input;
    let sample_size = sample_format.sample_size();
    let channels_usize = spec.channels as usize;

    // the number of samples in the file
    let sample_count = bytes
        .as_ref()
        .map(|b| (b.len() - 44) / sample_size / channels_usize)
        .or_else(|| samples.as_ref().map(|(_, l)| *l))
        .unwrap_or_default();
    // the number of audio samples which will be played
    let audio_len = (sample_count as f64 * ratio) as f32;
    let mut position = 0_f32; // the playback position

    // constants used for fading in and out
    let fade_basis = sample_count as f32 / 100_f32;
    let fade_out = fade_basis;
    let fade_in = audio_len - fade_basis;

    // rubato requires 10 extra bytes in the output buffer as a safety margin
    let post_len = (FRAME_SIZE as f64 / spec.channels as f64 * ratio + 10.0) as usize;

    // the output for the resampler
    let mut post_buf = vec![vec![0_f32; post_len]; channels_usize];
    // the input for the resampler
    let mut pre_buf = vec![vec![0_f32; FRAME_SIZE / spec.channels as usize]; channels_usize];
    // groups of samples ready to be sent to the output
    let mut out_buf = Vec::with_capacity(output_channels);

    let mut resampler =
        resampler_factory(ratio, channels_usize, FRAME_SIZE / spec.channels as usize)?;
    let output_volume = output_volume.load(Relaxed);

    let mut byte_chunks = bytes
        .as_ref()
        .map(|bytes| bytes[44..].chunks(FRAME_SIZE * sample_size));

    loop {
        match (byte_chunks.as_mut(), samples.as_ref()) {
            (None, Some((samples, _))) => {
                let samples = if let Ok(ProcessorMessage::Samples(samples)) = samples.recv() {
                    samples
                } else {
                    break;
                };

                let scale = 1_f32 / i16::MAX as f32;

                for (i, sample) in samples.chunks(channels_usize).enumerate() {
                    for (j, channel) in sample.iter().enumerate() {
                        let sample = *channel as f32 * scale;
                        pre_buf[j][i] = sample;
                    }
                }
            }
            (Some(chunks), None) => {
                let chunk = if let Some(chunk) = chunks.next() {
                    chunk
                } else {
                    break;
                };

                match sample_format {
                    SampleFormat::U8 => {
                        let scale = 1_f32 / u8::MAX as f32;

                        for (i, sample) in chunk.chunks(channels_usize).enumerate() {
                            for (j, channel) in sample.iter().enumerate() {
                                let sample = *channel as f32 * scale;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::I16 => {
                        let scale = 1_f32 / i16::MAX as f32;

                        for (i, sample) in chunk.chunks(2 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(2).enumerate() {
                                let sample = i16::from_le_bytes(channel.try_into()?) as f32 * scale;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::I32 => {
                        let scale = 1_f32 / i32::MAX as f32;

                        for (i, sample) in chunk.chunks(4 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(4).enumerate() {
                                let sample = i32::from_le_bytes(channel.try_into()?) as f32 * scale;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::F32 => {
                        for (i, sample) in chunk.chunks(4 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(4).enumerate() {
                                let sample = f32::from_le_bytes(channel.try_into()?);
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    SampleFormat::F64 => {
                        for (i, sample) in chunk.chunks(8 * channels_usize).enumerate() {
                            for (j, channel) in sample.chunks(8).enumerate() {
                                let sample = f64::from_le_bytes(channel.try_into()?) as f32;
                                pre_buf[j][i] = sample;
                            }
                        }
                    }
                    _ => return Err(ErrorKind::UnknownSampleFormat.into()),
                }
            }
            _ => break,
        }

        for channel in pre_buf.iter_mut() {
            wide_mul(channel, output_volume);
        }

        let (target_buffer, len) = if let Some(resampler) = &mut resampler {
            let processed = resampler.process_into_buffer(&pre_buf, &mut post_buf, None)?;
            (&mut post_buf, processed.1)
        } else {
            (&mut pre_buf, FRAME_SIZE / spec.channels as usize)
        };

        // im not sure how to refactor this loop to pass the needless range test
        #[allow(clippy::needless_range_loop)]
        for i in 0..len {
            let multiplier = if position < audio_len {
                let delta = audio_len - position;

                if delta < fade_out {
                    // calculate fade out multiplier
                    delta / fade_basis
                } else if delta > fade_in {
                    // calculate fade in multiplier
                    position / fade_basis
                } else {
                    1_f32 // no fade in or out
                }
            } else {
                0_f32 // the calculated audio_len is too short
            };

            position += 1_f32; // advance the position

            for j in 0..output_channels {
                // this handles when there are more output channels than input channels
                let sample = if j >= channels_usize {
                    target_buffer[0][i]
                } else {
                    target_buffer[j][i]
                };

                out_buf.push(sample * multiplier);
            }

            // send samples for each channel to the output
            #[cfg(not(target_family = "wasm"))]
            {
                let buffer = mem::take(&mut out_buf);
                processed_sender.send(buffer)?;
            }

            #[cfg(target_family = "wasm")]
            {
                if audio_buffer.canceled.load(Relaxed) {
                    break;
                }

                // enforce bounding on the buffer
                if let Ok(data) = audio_buffer.buffer.lock() {
                    drop(
                        audio_buffer
                            .condvar
                            .wait_while(data, |d| d.len() > (10_000 * channels_usize)),
                    );
                }

                if let Ok(mut data) = audio_buffer.buffer.lock() {
                    let mut buffer = mem::take(&mut out_buf);
                    data.append(&mut buffer);
                } else {
                    error!("failed to lock audio buffer");
                    break;
                }
            }
        }
    }

    #[cfg(not(target_family = "wasm"))]
    processed_sender.close()?;

    Ok(())
}

/// accepts the bytes of a wav file, returns the bytes of a sea file
/// encoding is performed in a blocking thread
async fn wav_to_sea(bytes: &[u8], residual_bits: f32) -> Result<Vec<u8>, Error> {
    if bytes.len() < 44 {
        return Err(ErrorKind::InvalidWav.into());
    }

    let spec = AudioHeader::try_from(&bytes[0..44]).map_err(|_| ErrorKind::InvalidWav)?;
    let channels = spec.channels;
    let sample_rate = spec.sample_rate;

    let sample_size = match spec.sample_format {
        SampleFormat::U8 => 1,
        SampleFormat::I16 => 2,
        SampleFormat::I32 | SampleFormat::F32 => 4,
        SampleFormat::F64 => 8,
        _ => 1,
    };

    let (input_sender, input_receiver) = unbounded();
    let (output_sender, output_receiver) = unbounded();
    let input_sender = input_sender.to_async();
    let output_receiver = output_receiver.to_async();

    spawn_blocking_with(
        move || {
            let settings = EncoderSettings {
                frames_per_chunk: FRAME_SIZE as u16 / channels as u16,
                vbr: true,
                residual_bits,
                ..Default::default()
            };

            let mut encoder = SeaEncoder::new(
                channels as u8,
                sample_rate,
                settings,
                input_receiver,
                output_sender,
            )
            .unwrap();
            while encoder.encode_frame().is_ok() {}
            encoder
        },
        FLUTTER_RUST_BRIDGE_HANDLER.thread_pool(),
    );

    let mut buffer = [0; FRAME_SIZE];

    for chunk in bytes[44..].chunks(FRAME_SIZE * sample_size) {
        let written = match spec.sample_format {
            SampleFormat::U8 => {
                for (j, sample) in chunk.iter().enumerate() {
                    buffer[j] = ((*sample as i16) - 128) << 8;
                }
                chunk.len()
            }
            SampleFormat::I16 => {
                for (i, sample_bytes) in chunk.chunks_exact(2).enumerate() {
                    buffer[i] = i16::from_le_bytes([sample_bytes[0], sample_bytes[1]]);
                }
                chunk.len() / 2
            }
            _ => break,
        };

        if written < FRAME_SIZE {
            buffer[written..].fill(0);
        }

        input_sender.send(ProcessorMessage::samples(buffer)).await?;
    }

    drop(input_sender);
    let mut data: Vec<u8> = Vec::new();

    while let Ok(ProcessorMessage::Data(bytes)) = output_receiver.recv().await {
        data.extend(bytes.iter());
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use crate::audio::player::wav_to_sea;
    use fast_log::Config;
    use log::LevelFilter::Debug;
    use log::info;
    use std::time::Instant;
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::sleep;

    #[ignore]
    #[tokio::test]
    async fn test_player() {
        _ = fast_log::init(Config::new().file("tests-player.log").level(Debug));

        let mut sea_bytes = Vec::new();
        let mut sea_file = File::open("../../assets/sounds/mute.sea").await.unwrap();
        sea_file.read_to_end(&mut sea_bytes).await.unwrap();

        let mut player = super::SoundPlayer::new(0.1);
        let handle = player.play(sea_bytes).await;

        sleep(std::time::Duration::from_secs(2)).await;
        handle.cancel();
        sleep(std::time::Duration::from_secs(1)).await;
    }

    #[ignore]
    #[tokio::test]
    async fn test_wav_to_sea() {
        _ = fast_log::init(Config::new().file("tests-wav-sea.log").level(Debug));

        let wav_files = vec![
            "../../assets/wav-sounds/call_ended.wav",
            "../../assets/wav-sounds/connected.wav",
            "../../assets/wav-sounds/deafen.wav",
            "../../assets/wav-sounds/disconnected.wav",
            "../../assets/wav-sounds/incoming.wav",
            "../../assets/wav-sounds/mute.wav",
            "../../assets/wav-sounds/outgoing.wav",
            "../../assets/wav-sounds/reconnected.wav",
            "../../assets/wav-sounds/undeafen.wav",
            "../../assets/wav-sounds/unmute.wav",
        ];

        for wav_file_str in wav_files {
            let mut wav_bytes = Vec::new();
            let mut wav_file = File::open(wav_file_str).await.unwrap();
            wav_file.read_to_end(&mut wav_bytes).await.unwrap();

            let now = Instant::now();
            let other_data = wav_to_sea(&wav_bytes, 5.0).await.unwrap();
            info!("wav to sea took {:?}", now.elapsed());
            info!("{}%", other_data.len() as f32 / wav_bytes.len() as f32);

            let mut output_file = File::create(wav_file_str.replace(".wav", ".sea"))
                .await
                .unwrap();
            output_file.write_all(&other_data).await.unwrap();
        }
    }
}
