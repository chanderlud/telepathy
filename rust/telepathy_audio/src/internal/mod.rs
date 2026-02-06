//! Internal implementation details.
//!
//! These modules contain internal implementation details used by the audio
//! processing pipeline. They are not intended for external use and may change
//! without notice between versions.
//!
//! ## Modules
//!
//! - `codec` - SEA codec encoding and decoding functions
//! - `processing` - SIMD-optimized audio processing functions
//! - `processor` - Core audio processor functions for input/output
//! - `state` - Processor state structures
//! - `traits` - AudioInput/AudioOutput traits and implementations
//! - `utils` - Internal utility functions (resampling, transitions)

use nnnoiseless::FRAME_SIZE;

pub mod buffer_pool;
pub mod codec;
pub mod processing;
pub mod processor;
pub mod state;
pub mod traits;
pub mod utils;

/// the maximum size in bytes of an audio frame
pub const NETWORK_FRAME: usize = FRAME_SIZE * size_of::<i16>();
