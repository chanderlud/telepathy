#![cfg(not(target_family = "wasm"))]

use bytes::BytesMut;
use nnnoiseless::FRAME_SIZE;
use telepathy_audio::sea::{
    codec::{
        bits::{BitPacker, BitUnpacker},
        common::SeaError,
        file::SeaFileHeader,
    },
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

fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let energy = samples
        .iter()
        .map(|&sample| {
            let sample_f32 = sample as f32;
            sample_f32 * sample_f32
        })
        .sum::<f32>();
    (energy / samples.len() as f32).sqrt()
}

#[test]
fn cbr_round_trip() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = sine_frame(FRAME_SIZE, 1, 440.0);
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(
        1,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded)
        .unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 500);
    assert!(rms(&decoded) >= rms(&frame) * 0.8);
    assert_ne!(frame, decoded);
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

    let header = header_from_encoder(
        1,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded)
        .unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 500);
    assert!(rms(&decoded) >= rms(&frame) * 0.8);
    assert_ne!(frame, decoded);
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

    let header = header_from_encoder(
        2,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded)
        .unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 500);

    let frame_left: Vec<i16> = frame.iter().step_by(2).copied().collect();
    let frame_right: Vec<i16> = frame.iter().skip(1).step_by(2).copied().collect();
    let decoded_left: Vec<i16> = decoded.iter().step_by(2).copied().collect();
    let decoded_right: Vec<i16> = decoded.iter().skip(1).step_by(2).copied().collect();

    assert!(max_abs_diff(&frame_left, &decoded_left) <= 500);
    assert!(max_abs_diff(&frame_right, &decoded_right) <= 500);
    assert_ne!(decoded_left, decoded_right);
}

#[test]
fn silence_round_trip() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = [0_i16; FRAME_SIZE];
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(
        1,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [1_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded)
        .unwrap();

    assert_eq!(frame.len(), decoded.len());
    assert!(decoded.iter().all(|&sample| (sample as i32).abs() <= 500));

    let mut decoded_second = [2_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded_second)
        .unwrap();
    assert!(
        decoded_second
            .iter()
            .all(|&sample| (sample as i32).abs() <= 500)
    );
}

#[test]
fn full_scale_round_trip() {
    let settings = EncoderSettings {
        residual_bits: 8.0,
        ..EncoderSettings::default()
    };
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let mut frame = [0_i16; FRAME_SIZE];
    for (i, sample) in frame.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample = ((2.0 * std::f32::consts::PI * 1_000.0 * t).sin() * i16::MAX as f32) as i16;
    }
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(
        1,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded)
        .unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 4_000);
}

#[test]
fn full_scale_round_trip_default_settings_tolerance() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let frame = [i16::MAX; FRAME_SIZE];
    let mut encoded = BytesMut::new();
    encoder.encode_frame(frame, &mut encoded).unwrap();

    let header = header_from_encoder(
        1,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    decoder
        .decode_frame(encoded.as_ref(), &mut decoded)
        .unwrap();

    assert!(max_abs_diff(&frame, &decoded) <= 20_000);
}

#[test]
fn encoder_chunk_size_stable_across_frames() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings).unwrap();
    let frame_a = sine_frame(FRAME_SIZE, 1, 440.0);
    let frame_b = sine_frame(FRAME_SIZE, 1, 880.0);
    let mut encoded = BytesMut::with_capacity(4096);
    let mut previous_payload: Option<Vec<u8>> = None;

    let mut expected_chunk_size = 0_u16;
    for i in 0..100 {
        let frame = if i % 2 == 0 { frame_a } else { frame_b };
        encoder.encode_frame(frame, &mut encoded).unwrap();
        let chunk_size = encoder.chunk_size();
        if i == 0 {
            expected_chunk_size = chunk_size;
        }
        assert_eq!(expected_chunk_size, chunk_size);

        if let Some(previous) = &previous_payload {
            assert_ne!(previous.as_slice(), encoded.as_ref());
        }
        previous_payload = Some(encoded.to_vec());
    }
}

#[test]
fn decoder_overwrites_stale_output_across_distinct_frames() {
    let settings = EncoderSettings::default();
    let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
    let first_frame = sine_frame(FRAME_SIZE, 1, 440.0);
    let second_frame = sine_frame(FRAME_SIZE, 1, 880.0);
    let mut first_encoded = BytesMut::new();
    let mut second_encoded = BytesMut::new();
    encoder
        .encode_frame(first_frame, &mut first_encoded)
        .unwrap();
    encoder
        .encode_frame(second_frame, &mut second_encoded)
        .unwrap();

    let header = header_from_encoder(
        1,
        settings.frames_per_chunk,
        SAMPLE_RATE,
        encoder.chunk_size(),
    );
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [i16::MIN; FRAME_SIZE];

    decoder
        .decode_frame(first_encoded.as_ref(), &mut decoded)
        .unwrap();
    assert_eq!(decoded.len(), FRAME_SIZE);
    assert!(max_abs_diff(&first_frame, &decoded) <= 500);
    assert!(!decoded.contains(&i16::MIN));

    decoded.fill(i16::MAX);
    decoder
        .decode_frame(second_encoded.as_ref(), &mut decoded)
        .unwrap();
    assert_eq!(decoded.len(), FRAME_SIZE);
    assert!(max_abs_diff(&second_frame, &decoded) <= 2_000);
    assert!(!decoded.contains(&i16::MAX));
}

#[test]
fn malformed_file_headers_are_rejected() {
    let too_short = [0_u8; 13];
    let bad_magic = [0_u8; 14];

    assert!(matches!(
        SeaFileHeader::from_frame(&too_short),
        Err(SeaError::InvalidFile)
    ));
    assert!(matches!(
        SeaFileHeader::from_frame(&bad_magic),
        Err(SeaError::InvalidFile)
    ));
}

#[test]
fn bitpacker_bitunpacker_round_trip() {
    for width in 1_u8..=8 {
        let max = (1_u16 << width) - 1;
        let values = [0_u8, (max / 2) as u8, max as u8, 1_u8, (max - 1) as u8];

        let mut packer = BitPacker::default();
        packer.reset();
        for value in values {
            packer.push(value as u32, width);
        }
        let packed = packer.finish().to_vec();

        let mut unpacker = BitUnpacker::new_const_bits(width);
        unpacker.reset_const(width);
        unpacker.process_bytes(&packed);
        let unpacked = unpacker.finish();

        assert!(unpacked.len() >= values.len());
        assert_eq!(&unpacked[..values.len()], &values);
        assert!(unpacked[values.len()..].iter().all(|&value| value == 0));
    }
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
    let truncated_payload_for_declared_size = [0x01_u8, 0x11, 3, 0x5A, 0x00, 0x00];
    let invalid_scale_factor_bits_nine = [0x01_u8, 0x91, 20, 0x5A];
    let invalid_chunk_type_zero = [0x00_u8, 0x11, 20, 0x5A];

    let res_short = decoder.decode_frame(&too_short, &mut output);
    let res_type = decoder.decode_frame(&invalid_chunk_type, &mut output);
    let res_sf = decoder.decode_frame(&invalid_scale_factor_bits, &mut output);
    let res_truncated = decoder.decode_frame(&truncated_payload_for_declared_size, &mut output);
    let res_sf_nine = decoder.decode_frame(&invalid_scale_factor_bits_nine, &mut output);
    let res_chunk_zero = decoder.decode_frame(&invalid_chunk_type_zero, &mut output);

    assert!(matches!(res_short, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_type, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_sf, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_truncated, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_sf_nine, Err(SeaError::InvalidFrame)));
    assert!(matches!(res_chunk_zero, Err(SeaError::InvalidFrame)));
}

#[test]
fn invalid_encoder_settings_are_rejected() {
    let invalid_scale_factor_bits = EncoderSettings {
        scale_factor_bits: 0,
        ..EncoderSettings::default()
    };
    let invalid_scale_factor_frames = EncoderSettings {
        scale_factor_frames: 0,
        ..EncoderSettings::default()
    };
    let invalid_chunk_divisibility = EncoderSettings {
        frames_per_chunk: 480,
        scale_factor_frames: 21,
        ..EncoderSettings::default()
    };
    let invalid_geometry = EncoderSettings {
        frames_per_chunk: 240,
        ..EncoderSettings::default()
    };
    let invalid_residual_bits_below_range = EncoderSettings {
        residual_bits: 0.9,
        ..EncoderSettings::default()
    };
    let invalid_residual_bits_above_range = EncoderSettings {
        residual_bits: 9.0,
        ..EncoderSettings::default()
    };
    let invalid_residual_bits_nan = EncoderSettings {
        residual_bits: f32::NAN,
        ..EncoderSettings::default()
    };
    let invalid_residual_bits_infinity = EncoderSettings {
        residual_bits: f32::INFINITY,
        ..EncoderSettings::default()
    };
    let invalid_residual_bits_negative_infinity = EncoderSettings {
        residual_bits: f32::NEG_INFINITY,
        ..EncoderSettings::default()
    };
    let invalid_scale_factor_bits_above_range = EncoderSettings {
        scale_factor_bits: 9,
        ..EncoderSettings::default()
    };

    let res_bits = SeaEncoder::new(1, SAMPLE_RATE, invalid_scale_factor_bits);
    let res_frames = SeaEncoder::new(1, SAMPLE_RATE, invalid_scale_factor_frames);
    let res_divisibility = SeaEncoder::new(1, SAMPLE_RATE, invalid_chunk_divisibility);
    let res_geometry = SeaEncoder::new(1, SAMPLE_RATE, invalid_geometry);
    let res_residual_below = SeaEncoder::new(1, SAMPLE_RATE, invalid_residual_bits_below_range);
    let res_residual_above = SeaEncoder::new(1, SAMPLE_RATE, invalid_residual_bits_above_range);
    let res_residual_nan = SeaEncoder::new(1, SAMPLE_RATE, invalid_residual_bits_nan);
    let res_residual_infinity = SeaEncoder::new(1, SAMPLE_RATE, invalid_residual_bits_infinity);
    let res_residual_negative_infinity =
        SeaEncoder::new(1, SAMPLE_RATE, invalid_residual_bits_negative_infinity);
    let res_scale_factor_above =
        SeaEncoder::new(1, SAMPLE_RATE, invalid_scale_factor_bits_above_range);
    let res_sample_rate_zero = SeaEncoder::new(1, 0, EncoderSettings::default());
    let res_channels_zero = SeaEncoder::new(0, SAMPLE_RATE, EncoderSettings::default());

    assert!(matches!(res_bits, Err(SeaError::InvalidParameters)));
    assert!(matches!(res_frames, Err(SeaError::InvalidParameters)));
    assert!(matches!(res_divisibility, Err(SeaError::InvalidParameters)));
    assert!(matches!(res_geometry, Err(SeaError::InvalidParameters)));
    assert!(matches!(
        res_residual_below,
        Err(SeaError::InvalidParameters)
    ));
    assert!(matches!(
        res_residual_above,
        Err(SeaError::InvalidParameters)
    ));
    assert!(matches!(res_residual_nan, Err(SeaError::InvalidParameters)));
    assert!(matches!(
        res_residual_infinity,
        Err(SeaError::InvalidParameters)
    ));
    assert!(matches!(
        res_residual_negative_infinity,
        Err(SeaError::InvalidParameters)
    ));
    assert!(matches!(
        res_scale_factor_above,
        Err(SeaError::InvalidParameters)
    ));
    assert!(matches!(
        res_sample_rate_zero,
        Err(SeaError::InvalidParameters)
    ));
    assert!(matches!(
        res_channels_zero,
        Err(SeaError::InvalidParameters)
    ));
}

#[test]
fn round_trip_fidelity_improves_with_residual_bits_cbr_and_vbr() {
    let residual_bits_values = [2.0_f32, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let frame = sine_frame(FRAME_SIZE, 1, 440.0);

    for vbr in [false, true] {
        let mut previous_max_abs_diff = i32::MAX;

        for residual_bits in residual_bits_values {
            let settings = EncoderSettings {
                vbr,
                residual_bits,
                ..EncoderSettings::default()
            };
            let mut encoder = SeaEncoder::new(1, SAMPLE_RATE, settings.clone()).unwrap();
            let mut encoded = BytesMut::new();
            encoder.encode_frame(frame, &mut encoded).unwrap();

            let header = header_from_encoder(
                1,
                settings.frames_per_chunk,
                SAMPLE_RATE,
                encoder.chunk_size(),
            );
            let mut decoder = SeaDecoder::new(header).unwrap();
            let mut decoded = [0_i16; FRAME_SIZE];
            decoder
                .decode_frame(encoded.as_ref(), &mut decoded)
                .unwrap();

            let current_max_abs_diff = max_abs_diff(&frame, &decoded);
            assert!(current_max_abs_diff <= previous_max_abs_diff);
            previous_max_abs_diff = current_max_abs_diff;
        }
    }
}
