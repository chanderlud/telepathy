//! Shared test doubles and helpers for telepathy-audio integration tests.

#![cfg(not(target_family = "wasm"))]
#![allow(dead_code)]

use atomic_float::AtomicF32;
use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use telepathy_audio::Error;
use telepathy_audio::FRAME_SIZE;
use telepathy_audio::internal::buffer_pool::DEFAULT_POOL_CAPACITY;
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_audio::io::traits::{AudioDataSource, ClosedOrFailed};

pub const TEST_SAMPLE_RATE: usize = 48_000;

pub fn patterned_samples(total_samples: usize) -> Vec<f32> {
    (0..total_samples)
        .map(|idx| {
            let value = ((idx as i32 * 173) % 32_000) - 16_000;
            value as f32 / i16::MAX as f32
        })
        .collect()
}

pub fn raw_frame_from_i16(samples: &[i16]) -> Bytes {
    let mut frame = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        frame.extend_from_slice(&sample.to_ne_bytes());
    }
    Bytes::from(frame)
}

pub fn raw_frame_with_start(start: i16) -> Bytes {
    let samples: Vec<i16> = (0..FRAME_SIZE).map(|idx| start + idx as i16).collect();
    raw_frame_from_i16(&samples)
}

pub fn bytes_to_i16_samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Generates a 440 Hz sine wave at half amplitude for a fixed number of samples.
pub struct TestAudioInput {
    samples_remaining: usize,
    phase: f64,
}

pub struct PatternAudioInput {
    samples: Vec<f32>,
    position: usize,
}

pub struct SineSource {
    state: Mutex<SineSourceState>,
}

impl SineSource {
    pub fn new(frames: usize, sample_rate: usize, frequency_hz: f64, amplitude: f32) -> Self {
        Self {
            state: Mutex::new(SineSourceState {
                frames_remaining: frames,
                phase: 0.0,
                phase_step: 2.0 * std::f64::consts::PI * frequency_hz / sample_rate as f64,
                amplitude,
            }),
        }
    }
}

impl PatternAudioInput {
    pub fn new(samples: Vec<f32>) -> Self {
        Self {
            samples,
            position: 0,
        }
    }
}

impl AudioInput for PatternAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
        if self.position >= self.samples.len() {
            return Ok(0);
        }

        let to_read = dst.len().min(self.samples.len() - self.position);
        dst[..to_read].copy_from_slice(&self.samples[self.position..self.position + to_read]);
        self.position += to_read;
        Ok(to_read)
    }
}

impl AudioDataSource for SineSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        self.try_recv()?.ok_or(ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        let mut state = self.state.lock().unwrap();
        if state.frames_remaining == 0 {
            return Ok(None);
        }

        let mut frame = Vec::with_capacity(FRAME_SIZE * 2);
        for _ in 0..FRAME_SIZE {
            let sample = (state.phase.sin() as f32 * state.amplitude * i16::MAX as f32)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            frame.extend_from_slice(&sample.to_ne_bytes());
            state.phase += state.phase_step;
        }
        state.frames_remaining -= 1;
        Ok(Some(Bytes::from(frame)))
    }
}

struct SineSourceState {
    frames_remaining: usize,
    phase: f64,
    phase_step: f64,
    amplitude: f32,
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

#[derive(Clone)]
pub struct RecordingAudioOutput {
    samples: Arc<Mutex<Vec<Vec<f32>>>>,
}

impl RecordingAudioOutput {
    pub fn new() -> (Self, Arc<Mutex<Vec<Vec<f32>>>>) {
        let samples = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                samples: samples.clone(),
            },
            samples,
        )
    }
}

impl AudioOutput for RecordingAudioOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        self.samples.lock().unwrap().push(samples.to_vec());
        Ok(0)
    }
}

pub struct FailingAudioOutput;

impl AudioOutput for FailingAudioOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, _samples: &[f32]) -> Result<usize, Error> {
        Err(Error::Processing("intentional output failure".to_string()))
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

/// Output that always reports full and records any `write_samples` calls.
///
/// Used to verify the processor skips writes when the sink is full.
#[derive(Clone)]
pub struct RecordingFullOutput {
    recorded: Arc<Mutex<Vec<Vec<f32>>>>,
}

impl RecordingFullOutput {
    pub fn new() -> (Self, Arc<Mutex<Vec<Vec<f32>>>>) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                recorded: recorded.clone(),
            },
            recorded,
        )
    }
}

impl AudioOutput for RecordingFullOutput {
    fn is_full(&self) -> bool {
        true
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        self.recorded.lock().unwrap().push(samples.to_vec());
        Ok(0)
    }
}

/// Test double whose `is_full()` responses follow a pre-programmed schedule.
///
/// Each call to `is_full()` pops the next scheduled value (defaulting to `false`
/// when the schedule is exhausted). When the popped value is `true`, the
/// `dropped_frames` counter increments. `write_samples` always records and
/// increments `written_frames` when invoked.
#[derive(Clone)]
pub struct PartiallyFullOutput {
    schedule: Arc<Mutex<VecDeque<bool>>>,
    dropped_frames: Arc<AtomicUsize>,
    written_frames: Arc<AtomicUsize>,
    recorded: Arc<Mutex<Vec<Vec<f32>>>>,
}

impl PartiallyFullOutput {
    pub fn new(
        fullness_schedule: impl Into<VecDeque<bool>>,
    ) -> (
        Self,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<Mutex<Vec<Vec<f32>>>>,
    ) {
        let dropped_frames = Arc::new(AtomicUsize::new(0));
        let written_frames = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                schedule: Arc::new(Mutex::new(fullness_schedule.into())),
                dropped_frames: dropped_frames.clone(),
                written_frames: written_frames.clone(),
                recorded: recorded.clone(),
            },
            dropped_frames,
            written_frames,
            recorded,
        )
    }
}

impl AudioOutput for PartiallyFullOutput {
    fn is_full(&self) -> bool {
        let full = self.schedule.lock().unwrap().pop_front().unwrap_or(false);
        if full {
            self.dropped_frames.fetch_add(1, Ordering::Relaxed);
        }
        full
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        self.written_frames.fetch_add(1, Ordering::Relaxed);
        self.recorded.lock().unwrap().push(samples.to_vec());
        Ok(0)
    }
}

pub struct PartialWriteOutput {
    loss_per_write: usize,
    writes: Arc<AtomicUsize>,
    recorded: Arc<Mutex<Vec<Vec<f32>>>>,
}

impl PartialWriteOutput {
    pub fn new(loss_per_write: usize) -> (Self, Arc<AtomicUsize>, Arc<Mutex<Vec<Vec<f32>>>>) {
        let writes = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                loss_per_write,
                writes: writes.clone(),
                recorded: recorded.clone(),
            },
            writes,
            recorded,
        )
    }
}

impl AudioOutput for PartialWriteOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.recorded.lock().unwrap().push(samples.to_vec());
        Ok(self.loss_per_write)
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

pub struct FailingSource;

impl AudioDataSource for FailingSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        Err(ClosedOrFailed::Failed(std::io::Error::other(
            "intentional source failure",
        )))
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        Err(ClosedOrFailed::Failed(std::io::Error::other(
            "intentional source failure",
        )))
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
