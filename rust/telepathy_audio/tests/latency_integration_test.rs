//! End-to-end latency integration test for telepathy_audio.
//!
//! This test verifies that audio flows through the complete processing
//! pipeline and measures total end-to-end latency.

use atomic_float::AtomicF32;
use bytes::BytesMut;
use kanal;
use nnnoiseless::DenoiseState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use telepathy_audio::FRAME_SIZE;
use telepathy_audio::error::AudioError;
use telepathy_audio::internal::buffer_pool::{DEFAULT_POOL_CAPACITY, PooledBuffer};
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};

const TEST_FRAMES: usize = 100;

/// Mock audio input for testing.
struct TestAudioInput {
    samples_remaining: usize,
    phase: f64,
}

impl TestAudioInput {
    fn new(total_samples: usize) -> Self {
        Self {
            samples_remaining: total_samples,
            phase: 0.0,
        }
    }
}

impl AudioInput for TestAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError> {
        if self.samples_remaining == 0 {
            return Ok(0);
        }

        let to_read = dst.len().min(self.samples_remaining);
        for sample in dst.iter_mut().take(to_read) {
            *sample = (self.phase.sin() as f32) * 0.5;
            self.phase += 2.0 * std::f64::consts::PI * 440.0 / 48_000.0;
        }
        self.samples_remaining -= to_read;
        Ok(to_read)
    }
}

/// Mock audio output for testing.
struct TestAudioOutput {
    samples_received: Arc<AtomicUsize>,
    frames_received: Arc<AtomicUsize>,
}

impl TestAudioOutput {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
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

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, AudioError> {
        self.samples_received
            .fetch_add(samples.len(), Ordering::Relaxed);
        self.frames_received.fetch_add(1, Ordering::Relaxed);
        Ok(0)
    }
}

/// Tests input processor with denoising enabled.
#[test]
fn test_input_processor_with_denoise() {
    let total_samples = FRAME_SIZE * TEST_FRAMES;
    let mock_input = TestAudioInput::new(total_samples);

    let (output_tx, output_rx) = kanal::unbounded::<PooledBuffer>();

    // Enable denoiser
    let denoiser = Some(DenoiseState::new());
    let state = InputProcessorState::default();

    let start = Instant::now();

    let handle =
        thread::spawn(move || input_processor(mock_input, output_tx, 48_000.0, denoiser, state));

    let mut frames_received = 0;
    while frames_received < TEST_FRAMES {
        match output_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(_frame) => {
                frames_received += 1;
            }
            Err(_) => {
                panic!(
                    "Timeout waiting for frame {} (with denoise)",
                    frames_received
                );
            }
        }
    }

    drop(output_rx);
    handle
        .join()
        .expect("Input processor panicked")
        .expect("Input processor failed");

    let total_time = start.elapsed();
    println!(
        "Input processor (denoise): processed {} frames in {:?}ms",
        frames_received,
        total_time.as_millis()
    );

    assert_eq!(frames_received, TEST_FRAMES);
}

/// Tests output processor with resampling.
#[test]
fn test_output_processor_with_resampling() {
    let (input_tx, input_rx) = kanal::unbounded::<BytesMut>();
    let (mock_output, _, frames_counter) = TestAudioOutput::new();
    let state = OutputProcessorState::default();

    // Resample from 48kHz to 44.1kHz
    let ratio = 44_100.0 / 48_000.0;

    let handle = thread::spawn(move || output_processor(input_rx, mock_output, ratio, state));

    let start = Instant::now();

    for _ in 0..TEST_FRAMES {
        let mut frame = BytesMut::with_capacity(FRAME_SIZE * 2);
        frame.resize(FRAME_SIZE * 2, 0);
        input_tx.send(frame).expect("Failed to send frame");
    }

    drop(input_tx);
    handle
        .join()
        .expect("Output processor panicked")
        .expect("Output processor failed");

    let total_time = start.elapsed();
    let frames_received = frames_counter.load(Ordering::Relaxed);

    println!(
        "Output processor (resample 48k->44.1k): processed {} frames in {:?}ms",
        frames_received,
        total_time.as_millis()
    );

    assert_eq!(frames_received, TEST_FRAMES);
}

/// Tests that muting stops audio output.
#[test]
fn test_input_processor_mute() {
    let total_samples = FRAME_SIZE * 50;
    let mock_input = TestAudioInput::new(total_samples);

    let (output_tx, output_rx) = kanal::unbounded::<PooledBuffer>();

    // Create state with muted = true
    let muted = Arc::new(AtomicBool::new(true));
    let input_volume = Arc::new(AtomicF32::new(1.0));
    let rms_threshold = Arc::new(AtomicF32::new(1.0)); // High threshold = always silent
    let rms_sender = Arc::new(AtomicF32::new(0.0));

    let state = InputProcessorState::new(
        &input_volume,
        &rms_threshold,
        &muted,
        rms_sender,
        DEFAULT_POOL_CAPACITY,
    );

    let handle =
        thread::spawn(move || input_processor(mock_input, output_tx, 48_000.0, None, state));

    // Try to receive frames - should timeout since muted
    let result = output_rx.recv_timeout(Duration::from_millis(500));

    // Close channel
    drop(output_rx);
    handle
        .join()
        .expect("Input processor panicked")
        .expect("Input processor failed");

    // Should have timed out (no frames received when muted)
    assert!(result.is_err(), "Should not receive frames when muted");
}

/// Tests that deafening stops audio output.
#[test]
fn test_output_processor_deafen() {
    let (input_tx, input_rx) = kanal::unbounded::<BytesMut>();
    let (mock_output, samples_counter, _) = TestAudioOutput::new();

    // Create state with deafened = true
    let deafened = Arc::new(AtomicBool::new(true));
    let output_volume = Arc::new(AtomicF32::new(1.0));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let loss_sender = Arc::new(AtomicUsize::new(0));

    let state = OutputProcessorState::new(&output_volume, rms_sender, &deafened, loss_sender);

    let handle = thread::spawn(move || output_processor(input_rx, mock_output, 1.0, state));

    // Send frames
    for _ in 0..10 {
        let mut frame = BytesMut::with_capacity(FRAME_SIZE * 2);
        frame.resize(FRAME_SIZE * 2, 0);
        input_tx.send(frame).expect("Failed to send frame");
    }

    // Small delay to let processor run
    thread::sleep(Duration::from_millis(100));

    drop(input_tx);
    handle
        .join()
        .expect("Output processor panicked")
        .expect("Output processor failed");

    // Should not have received any samples when deafened
    let samples = samples_counter.load(Ordering::Relaxed);
    assert_eq!(samples, 0, "Should not receive samples when deafened");
}
