//! Audio input/output traits and platform-specific implementations.
//!
//! This module defines the core traits for audio input and output operations,
//! along with platform-specific implementations using ring buffers and web audio.
//!
//! ## Traits
//!
//! - [`AudioInput`] - Trait for reading audio samples from a source
//! - [`AudioOutput`] - Trait for writing audio samples to a destination
//!
//! ## Implementations
//!
//! - [`RingBufferInput`] - Lock-free ring buffer input for native platforms (uses rtrb)
//! - [`RingBufferOutput`] - Lock-free ring buffer output for native platforms (uses rtrb)
//! - [`WebOutput`] - Shared buffer output for WASM (uses Arc<Mutex<Vec<f32>>>)
//!
//! ## Buffer Size
//!
//! [`CHANNEL_SIZE`] (2,400 samples) defines the buffer capacity for audio buffers.
//! At 48kHz, this represents 50ms of audio data.

use crate::error::AudioError;

use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::sync::{Condvar, Mutex};

/// Buffer size for audio data (2,400 samples).
///
/// This value is used as the capacity for:
/// - Native: rtrb ring buffer capacity (input and output)
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

/// Lock-free ring buffer audio input for native platforms.
///
/// Uses `rtrb::Consumer` with the chunks API for efficient bulk reads,
/// reducing per-sample overhead compared to channel-based approaches.
///
/// ## Blocking Behavior
///
/// The `read_into` implementation blocks using a `Condvar` when insufficient
/// samples are available. This is appropriate for the non-real-time processor
/// thread and prevents busy-waiting. The producer (audio stream callback)
/// notifies the condvar after each write, ensuring low-latency wakeup.
///
/// ## EOF Detection
///
/// When the producer is dropped, `is_abandoned()` returns `true`, signaling
/// end-of-stream.
#[cfg(not(target_family = "wasm"))]
pub struct RingBufferInput {
    /// The consumer end of the ring buffer for audio samples.
    consumer: rtrb::Consumer<f32>,

    /// Condvar for signaling when new samples are available.
    ///
    /// The producer (audio stream callback) calls `notify_one()` after writing
    /// samples. The consumer waits on this condvar when the ring buffer doesn't
    /// have enough samples available.
    notify: Arc<Condvar>,

    /// Mutex required for condvar wait protocol.
    ///
    /// This is a dummy mutex (contains no data) used solely to satisfy the
    /// condvar API requirements. The actual synchronization is handled by the
    /// lock-free ring buffer.
    mutex: Mutex<()>,
}

#[cfg(not(target_family = "wasm"))]
impl RingBufferInput {
    pub fn new(consumer: rtrb::Consumer<f32>, notify: Arc<Condvar>) -> Self {
        Self {
            consumer,
            notify,
            mutex: Mutex::new(()),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl AudioInput for RingBufferInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError> {
        // block until enough slots are available
        let target = dst.len();
        loop {
            let available = self.consumer.slots();
            if available >= target {
                break; // there are enough slots available
            } else if self.consumer.is_abandoned() {
                return Ok(0); // EOF
            }
            let guard = self.mutex.lock().unwrap();
            drop(self.notify.wait(guard).unwrap());
            continue;
        }
        // read at most the number of samples that will fit in dst
        let chunk = self.consumer.read_chunk(target)?;
        // written will be <= dst.len()
        let written = chunk.len();
        // copy samples into dst, consuming chunk
        for (i, o) in chunk.into_iter().zip(dst) {
            *o = i;
        }
        Ok(written)
    }
}

/// Lock-free ring buffer audio output for native platforms.
///
/// Uses `rtrb::Producer` with the chunks API (`write_chunk_uninit()`) for
/// efficient bulk writes, eliminating per-sample overhead compared to
/// channel-based approaches.
///
/// ## Non-blocking Behavior
///
/// Unlike [`RingBufferInput`], this output is fully non-blocking and does not
/// use a `Condvar`. When the ring buffer is full, samples are simply dropped
/// rather than blocking the producer thread. This is the correct behavior for
/// audio output: it is better to drop samples than to block the processor
/// thread and cause cascading latency.
///
/// ## Chunk-based Writes
///
/// The `write_samples` implementation uses `write_chunk_uninit()` to acquire a
/// contiguous writable region, then fills it using `fill_from_iter()`. The chunk
/// is automatically committed when dropped, requiring only 1-2 atomic operations
/// regardless of sample count.
#[cfg(not(target_family = "wasm"))]
pub struct RingBufferOutput {
    /// The producer end of the ring buffer for audio samples.
    producer: rtrb::Producer<f32>,
}

#[cfg(not(target_family = "wasm"))]
impl RingBufferOutput {
    /// Creates a new `RingBufferOutput` wrapping the given producer.
    ///
    /// # Arguments
    ///
    /// * `producer` - The producer end of an `rtrb::RingBuffer<f32>`
    pub fn new(producer: rtrb::Producer<f32>) -> Self {
        Self { producer }
    }
}

#[cfg(not(target_family = "wasm"))]
impl AudioOutput for RingBufferOutput {
    fn is_full(&self) -> bool {
        self.producer.slots() == 0
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
        let available = self.producer.slots();
        if available == 0 {
            return Ok(samples.len());
        }

        let to_write = samples.len().min(available);
        match self.producer.write_chunk_uninit(to_write) {
            Ok(chunk) => {
                chunk.fill_from_iter(samples[..to_write].iter().copied());
                Ok(samples.len() - to_write)
            }
            Err(_) => Ok(samples.len()),
        }
    }
}

/// Web-based audio output using a shared buffer.
///
/// This struct provides audio output for WASM targets by writing samples to a
/// shared buffer that can be consumed by cpal
///
/// ## Buffer Behavior
///
/// - Maximum capacity: [`CHANNEL_SIZE`] (2,400 samples)
/// - [`is_full()`](Self::is_full) returns `true` when buffer length >= CHANNEL_SIZE
/// - [`write_samples()`](AudioOutput::write_samples) drops samples when buffer is full
/// - The buffer should be drained by the consumer regularly
#[cfg(target_family = "wasm")]
#[derive(Default)]
pub struct WebOutput {
    /// The shared buffer for audio samples.
    ///
    /// Protected by a mutex for thread-safe access between the processor
    /// thread and the JavaScript audio callback.
    pub buf: Arc<wasm_sync::Mutex<Vec<f32>>>,
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
