use crate::sea::codec::{
    common::SeaError,
    file::{SeaFile, SeaFileHeader},
};
use bytes::BytesMut;
use nnnoiseless::FRAME_SIZE;

#[derive(Debug, Clone, PartialEq)]
pub struct EncoderSettings {
    pub scale_factor_bits: u8,
    pub scale_factor_frames: u8,
    pub residual_bits: f32, // 2-8
    pub frames_per_chunk: u16,
    pub vbr: bool,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            frames_per_chunk: 480,
            scale_factor_bits: 4,
            scale_factor_frames: 20,
            residual_bits: 3.0,
            vbr: false,
        }
    }
}

pub struct SeaEncoder {
    file: SeaFile,
    written_frames: u32,
}

impl SeaEncoder {
    fn validate_parameters(
        channels: u8,
        sample_rate: u32,
        settings: &EncoderSettings,
    ) -> Result<(), SeaError> {
        if channels == 0
            || sample_rate == 0
            || settings.frames_per_chunk == 0
            || settings.scale_factor_bits == 0
            || settings.scale_factor_frames == 0
            || !(2.0..=8.0).contains(&settings.residual_bits)
        {
            return Err(SeaError::InvalidParameters);
        }

        Ok(())
    }

    pub fn new(
        channels: u8,
        sample_rate: u32,
        settings: EncoderSettings,
    ) -> Result<Self, SeaError> {
        Self::validate_parameters(channels, sample_rate, &settings)?;

        let header = SeaFileHeader {
            version: 1,
            channels,
            chunk_size: 0, // will be set later by the first chunk
            frames_per_chunk: settings.frames_per_chunk,
            sample_rate,
        };

        Ok(SeaEncoder {
            file: SeaFile::new(header, &settings)?,
            written_frames: 0,
        })
    }

    pub fn encode_frame(
        &mut self,
        frame: [i16; FRAME_SIZE],
        buffer: &mut BytesMut,
    ) -> Result<(), SeaError> {
        let frames = self.file.header.frames_per_chunk as usize;

        let encoded_chunk = self.file.make_chunk(&frame)?;
        assert_eq!(encoded_chunk.len(), self.file.header.chunk_size as usize);

        // encoded chunk is smaller than the original buffer, truncate it
        buffer.resize(encoded_chunk.len(), 0);
        // copy encoded data into truncated buffer
        buffer.copy_from_slice(&encoded_chunk);
        self.written_frames += frames as u32;

        Ok(())
    }

    pub fn chunk_size(&self) -> u16 {
        self.file.header.chunk_size
    }
}

#[cfg(test)]
mod tests {
    use super::{EncoderSettings, SeaEncoder};
    use crate::sea::{codec::common::SeaError, codec::file::SeaFileHeader, decoder::SeaDecoder};
    use bytes::BytesMut;
    use nnnoiseless::FRAME_SIZE;
    use std::f64::consts::PI;

    fn sine_frame(sample_rate: f64, frequency: f64, amplitude: f64) -> [i16; FRAME_SIZE] {
        std::array::from_fn(|i| {
            let sample = (2.0 * PI * frequency * i as f64 / sample_rate).sin() * amplitude;
            sample as i16
        })
    }

    fn compute_snr_db(input: &[i16; FRAME_SIZE], output: &[i16; FRAME_SIZE]) -> f64 {
        let signal_power = input
            .iter()
            .map(|&x| {
                let v = x as f64;
                v * v
            })
            .sum::<f64>()
            / FRAME_SIZE as f64;
        let noise_power = input
            .iter()
            .zip(output.iter())
            .map(|(&src, &dst)| {
                let e = src as f64 - dst as f64;
                e * e
            })
            .sum::<f64>()
            / FRAME_SIZE as f64;
        10.0 * (signal_power / noise_power.max(1e-12)).log10()
    }

    #[test]
    fn encoder_settings_default_values() {
        let settings = EncoderSettings::default();
        assert_eq!(settings.frames_per_chunk, 480);
        assert_eq!(settings.scale_factor_bits, 4);
        assert_eq!(settings.scale_factor_frames, 20);
        assert_eq!(settings.residual_bits, 3.0);
        assert!(!settings.vbr);
    }

    #[test]
    fn sea_encoder_new_valid_cbr() {
        let encoder = SeaEncoder::new(1, 16_000, EncoderSettings::default());
        assert!(encoder.is_ok());
        assert_eq!(encoder.unwrap().chunk_size(), 0);
    }

    #[test]
    fn sea_encoder_new_valid_vbr() {
        let settings = EncoderSettings {
            vbr: true,
            ..Default::default()
        };
        let encoder = SeaEncoder::new(1, 16_000, settings);
        assert!(encoder.is_ok());
        assert_eq!(encoder.unwrap().chunk_size(), 0);
    }

    #[test]
    fn sea_encoder_new_invalid_zero_channels_returns_invalid_parameters() {
        let result = SeaEncoder::new(0, 16_000, EncoderSettings::default());
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn sea_encoder_new_invalid_zero_sample_rate_returns_invalid_parameters() {
        let result = SeaEncoder::new(1, 0, EncoderSettings::default());
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn sea_encoder_new_invalid_zero_frames_per_chunk_returns_invalid_parameters() {
        let settings = EncoderSettings {
            frames_per_chunk: 0,
            ..Default::default()
        };
        let result = SeaEncoder::new(1, 16_000, settings);
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn sea_encoder_new_invalid_zero_scale_factor_frames_returns_invalid_parameters() {
        let settings = EncoderSettings {
            scale_factor_frames: 0,
            ..Default::default()
        };
        let result = SeaEncoder::new(1, 16_000, settings);
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn sea_encoder_new_invalid_zero_scale_factor_bits_returns_invalid_parameters() {
        let settings = EncoderSettings {
            scale_factor_bits: 0,
            ..Default::default()
        };
        let result = SeaEncoder::new(1, 16_000, settings);
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn sea_encoder_new_invalid_residual_bits_low_returns_invalid_parameters() {
        let settings = EncoderSettings {
            residual_bits: 1.5,
            ..Default::default()
        };
        let result = SeaEncoder::new(1, 16_000, settings);
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn sea_encoder_new_invalid_residual_bits_high_returns_invalid_parameters() {
        let settings = EncoderSettings {
            residual_bits: 8.5,
            ..Default::default()
        };
        let result = SeaEncoder::new(1, 16_000, settings);
        assert!(matches!(result, Err(SeaError::InvalidParameters)));
    }

    #[test]
    fn encode_frame_cbr_buffer_resized_and_non_empty() {
        let mut encoder = SeaEncoder::new(1, 16_000, EncoderSettings::default()).unwrap();
        let mut buffer = BytesMut::new();
        let frame = std::array::from_fn(|i| i as i16);

        encoder.encode_frame(frame, &mut buffer).unwrap();

        assert!(!buffer.is_empty());
        assert!(buffer.len() < FRAME_SIZE * 2);
        assert_ne!(encoder.chunk_size(), 0);
        assert_eq!(encoder.chunk_size() as usize, buffer.len());
    }

    #[test]
    fn encode_frame_vbr_buffer_resized_and_non_empty() {
        let settings = EncoderSettings {
            vbr: true,
            ..Default::default()
        };
        let mut encoder = SeaEncoder::new(1, 16_000, settings).unwrap();
        let mut buffer = BytesMut::new();
        let frame = std::array::from_fn(|i| i as i16);

        encoder.encode_frame(frame, &mut buffer).unwrap();

        assert!(!buffer.is_empty());
        assert!(buffer.len() < FRAME_SIZE * 2);
        assert_ne!(encoder.chunk_size(), 0);
        assert_eq!(encoder.chunk_size() as usize, buffer.len());
    }

    #[test]
    fn chunk_size_consistent_multiple_frames_cbr() {
        let mut encoder = SeaEncoder::new(1, 16_000, EncoderSettings::default()).unwrap();
        let mut buffer = BytesMut::new();
        let frame = std::array::from_fn(|i| (i as i16) - 240);

        encoder.encode_frame(frame, &mut buffer).unwrap();
        let first = encoder.chunk_size();

        encoder.encode_frame(frame, &mut buffer).unwrap();
        let second = encoder.chunk_size();

        encoder.encode_frame(frame, &mut buffer).unwrap();
        let third = encoder.chunk_size();

        assert_eq!(first, second);
        assert_eq!(second, third);
    }

    #[test]
    fn chunk_size_consistent_multiple_frames_vbr() {
        let settings = EncoderSettings {
            vbr: true,
            ..Default::default()
        };
        let mut encoder = SeaEncoder::new(1, 16_000, settings).unwrap();
        let mut buffer = BytesMut::new();
        let frame = std::array::from_fn(|i| (i as i16) - 240);

        encoder.encode_frame(frame, &mut buffer).unwrap();
        let first = encoder.chunk_size();

        encoder.encode_frame(frame, &mut buffer).unwrap();
        let second = encoder.chunk_size();

        encoder.encode_frame(frame, &mut buffer).unwrap();
        let third = encoder.chunk_size();

        assert_eq!(first, second);
        assert_eq!(second, third);
    }

    #[test]
    fn round_trip_cbr_snr_above_20_db() {
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
        decoder.decode_frame(&encoded, &mut output).unwrap();

        let snr_db = compute_snr_db(&input, &output);
        assert!(snr_db > 20.0, "SNR too low: {snr_db} dB");
    }

    #[test]
    fn round_trip_vbr_snr_above_20_db() {
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
        decoder.decode_frame(&encoded, &mut output).unwrap();

        let snr_db = compute_snr_db(&input, &output);
        assert!(snr_db > 20.0, "SNR too low: {snr_db} dB");
    }
}
