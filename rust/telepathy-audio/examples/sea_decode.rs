use nnnoiseless::FRAME_SIZE;
use std::env;
use std::fs;
use std::io;
use telepathy_audio::sea::{
    codec::{common::SeaError, file::SeaFileHeader},
    decoder::SeaDecoder,
};

const EXPECTED_SAMPLE_RATE: u32 = 48_000;
const EXPECTED_CHANNELS: u8 = 1;
const BYTES_PER_SAMPLE: usize = 2;
const SEA_HEADER_SIZE: usize = 14;

fn usage(program: &str) {
    eprintln!("Usage: {program} <input.sea> <output.raw>");
    eprintln!("  output.raw: signed 16-bit PCM, mono, 48kHz, little-endian");
}

fn sea_to_io(err: SeaError) -> io::Error {
    io::Error::other(format!("SEA decode error: {err:?}"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    let program = args
        .next()
        .unwrap_or_else(|| String::from("sea_decode_to_raw_pcm"));
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
    if input_bytes.len() < SEA_HEADER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "input SEA file is smaller than header size",
        )
        .into());
    }

    let header = SeaFileHeader::from_frame(&input_bytes[..SEA_HEADER_SIZE]).map_err(sea_to_io)?;
    if header.channels != EXPECTED_CHANNELS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "expected mono SEA file (channels=1), found channels={}",
                header.channels
            ),
        )
        .into());
    }
    if header.sample_rate != EXPECTED_SAMPLE_RATE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "expected 48000 Hz SEA file, found {} Hz",
                header.sample_rate
            ),
        )
        .into());
    }

    let chunk_size = usize::from(header.chunk_size);
    let payload = &input_bytes[SEA_HEADER_SIZE..];
    if payload.len() % chunk_size != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "payload size {} is not divisible by chunk size {}",
                payload.len(),
                chunk_size
            ),
        )
        .into());
    }

    let chunk_count = payload.len() / chunk_size;
    let mut decoder = SeaDecoder::new(header).map_err(sea_to_io)?;
    let mut frame = [0_i16; FRAME_SIZE];
    let mut output_bytes = Vec::with_capacity(chunk_count * FRAME_SIZE * BYTES_PER_SAMPLE);

    for chunk in payload.chunks_exact(chunk_size) {
        decoder.decode_frame(chunk, &mut frame).map_err(sea_to_io)?;
        for sample in frame {
            output_bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }

    fs::write(&output_path, output_bytes)?;
    println!("Decoded {chunk_count} SEA chunks to raw PCM.");

    Ok(())
}
