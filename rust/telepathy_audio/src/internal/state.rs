//! Processor state structures for audio input/output.
//!
//! This module contains the state structures that manage configuration
//! and statistics for audio processing pipelines.
//!
//! ## Purpose
//!
//! These state structures enable real-time control of audio processing
//! parameters without stopping and restarting streams. All state is
//! managed via atomic operations for lock-free, thread-safe access.
//!
//! ## Statistics Collection
//!
//! Both state types include RMS (Root Mean Square) senders for audio level
//! monitoring. The processor updates these atomics with the maximum RMS
//! value encountered, which can be read by the application for visualization
//! or voice activity detection.

use crate::constants::{
    VAD_MAX_SPEECH_S, VAD_MIN_SILENCE_MS, VAD_MIN_SPEECH_MS, VAD_SILENCE_CEILING,
    VAD_SPEECH_PAD_MS, VAD_THRESHOLD,
};
use crate::internal::NETWORK_FRAME;
use crate::internal::buffer_pool::BufferPool;
use atomic_float::AtomicF32;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

pub trait InputState {
    fn is_muted(&self) -> bool;

    fn reset_silence(&mut self);
}

/// State for the input audio processor.
///
/// This structure holds atomic references to shared state that can be
/// modified during processing (e.g., volume, mute state).
pub struct InputProcessorState {
    pub(crate) input_volume: Arc<AtomicF32>,
    pub(crate) rms_threshold: Arc<AtomicF32>,
    pub(crate) muted: Arc<AtomicBool>,
    pub(crate) rms_sender: Arc<AtomicF32>,
    pub(crate) buffer_pool: Arc<BufferPool>,
    pub(crate) silence_length: u16,
}

impl InputProcessorState {
    /// Creates a new input processor state.
    ///
    /// # Arguments
    ///
    /// * `input_volume` - Atomic reference to the input volume multiplier
    /// * `rms_threshold` - Atomic reference to the RMS threshold for silence detection
    /// * `muted` - Atomic reference to the mute state
    /// * `rms_sender` - Atomic reference for sending RMS values to statistics
    pub fn new(
        input_volume: &Arc<AtomicF32>,
        rms_threshold: &Arc<AtomicF32>,
        muted: &Arc<AtomicBool>,
        rms_sender: Arc<AtomicF32>,
        pool_size: usize,
    ) -> Self {
        Self {
            input_volume: input_volume.clone(),
            rms_threshold: rms_threshold.clone(),
            muted: muted.clone(),
            rms_sender,
            buffer_pool: Arc::new(BufferPool::new(pool_size, NETWORK_FRAME)),
            silence_length: 0,
        }
    }

    /// Gets the current input volume multiplier.
    pub(crate) fn input_volume(&self) -> f32 {
        self.input_volume.load(Relaxed)
    }

    /// Gets the current RMS threshold for silence detection.
    pub(crate) fn rms_threshold(&self) -> f32 {
        self.rms_threshold.load(Relaxed)
    }

    /// Sends the RMS value to statistics (uses max to keep highest value).
    pub(crate) fn send_rms(&self, rms: f32) {
        self.rms_sender.fetch_max(rms, Relaxed);
    }

    /// Returns a reference to the buffer pool.
    pub(crate) fn buffer_pool(&self) -> &Arc<BufferPool> {
        &self.buffer_pool
    }
}

impl Default for InputProcessorState {
    fn default() -> Self {
        Self {
            input_volume: Arc::new(AtomicF32::new(1.0)),
            rms_threshold: Arc::new(AtomicF32::new(1.0)),
            muted: Default::default(),
            rms_sender: Default::default(),
            buffer_pool: Default::default(),
            silence_length: 0,
        }
    }
}

impl InputState for InputProcessorState {
    fn is_muted(&self) -> bool {
        self.muted.load(Relaxed)
    }

    fn reset_silence(&mut self) {
        self.silence_length = 0;
    }
}

/// State for the output audio processor.
///
/// This structure holds atomic references to shared state that can be
/// modified during processing (e.g., volume, deafen state).
pub struct OutputProcessorState {
    pub(crate) output_volume: Arc<AtomicF32>,
    pub(crate) rms_sender: Arc<AtomicF32>,
    pub(crate) deafened: Arc<AtomicBool>,
    pub(crate) loss_sender: Arc<AtomicUsize>,
}

impl OutputProcessorState {
    /// Creates a new output processor state.
    ///
    /// # Arguments
    ///
    /// * `output_volume` - Atomic reference to the output volume multiplier
    /// * `rms_sender` - Atomic reference for sending RMS values to statistics
    /// * `deafened` - Atomic reference to the deafen state
    /// * `loss_sender` - Atomic reference for sending packet loss counts
    pub fn new(
        output_volume: &Arc<AtomicF32>,
        rms_sender: Arc<AtomicF32>,
        deafened: &Arc<AtomicBool>,
        loss_sender: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            output_volume: output_volume.clone(),
            rms_sender,
            deafened: deafened.clone(),
            loss_sender,
        }
    }

    /// Gets the current output volume multiplier.
    pub(crate) fn output_volume(&self) -> f32 {
        self.output_volume.load(Relaxed)
    }

    /// Checks if the output is currently deafened.
    pub(crate) fn is_deafened(&self) -> bool {
        self.deafened.load(Relaxed)
    }

    /// Sends the RMS value to statistics (uses max to keep highest value).
    pub(crate) fn send_rms(&self, rms: f32) {
        self.rms_sender.fetch_max(rms, Relaxed);
    }

    /// Records packet loss by adding to the loss counter.
    pub(crate) fn send_loss(&self, loss: usize) {
        self.loss_sender.fetch_add(loss, Relaxed);
    }
}

impl Default for OutputProcessorState {
    fn default() -> Self {
        Self {
            output_volume: Arc::new(AtomicF32::new(1.0)),
            rms_sender: Arc::new(Default::default()),
            deafened: Arc::new(Default::default()),
            loss_sender: Arc::new(Default::default()),
        }
    }
}

pub struct SpeechParams {
    pub(crate) frame_size_samples: usize,
    pub(crate) threshold: f32,
    pub(crate) min_speech_samples: usize,
    pub(crate) max_speech_samples: f32,
    pub(crate) min_silence_samples: usize,
    pub(crate) min_silence_samples_at_max_speech: usize,
    pub(crate) speech_pad_samples: usize,
}

impl SpeechParams {
    pub fn new(sample_rate: usize, frame_size_samples: usize) -> Self {
        let sr_per_ms = sample_rate / 1000;

        Self {
            frame_size_samples,
            threshold: VAD_THRESHOLD,
            min_speech_samples: VAD_MIN_SPEECH_MS * sr_per_ms,
            max_speech_samples: VAD_MAX_SPEECH_S * sample_rate as f32,
            min_silence_samples: VAD_MIN_SILENCE_MS * sr_per_ms,
            min_silence_samples_at_max_speech: 98 * sr_per_ms,
            speech_pad_samples: VAD_SPEECH_PAD_MS * sr_per_ms,
        }
    }
}

#[derive(Default)]
pub struct SpeechState {
    pub(crate) current_sample: usize,
    pub(crate) triggered: bool,
    pub(crate) temp_end: usize,
    pub(crate) prev_end: usize,
    pub(crate) next_start: usize,
}

impl SpeechState {
    pub fn update(&mut self, params: &SpeechParams, speech_prob: f32) -> bool {
        self.current_sample += params.frame_size_samples;

        if speech_prob >= params.threshold {
            if self.next_start == 0 {
                self.next_start = self
                    .current_sample
                    .saturating_sub(params.frame_size_samples + params.speech_pad_samples);
            }
            self.temp_end = 0;
            if !self.triggered
                && self.current_sample.saturating_sub(self.next_start) >= params.min_speech_samples
            {
                self.triggered = true;
                self.prev_end = 0;
            }
        } else if self.triggered {
            if self.temp_end == 0 {
                self.temp_end = self.current_sample;
            }

            let segment_len = self.current_sample.saturating_sub(self.next_start) as f32;
            let min_silence = if segment_len >= params.max_speech_samples {
                params.min_silence_samples_at_max_speech
            } else {
                params.min_silence_samples
            };

            if self.current_sample.saturating_sub(self.temp_end) >= min_silence {
                self.triggered = false;
                self.prev_end = self.temp_end + params.speech_pad_samples;
                self.next_start = 0;
                self.temp_end = 0;
            }
        }

        self.triggered || self.temp_end != 0
    }
}

pub struct VadProcessorState {
    pub(crate) rms_threshold: Arc<AtomicF32>,
    pub(crate) muted: Arc<AtomicBool>,
    pub(crate) silence_length: u16,
    pub(crate) silence_ceiling: f32,
}

impl VadProcessorState {
    /// Creates a new input processor state.
    ///
    /// # Arguments
    ///
    /// * `input_volume` - Atomic reference to the input volume multiplier
    /// * `rms_threshold` - Atomic reference to the RMS threshold for silence detection
    /// * `muted` - Atomic reference to the mute state
    /// * `rms_sender` - Atomic reference for sending RMS values to statistics
    pub fn new(rms_threshold: &Arc<AtomicF32>, muted: &Arc<AtomicBool>) -> Self {
        Self {
            rms_threshold: rms_threshold.clone(),
            muted: muted.clone(),
            silence_length: 0,
            silence_ceiling: VAD_SILENCE_CEILING,
        }
    }

    pub fn with_silence_ceiling(mut self, silence_ceiling: f32) -> Self {
        self.silence_ceiling = silence_ceiling;
        self
    }

    pub fn silence_ceiling(&self) -> f32 {
        self.silence_ceiling
    }
}

impl InputState for VadProcessorState {
    fn is_muted(&self) -> bool {
        self.muted.load(Relaxed)
    }

    fn reset_silence(&mut self) {
        self.silence_length = 0;
    }
}
