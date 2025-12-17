use crate::error::Error;
use kanal::{Receiver, Sender};
use log::info;
use sea_codec::ProcessorMessage;
use sea_codec::codec::file::SeaFileHeader;
use sea_codec::decoder::SeaDecoder;
use sea_codec::encoder::{EncoderSettings, SeaEncoder, SeaEncoderState};

pub(crate) fn encoder(
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    sample_rate: u32,
    vbr: bool,
    residual_bits: f32,
    room: bool,
) -> Result<(), Error> {
    let settings = EncoderSettings {
        residual_bits,
        vbr,
        ..Default::default()
    };
    let mut encoder = SeaEncoder::new(1, sample_rate, settings, receiver, sender)?;
    if room {
        // skip Start state in rooms
        encoder.state = SeaEncoderState::WritingFrames;
    }
    // encode frames until receiver closes
    while encoder.encode_frame().is_ok() {}
    info!("Encoder finished");
    Ok(())
}

pub(crate) fn decoder(
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    header: Option<SeaFileHeader>,
) -> Result<(), Error> {
    let mut decoder = SeaDecoder::new(receiver, sender, header)?;
    // decode frames until receiver closes
    while decoder.decode_frame().is_ok() {}
    info!("Decoder finished");
    Ok(())
}
