//! Audio codec encoding and decoding functions.
//!
//! This module provides functions for encoding and decoding audio using
//! the SEA codec. It wraps the sea_codec library and provides a simple
//! interface for the audio processing pipeline.
//!
//! ## Threading Model
//!
//! Both [`encoder`] and [`decoder`] are designed to run in dedicated threads.
//! They process frames in a loop until their input channel closes, making
//! them suitable for long-running audio streams.
//!
//! ## Frame Format
//!
//! - Input to encoder: `[i16; 480]` (480 samples, 10ms at 48kHz)
//! - Output from encoder: `Bytes` (variable length, depends on settings)
//! - Input to decoder: `Bytes` (encoded frames)
//! - Output from decoder: `[i16; 480]` (reconstructed samples)

use crate::error::AudioError;
use bytes::Bytes;
use kanal::{Receiver, Sender};
use log::info;
use nnnoiseless::FRAME_SIZE;
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
/// * `skip_start` - Whether this stream skips Start state
///
/// # Returns
///
/// Returns `Ok(())` when encoding completes, or an error if encoding fails.
pub fn encoder(
    receiver: Receiver<[i16; FRAME_SIZE]>,
    sender: Sender<Bytes>,
    sample_rate: u32,
    vbr: bool,
    residual_bits: f32,
    skip_start: bool,
) -> Result<(), AudioError> {
    let settings = EncoderSettings {
        residual_bits,
        vbr,
        ..Default::default()
    };
    let mut encoder = SeaEncoder::new(1, sample_rate, settings, receiver, sender)?;
    if skip_start {
        // skip Start state (will not send header)
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
/// * `header` - File header used when encoder skips start
///
/// # Returns
///
/// Returns `Ok(())` when decoding completes, or an error if decoding fails.
pub fn decoder(
    receiver: Receiver<Bytes>,
    sender: Sender<[i16; FRAME_SIZE]>,
    header: Option<SeaFileHeader>,
) -> Result<(), AudioError> {
    let mut decoder = SeaDecoder::new(receiver, sender, header)?;
    // decode frames until receiver closes
    while decoder.decode_frame().is_ok() {}
    info!("Decoder finished");
    Ok(())
}
