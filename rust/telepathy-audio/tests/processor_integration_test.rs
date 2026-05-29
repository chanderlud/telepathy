#![cfg(not(target_family = "wasm"))]

mod common;

use atomic_float::AtomicF32;
use bytes::{Bytes, BytesMut};
use common::{
    FailingAudioOutput, FailingSource, FullAudioOutput, PartialWriteOutput, PartiallyFullOutput,
    PatternAudioInput, QueueSource, RecordingAudioOutput, SineSource, TEST_SAMPLE_RATE,
    TestAudioInput, TestAudioOutput, bytes_to_i16_samples, make_input_state, make_output_state,
    raw_frame_from_i16, raw_frame_with_start,
};
use nnnoiseless::DenoiseState;
use telepathy_audio::Error;
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

fn flatten_recorded(recorded: &Arc<std::sync::Mutex<Vec<Vec<f32>>>>) -> Vec<f32> {
    recorded.lock().unwrap().iter().flatten().copied().collect()
}

fn assert_i16_content_close(actual: &[f32], expected: &[i16], tolerance: i16) {
    assert_eq!(actual.len(), expected.len());
    let scale = 1.0_f32 / i16::MAX as f32;
    for (idx, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
        let expected = *expected as f32 * scale;
        let diff = (actual - expected).abs();
        let allowed = tolerance as f32 * scale;
        assert!(
            diff <= allowed,
            "sample {idx} mismatch: expected {expected}, got {actual}, diff {diff}"
        );
    }
}

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

    let mut encoded_frames = Vec::new();
    for _ in 0..10 {
        let buf = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("expected encoded frame");
        assert!(buf.as_ref().len() < FRAME_SIZE * 2);
        encoded_frames.push(Bytes::copy_from_slice(buf.as_ref()));
    }

    handle.join().unwrap().unwrap();

    let header = SeaFileHeader {
        version: 1,
        channels: 1,
        chunk_size: encoded_frames[0].len() as u16,
        frames_per_chunk: FRAME_SIZE as u16,
        sample_rate: TEST_SAMPLE_RATE as u32,
    };
    let mut decoder = SeaDecoder::new(header).unwrap();
    let mut decoded = [0_i16; FRAME_SIZE];
    for frame in encoded_frames {
        decoder.decode_frame(&frame, &mut decoded).unwrap();
        assert_eq!(decoded.len(), FRAME_SIZE);
        assert!(decoded.iter().any(|sample| sample.abs() > 1_000));
    }
}

#[test]
fn output_processor_decodes_frames_with_sea() {
    let mut encoder =
        SeaEncoder::new(1, TEST_SAMPLE_RATE as u32, EncoderSettings::default()).unwrap();
    let mut encoded_frames = Vec::new();
    let mut original_frames = Vec::new();
    let mut phase = 0.0_f64;

    for _ in 0..10 {
        let frame = std::array::from_fn(|_| {
            let sample = (phase.sin() * 16_000.0) as i16;
            phase += 2.0 * std::f64::consts::PI * 440.0 / TEST_SAMPLE_RATE as f64;
            sample
        });
        original_frames.extend_from_slice(&frame);
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
    let (output, recorded) = RecordingAudioOutput::new();

    output_processor(
        source,
        output,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        Some(decoder),
    )
    .unwrap();

    let samples = flatten_recorded(&recorded);
    assert_eq!(samples.len(), FRAME_SIZE * 10);
    assert_i16_content_close(&samples, &original_frames, 500);
}

#[test]
fn output_processor_raw_frame_writes_exact_sample_values() {
    let input_samples = [i16::MIN, -12_000, 0, 12_000, i16::MAX];
    let mut frame_samples = vec![0_i16; FRAME_SIZE];
    frame_samples[..input_samples.len()].copy_from_slice(&input_samples);
    let source = QueueSource::new(vec![raw_frame_from_i16(&frame_samples)]);
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

    let samples = flatten_recorded(&recorded);
    assert_i16_content_close(&samples[..input_samples.len()], &input_samples, 1);
    assert_eq!(samples.len(), FRAME_SIZE);
}

#[test]
fn output_processor_ignores_malformed_raw_frame() {
    let source = QueueSource::new(vec![Bytes::from_static(&[1, 2, 3])]);
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

    assert!(recorded.lock().unwrap().is_empty());
}

#[test]
fn output_processor_source_failures_propagate() {
    let (output, _recorded) = RecordingAudioOutput::new();

    let err = output_processor(
        FailingSource,
        output,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        None,
    )
    .unwrap_err();

    assert!(
        matches!(err, telepathy_audio::Error::Processing(msg) if msg.contains("intentional source failure"))
    );
}

#[test]
fn output_processor_sink_failures_propagate() {
    let source = QueueSource::new(vec![raw_frame_with_start(0)]);

    let err = output_processor(
        source,
        FailingAudioOutput,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        None,
    )
    .unwrap_err();

    assert!(
        matches!(err, telepathy_audio::Error::Processing(msg) if msg.contains("intentional output failure"))
    );
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

fn rms_i16_from_bytes(frame: &[u8]) -> f32 {
    let mut sum_sq = 0.0_f64;
    let mut count = 0_usize;
    for chunk in frame.chunks_exact(2) {
        let sample = i16::from_ne_bytes([chunk[0], chunk[1]]) as f64;
        sum_sq += sample * sample;
        count += 1;
    }
    ((sum_sq / count as f64).sqrt()) as f32
}

#[test]
fn input_processor_volume_halved_halves_rms() {
    let rms_a = run_input_and_measure_rms(1.0);
    let rms_b = run_input_and_measure_rms(0.5);

    assert!(rms_a > 1.0, "expected non-zero baseline RMS, got {rms_a}");
    let ratio = rms_b / rms_a;
    assert!((0.45..=0.55).contains(&ratio), "expected ratio near 0.5, got {ratio}");
}

#[test]
fn input_processor_volume_zero_yields_silence() {
    let input = TestAudioInput::new(FRAME_SIZE * 12);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();
    let state = make_input_state(
        &Arc::new(AtomicF32::new(0.0)),
        &Arc::new(AtomicF32::new(0.0)),
        &Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicF32::new(0.0)),
    );

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

    while let Ok(frame) = rx.recv_timeout(Duration::from_millis(200)) {
        assert!(rms_i16_from_bytes(frame.as_ref()) < 1.0);
    }
    handle.join().unwrap().unwrap();
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

    let rms = rms_sender.load(Ordering::Relaxed);
    assert!(
        (10_000.0..13_500.0).contains(&rms),
        "expected half-amplitude sine RMS in i16 units, got {rms}"
    );
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
fn input_processor_resumes_after_silence_when_signal_returns() {
    let silent_frames = MINIMUM_SILENCE_LENGTH + 10;
    let signal_frames = 8;
    let mut samples = vec![0.0_f32; silent_frames * FRAME_SIZE];
    let mut phase = 0.0_f64;
    for _ in 0..(signal_frames * FRAME_SIZE) {
        samples.push((phase.sin() as f32) * 0.8);
        phase += 2.0 * std::f64::consts::PI * 440.0 / TEST_SAMPLE_RATE as f64;
    }

    let input = PatternAudioInput::new(samples);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();

    let muted = Arc::new(AtomicBool::new(false));
    let input_volume = Arc::new(AtomicF32::new(1.0));
    let rms_threshold = Arc::new(AtomicF32::new(1.0));
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

    let mut frames = Vec::new();
    while let Ok(frame) = rx.recv_timeout(Duration::from_millis(200)) {
        frames.push(frame);
    }
    handle.join().unwrap().unwrap();

    assert!(frames.len() >= MINIMUM_SILENCE_LENGTH + signal_frames);
    for frame in frames.iter().take(MINIMUM_SILENCE_LENGTH) {
        assert!(rms_i16_from_bytes(frame.as_ref()) < 1.0);
    }
    let non_silent = frames
        .iter()
        .skip(MINIMUM_SILENCE_LENGTH)
        .filter(|frame| rms_i16_from_bytes(frame.as_ref()) > 1.0)
        .count();
    assert!(non_silent >= signal_frames);
    assert!(rms_sender.load(Ordering::Relaxed) > 1.0);
}

#[test]
fn input_processor_boundary_silence_length_equals_threshold() {
    let signal_frames = 6;
    let mut samples = vec![0.0_f32; MINIMUM_SILENCE_LENGTH * FRAME_SIZE];
    let mut phase = 0.0_f64;
    for _ in 0..(signal_frames * FRAME_SIZE) {
        samples.push((phase.sin() as f32) * 0.8);
        phase += 2.0 * std::f64::consts::PI * 440.0 / TEST_SAMPLE_RATE as f64;
    }

    let input = PatternAudioInput::new(samples);
    let (tx, rx) = mpsc::channel::<PooledBuffer>();
    let state = make_input_state(
        &Arc::new(AtomicF32::new(1.0)),
        &Arc::new(AtomicF32::new(1.0)),
        &Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicF32::new(0.0)),
    );

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

    let mut frames = Vec::new();
    while let Ok(frame) = rx.recv_timeout(Duration::from_millis(200)) {
        frames.push(frame);
    }
    handle.join().unwrap().unwrap();

    assert!(frames.len() >= MINIMUM_SILENCE_LENGTH + signal_frames);
    assert!(
        frames
            .iter()
            .take(MINIMUM_SILENCE_LENGTH)
            .all(|frame| rms_i16_from_bytes(frame.as_ref()) < 1.0)
    );
    assert!(
        frames
            .iter()
            .skip(MINIMUM_SILENCE_LENGTH)
            .take(signal_frames)
            .all(|frame| rms_i16_from_bytes(frame.as_ref()) > 1.0)
    );
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

    struct RecordingFullOutput {
        recorded: Arc<std::sync::Mutex<Vec<Vec<f32>>>>,
    }
    impl telepathy_audio::internal::traits::AudioOutput for RecordingFullOutput {
        fn is_full(&self) -> bool {
            true
        }
        fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
            self.recorded.lock().unwrap().push(samples.to_vec());
            Ok(0)
        }
    }

    let source = QueueSource::new((0..3).map(|_| Bytes::from(vec![0u8; FRAME_SIZE * 2])).collect());
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    output_processor(
        source,
        RecordingFullOutput {
            recorded: recorded.clone(),
        },
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        None,
    )
    .unwrap();
    assert!(recorded.lock().unwrap().is_empty());
}

#[test]
fn output_processor_partial_full_window_counts_only_dropped() {
    let mut frames = Vec::new();
    for _ in 0..10 {
        frames.push(Bytes::from(vec![0u8; FRAME_SIZE * 2]));
    }
    let source = QueueSource::new(frames);
    let (output, dropped_frames, written_frames, _recorded) = PartiallyFullOutput::new(true);

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

    let dropped_count = dropped_frames.load(Ordering::Relaxed);
    assert_eq!(written_frames.load(Ordering::Relaxed), 5);
    assert_eq!(loss_sender.load(Ordering::Relaxed), FRAME_SIZE * dropped_count);
}

#[test]
fn input_processor_passes_all_frames_with_denoiser() {
    let input_rate = 16_000;
    let output_rate = TEST_SAMPLE_RATE;
    let total_samples = FRAME_SIZE * TEST_FRAMES;
    let input = TestAudioInput::new(total_samples);

    let (output_tx, output_rx) = mpsc::channel::<PooledBuffer>();

    let denoiser = Some(DenoiseState::new());
    let state = InputProcessorState::default();

    let handle = thread::spawn(move || {
        input_processor(
            input,
            MpscSink::new(output_tx),
            input_rate,
            output_rate,
            denoiser,
            state,
            None,
        )
    });

    let expected_frames = total_samples * output_rate / input_rate / FRAME_SIZE;
    let mut frames_received: Vec<Vec<i16>> = Vec::new();
    while frames_received.len() < expected_frames + 2 {
        match output_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(frame) => {
                assert_eq!(frame.as_ref().len(), FRAME_SIZE * 2);
                let decoded = bytes_to_i16_samples(frame.as_ref());
                assert_eq!(decoded.len(), FRAME_SIZE);
                assert!(decoded.iter().all(|s| (*s as i32) >= i16::MIN as i32));
                assert!(decoded.iter().all(|s| (*s as i32) <= i16::MAX as i32));
                frames_received.push(decoded)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("timeout waiting for denoised frames"),
        }
    }

    handle
        .join()
        .expect("Input processor panicked")
        .expect("Input processor failed");

    assert!(frames_received.len().abs_diff(expected_frames) <= 1);
    assert!(
        frames_received
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 100)
    );
}

#[test]
fn output_processor_resampling_delivers_all_frames() {
    let input_frames = TEST_FRAMES;
    let source = SineSource::new(input_frames, TEST_SAMPLE_RATE, 440.0, 0.7);
    let (output, recorded) = RecordingAudioOutput::new();
    let state = OutputProcessorState::default();

    let input_rate = TEST_SAMPLE_RATE;
    let output_rate = 44_100;

    output_processor(source, output, input_rate, output_rate, state, None).unwrap();

    let samples = flatten_recorded(&recorded);
    let expected_samples = TEST_FRAMES * FRAME_SIZE * output_rate / input_rate;
    assert!(
        samples.len().abs_diff(expected_samples) <= TEST_FRAMES,
        "expected approximately {expected_samples} resampled samples, got {}",
        samples.len()
    );
    let peak = samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
    let expected_peak = 0.7_f32;
    assert!((peak - expected_peak).abs() <= expected_peak * 0.05);

    let mut crossing_positions = Vec::new();
    for (idx, pair) in samples.windows(2).enumerate() {
        if pair[0] <= 0.0 && pair[1] > 0.0 {
            crossing_positions.push(idx);
        }
    }
    assert!(crossing_positions.len() > 3);
    assert!(crossing_positions.windows(2).all(|w| w[1] > w[0]));
}

#[test]
fn output_processor_resampling_48k_to_96k() {
    let frames: Vec<Bytes> = (0..20)
        .map(|idx| raw_frame_with_start((idx as i16) * 20))
        .collect();
    let source = QueueSource::new(frames);
    let (output, recorded) = RecordingAudioOutput::new();

    output_processor(
        source,
        output,
        48_000,
        96_000,
        OutputProcessorState::default(),
        None,
    )
    .unwrap();

    let samples = flatten_recorded(&recorded);
    assert!(samples.windows(2).any(|pair| pair[1] > pair[0]));
}

#[test]
fn output_processor_resampling_48k_to_8k() {
    let frames: Vec<Bytes> = (0..20)
        .map(|idx| raw_frame_with_start((idx as i16) * 20))
        .collect();
    let source = QueueSource::new(frames);
    let (output, recorded) = RecordingAudioOutput::new();

    output_processor(
        source,
        output,
        48_000,
        8_000,
        OutputProcessorState::default(),
        None,
    )
    .unwrap();

    let samples = flatten_recorded(&recorded);
    assert!(samples.windows(2).any(|pair| pair[1] > pair[0]));
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
    let state = make_input_state(&input_volume, &rms_threshold, &muted, rms_sender.clone());

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
    assert_eq!(rms_sender.load(Ordering::Relaxed), 0.0);
}

#[test]
fn input_processor_unmute_resumes_emission() {
    struct SlowConstantInput {
        frames_remaining: usize,
        value: f32,
    }
    impl telepathy_audio::internal::traits::AudioInput for SlowConstantInput {
        fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
            if self.frames_remaining == 0 {
                return Ok(0);
            }
            for sample in dst.iter_mut().take(FRAME_SIZE) {
                *sample = self.value;
            }
            self.frames_remaining -= 1;
            thread::sleep(Duration::from_millis(1));
            Ok(FRAME_SIZE)
        }
    }

    let input = SlowConstantInput {
        frames_remaining: 400,
        value: 0.25,
    };
    let (output_tx, output_rx) = mpsc::channel::<PooledBuffer>();

    let muted = Arc::new(AtomicBool::new(true));
    let input_volume = Arc::new(AtomicF32::new(1.0));
    let rms_threshold = Arc::new(AtomicF32::new(0.0));
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

    thread::sleep(Duration::from_millis(20));
    muted.store(false, Ordering::Relaxed);

    let frame = output_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("expected frame after unmute");
    let decoded = bytes_to_i16_samples(frame.as_ref());
    let expected = (0.25 * i16::MAX as f32).round() as i16;
    assert_eq!(decoded.len(), FRAME_SIZE);
    assert!(
        decoded
            .iter()
            .all(|sample| (*sample - expected).unsigned_abs() <= 1)
    );

    handle.join().unwrap().unwrap();
}

#[test]
fn output_processor_propagates_decoder_error() {
    let header = SeaFileHeader {
        version: 1,
        channels: 1,
        chunk_size: 64,
        frames_per_chunk: FRAME_SIZE as u16,
        sample_rate: TEST_SAMPLE_RATE as u32,
    };
    let decoder = SeaDecoder::new(header).unwrap();
    let source = QueueSource::new(vec![Bytes::from_static(&[1, 2, 3, 4, 5])]);

    let err = output_processor(
        source,
        RecordingAudioOutput::new().0,
        TEST_SAMPLE_RATE,
        TEST_SAMPLE_RATE,
        OutputProcessorState::default(),
        Some(decoder),
    )
    .unwrap_err();

    assert!(matches!(err, telepathy_audio::Error::Processing(msg) if msg.contains("Codec error")));
}

#[test]
fn output_processor_records_partial_write_loss() {
    let frames: Vec<Bytes> = (0..7).map(|_| raw_frame_with_start(32)).collect();
    let source = QueueSource::new(frames);
    let (output, writes, _recorded) = PartialWriteOutput::new(11);

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

    assert_eq!(loss_sender.load(Ordering::Relaxed), writes.load(Ordering::Relaxed) * 11);
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
