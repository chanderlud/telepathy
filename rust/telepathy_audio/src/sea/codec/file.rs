use super::{
    bits::{BitPacker, BitUnpacker},
    chunk::{SeaChunk, SeaChunkType},
    common::{SEAC_MAGIC, SeaEncoderTrait, SeaError},
    decoder::Decoder,
    encoder_cbr::CbrEncoder,
    encoder_vbr::VbrEncoder,
    lms::SeaLMS,
};
use crate::sea::encoder::EncoderSettings;

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

    pub fn from_frame(frame: &[u8]) -> Result<Self, SeaError> {
        if frame.len() < 14 {
            return Err(SeaError::InvalidFile);
        }

        let magic = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]);
        if magic != SEAC_MAGIC {
            return Err(SeaError::InvalidFile);
        }
        let version = frame[4];
        let channels = frame[5];
        let chunk_size = u16::from_le_bytes([frame[6], frame[7]]);
        let frames_per_chunk = u16::from_le_bytes([frame[8], frame[9]]);
        let sample_rate = u32::from_le_bytes([frame[10], frame[11], frame[12], frame[13]]);

        let res = Self {
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

pub(crate) enum ActiveEncoder {
    Cbr(CbrEncoder),
    Vbr(VbrEncoder),
}

pub(crate) struct ChunkSerializer {
    pub(crate) packer: BitPacker,
    pub(crate) unpacker: BitUnpacker,
}

impl ChunkSerializer {
    fn new() -> Self {
        Self {
            packer: BitPacker::new(),
            unpacker: BitUnpacker::new_const_bits(1),
        }
    }
}

pub struct SeaFile {
    pub header: SeaFileHeader,

    pub(crate) decoder: Option<Decoder>,

    pub(crate) encoder: Option<ActiveEncoder>,
    pub(crate) encoder_settings: Option<EncoderSettings>,

    // Scratch buffers for chunk serialization/deserialization
    chunk_serializer: ChunkSerializer,
    scratch_lms: Vec<SeaLMS>,
    scratch_scale_factors: Vec<u8>,
    scratch_vbr_residual_sizes: Vec<u8>,
    scratch_residuals: Vec<u8>,
    scratch_vbr_bitlengths: Vec<u8>,
    lms_snapshot: Vec<SeaLMS>,
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
            chunk_serializer: ChunkSerializer::new(),
            scratch_lms: Vec::new(),
            scratch_scale_factors: Vec::new(),
            scratch_vbr_residual_sizes: Vec::new(),
            scratch_residuals: Vec::new(),
            scratch_vbr_bitlengths: Vec::new(),
            lms_snapshot: Vec::new(),
        })
    }

    pub fn new_for_decoding(header: SeaFileHeader) -> Self {
        SeaFile {
            header,
            decoder: None,
            encoder: None,
            encoder_settings: None,
            chunk_serializer: ChunkSerializer::new(),
            scratch_lms: Vec::new(),
            scratch_scale_factors: Vec::new(),
            scratch_vbr_residual_sizes: Vec::new(),
            scratch_residuals: Vec::new(),
            scratch_vbr_bitlengths: Vec::new(),
            lms_snapshot: Vec::new(),
        }
    }

    pub fn make_chunk(&mut self, samples: &[i16], output: &mut Vec<u8>) -> Result<(), SeaError> {
        let encoder_settings = self.encoder_settings.as_ref().unwrap();
        let encoder = self.encoder.as_mut().unwrap();

        // Snapshot LMS using clone_from to reuse allocation
        match encoder {
            ActiveEncoder::Cbr(encoder) => {
                self.lms_snapshot.clone_from(encoder.get_lms());
            }
            ActiveEncoder::Vbr(encoder) => {
                self.lms_snapshot.clone_from(encoder.get_lms());
            }
        }

        // Encode into scratch buffers
        // For CBR, residual_bits will remain empty; for VBR it will be populated
        match encoder {
            ActiveEncoder::Cbr(encoder) => {
                self.scratch_vbr_residual_sizes.clear();
                encoder.encode_into(
                    samples,
                    &mut self.scratch_scale_factors,
                    &mut self.scratch_residuals,
                    &mut self.scratch_vbr_residual_sizes,
                );
            }
            ActiveEncoder::Vbr(encoder) => encoder.encode_into(
                samples,
                &mut self.scratch_scale_factors,
                &mut self.scratch_residuals,
                &mut self.scratch_vbr_residual_sizes,
            ),
        }

        // Serialize chunk directly into caller-provided output buffer
        SeaChunk::serialize_into(
            &self.header,
            &self.lms_snapshot,
            encoder_settings,
            &self.scratch_scale_factors,
            &self.scratch_vbr_residual_sizes,
            &self.scratch_residuals,
            output,
            &mut self.chunk_serializer.packer,
        );

        if self.header.chunk_size == 0 {
            self.header.chunk_size = output.len() as u16;
        }

        let full_samples_len =
            self.header.frames_per_chunk as usize * self.header.channels as usize;

        if samples.len() == full_samples_len {
            assert_eq!(self.header.chunk_size, output.len() as u16);
        }

        Ok(())
    }

    pub fn samples_from_frame(
        &mut self,
        frame: &[u8],
        output: &mut [i16; 480],
    ) -> Result<(), SeaError> {
        // Parse chunk into scratch buffers
        let chunk_info = SeaChunk::parse_into(
            frame,
            &self.header,
            &mut self.scratch_lms,
            &mut self.scratch_scale_factors,
            &mut self.scratch_vbr_residual_sizes,
            &mut self.scratch_residuals,
            &mut self.scratch_vbr_bitlengths,
            &mut self.chunk_serializer.unpacker,
        )?;

        if self.decoder.is_none() {
            self.decoder = Some(Decoder::init(
                self.header.channels as usize,
                chunk_info.scale_factor_bits as usize,
            ));
        }
        let decoder = self.decoder.as_mut().unwrap();

        let expected_len = self.header.frames_per_chunk as usize * self.header.channels as usize;
        if output.len() < expected_len {
            return Err(SeaError::InvalidFrame);
        }

        match chunk_info.chunk_type {
            SeaChunkType::Cbr => decoder.decode_cbr_into(
                &chunk_info,
                &mut self.scratch_lms,
                &self.scratch_scale_factors,
                &self.scratch_residuals,
                &mut output[..expected_len],
            ),
            SeaChunkType::Vbr => decoder.decode_vbr_into(
                &chunk_info,
                &mut self.scratch_lms,
                &self.scratch_scale_factors,
                &self.scratch_vbr_residual_sizes,
                &self.scratch_residuals,
                &mut output[..expected_len],
            ),
        }

        Ok(())
    }
}
