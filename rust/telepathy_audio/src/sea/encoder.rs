use crate::internal::buffer_pool::PooledBuffer;
use crate::sea::codec::{
    common::SeaError,
    file::{SeaFile, SeaFileHeader},
};
use kanal::{Receiver, Sender};
use nnnoiseless::FRAME_SIZE;

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
    receiver: Receiver<PooledBuffer>,
    sender: Sender<PooledBuffer>,
    file: SeaFile,
    written_frames: u32,
}

impl SeaEncoder {
    pub fn new(
        channels: u8,
        sample_rate: u32,
        settings: EncoderSettings,
        receiver: Receiver<PooledBuffer>,
        sender: Sender<PooledBuffer>,
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
            receiver,
            sender,
            written_frames: 0,
        })
    }

    pub fn encode_frame(&mut self) -> Result<(), SeaError> {
        let frames = self.file.header.frames_per_chunk as usize;

        let mut buffer = self.receiver.recv()?;
        let inner = buffer.inner_mut();

        if !inner.is_empty() {
            let encoded_chunk = self.file.make_chunk(unsafe {
                std::slice::from_raw_parts(inner.as_ptr() as *const i16, FRAME_SIZE)
            })?;
            assert_eq!(encoded_chunk.len(), self.file.header.chunk_size as usize);

            // encoded chunk is smaller than the original buffer, truncate it
            inner.resize(encoded_chunk.len(), 0);
            // copy encoded data into truncated buffer
            inner.copy_from_slice(&encoded_chunk);
            // send the original buffer w/ encoded data inside it now
            self.sender.send(buffer)?;
            self.written_frames += frames as u32;
        }

        Ok(())
    }
}
