//! Channel adapter implementations.
//!
//! `telepathy-audio` is trait-based (`AudioDataSink` / `AudioDataSource`) and does not require
//! any specific channel crate. This module provides **ready-to-use** adapters for
//! `std::sync::mpsc` so you can get started quickly without writing boilerplate.
//!
//! ## When to use `std::sync::mpsc`
//!
//! `std::sync::mpsc` is a good default for:
//! - Simple native apps and quick prototypes
//! - A small number of channels at moderate throughput
//!
//! For higher-throughput or more advanced channel features (multi-producer / multi-consumer,
//! bounded queues with better performance characteristics, select, etc.), consider using
//! `crossbeam` (or another channel crate) and implementing the traits yourself.
//!
//! ## Example
//!
//! ```rust,no_run
//! use bytes::Bytes;
//! use std::sync::mpsc;
//! use telepathy_audio::{AudioHost, AudioOutputBuilder};
//! use telepathy_audio::adapters::MpscSource;
//!
//! let host = AudioHost::new();
//! let (_tx, rx) = mpsc::channel::<Bytes>();
//! let _output = AudioOutputBuilder::new()
//!     .sample_rate(48_000)
//!     .source(MpscSource::new(rx))
//!     .build(&host)
//!     .unwrap();
//! ```
//!
//! For input delivery, use [`MpscSink`].

use crate::internal::buffer_pool::PooledBuffer;
use crate::io::traits::{AudioDataSink, AudioDataSource, ClosedOrFailed};
use bytes::Bytes;
use std::sync::mpsc;

/// `std::sync::mpsc` adapter implementing [`AudioDataSink`].
///
/// Wraps an `mpsc::Sender<PooledBuffer>` and maps send failures to
/// [`ClosedOrFailed::Closed`].
#[derive(Clone)]
pub struct MpscSink(mpsc::Sender<PooledBuffer>);

impl MpscSink {
    /// Create a new sink from an `mpsc::Sender`.
    pub fn new(sender: mpsc::Sender<PooledBuffer>) -> Self {
        Self(sender)
    }
}

impl AudioDataSink for MpscSink {
    fn send(&self, data: PooledBuffer) -> Result<(), ClosedOrFailed> {
        self.0.send(data).map_err(|_| ClosedOrFailed::Closed)
    }
}

/// `std::sync::mpsc` adapter implementing [`AudioDataSource`].
///
/// Wraps an `mpsc::Receiver<Bytes>` and maps receive failures to
/// [`ClosedOrFailed::Closed`].
pub struct MpscSource(mpsc::Receiver<Bytes>);

impl MpscSource {
    /// Create a new source from an `mpsc::Receiver`.
    pub fn new(receiver: mpsc::Receiver<Bytes>) -> Self {
        Self(receiver)
    }
}

impl AudioDataSource for MpscSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        self.0.recv().map_err(|_| ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        match self.0.try_recv() {
            Ok(b) => Ok(Some(b)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(ClosedOrFailed::Closed),
        }
    }
}
