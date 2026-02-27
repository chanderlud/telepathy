//! Channel adapters for `telepathy_audio` I/O traits.
//!
//! `telepathy_audio` is intentionally decoupled from any specific channel library.
//! The main `telepathy` crate continues to use `kanal` internally by providing
//! adapters that implement the trait-based I/O surface.

use bytes::Bytes;
use telepathy_audio::{AudioDataSink, AudioDataSource, ClosedOrFailed, PooledBuffer};

/// An `AudioDataSink` backed by a `kanal` channel.
pub struct KanalSink {
    sender: kanal::Sender<PooledBuffer>,
}

impl KanalSink {
    pub fn new(sender: kanal::AsyncSender<PooledBuffer>) -> Self {
        Self {
            sender: sender.to_sync(),
        }
    }
}

impl AudioDataSink for KanalSink {
    fn send(&self, data: PooledBuffer) -> Result<(), ClosedOrFailed> {
        self.sender.send(data).map_err(|_| ClosedOrFailed::Closed)
    }
}

/// An `AudioDataSource` backed by a `kanal` channel.
pub struct KanalSource {
    receiver: kanal::Receiver<Bytes>,
}

impl KanalSource {
    pub fn new(receiver: kanal::Receiver<Bytes>) -> Self {
        Self { receiver }
    }
}

impl AudioDataSource for KanalSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        self.receiver.recv().map_err(|_| ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        self.receiver.try_recv().map_err(|_| ClosedOrFailed::Closed)
    }
}
