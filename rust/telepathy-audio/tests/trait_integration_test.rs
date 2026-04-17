#![cfg(not(target_family = "wasm"))]

mod common;

use bytes::Bytes;
use common::{QueueSource, TEST_SAMPLE_RATE, TestAudioInput, TestAudioOutput};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::io::AudioDataSink;
use telepathy_audio::io::traits::ClosedOrFailed;
use telepathy_audio::{Error, FRAME_SIZE};

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
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
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
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
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

#[test]
fn custom_source_drives_output_processor() {
    let (output, _samples_counter, frames_counter) = TestAudioOutput::new();

    let mut frames = Vec::new();
    for _ in 0..10 {
        frames.push(Bytes::from_static(&[0u8; FRAME_SIZE * 2]));
    }
    let source = QueueSource::new(frames);

    output_processor(
        source,
        output,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        None,
    )
    .unwrap();

    assert_eq!(frames_counter.load(Relaxed), 10);
}
