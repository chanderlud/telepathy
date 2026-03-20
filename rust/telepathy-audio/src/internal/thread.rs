//! Thread abstraction for cross-platform compatibility.
//!
//! This module provides a unified threading API that works across both native
//! and WASM targets:
//!
//! - **Native**: Re-exports `std::thread` (OS threads)
//! - **WASM**: Re-exports `wasm_thread` (Web Workers)
//!
//! The `wasm_thread` crate provides a near-identical API to `std::thread`,
//! allowing the rest of the codebase to use `crate::internal::thread::spawn`
//! and `crate::internal::thread::JoinHandle` without conditional compilation
//! at each call site.
//!
//! ## Safe Spawning on WASM
//!
//! [`safe_spawn`] wraps `thread::spawn` with `std::panic::catch_unwind` on
//! WASM targets. If threading is unavailable (e.g., missing COOP/COEP headers
//! for `SharedArrayBuffer`), the spawn panics; `safe_spawn` catches this and
//! returns `Err(AudioError::WasmThreading(...))` instead of crashing.

#[cfg(not(target_family = "wasm"))]
pub(crate) use std::thread::*;

#[cfg(target_family = "wasm")]
pub(crate) use wasm_thread::*;

use crate::error::Error;

/// Spawns a thread, catching panics on WASM when threading is unavailable.
///
/// On native targets this is a simple wrapper around `std::thread::spawn`.
/// On WASM targets the call is wrapped with `std::panic::catch_unwind` so
/// that a missing `SharedArrayBuffer` (or any other spawn-time panic) is
/// converted into `AudioError::WasmThreading` instead of aborting.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn safe_spawn<F>(f: F) -> std::result::Result<JoinHandle<()>, Error>
where
    F: FnOnce() + Send + 'static,
{
    Ok(spawn(f))
}

#[cfg(target_family = "wasm")]
pub(crate) fn safe_spawn<F>(f: F) -> std::result::Result<JoinHandle<()>, Error>
where
    F: FnOnce() + Send + 'static,
{
    use std::panic::{AssertUnwindSafe, catch_unwind};

    match catch_unwind(AssertUnwindSafe(|| spawn(f))) {
        Ok(handle) => Ok(handle),
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "thread::spawn panicked (threading may be unavailable)".to_string()
            };
            Err(Error::WasmThreading(msg))
        }
    }
}
