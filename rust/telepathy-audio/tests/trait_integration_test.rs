#![cfg(not(target_family = "wasm"))]

mod common;

use common::{
    PatternAudioInput, QueueSource, RecordingAudioOutput, TEST_SAMPLE_RATE, TestAudioInput,
    bytes_to_i16_samples, patterned_samples, raw_frame_with_start,
};
use std::sync::{Arc, Mutex};

use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::io::AudioDataSink;
use telepathy_audio::io::traits::ClosedOrFailed;
use telepathy_audio::{Error, FRAME_SIZE};

#[test]
fn callback_sink_receives_frames() {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let frames_clone = frames.clone();

    let sink: Box<dyn Fn(PooledBuffer) + Send + 'static> = Box::new(move |buf| {
        frames_clone
            .lock()
            .unwrap()
            .push(bytes_to_i16_samples(buf.as_ref()));
    });

    let input_samples = patterned_samples(FRAME_SIZE * 10);
    let input = PatternAudioInput::new(input_samples);
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

    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 10);
    assert!(frames.iter().all(|frame| frame.len() == FRAME_SIZE));
    assert_eq!(&frames[0][..4], &[-16_000, -15_827, -15_654, -15_481]);
    assert_eq!(frames[9][FRAME_SIZE - 1], 14_227);
}

#[derive(Clone)]
struct FailingSink;

impl AudioDataSink for FailingSink {
    fn send(&self, _data: PooledBuffer) -> Result<(), ClosedOrFailed> {
        Err(ClosedOrFailed::Failed(std::io::Error::other(
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
    let mut frames = Vec::new();
    for frame_idx in 0..10 {
        frames.push(raw_frame_with_start((frame_idx * 100) as i16));
    }
    let source = QueueSource::new(frames);
    let (output, recorded) = RecordingAudioOutput::new();

    output_processor(
        source,
        output,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        None,
    )
    .unwrap();

    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 10);
    assert!(recorded.iter().all(|frame| frame.len() == FRAME_SIZE));
    let scale = 1.0_f32 / i16::MAX as f32;
    assert_eq!(recorded[0][0], 0.0);
    assert!((recorded[0][1] - scale).abs() < 1e-7);
    assert!((recorded[9][0] - 900.0 * scale).abs() < 1e-7);
}
