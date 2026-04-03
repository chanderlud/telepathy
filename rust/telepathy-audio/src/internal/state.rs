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

use crate::internal::NETWORK_FRAME;
use crate::internal::buffer_pool::BufferPool;
use atomic_float::AtomicF32;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

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

    /// Checks if the input is currently muted.
    pub(crate) fn is_muted(&self) -> bool {
        self.muted.load(Relaxed)
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
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::buffer_pool::DEFAULT_POOL_CAPACITY;

    fn approx_eq(a: f32, b: f32) {
        assert!((a - b).abs() <= f32::EPSILON * 4.0);
    }

    #[test]
    fn input_state_default_volume_is_one() {
        let state = InputProcessorState::default();
        approx_eq(state.input_volume(), 1.0);
    }

    #[test]
    fn input_state_default_not_muted() {
        let state = InputProcessorState::default();
        assert!(!state.is_muted());
    }

    #[test]
    fn input_state_default_rms_threshold_is_one() {
        let state = InputProcessorState::default();
        approx_eq(state.rms_threshold(), 1.0);
    }

    #[test]
    fn input_state_volume_reflects_atomic() {
        let input_volume = Arc::new(AtomicF32::new(1.0));
        let rms_threshold = Arc::new(AtomicF32::new(1.0));
        let muted = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let state = InputProcessorState::new(
            &input_volume,
            &rms_threshold,
            &muted,
            rms_sender,
            DEFAULT_POOL_CAPACITY,
        );

        input_volume.store(0.42, Relaxed);
        approx_eq(state.input_volume(), 0.42);
    }

    #[test]
    fn input_state_mute_reflects_atomic() {
        let input_volume = Arc::new(AtomicF32::new(1.0));
        let rms_threshold = Arc::new(AtomicF32::new(1.0));
        let muted = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let state = InputProcessorState::new(
            &input_volume,
            &rms_threshold,
            &muted,
            rms_sender,
            DEFAULT_POOL_CAPACITY,
        );

        muted.store(true, Relaxed);
        assert!(state.is_muted());
    }

    #[test]
    fn input_state_rms_threshold_reflects_atomic() {
        let input_volume = Arc::new(AtomicF32::new(1.0));
        let rms_threshold = Arc::new(AtomicF32::new(1.0));
        let muted = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let state = InputProcessorState::new(
            &input_volume,
            &rms_threshold,
            &muted,
            rms_sender,
            DEFAULT_POOL_CAPACITY,
        );

        rms_threshold.store(0.27, Relaxed);
        approx_eq(state.rms_threshold(), 0.27);
    }

    #[test]
    fn input_state_send_rms_uses_max() {
        let input_volume = Arc::new(AtomicF32::new(1.0));
        let rms_threshold = Arc::new(AtomicF32::new(1.0));
        let muted = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let state = InputProcessorState::new(
            &input_volume,
            &rms_threshold,
            &muted,
            rms_sender.clone(),
            DEFAULT_POOL_CAPACITY,
        );

        state.send_rms(0.3);
        state.send_rms(0.1);

        approx_eq(rms_sender.load(Relaxed), 0.3);
    }

    #[test]
    fn input_state_send_rms_updates_upward() {
        let input_volume = Arc::new(AtomicF32::new(1.0));
        let rms_threshold = Arc::new(AtomicF32::new(1.0));
        let muted = Arc::new(AtomicBool::new(false));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let state = InputProcessorState::new(
            &input_volume,
            &rms_threshold,
            &muted,
            rms_sender.clone(),
            DEFAULT_POOL_CAPACITY,
        );

        state.send_rms(0.1);
        state.send_rms(0.9);

        approx_eq(rms_sender.load(Relaxed), 0.9);
    }

    #[test]
    fn output_state_default_volume_is_one() {
        let state = OutputProcessorState::default();
        approx_eq(state.output_volume(), 1.0);
    }

    #[test]
    fn output_state_default_not_deafened() {
        let state = OutputProcessorState::default();
        assert!(!state.is_deafened());
    }

    #[test]
    fn output_state_volume_reflects_atomic() {
        let output_volume = Arc::new(AtomicF32::new(1.0));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let deafened = Arc::new(AtomicBool::new(false));
        let loss_sender = Arc::new(AtomicUsize::new(0));
        let state =
            OutputProcessorState::new(&output_volume, rms_sender, &deafened, loss_sender);

        output_volume.store(0.63, Relaxed);
        approx_eq(state.output_volume(), 0.63);
    }

    #[test]
    fn output_state_deafen_reflects_atomic() {
        let output_volume = Arc::new(AtomicF32::new(1.0));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let deafened = Arc::new(AtomicBool::new(false));
        let loss_sender = Arc::new(AtomicUsize::new(0));
        let state =
            OutputProcessorState::new(&output_volume, rms_sender, &deafened, loss_sender);

        deafened.store(true, Relaxed);
        assert!(state.is_deafened());
    }

    #[test]
    fn output_state_send_rms_uses_max() {
        let output_volume = Arc::new(AtomicF32::new(1.0));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let deafened = Arc::new(AtomicBool::new(false));
        let loss_sender = Arc::new(AtomicUsize::new(0));
        let state = OutputProcessorState::new(
            &output_volume,
            rms_sender.clone(),
            &deafened,
            loss_sender,
        );

        state.send_rms(0.4);
        state.send_rms(0.2);

        approx_eq(rms_sender.load(Relaxed), 0.4);
    }

    #[test]
    fn output_state_send_loss_accumulates() {
        let output_volume = Arc::new(AtomicF32::new(1.0));
        let rms_sender = Arc::new(AtomicF32::new(0.0));
        let deafened = Arc::new(AtomicBool::new(false));
        let loss_sender = Arc::new(AtomicUsize::new(0));
        let state = OutputProcessorState::new(
            &output_volume,
            rms_sender,
            &deafened,
            loss_sender.clone(),
        );

        state.send_loss(3);
        state.send_loss(3);

        assert_eq!(loss_sender.load(Relaxed), 6);
    }
}
