//! # SEA Codec - Streaming Audio Encoder/Decoder
//!
//! A low-complexity lossy audio codec using the LMS (Least Mean Squares) adaptive
//! filter algorithm, designed for real-time streaming audio in telepathy.
//!
//! ## Origin & License
//!
//! This module is based on the **SEA codec** (Simple Embedded Audio Codec) by
//! **Dani Biró**, originally published at <https://github.com/Daninet/sea-codec>
//! under the MIT License (Copyright © 2025 Dani Biró). The SEA codec itself draws
//! inspiration from the [QOA codec](https://qoaformat.org/).
//!
//! The original MIT license is fully compatible with this project's MIT license.
//!
//! ## Vendored & Modified
//!
//! This is a **vendored** (embedded) copy of the SEA codec, not an external
//! dependency. The following modifications were made to tailor it for real-time
//! streaming audio in telepathy-audio:
//!
//! - **Streaming API** — Converted from a file-based API to a frame-based streaming
//!   API suitable for real-time encoding and decoding.
//! - **Fixed Frame Size** — Hardcoded to 480 samples ([`nnnoiseless::FRAME_SIZE`])
//!   for consistent real-time processing.
//! - **Simplified Header** — Removed metadata support and the `total_frames` field,
//!   which are unnecessary in a streaming context.
//! - **Memory Management** — Integrated with [`bytes::BytesMut`] for efficient
//!   zero-copy buffer handling.
//! - **Removed File I/O** — Eliminated all file reading/writing in favour of
//!   in-memory encoding and decoding.
//! - **Decoder API** — Modified `samples_from_frame` to write into a fixed-size
//!   output buffer (`[i16; 480]`) instead of returning a `Vec`.
//!
//! ## Preserved from Upstream
//!
//! The core codec algorithm is **unchanged**:
//!
//! - LMS adaptive filter
//! - Original quantization tables (QT / DQT)
//! - CBR and VBR encoding modes
//! - Bitpacking implementation
//! - Scale factor encoding
//!
//! ## Usage
//!
//! This module is used internally by the audio processor
//! (`crate::internal::processor`) to compress captured audio frames before network
//! transmission and to decompress received frames for playback.
//!
//! ## Module Structure
//!
//! - [`encoder`] — Public streaming encoder API
//! - [`decoder`] — Public streaming decoder API
//! - [`codec`]   — Internal codec implementation (LMS, quantization, bitpacking,
//!   chunk format)
//!
//! ## References
//!
//! - [SEA codec repository](https://github.com/Daninet/sea-codec)
//! - [QOA codec format](https://qoaformat.org/)
//! - [SEA browser demo](https://daninet.github.io/sea-codec/)

pub mod codec;
pub mod decoder;
pub mod encoder;
