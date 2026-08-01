use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use telepathy_audio::Error;
use telepathy_audio::devices::MockAudioHost;
use telepathy_audio::internal::traits::{AudioInput, AudioOutput};

const SAMPLE_RATE: u32 = 48_000;
const FRAME_STEP: f32 = 1.0 / 4096.0;
const CAPTURE_CAPACITY: usize = 512;

pub(super) type TestAudioHost = MockAudioHost<SequencedInput, CapturingOutput>;

pub(super) fn host() -> (TestAudioHost, FrameCapture) {
    let capture = FrameCapture::default();
    let host = MockAudioHost::new(
        SequencedInput::new(),
        SAMPLE_RATE,
        CapturingOutput::new(capture.clone()),
        SAMPLE_RATE,
    );
    (host, capture)
}

#[derive(Debug, Clone)]
pub(super) struct FrameCapture {
    indices: Arc<Mutex<VecDeque<usize>>>,
}

impl FrameCapture {
    fn record(&self, index: usize) {
        let mut indices = self.indices.lock().unwrap();
        if indices.len() == CAPTURE_CAPACITY {
            indices.pop_front();
        }
        indices.push_back(index);
    }

    #[cfg(test)]
    pub(super) fn drain(&self) -> Vec<usize> {
        self.indices.lock().unwrap().drain(..).collect()
    }
}

impl Default for FrameCapture {
    fn default() -> Self {
        Self {
            indices: Arc::new(Mutex::new(VecDeque::with_capacity(CAPTURE_CAPACITY))),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SequencedInput {
    counter: Arc<AtomicUsize>,
}

impl SequencedInput {
    fn new() -> Self {
        Self {
            counter: Arc::new(AtomicUsize::new(1)),
        }
    }
}

impl AudioInput for SequencedInput {
    fn read_into(&mut self, samples: &mut [f32]) -> Result<usize, Error> {
        let frame_seconds = samples.len() as f64 / SAMPLE_RATE as f64;
        if frame_seconds > 0.0 {
            thread::sleep(Duration::from_secs_f64(frame_seconds));
        }
        let index = self.counter.fetch_add(1, Ordering::Relaxed);
        samples.fill(index as f32 * FRAME_STEP);
        Ok(samples.len())
    }
}

#[derive(Debug, Clone)]
pub(super) struct CapturingOutput {
    capture: FrameCapture,
}

impl CapturingOutput {
    fn new(capture: FrameCapture) -> Self {
        Self { capture }
    }
}

impl AudioOutput for CapturingOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, samples: &[f32]) -> Result<usize, Error> {
        let index = (samples[0] / FRAME_STEP).round() as usize;
        self.capture.record(index);
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_decodes_samples_and_drains_atomically() {
        let capture = FrameCapture::default();
        let mut output = CapturingOutput::new(capture.clone());
        output.write_samples(&[3.0 * FRAME_STEP]).unwrap();

        assert_eq!(capture.drain(), vec![3]);
        assert!(capture.drain().is_empty());
    }

    #[test]
    fn capture_discards_oldest_index_at_capacity() {
        let capture = FrameCapture::default();
        let mut output = CapturingOutput::new(capture.clone());
        for index in 1..=CAPTURE_CAPACITY + 1 {
            output.write_samples(&[index as f32 * FRAME_STEP]).unwrap();
        }

        let indices = capture.drain();
        assert_eq!(indices.len(), CAPTURE_CAPACITY);
        assert_eq!(indices.first(), Some(&2));
        assert_eq!(indices.last(), Some(&(CAPTURE_CAPACITY + 1)));
    }
}
