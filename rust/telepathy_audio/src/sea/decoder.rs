use crate::sea::codec::{
    common::SeaError,
    file::{SeaFile, SeaFileHeader},
};
use bytes::BytesMut;
use kanal::{Receiver, Sender};

pub struct SeaDecoder {
    receiver: Receiver<BytesMut>,
    sender: Sender<BytesMut>,
    file: SeaFile,
    frames_read: usize,
}

impl SeaDecoder {
    pub fn new(
        receiver: Receiver<BytesMut>,
        sender: Sender<BytesMut>,
        header: Option<SeaFileHeader>,
    ) -> Result<Self, SeaError> {
        let file = SeaFile::from_reader(&receiver, header)?;

        Ok(Self {
            receiver,
            sender,
            file,
            frames_read: 0,
        })
    }

    pub fn decode_frame(&mut self) -> Result<(), SeaError> {
        let message = self.file.samples_from_reader(&self.receiver)?;

        self.frames_read += self.file.header.frames_per_chunk as usize;
        self.sender.send(message)?;
        Ok(())
    }

    pub fn get_header(&self) -> SeaFileHeader {
        self.file.header.clone()
    }
}
