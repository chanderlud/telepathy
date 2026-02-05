use crate::codec::{
    common::SeaError,
    file::{SeaFile, SeaFileHeader},
};
use bytes::Bytes;
use kanal::{Receiver, Sender};

pub enum SeaEncoderState {
    Start,
    WritingFrames,
    Finished,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EncoderSettings {
    pub scale_factor_bits: u8,
    pub scale_factor_frames: u8,
    pub residual_bits: f32, // 1-8
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
    receiver: Receiver<Bytes>,
    sender: Sender<Bytes>,
    file: SeaFile,
    pub state: SeaEncoderState,
    written_frames: u32,
}

impl SeaEncoder {
    pub fn new(
        channels: u8,
        sample_rate: u32,
        settings: EncoderSettings,
        receiver: Receiver<Bytes>,
        sender: Sender<Bytes>,
    ) -> Result<Self, SeaError> {
        let header = SeaFileHeader {
            version: 1,
            channels,
            chunk_size: 0, // will be set later by the first chunk
            frames_per_chunk: settings.frames_per_chunk,
            sample_rate,
        };

        Ok(SeaEncoder {
            file: SeaFile::new(header, &settings)?,
            state: SeaEncoderState::Start,
            receiver,
            sender,
            written_frames: 0,
        })
    }

    pub fn encode_frame(&mut self) -> Result<(), SeaError> {
        if matches!(self.state, SeaEncoderState::Finished) {
            return Err(SeaError::EncoderClosed);
        }

        let frames = self.file.header.frames_per_chunk as usize;

        let buffer = self.receiver.recv()?;

        if !buffer.is_empty() {
            let samples = unsafe {
                std::slice::from_raw_parts(
                    buffer.as_ptr() as *const i16,
                    buffer.len() / size_of::<i16>(),
                )
            };
            let encoded_chunk = self.file.make_chunk(samples)?;

            assert_eq!(encoded_chunk.len(), self.file.header.chunk_size as usize);

            // we need to write file header after the first chunk is generated
            if matches!(self.state, SeaEncoderState::Start) {
                self.sender
                    .send(Bytes::from(self.file.header.serialize()))?;
                self.state = SeaEncoderState::WritingFrames;
            }

            self.sender.send(Bytes::from(encoded_chunk))?;
            self.written_frames += frames as u32;
        }

        Ok(())
    }

    pub fn finalize(&mut self) {
        _ = self.sender.close();
        self.state = SeaEncoderState::Finished;
    }
}
