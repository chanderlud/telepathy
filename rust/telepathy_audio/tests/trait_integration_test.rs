#![cfg(not(target_family = "wasm"))]

use bytes::Bytes;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};
use telepathy_audio::{AudioDataSink, AudioDataSource, ClosedOrFailed, Error, FRAME_SIZE};

struct TestAudioInput {
    samples_remaining: usize,
}

impl TestAudioInput {
    fn new(total_samples: usize) -> Self {
        Self {
            samples_remaining: total_samples,
        }
    }
}

impl AudioInput for TestAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
        if self.samples_remaining == 0 {
            return Ok(0);
        }
        let to_read = dst.len().min(self.samples_remaining);
        for s in dst.iter_mut().take(to_read) {
            *s = 0.1;
        }
        self.samples_remaining -= to_read;
        Ok(to_read)
    }
}

struct CountingOutput {
    frames: Arc<AtomicUsize>,
}

impl AudioOutput for CountingOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        if !samples.is_empty() {
            self.frames.fetch_add(1, Relaxed);
        }
        Ok(0)
    }
}

#[test]
fn callback_sink_receives_frames() {
    let frames = Arc::new(AtomicUsize::new(0));
    let frames_clone = frames.clone();

    let sink: Box<dyn Fn(PooledBuffer) + Send + 'static> = Box::new(move |_buf| {
        frames_clone.fetch_add(1, Relaxed);
    });

    let input = TestAudioInput::new(FRAME_SIZE * 10);
    input_processor(
        input,
        sink,
        48_000,
        48_000,
        None,
        InputProcessorState::default(),
        None,
    )
    .unwrap();

    assert!(frames.load(Relaxed) > 0);
}

#[derive(Clone)]
struct FailingSink;

impl AudioDataSink for FailingSink {
    fn send(&self, _data: PooledBuffer) -> Result<(), ClosedOrFailed> {
        Err(ClosedOrFailed::Failed(io::Error::new(
            io::ErrorKind::Other,
            "intentional error",
        )))
    }
}

#[test]
fn sink_errors_propagate() {
    let input = TestAudioInput::new(FRAME_SIZE * 2);
    let err = input_processor(
        input,
        FailingSink,
        48_000,
        48_000,
        None,
        InputProcessorState::default(),
        None,
    )
    .unwrap_err();

    match err {
        Error::Processing(msg) => assert!(msg.contains("intentional")),
        other => panic!("expected Channel error, got {:?}", other),
    }
}

struct QueueSource {
    inner: Mutex<VecDeque<Bytes>>,
}

impl QueueSource {
    fn new(frames: Vec<Bytes>) -> Self {
        Self {
            inner: Mutex::new(frames.into()),
        }
    }
}

impl AudioDataSource for QueueSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        let mut guard = self.inner.lock().unwrap();
        guard.pop_front().ok_or_else(|| ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        let mut guard = self.inner.lock().unwrap();
        Ok(guard.pop_front())
    }
}

#[test]
fn custom_source_drives_output_processor() {
    let frames_counter = Arc::new(AtomicUsize::new(0));
    let output = CountingOutput {
        frames: frames_counter.clone(),
    };

    let mut frames = Vec::new();
    for _ in 0..10 {
        frames.push(Bytes::from_static(&[0u8; 960]));
    }
    let source = QueueSource::new(frames);

    output_processor(
        source,
        output,
        48_000,
        48_000,
        OutputProcessorState::default(),
        None,
    )
    .unwrap();

    assert!(frames_counter.load(Relaxed) > 0);
}
