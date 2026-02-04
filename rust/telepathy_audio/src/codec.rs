//! Audio codec encoding and decoding functions.
//!
//! This module provides functions for encoding and decoding audio using
//! the SEA codec. It wraps the sea_codec library and provides a simple
//! interface for the audio processing pipeline.

use crate::error::AudioError;
use kanal::{Receiver, Sender};
use log::info;
use sea_codec::ProcessorMessage;
use sea_codec::codec::file::SeaFileHeader;
use sea_codec::decoder::SeaDecoder;
use sea_codec::encoder::{EncoderSettings, SeaEncoder, SeaEncoderState};

/// Encodes audio frames using the SEA codec.
///
/// This function creates an encoder and processes frames until the
/// receiver channel closes.
///
/// # Arguments
///
/// * `receiver` - Channel receiver for input audio frames
/// * `sender` - Channel sender for encoded frames
/// * `sample_rate` - Sample rate of the audio
/// * `vbr` - Whether to use variable bit rate encoding
/// * `residual_bits` - Number of residual bits for encoding quality
/// * `room` - Whether this is a room call (skips Start state)
///
/// # Returns
///
/// Returns `Ok(())` when encoding completes, or an error if encoding fails.
pub fn encoder(
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    sample_rate: u32,
    vbr: bool,
    residual_bits: f32,
    room: bool,
) -> Result<(), AudioError> {
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

/// Decodes audio frames using the SEA codec.
///
/// This function creates a decoder and processes frames until the
/// receiver channel closes.
///
/// # Arguments
///
/// * `receiver` - Channel receiver for encoded audio frames
/// * `sender` - Channel sender for decoded frames
/// * `header` - Optional file header for rooms (hard-coded configuration)
///
/// # Returns
///
/// Returns `Ok(())` when decoding completes, or an error if decoding fails.
pub fn decoder(
    receiver: Receiver<ProcessorMessage>,
    sender: Sender<ProcessorMessage>,
    header: Option<SeaFileHeader>,
) -> Result<(), AudioError> {
    let mut decoder = SeaDecoder::new(receiver, sender, header)?;
    // decode frames until receiver closes
    while decoder.decode_frame().is_ok() {}
    info!("Decoder finished");
    Ok(())
}
