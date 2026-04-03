#![cfg(not(target_family = "wasm"))]

use bytes::BytesMut;
use criterion::{Criterion, criterion_group, criterion_main};
use nnnoiseless::FRAME_SIZE;
use std::hint::black_box;
use telepathy_audio::sea::{
    codec::{bits::{BitPacker, BitUnpacker}, file::SeaFileHeader},
    decoder::SeaDecoder,
    encoder::{EncoderSettings, SeaEncoder},
};

const SAMPLE_RATE: u32 = 48_000;

fn sine_frame(freq_hz: f32) -> [i16; FRAME_SIZE] {
    let mut frame = [0_i16; FRAME_SIZE];
    for (i, sample) in frame.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample = ((2.0 * std::f32::consts::PI * freq_hz * t).sin() * 16_000.0) as i16;
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

pub fn bench_sea_codec(c: &mut Criterion) {
    let mut group = c.benchmark_group("sea_codec");
    group.sample_size(100);

    let cbr_frame = sine_frame(440.0);
    let vbr_frame = sine_frame(660.0);

    group.bench_function("bench_cbr_encode", |b| {
        let settings = EncoderSettings::default();
        let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings).unwrap();
        let mut buffer = BytesMut::with_capacity(4096);
        b.iter(|| {
            encoder
                .encode_frame(black_box(cbr_frame), black_box(&mut buffer))
                .unwrap();
            black_box(&buffer);
        });
    });

    group.bench_function("bench_vbr_encode", |b| {
        let settings = EncoderSettings {
            vbr: true,
            ..EncoderSettings::default()
        };
        let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings).unwrap();
        let mut buffer = BytesMut::with_capacity(4096);
        b.iter(|| {
            encoder
                .encode_frame(black_box(vbr_frame), black_box(&mut buffer))
                .unwrap();
            black_box(&buffer);
        });
    });

    group.bench_function("bench_cbr_decode", |b| {
        let settings = EncoderSettings::default();
        let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
        let mut encoded = BytesMut::new();
        encoder.encode_frame(cbr_frame, &mut encoded).unwrap();
        let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
        let mut decoder = SeaDecoder::new(header).unwrap();
        let mut output = [0_i16; FRAME_SIZE];

        b.iter(|| {
            decoder
                .decode_frame(black_box(encoded.as_ref()), black_box(&mut output))
                .unwrap();
            black_box(&output);
        });
    });

    group.bench_function("bench_vbr_decode", |b| {
        let settings = EncoderSettings {
            vbr: true,
            ..EncoderSettings::default()
        };
        let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
        let mut encoded = BytesMut::new();
        encoder.encode_frame(vbr_frame, &mut encoded).unwrap();
        let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
        let mut decoder = SeaDecoder::new(header).unwrap();
        let mut output = [0_i16; FRAME_SIZE];

        b.iter(|| {
            decoder
                .decode_frame(black_box(encoded.as_ref()), black_box(&mut output))
                .unwrap();
            black_box(&output);
        });
    });

    group.bench_function("bench_cbr_encode_decode_roundtrip", |b| {
        let settings = EncoderSettings::default();
        let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
        let mut encoded = BytesMut::new();
        encoder.encode_frame(cbr_frame, &mut encoded).unwrap();

        let header = header_from_encoder(1, settings.frames_per_chunk, SAMPLE_RATE, encoder.chunk_size());
        let mut decoder = SeaDecoder::new(header).unwrap();
        let mut output = [0_i16; FRAME_SIZE];

        b.iter(|| {
            encoder
                .encode_frame(black_box(cbr_frame), black_box(&mut encoded))
                .unwrap();
            decoder
                .decode_frame(black_box(encoded.as_ref()), black_box(&mut output))
                .unwrap();
            black_box(&output);
        });
    });

    group.bench_function("bench_bitpacker_roundtrip", |b| {
        let residuals: Vec<u8> = (0..FRAME_SIZE).map(|i| (i % 8) as u8).collect();
        let mut packer = BitPacker::new();
        let mut unpacker = BitUnpacker::new_const_bits(3);

        b.iter(|| {
            packer.reset();
            for value in residuals.iter() {
                packer.push(*value as u32, 3);
            }
            let packed = packer.finish();

            unpacker.reset_const(3);
            unpacker.process_bytes(packed);
            let unpacked = unpacker.finish();

            black_box(unpacked);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_sea_codec);
criterion_main!(benches);
