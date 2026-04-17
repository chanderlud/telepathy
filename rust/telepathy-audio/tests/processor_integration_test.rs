#![cfg(not(target_family = "wasm"))]

mod common;

use atomic_float::AtomicF32;
use bytes::{Bytes, BytesMut};
use common::{
    FullAudioOutput, QueueSource, TEST_SAMPLE_RATE, TestAudioInput, TestAudioOutput,
    make_input_state, make_output_state,
};
use nnnoiseless::DenoiseState;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;
use telepathy_audio::FRAME_SIZE;
use telepathy_audio::adapters::{MpscSink, MpscSource};
use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::internal::processor::{input_processor, output_processor};
use telepathy_audio::internal::state::{InputProcessorState, OutputProcessorState};
use telepathy_audio::sea::codec::file::SeaFileHeader;
use telepathy_audio::sea::decoder::SeaDecoder;
use telepathy_audio::sea::encoder::{EncoderSettings, SeaEncoder};

const MINIMUM_SILENCE_LENGTH: usize = 40;
const TEST_FRAMES: usize = 100;

#[test]
fn input_processor_encodes_frames_with_sea() {
    let total_samples = FRAME_SIZE * 10;
    let input = TestAudioInput::new(total_samples);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();
    let encoder = SeaEncoder::new(1, TEST_SAMPLE_RATE as u32, EncoderSettings::default()).unwrap();

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(tx),
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            None,
            InputProcessorState::default(),
            Some(encoder),
        )
    });

    for _ in 0..10 {
        let buf = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("expected encoded frame");
        assert!(buf.as_ref().len() < FRAME_SIZE * 2);
    }

    handle.join().unwrap().unwrap();
}

#[test]
fn output_processor_decodes_frames_with_sea() {
    let mut encoder =
        SeaEncoder::new(1, TEST_SAMPLE_RATE as u32, EncoderSettings::default()).unwrap();
    let mut encoded_frames = Vec::new();
    let mut phase = 0.0_f64;

    for _ in 0..10 {
        let frame = std::array::from_fn(|_| {
            let sample = (phase.sin() * 16_000.0) as i16;
            phase += 2.0 * std::f64::consts::PI * 440.0 / TEST_SAMPLE_RATE as f64;
            sample
        });
        let mut encoded = BytesMut::new();
        encoder.encode_frame(frame, &mut encoded).unwrap();
        encoded_frames.push(encoded.freeze());
    }

    let header = SeaFileHeader {
        version: 1,
        channels: 1,
        chunk_size: encoder.chunk_size(),
        frames_per_chunk: FRAME_SIZE as u16,
        sample_rate: TEST_SAMPLE_RATE as u32,
    };
    let decoder = SeaDecoder::new(header).unwrap();
    let source = QueueSource::new(encoded_frames);
    let (output, samples_counter, frames_counter) = TestAudioOutput::new();

    output_processor(
        source,
        output,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        Some(decoder),
    )
    .unwrap();

    assert_eq!(frames_counter.load(Ordering::Relaxed), 10);
    assert!(samples_counter.load(Ordering::Relaxed) > 0);
}

fn run_input_and_measure_rms(input_volume: f32) -> f32 {
    let input = TestAudioInput::new(FRAME_SIZE * 20);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();

    let muted = Arc::new(AtomicBool::new(false));
    let input_volume = Arc::new(AtomicF32::new(input_volume));
    let rms_threshold = Arc::new(AtomicF32::new(0.0));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let state = make_input_state(&input_volume, &rms_threshold, &muted, rms_sender);

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(tx),
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            None,
            state,
            None,
        )
    });

    let mut sum_sq = 0.0_f64;
    let mut count = 0_usize;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                for chunk in frame.as_ref().chunks_exact(2) {
                    let sample = i16::from_ne_bytes([chunk[0], chunk[1]]);
                    let sample = f64::from(sample);
                    sum_sq += sample * sample;
                    count += 1;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        }
    }

    handle.join().unwrap().unwrap();
    assert!(count > 0);
    (sum_sq / count as f64).sqrt() as f32
}

#[test]
fn input_processor_volume_halved_halves_rms() {
    let rms_a = run_input_and_measure_rms(1.0);
    let rms_b = run_input_and_measure_rms(0.5);

    let ratio = rms_b / rms_a;
    assert!(
        (ratio - 0.5).abs() <= 0.1,
        "expected ratio near 0.5, got {ratio}"
    );
}

#[test]
fn input_processor_updates_rms_atomic() {
    let input = TestAudioInput::new(FRAME_SIZE * 5);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();

    let muted = Arc::new(AtomicBool::new(false));
    let input_volume = Arc::new(AtomicF32::new(1.0));
    let rms_threshold = Arc::new(AtomicF32::new(0.0));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let state = make_input_state(&input_volume, &rms_threshold, &muted, rms_sender.clone());

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(tx),
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            None,
            state,
            None,
        )
    });

    while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
    handle.join().unwrap().unwrap();

    assert!(rms_sender.load(Ordering::Relaxed) > 0.0);
}

#[test]
fn input_processor_drops_frames_after_silence_threshold() {
    let total_samples = FRAME_SIZE * (MINIMUM_SILENCE_LENGTH + 10);
    let input = TestAudioInput::new(total_samples);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();

    let muted = Arc::new(AtomicBool::new(false));
    let input_volume = Arc::new(AtomicF32::new(1.0));
    let rms_threshold = Arc::new(AtomicF32::new(f32::MAX));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let state = make_input_state(&input_volume, &rms_threshold, &muted, rms_sender);

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(tx),
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            None,
            state,
            None,
        )
    });

    for _ in 0..MINIMUM_SILENCE_LENGTH {
        rx.recv_timeout(Duration::from_millis(200))
            .expect("expected frame before silence suppression threshold");
    }

    if rx.recv_timeout(Duration::from_millis(200)).is_ok() {
        panic!("received frame after silence suppression threshold");
    }

    handle.join().unwrap().unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn output_processor_counts_dropped_samples_when_full() {
    let mut frames = Vec::new();
    for _ in 0..10 {
        frames.push(Bytes::from(vec![0u8; FRAME_SIZE * 2]));
    }

    let source = QueueSource::new(frames);
    let output = FullAudioOutput;

    let output_volume = Arc::new(AtomicF32::new(1.0));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let deafened = Arc::new(AtomicBool::new(false));
    let loss_sender = Arc::new(AtomicUsize::new(0));
    let state = make_output_state(&output_volume, rms_sender, &deafened, loss_sender.clone());

    output_processor(
        source,
        output,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        state,
        None,
    )
    .unwrap();

    assert_eq!(loss_sender.load(Ordering::Relaxed), FRAME_SIZE * 10);
}

#[test]
fn input_processor_passes_all_frames_with_denoiser() {
    let total_samples = FRAME_SIZE * TEST_FRAMES;
    let input = TestAudioInput::new(total_samples);

    let (output_tx, output_rx) = mpsc::channel::<PooledBuffer>();

    let denoiser = Some(DenoiseState::new());
    let state = InputProcessorState::default();

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(output_tx),
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            denoiser,
            state,
            None,
        )
    });

    let mut frames_received = 0;
    while frames_received < TEST_FRAMES {
        match output_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(_frame) => frames_received += 1,
            Err(_) => panic!(
                "Timeout waiting for frame {} (with denoise)",
                frames_received
            ),
        }
    }

    handle
        .join()
        .expect("Input processor panicked")
        .expect("Input processor failed");

    assert_eq!(frames_received, TEST_FRAMES);
}

#[test]
fn output_processor_resampling_delivers_all_frames() {
    let (input_tx, input_rx) = mpsc::channel::<Bytes>();
    let (output, _samples_counter, frames_counter) = TestAudioOutput::new();
    let state = OutputProcessorState::default();

    let input_rate = TEST_SAMPLE_RATE;
    let output_rate = 44_100;

    let handle = thread::spawn(move || {
        output_processor(
            MpscSource::new(input_rx),
            output,
            input_rate,
            output_rate,
            state,
            None,
        )
    });

    for _ in 0..TEST_FRAMES {
        let mut frame = BytesMut::with_capacity(FRAME_SIZE * 2);
        frame.resize(FRAME_SIZE * 2, 0);
        input_tx.send(frame.freeze()).expect("Failed to send frame");
    }

    drop(input_tx);
    handle
        .join()
        .expect("Output processor panicked")
        .expect("Output processor failed");

    assert_eq!(frames_counter.load(Ordering::Relaxed), TEST_FRAMES);
}

#[test]
fn input_processor_muted_sends_no_frames() {
    let total_samples = FRAME_SIZE * 50;
    let input = TestAudioInput::new(total_samples);

    let (output_tx, output_rx) = mpsc::channel::<PooledBuffer>();

    let muted = Arc::new(AtomicBool::new(true));
    let input_volume = Arc::new(AtomicF32::new(1.0));
    let rms_threshold = Arc::new(AtomicF32::new(1.0));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let state = make_input_state(&input_volume, &rms_threshold, &muted, rms_sender);

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(output_tx),
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            None,
            state,
            None,
        )
    });

    let mut frames_received = 0;
    loop {
        match output_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(_) => frames_received += 1,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("Input processor did not terminate after consuming all input");
            }
        }
    }

    handle
        .join()
        .expect("Input processor panicked")
        .expect("Input processor failed");

    assert_eq!(
        frames_received, 0,
        "muted input processor must not emit frames"
    );
}

#[test]
fn output_processor_deafened_writes_no_samples() {
    let (input_tx, input_rx) = mpsc::channel::<Bytes>();
    let (output, samples_counter, _frames_counter) = TestAudioOutput::new();

    let deafened = Arc::new(AtomicBool::new(true));
    let output_volume = Arc::new(AtomicF32::new(1.0));
    let rms_sender = Arc::new(AtomicF32::new(0.0));
    let loss_sender = Arc::new(AtomicUsize::new(0));
    let state = make_output_state(&output_volume, rms_sender, &deafened, loss_sender);

    for _ in 0..10 {
        let mut frame = BytesMut::with_capacity(FRAME_SIZE * 2);
        frame.resize(FRAME_SIZE * 2, 0);
        input_tx.send(frame.freeze()).expect("Failed to send frame");
    }
    drop(input_tx);

    let handle = thread::spawn(move || {
        output_processor(
            MpscSource::new(input_rx),
            output,
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            state,
            None,
        )
    });

    handle
        .join()
        .expect("Output processor panicked")
        .expect("Output processor failed");

    assert_eq!(
        samples_counter.load(Ordering::Relaxed),
        0,
        "deafened output processor must not write any samples"
    );
}
