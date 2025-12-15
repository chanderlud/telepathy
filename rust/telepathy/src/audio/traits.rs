#[cfg(target_family = "wasm")]
use crate::audio::WebOutput;
#[cfg(not(target_family = "wasm"))]
use crate::audio::{ChannelInput, ChannelOutput};
use crate::error::Error;
#[cfg(target_family = "wasm")]
use crate::telepathy::CHANNEL_SIZE;
#[cfg(not(target_family = "wasm"))]
use kanal::{Receiver, Sender};
#[cfg(target_family = "wasm")]
use std::sync::Arc;

pub(crate) trait AudioInput {
    /// attempt to fill `dst` with samples.
    ///
    /// returns:
    /// - Ok(n): number of samples written (0 means end-of-stream)
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error>;
}

pub(crate) trait AudioOutput {
    /// true if pushing more audio would overflow / backlog too much
    fn is_full(&self) -> bool;

    /// writes as many samples as it can.
    /// returns how many samples were dropped (loss)
    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error>;
}

#[cfg(not(target_family = "wasm"))]
impl From<Receiver<f32>> for ChannelInput {
    fn from(receiver: Receiver<f32>) -> Self {
        Self { receiver }
    }
}

#[cfg(not(target_family = "wasm"))]
impl AudioInput for ChannelInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
        let mut written = 0;

        for slot in dst.iter_mut() {
            match self.receiver.recv() {
                Ok(sample) => {
                    *slot = sample;
                    written += 1;
                }
                Err(_) => break, // channel closed
            }
        }

        Ok(written) // 0 => end-of-stream
    }
}

#[cfg(not(target_family = "wasm"))]
impl From<Sender<f32>> for ChannelOutput {
    fn from(sender: Sender<f32>) -> Self {
        Self { sender }
    }
}

#[cfg(not(target_family = "wasm"))]
impl AudioOutput for ChannelOutput {
    fn is_full(&self) -> bool {
        self.sender.is_full()
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        let mut failed = 0usize;

        for &sample in samples {
            if !self.sender.try_send(sample)? {
                failed += 1;
            }
        }

        Ok(failed)
    }
}

#[cfg(target_family = "wasm")]
impl WebOutput {
    pub(crate) fn new(buf: Arc<wasm_sync::Mutex<Vec<f32>>>) -> Self {
        Self { buf }
    }
}

#[cfg(target_family = "wasm")]
impl AudioOutput for WebOutput {
    fn is_full(&self) -> bool {
        self.buf
            .lock()
            .map(|data| data.len() >= CHANNEL_SIZE)
            .unwrap_or(true)
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        let mut data = self.buf.lock().unwrap();

        if data.len() >= CHANNEL_SIZE {
            return Ok(samples.len());
        }

        let space = CHANNEL_SIZE - data.len();
        let take = space.min(samples.len());
        data.extend_from_slice(&samples[..take]);

        Ok(samples.len() - take)
    }
}
