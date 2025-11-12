use crate::ProcessorMessage;
use crate::codec::{
    common::SeaError,
    file::{SeaFile, SeaFileHeader},
};
use kanal::{Receiver, Sender};

pub struct SeaDecoder {
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    file: SeaFile,
    frames_read: usize,
}

impl SeaDecoder {
    pub fn new(
        receiver: Receiver<ProcessorMessage>,
        sender: Sender<ProcessorMessage>,
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

        self.frames_read += 480;
        self.sender.send(message)?;
        Ok(())
    }

    pub fn finalize(&mut self) {
        _ = self.sender.close();
    }

    pub fn get_header(&self) -> SeaFileHeader {
        self.file.header.clone()
    }
}
