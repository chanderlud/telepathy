//! Internal implementation details.
//!
//! **This module is not part of the public API.**
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

pub(crate) mod codec;
pub(crate) mod processing;
pub(crate) mod processor;
pub(crate) mod state;
pub(crate) mod traits;
pub(crate) mod utils;
