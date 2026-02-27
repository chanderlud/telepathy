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
