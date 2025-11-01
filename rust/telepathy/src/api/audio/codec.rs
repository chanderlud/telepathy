use kanal::{Receiver, Sender};
use log::{info, warn};
use sea_codec::ProcessorMessage;
use sea_codec::decoder::SeaDecoder;
use sea_codec::encoder::{EncoderSettings, SeaEncoder};

pub(crate) fn encoder(
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    sample_rate: u32,
    vbr: bool,
    residual_bits: f32,
) {
    let settings = EncoderSettings {
        frames_per_chunk: 480,
        scale_factor_frames: 20,
        residual_bits,
        vbr,
        ..Default::default()
    };

    if let Ok(mut encoder) = SeaEncoder::new(1, sample_rate, settings, receiver, sender) {
        while encoder.encode_frame().is_ok() {}
        info!("Encoder finished");
    } else {
        warn!("Encoder did not start successfully");
    }
}

pub(crate) fn decoder(receiver: Receiver<ProcessorMessage>, sender: Sender<ProcessorMessage>) {
    if let Ok(mut decoder) = SeaDecoder::new(receiver, sender) {
        while decoder.decode_frame().is_ok() {}
        info!("Decoder finished");
    } else {
        warn!("Decoder did not start successfully");
    }
}
