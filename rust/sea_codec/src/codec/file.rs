use super::{
    chunk::SeaChunkType,
    common::{
        SEAC_MAGIC, SeaEncoderTrait, SeaError, read_u8, read_u16_le, read_u32_be, read_u32_le,
    },
    decoder::Decoder,
    encoder_cbr::CbrEncoder,
    encoder_vbr::VbrEncoder,
};
use crate::{ProcessorMessage, codec::chunk::SeaChunk, encoder::EncoderSettings};
use kanal::Receiver;
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct SeaFileHeader {
    pub version: u8,
    pub channels: u8,
    pub chunk_size: u16,
    pub frames_per_chunk: u16,
    pub sample_rate: u32,
}

impl SeaFileHeader {
    fn validate(&self) -> Result<(), SeaError> {
        if self.version != 1 {
            return Err(SeaError::UnsupportedVersion);
        }
        if self.channels == 0
            || self.chunk_size < 16
            || self.frames_per_chunk == 0
            || self.sample_rate == 0
        {
            return Err(SeaError::InvalidFile);
        }
        Ok(())
    }

    pub fn from_reader(receiver: &Receiver<ProcessorMessage>) -> Result<Self, SeaError> {
        let buffer = match receiver.recv()? {
            ProcessorMessage::Data(data) => data,
            _ => return Err(SeaError::InvalidFrame),
        };

        let mut reader = Cursor::new(buffer);

        let magic = read_u32_be(&mut reader)?;
        if magic != SEAC_MAGIC {
            return Err(SeaError::InvalidFile);
        }
        let version = read_u8(&mut reader)?;
        let channels = read_u8(&mut reader)?;
        let chunk_size = read_u16_le(&mut reader)?;
        let frames_per_chunk = read_u16_le(&mut reader)?;
        let sample_rate = read_u32_le(&mut reader)?;

        let res: SeaFileHeader = Self {
            version,
            channels,
            chunk_size,
            frames_per_chunk,
            sample_rate,
        };

        res.validate()?;
        Ok(res)
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut output = Vec::new();

        output.extend_from_slice(&SEAC_MAGIC.to_be_bytes());
        output.extend_from_slice(&self.version.to_le_bytes());
        output.extend_from_slice(&self.channels.to_le_bytes());
        output.extend_from_slice(&self.chunk_size.to_le_bytes());
        output.extend_from_slice(&self.frames_per_chunk.to_le_bytes());
        output.extend_from_slice(&self.sample_rate.to_le_bytes());

        output
    }
}

enum ActiveEncoder {
    Cbr(CbrEncoder),
    Vbr(VbrEncoder),
}

pub struct SeaFile {
    pub header: SeaFileHeader,

    decoder: Option<Decoder>,

    encoder: Option<ActiveEncoder>,
    encoder_settings: Option<EncoderSettings>,
}

impl SeaFile {
    pub fn new(
        header: SeaFileHeader,
        encoder_settings: &EncoderSettings,
    ) -> Result<Self, SeaError> {
        let encoder = if encoder_settings.vbr {
            let vbr_encoder = VbrEncoder::new(&header, &encoder_settings.clone());
            Some(ActiveEncoder::Vbr(vbr_encoder))
        } else {
            let cbr_encoder = CbrEncoder::new(&header, &encoder_settings.clone());
            Some(ActiveEncoder::Cbr(cbr_encoder))
        };

        Ok(SeaFile {
            header,
            decoder: None,
            encoder,
            encoder_settings: Some(encoder_settings.clone()),
        })
    }

    pub fn from_reader(
        receiver: &Receiver<ProcessorMessage>,
        header: Option<SeaFileHeader>,
    ) -> Result<Self, SeaError> {
        let header = match header {
            Some(h) => {
                h.validate()?;
                h
            }
            None => SeaFileHeader::from_reader(receiver)?,
        };

        Ok(SeaFile {
            header,
            decoder: None,
            encoder: None,
            encoder_settings: None,
        })
    }

    pub fn make_chunk(&mut self, samples: &[i16]) -> Result<Vec<u8>, SeaError> {
        let encoder_settings = self.encoder_settings.as_ref().unwrap();
        let encoder = self.encoder.as_mut().unwrap();

        let initial_lms = match encoder {
            ActiveEncoder::Cbr(encoder) => encoder.get_lms().clone(),
            ActiveEncoder::Vbr(encoder) => encoder.get_lms().clone(),
        };

        let encoded = match encoder {
            ActiveEncoder::Cbr(encoder) => encoder.encode(samples),
            ActiveEncoder::Vbr(encoder) => encoder.encode(samples),
        };

        let chunk = SeaChunk::new(
            &self.header,
            &initial_lms,
            encoder_settings,
            encoded.scale_factors,
            encoded.residual_bits,
            encoded.residuals,
        );
        let output = chunk.serialize();

        if self.header.chunk_size == 0 {
            self.header.chunk_size = output.len() as u16;
        }

        let full_samples_len =
            self.header.frames_per_chunk as usize * self.header.channels as usize;

        if samples.len() == full_samples_len {
            assert_eq!(self.header.chunk_size, output.len() as u16);
        }

        Ok(output)
    }

    pub fn samples_from_reader(
        &mut self,
        receiver: &Receiver<ProcessorMessage>,
    ) -> Result<ProcessorMessage, SeaError> {
        let encoded = match receiver.recv()? {
            ProcessorMessage::Data(data) => data,
            _ => return Err(SeaError::InvalidFrame),
        };

        let chunk = SeaChunk::from_slice(&encoded, &self.header)?;

        if self.decoder.is_none() {
            self.decoder = Some(Decoder::init(
                self.header.channels as usize,
                chunk.scale_factor_bits as usize,
            ));
        }
        let decoder = self.decoder.as_mut().unwrap();
        let decoded = match chunk.chunk_type {
            SeaChunkType::Cbr => decoder.decode_cbr(&chunk),
            SeaChunkType::Vbr => decoder.decode_vbr(&chunk),
        };

        let expected_len = self.header.frames_per_chunk as usize * self.header.channels as usize;
        if decoded.len() != expected_len {
            Err(SeaError::InvalidFrame)
        } else {
            Ok(ProcessorMessage::Samples(decoded.try_into().unwrap()))
        }
    }
}
