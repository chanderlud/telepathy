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
//! ## Typical Usage
//!
//! State structures are typically created internally by `AudioInputBuilder`
//! and `AudioOutputBuilder`. However, they can be constructed manually for
//! advanced use cases:
//!
//! ```rust,no_run
//! use telepathy_audio::internal::state::InputProcessorState;
//! use std::sync::Arc;
//! use atomic_float::AtomicF32;
//! use std::sync::atomic::AtomicBool;
//!
//! // Create shared atomics
//! let volume = Arc::new(AtomicF32::new(1.0));
//! let threshold = Arc::new(AtomicF32::new(0.01));
//! let muted = Arc::new(AtomicBool::new(false));
//! let rms = Arc::new(AtomicF32::new(0.0));
//!
//! // Create state
//! let state = InputProcessorState::new(&volume, &threshold, &muted, rms);
//!
//! // State can now be passed to input_processor function
//! ```
//!
//! ## Statistics Collection
//!
//! Both state types include RMS (Root Mean Square) senders for audio level
//! monitoring. The processor updates these atomics with the maximum RMS
//! value encountered, which can be read by the application for visualization
//! or voice activity detection.

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
    ) -> Self {
        Self {
            input_volume: input_volume.clone(),
            rms_threshold: rms_threshold.clone(),
            muted: muted.clone(),
            rms_sender,
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
}

impl Default for InputProcessorState {
    fn default() -> Self {
        Self {
            input_volume: Arc::new(AtomicF32::new(1.0)),
            rms_threshold: Arc::new(AtomicF32::new(1.0)),
            muted: Arc::new(Default::default()),
            rms_sender: Arc::new(Default::default()),
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
