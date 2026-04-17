//! Shared test doubles and helpers for telepathy-audio integration tests.

#![cfg(not(target_family = "wasm"))]
#![allow(dead_code)]

use atomic_float::AtomicF32;
use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use telepathy_audio::Error;
use telepathy_audio::internal::buffer_pool::DEFAULT_POOL_CAPACITY;
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_audio::io::traits::{AudioDataSource, ClosedOrFailed};

pub const TEST_SAMPLE_RATE: usize = 48_000;

/// Generates a 440 Hz sine wave at half amplitude for a fixed number of samples.
pub struct TestAudioInput {
    samples_remaining: usize,
    phase: f64,
}

impl TestAudioInput {
    pub fn new(total_samples: usize) -> Self {
        Self {
            samples_remaining: total_samples,
            phase: 0.0,
        }
    }
}

impl AudioInput for TestAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
        if self.samples_remaining == 0 {
            return Ok(0);
        }

        let to_read = dst.len().min(self.samples_remaining);
        for sample in dst.iter_mut().take(to_read) {
            *sample = (self.phase.sin() as f32) * 0.5;
            self.phase += 2.0 * std::f64::consts::PI * 440.0 / TEST_SAMPLE_RATE as f64;
        }
        self.samples_remaining -= to_read;
        Ok(to_read)
    }
}

/// Counting output that records the number of samples and frames written.
pub struct TestAudioOutput {
    samples_received: Arc<AtomicUsize>,
    frames_received: Arc<AtomicUsize>,
}

impl TestAudioOutput {
    pub fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let samples = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        (
            Self {
                samples_received: samples.clone(),
                frames_received: frames.clone(),
            },
            samples,
            frames,
        )
    }
}

impl AudioOutput for TestAudioOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        self.samples_received
            .fetch_add(samples.len(), Ordering::Relaxed);
        self.frames_received.fetch_add(1, Ordering::Relaxed);
        Ok(0)
    }
}

/// Output that always reports itself as full, used to drive loss-tracking tests.
pub struct FullAudioOutput;

impl AudioOutput for FullAudioOutput {
    fn is_full(&self) -> bool {
        true
    }

    fn write_samples(&mut self, _samples: &[f32]) -> Result<usize, Error> {
        Ok(0)
    }
}

/// `AudioDataSource` backed by a pre-populated queue of encoded frames.
pub struct QueueSource {
    inner: Mutex<VecDeque<Bytes>>,
}

impl QueueSource {
    pub fn new(frames: Vec<Bytes>) -> Self {
        Self {
            inner: Mutex::new(frames.into()),
        }
    }
}

impl AudioDataSource for QueueSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        let mut guard = self.inner.lock().unwrap();
        guard.pop_front().ok_or(ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        let mut guard = self.inner.lock().unwrap();
        Ok(guard.pop_front())
    }
}

/// Builds an [`InputProcessorState`] wired to caller-owned atomics.
pub fn make_input_state(
    input_volume: &Arc<AtomicF32>,
    rms_threshold: &Arc<AtomicF32>,
    muted: &Arc<AtomicBool>,
    rms_sender: Arc<AtomicF32>,
) -> InputProcessorState {
    InputProcessorState::new(
        input_volume,
        rms_threshold,
        muted,
        rms_sender,
        DEFAULT_POOL_CAPACITY,
    )
}

/// Builds an [`OutputProcessorState`] wired to caller-owned atomics.
pub fn make_output_state(
    output_volume: &Arc<AtomicF32>,
    rms_sender: Arc<AtomicF32>,
    deafened: &Arc<AtomicBool>,
    loss_sender: Arc<AtomicUsize>,
) -> OutputProcessorState {
    OutputProcessorState::new(output_volume, rms_sender, deafened, loss_sender)
}
