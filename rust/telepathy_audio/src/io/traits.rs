//! Trait-based data delivery abstractions for audio I/O.
//!
//! These traits decouple `telepathy_audio` from any specific channel implementation.
//! Consumers can provide their own sink/source implementations (e.g. flume, crossbeam,
//! tokio mpsc) while keeping the processing pipeline unchanged.

use crate::internal::buffer_pool::PooledBuffer;
use bytes::Bytes;

/// A destination for processed audio input data.
///
/// Implementations are used by the input processor thread to deliver each processed
/// audio frame (encoded or raw, depending on configuration).
pub trait AudioDataSink: Send + 'static {
    /// Deliver a processed audio buffer.
    fn send(&self, data: PooledBuffer) -> Result<(), ClosedOrFailed>;
}

impl AudioDataSink for Box<dyn Fn(PooledBuffer) + Send + 'static> {
    fn send(&self, data: PooledBuffer) -> Result<(), ClosedOrFailed> {
        self(data);
        Ok(())
    }
}

/// A source of audio output data.
///
/// Implementations are used by the output processor thread to receive audio frames
/// to be decoded (optional) and played.
pub trait AudioDataSource: Send + 'static {
    /// Receive the next audio frame (blocking).
    fn recv(&self) -> Result<Bytes, ClosedOrFailed>;

    /// Attempt to receive the next audio frame (non-blocking).
    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed>;
}

impl AudioDataSource for Box<dyn AudioDataSource> {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        (**self).recv()
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        (**self).try_recv()
    }
}

pub enum ClosedOrFailed {
    Closed,
    Failed(std::io::Error),
}
