use crate::sea::codec::{
    common::SeaError,
    file::{SeaFile, SeaFileHeader},
};
use nnnoiseless::FRAME_SIZE;

pub struct SeaDecoder {
    file: SeaFile,
    frames_read: usize,
}

impl SeaDecoder {
    pub fn new(header: SeaFileHeader) -> Result<Self, SeaError> {
        Ok(Self {
            file: SeaFile {
                header,
                decoder: None,
                encoder: None,
                encoder_settings: None,
            },
            frames_read: 0,
        })
    }

    pub fn decode_frame(
        &mut self,
        frame: &[u8],
        output: &mut [i16; FRAME_SIZE],
    ) -> Result<(), SeaError> {
        self.file.samples_from_frame(frame, output)?;
        self.frames_read += self.file.header.frames_per_chunk as usize;
        Ok(())
    }

    pub fn get_header(&self) -> SeaFileHeader {
        self.file.header.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::SeaDecoder;
    use crate::sea::{
        codec::{common::SeaError, file::SeaFileHeader},
        encoder::{EncoderSettings, SeaEncoder},
    };
    use bytes::BytesMut;
    use nnnoiseless::FRAME_SIZE;
    use std::f64::consts::PI;

    fn sine_frame(sample_rate: f64, frequency: f64, amplitude: f64) -> [i16; FRAME_SIZE] {
        std::array::from_fn(|i| {
            let sample = (2.0 * PI * frequency * i as f64 / sample_rate).sin() * amplitude;
            sample as i16
        })
    }

    #[test]
    fn sea_decoder_new_and_get_header_round_trip() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 64,
            frames_per_chunk: 480,
            sample_rate: 48_000,
        };
        let decoder = SeaDecoder::new(header.clone()).unwrap();
        let actual = decoder.get_header();

        assert_eq!(actual.version, header.version);
        assert_eq!(actual.channels, header.channels);
        assert_eq!(actual.chunk_size, header.chunk_size);
        assert_eq!(actual.frames_per_chunk, header.frames_per_chunk);
        assert_eq!(actual.sample_rate, header.sample_rate);
    }

    #[test]
    fn decode_frame_rejects_too_short_slice() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 64,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let mut decoder = SeaDecoder::new(header).unwrap();
        let encoded = vec![0u8; 63];
        let mut output = [0i16; FRAME_SIZE];

        let result = decoder.decode_frame(&encoded, &mut output);
        assert!(matches!(result, Err(SeaError::InvalidFrame)));
    }

    #[test]
    fn decode_frame_rejects_bad_chunk_type() {
        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: 64,
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let mut decoder = SeaDecoder::new(header).unwrap();
        let mut encoded = vec![0u8; 64];
        encoded[0] = 0xFF;
        let mut output = [0i16; FRAME_SIZE];

        let result = decoder.decode_frame(&encoded, &mut output);
        assert!(matches!(result, Err(SeaError::InvalidFrame)));
    }

    #[test]
    fn decode_frame_accepts_valid_cbr_frame() {
        let mut encoder = SeaEncoder::new(1, 16_000, EncoderSettings::default()).unwrap();
        let input = sine_frame(16_000.0, 440.0, 16_000.0);
        let mut encoded = BytesMut::new();
        encoder.encode_frame(input, &mut encoded).unwrap();

        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: encoder.chunk_size(),
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let mut decoder = SeaDecoder::new(header).unwrap();
        let mut output = [0i16; FRAME_SIZE];
        let result = decoder.decode_frame(&encoded, &mut output);

        assert!(result.is_ok());
        assert!(output.iter().any(|&x| x != 0));
    }

    #[test]
    fn decode_frame_accepts_valid_vbr_frame() {
        let settings = EncoderSettings {
            vbr: true,
            ..Default::default()
        };
        let mut encoder = SeaEncoder::new(1, 16_000, settings).unwrap();
        let input = sine_frame(16_000.0, 440.0, 16_000.0);
        let mut encoded = BytesMut::new();
        encoder.encode_frame(input, &mut encoded).unwrap();

        let header = SeaFileHeader {
            version: 1,
            channels: 1,
            chunk_size: encoder.chunk_size(),
            frames_per_chunk: 480,
            sample_rate: 16_000,
        };
        let mut decoder = SeaDecoder::new(header).unwrap();
        let mut output = [0i16; FRAME_SIZE];
        let result = decoder.decode_frame(&encoded, &mut output);

        assert!(result.is_ok());
        assert!(output.iter().any(|&x| x != 0));
    }
}
