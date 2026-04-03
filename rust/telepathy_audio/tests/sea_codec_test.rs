#![cfg(not(target_family = "wasm"))]

use bytes::BytesMut;
use nnnoiseless::FRAME_SIZE;
use telepathy_audio::sea::{
    codec::{bits::{BitPacker, BitUnpacker}, common::SeaError, file::SeaFileHeader},
    decoder::SeaDecoder,
    encoder::{EncoderSettings, SeaEncoder},
};

const SAMPLE_RATE: u32 = 48_000;

fn sine_frame(samples_per_channel: usize, channels: usize, freq_hz: f32) -> [i16; FRAME_SIZE] {
    let mut frame = [0_i16; FRAME_SIZE];
    for i in 0..samples_per_channel {
        let t = i as f32 / SAMPLE_RATE as f32;
        let base = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
        for ch in 0..channels {
            let sample = if ch == 0 {
                base
            } else {
                (2.0 * std::f32::consts::PI * (freq_hz + 55.0) * t).sin()
            };
            frame[i * channels + ch] = (sample * 16_000.0) as i16;
        }
    }
    frame
}

fn header_from_encoder(
    channels: u8,
    frames_per_chunk: u16,
    sample_rate: u32,
    chunk_size: u16,
) -> SeaFileHeader {
    SeaFileHeader {
        version: 1,
        channels,
        chunk_size,
        frames_per_chunk,
        sample_rate,
    }
}

fn max_abs_diff(a: &[i16], b: &[i16]) -> i32 {
    a.iter()
        .zip(b.iter())
        .map(|(lhs, rhs)| (*lhs as i32 - *rhs as i32).abs())
        .max()
        .unwrap_or(0)
}

#[test]
fn cbr_round_trip() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = sine_frame(FRAME_SIZE, 1, 440.0);
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder.decode_frame(encoded.as_ref(), &mut decoded).unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 500);
}

#[test]
fn vbr_round_trip() {
    let settings = EncoderSettings {
        vbr: true,
        residual_bits: 3.0,
        ..EncoderSettings::default()
    };
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = sine_frame(FRAME_SIZE, 1, 440.0);
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder.decode_frame(encoded.as_ref(), &mut decoded).unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 500);
}

#[test]
fn multi_channel_cbr_round_trip() {
    let settings = EncoderSettings {
        frames_per_chunk: (FRAME_SIZE / 2) as u16,
        ..EncoderSettings::default()
    };
    let mut encoder = SeaEncoder::new(2, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = sine_frame(FRAME_SIZE / 2, 2, 440.0);
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(2, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder.decode_frame(encoded.as_ref(), &mut decoded).unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 500);
}

#[test]
fn silence_round_trip() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = [0_i16; FRAME_SIZE];
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [1_i16; FRAME_SIZE];
    decoder.decode_frame(encoded.as_ref(), &mut decoded).unwrap();

    assert!(decoded.iter().all(|&sample| (sample as i32).abs() <= 500));
}

#[test]
fn full_scale_round_trip() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = [i16::MAX; FRAME_SIZE];
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder.decode_frame(encoded.as_ref(), &mut decoded).unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 20_000);
}

#[test]
fn encoder_chunk_size_stable_across_frames() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings).unwrap();
    let frame = sine_frame(FRAME_SIZE, 1, 440.0);
    let mut encoded = BytesMut::with_capacity(4096);

    let mut expected_chunk_size = 0_u16;
    for i in 0..100 {
        encoder.encode_frame(frame, &mut encoded).unwrap();
        let chunk_size = encoder.chunk_size();
        if i == 0 {
            expected_chunk_size = chunk_size;
        }
        assert_eq!(expected_chunk_size, chunk_size);
    }
}

#[test]
fn decoder_reuses_scratch_buffers_across_frames() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = sine_frame(FRAME_SIZE, 1, 440.0);
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];

    for _ in 0..100 {
        decoded.fill(0);
        decoder.decode_frame(encoded.as_ref(), &mut decoded).unwrap();
        assert!(decoded.iter().any(|&sample| sample != 0));
    }
}

#[test]
fn bitpacker_bitunpacker_round_trip() {
    let values: [u8; 16] = [0, 1, 2, 7, 3, 4, 5, 6, 1, 0, 7, 2, 3, 5, 4, 6];
    let mut packer = BitPacker::new();
    packer.reset();
    for value in values {
        packer.push(value as u32, 3);
    }
    let packed = packer.finish().to_vec();

    let mut unpacker = BitUnpacker::new_const_bits(3);
    unpacker.reset_const(3);
    unpacker.process_bytes(&packed);
    let unpacked = unpacker.finish();

    assert_eq!(&unpacked[..values.len()], &values);
}

#[test]
fn invalid_frame_paths() {
    let header = SeaFileHeader {
        version: 1,
        channels: 1,
        chunk_size: 128,
        frames_per_chunk: FRAME_SIZE as u16,
        sample_rate: SAMPLE_RATE,
    };
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut output = [0_i16; FRAME_SIZE];

    let too_short = [0_u8; 2];
    let invalid_chunk_type = [0xFF_u8, 0x11, 20, 0x5A];
    let invalid_scale_factor_bits = [0x01_u8, 0x01, 20, 0x5A];

    let res_short = decoder.decode_frame(&too_short, &mut output);
    let res_type = decoder.decode_frame(&invalid_chunk_type, &mut output);
    let res_sf = decoder.decode_frame(&invalid_scale_factor_bits, &mut output);

    assert!(matches!(res_short, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_type, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_sf, Err(SeaError::InvalidFrame)));
}
