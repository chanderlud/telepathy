use bytes::Bytes;

pub mod codec;
pub mod decoder;
pub mod encoder;

/// a message containing either a frame of audio or silence
#[derive(Debug)]
pub enum ProcessorMessage {
    Data(Bytes),
    Samples(Box<[i16; 480]>),
    Silence,
}

/// common processor message constructors
impl ProcessorMessage {
    pub fn slice(bytes: &'static [u8]) -> Self {
        Self::Data(Bytes::from(bytes))
    }

    pub fn silence() -> Self {
        Self::Silence
    }

    pub fn bytes(frame: Bytes) -> Self {
        Self::Data(frame)
    }

    pub fn samples(samples: [i16; 480]) -> Self {
        Self::Samples(Box::new(samples))
    }
}
