use kanal::{Receiver, Sender};
use log::{info, warn};
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
) {
    let settings = EncoderSettings {
        frames_per_chunk: 480,
        scale_factor_frames: 20,
        residual_bits,
        vbr,
        ..Default::default()
    };

    if let Ok(mut encoder) = SeaEncoder::new(1, sample_rate, settings, receiver, sender) {
        if room {
            // skip Start state in rooms
            encoder.state = SeaEncoderState::WritingFrames;
        }

        while encoder.encode_frame().is_ok() {}
        info!("Encoder finished");
    } else {
        warn!("Encoder did not start successfully");
    }
}

pub(crate) fn decoder(
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    header: Option<SeaFileHeader>,
) {
    if let Ok(mut decoder) = SeaDecoder::new(receiver, sender, header) {
        while decoder.decode_frame().is_ok() {}
        info!("Decoder finished");
    } else {
        warn!("Decoder did not start successfully");
    }
}
