//! Audio input/output traits and channel-based implementations.
//!
//! This module defines the core traits for audio input and output operations,
//! along with platform-specific implementations using channels and web audio.
//!
//! ## Traits
//!
//! - [`AudioInput`] - Trait for reading audio samples from a source
//! - [`AudioOutput`] - Trait for writing audio samples to a destination
//!
//! ## Implementations
//!
//! - [`ChannelInput`] - Channel-based input for native platforms (uses kanal)
//! - [`ChannelOutput`] - Channel-based output for native platforms (uses kanal)
//! - [`WebOutput`] - Shared buffer output for WASM (uses Arc<Mutex<Vec<f32>>>)
//!
//! ## Buffer Size
//!
//! [`CHANNEL_SIZE`] (2,400 samples) defines the buffer capacity for audio channels.
//! At 48kHz, this represents 50ms of audio data.

use crate::error::AudioError;

#[cfg(not(target_family = "wasm"))]
use kanal::{Receiver, Sender};
#[cfg(target_family = "wasm")]
use std::sync::Arc;

/// Channel buffer size for audio data (2,400 samples).
///
/// This value is used as the capacity for:
/// - Native: kanal channel bounds
/// - WASM: WebOutput shared buffer maximum size
///
/// At 48kHz sample rate, this represents 50ms of audio (2400 / 48000 = 0.05s).
pub const CHANNEL_SIZE: usize = 2_400;

/// Trait for reading audio samples from an input source.
pub trait AudioInput {
    /// Attempts to fill `dst` with samples.
    ///
    /// Returns:
    /// - `Ok(n)`: number of samples written (0 means end-of-stream)
    /// - `Err(_)`: an error occurred during reading
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError>;
}

/// Trait for writing audio samples to an output destination.
pub trait AudioOutput {
    /// Returns true if pushing more audio would overflow / backlog too much.
    fn is_full(&self) -> bool;

    /// Writes as many samples as it can.
    /// Returns how many samples were dropped (loss).
    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError>;
}

/// Channel-based audio input for native platforms.
#[cfg(not(target_family = "wasm"))]
pub struct ChannelInput {
    /// The receiver for audio samples.
    pub receiver: Receiver<f32>,
}

/// Channel-based audio output for native platforms.
#[cfg(not(target_family = "wasm"))]
pub struct ChannelOutput {
    /// The sender for audio samples.
    pub sender: Sender<f32>,
}

/// Web-based audio output using a shared buffer.
///
/// This struct provides audio output for WASM targets by writing samples to a
/// shared buffer that can be consumed by a Web Audio API AudioWorklet or
/// ScriptProcessorNode.
///
/// ## Architecture
///
/// ```text
/// ┌─────────────┐     ┌──────────────────┐     ┌──────────────┐
/// │ Processor   │────▶│ WebOutput        │────▶│ AudioWorklet │
/// │ Thread      │     │ (shared buffer)  │     │ (JS)         │
/// └─────────────┘     └──────────────────┘     └──────────────┘
///                      Arc<Mutex<Vec<f32>>>
/// ```
///
/// ## Buffer Behavior
///
/// - Maximum capacity: [`CHANNEL_SIZE`] (2,400 samples)
/// - [`is_full()`](Self::is_full) returns `true` when buffer length >= CHANNEL_SIZE
/// - [`write_samples()`](AudioOutput::write_samples) drops samples when buffer is full
/// - The buffer should be drained by the consumer (AudioWorklet) regularly
///
/// ## Example Usage
///
/// ```rust,no_run
/// # #[cfg(target_family = "wasm")]
/// # fn example() {
/// use std::sync::Arc;
/// use wasm_sync::Mutex;
///
/// // Create shared buffer
/// let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
///
/// // Pass to WebOutput (internal use)
/// // let output = WebOutput::new(buffer.clone());
///
/// // In AudioWorklet: drain samples from buffer
/// // if let Ok(mut data) = buffer.lock() {
/// //     let samples: Vec<f32> = data.drain(..).collect();
/// //     // ... use samples
/// // }
/// # }
/// ```
#[cfg(target_family = "wasm")]
#[derive(Default)]
pub struct WebOutput {
    /// The shared buffer for audio samples.
    ///
    /// Protected by a mutex for thread-safe access between the processor
    /// thread and the JavaScript audio callback.
    pub buf: Arc<wasm_sync::Mutex<Vec<f32>>>,
}

#[cfg(not(target_family = "wasm"))]
impl From<Receiver<f32>> for ChannelInput {
    fn from(receiver: Receiver<f32>) -> Self {
        Self { receiver }
    }
}

#[cfg(not(target_family = "wasm"))]
impl AudioInput for ChannelInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError> {
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

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
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
    /// Creates a new WebOutput with the given shared buffer.
    ///
    /// Pre-allocates capacity up to [`CHANNEL_SIZE`] to avoid reallocations
    /// during audio processing.
    ///
    /// # Arguments
    ///
    /// * `buf` - Shared buffer that will be written to by the processor and
    ///   read by the Web Audio API consumer
    pub fn new(buf: Arc<wasm_sync::Mutex<Vec<f32>>>) -> Self {
        // This buffer is bounded by CHANNEL_SIZE; reserve upfront to avoid growth reallocations.
        if let Ok(mut data) = buf.lock() {
            let current_capacity = data.capacity();
            if current_capacity < CHANNEL_SIZE {
                data.reserve(CHANNEL_SIZE - current_capacity);
            }
        }

        Self { buf }
    }
}

#[cfg(target_family = "wasm")]
impl AudioOutput for WebOutput {
    /// Returns `true` if the buffer has reached capacity ([`CHANNEL_SIZE`]).
    ///
    /// When full, callers should either wait for the consumer to drain samples
    /// or accept that new samples will be dropped.
    fn is_full(&self) -> bool {
        self.buf
            .lock()
            .map(|data| data.len() >= CHANNEL_SIZE)
            .unwrap_or(true)
    }

    /// Writes samples to the shared buffer, dropping excess if full.
    ///
    /// # Returns
    ///
    /// Returns the number of samples that were **dropped** (not written).
    /// - `Ok(0)` means all samples were written successfully
    /// - `Ok(n)` means `n` samples were dropped due to buffer being full
    ///
    /// # Behavior
    ///
    /// - If buffer is at capacity, returns `samples.len()` (all dropped)
    /// - Otherwise, writes as many samples as space allows
    /// - Never grows buffer beyond [`CHANNEL_SIZE`]
    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
        let mut data = self
            .buf
            .lock()
            .map_err(|e| AudioError::Processing(e.to_string()))?;

        if data.len() >= CHANNEL_SIZE {
            return Ok(samples.len());
        }

        let space = CHANNEL_SIZE - data.len();
        let take = space.min(samples.len());
        // Ensure we never grow past CHANNEL_SIZE without reserving.
        let needed = data.len() + take;
        let current_capacity = data.capacity();
        if current_capacity < needed {
            data.reserve(needed - current_capacity);
        }
        data.extend_from_slice(&samples[..take]);

        Ok(samples.len() - take)
    }
}
