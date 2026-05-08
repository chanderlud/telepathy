use bytes::BytesMut;
use nnnoiseless::FRAME_SIZE;
use std::env;
use std::fs;
use std::io;
use telepathy_audio::sea::{
    codec::{common::SeaError, file::SeaFileHeader},
    encoder::{EncoderSettings, SeaEncoder},
};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u8 = 1;
const BYTES_PER_SAMPLE: usize = 2;
const SEA_HEADER_SIZE: usize = 14;

fn usage(program: &str) {
    eprintln!("Usage: {program} <input.raw> <output.sea>");
    eprintln!("  input.raw: signed 16-bit PCM, mono, 48kHz, little-endian");
}

fn sea_to_io(err: SeaError) -> io::Error {
    io::Error::other(format!("SEA encode error: {err:?}"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    let program = args
        .next()
        .unwrap_or_else(|| String::from("sea_encode_raw_pcm"));
    let Some(input_path) = args.next() else {
        usage(&program);
        return Ok(());
    };
    let Some(output_path) = args.next() else {
        usage(&program);
        return Ok(());
    };
    if args.next().is_some() {
        usage(&program);
        return Ok(());
    }

    let input_bytes = fs::read(&input_path)?;
    if input_bytes.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "input file is empty").into());
    }
    if input_bytes.len() % BYTES_PER_SAMPLE != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input length must be divisible by 2 (i16 samples)",
        )
        .into());
    }

    let samples: Vec<i16> = input_bytes
        .chunks_exact(BYTES_PER_SAMPLE)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    let mut encoder =
        SeaEncoder::new(CHANNELS, SAMPLE_RATE, EncoderSettings::default()).map_err(sea_to_io)?;

    let total_samples = samples.len();
    let total_frames = total_samples.div_ceil(FRAME_SIZE);
    let mut sea_bytes = Vec::with_capacity(SEA_HEADER_SIZE + total_frames.saturating_mul(256));
    let mut encoded_frame = BytesMut::new();

    for frame_index in 0..total_frames {
        let start = frame_index * FRAME_SIZE;
        let end = (start + FRAME_SIZE).min(total_samples);

        let mut frame = [0_i16; FRAME_SIZE];
        frame[..(end - start)].copy_from_slice(&samples[start..end]);

        encoder.encode_frame(frame, &mut encoded_frame).map_err(sea_to_io)?;

        if frame_index == 0 {
            let header = SeaFileHeader {
                version: 1,
                channels: CHANNELS,
                chunk_size: encoder.chunk_size(),
                frames_per_chunk: FRAME_SIZE as u16,
                sample_rate: SAMPLE_RATE,
            };
            sea_bytes.extend_from_slice(&header.serialize());
        }

        sea_bytes.extend_from_slice(encoded_frame.as_ref());
    }

    fs::write(&output_path, sea_bytes)?;

    let padded_samples = total_frames * FRAME_SIZE - total_samples;
    println!("Encoded {total_samples} samples into {total_frames} SEA chunks.");
    if padded_samples > 0 {
        println!(
            "Last chunk was zero-padded with {padded_samples} sample(s) to complete {FRAME_SIZE}-sample frame."
        );
    }

    Ok(())
}
