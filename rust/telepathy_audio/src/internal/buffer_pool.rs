//! Lock-free buffer pool for reducing allocator pressure.
//!
//! This module provides a pre-allocated pool of `BytesMut` buffers that can be
//! reused across audio frame transmissions. With audio frames arriving at ~100 Hz
//! (48kHz sample rate, 480-sample frames), this significantly reduces allocation
//! overhead compared to creating new `Bytes` buffers for each frame.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     acquire()      ┌──────────────┐     freeze()     ┌─────────────┐
//! │   BufferPool    │ ────────────────▶  │ PooledBuffer │ ──────────────▶  │ PooledBytes │
//! │  (ArrayQueue)   │                    │  (BytesMut)  │                  │  (Bytes)    │
//! └─────────────────┘                    └──────────────┘                  └─────────────┘
//!         ▲                                                                       │
//!         │                          Drop (return via try_mut)                    │
//!         └───────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Buffer Recovery
//!
//! When a `PooledBytes` is dropped, it attempts to recover the underlying buffer
//! using `Bytes::try_mut()`. If successful (reference count is 1), the buffer is
//! cleared and returned to the pool. If the `Bytes` was cloned or is still shared,
//! the buffer is simply dropped.
//!
//! ## Thread Safety
//!
//! The pool uses `crossbeam::queue::ArrayQueue` for lock-free concurrent access.
//! Multiple threads can safely acquire and return buffers simultaneously.

use crate::internal::NETWORK_FRAME;
use bytes::{Bytes, BytesMut};
use crossbeam::queue::ArrayQueue;
use std::ops::Deref;
use std::sync::Arc;

/// Default pool capacity (number of pre-allocated buffers).
///
/// With 10ms frames at 48kHz, 128 buffers provides ~1.28 seconds of buffering.
pub const DEFAULT_POOL_CAPACITY: usize = 128;

/// A lock-free pool of reusable byte buffers.
///
/// This pool pre-allocates a fixed number of `BytesMut` buffers at construction
/// time. Buffers can be acquired for use and are automatically returned to the
/// pool when the `PooledBuffer` wrapper is dropped.
///
/// If the pool is exhausted (all buffers in use), `acquire()` will allocate a
/// new buffer as a fallback. This ensures the audio pipeline never blocks, though
/// frequent fallback allocations indicate the pool size should be increased.
pub struct BufferPool {
    /// Lock-free queue of available buffers
    queue: ArrayQueue<BytesMut>,
    /// Size of each buffer in bytes
    buffer_size: usize,
}

impl BufferPool {
    /// Creates a new buffer pool with pre-allocated buffers.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Number of buffers to pre-allocate
    /// * `buffer_size` - Size of each buffer in bytes (typically `FRAME_SIZE * 2`)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use telepathy_audio::internal::buffer_pool::BufferPool;
    ///
    /// // Create pool for 960-byte frames (480 samples * 2 bytes/sample)
    /// let pool = BufferPool::new(128, 960);
    /// ```
    pub fn new(capacity: usize, buffer_size: usize) -> Self {
        let queue = ArrayQueue::new(capacity);

        // Pre-allocate all buffers
        for _ in 0..capacity {
            let mut buffer = BytesMut::with_capacity(buffer_size);
            // Set length to buffer_size so the buffer is immediately usable
            buffer.resize(buffer_size, 0);
            // Ignore push failure - queue is sized exactly for capacity
            let _ = queue.push(buffer);
        }

        Self { queue, buffer_size }
    }

    /// Acquires a buffer from the pool.
    ///
    /// If a buffer is available in the pool, it is returned wrapped in a
    /// `PooledBuffer`. If the pool is empty, a new buffer is allocated as
    /// a fallback to prevent blocking.
    ///
    /// # Arguments
    ///
    /// * `pool` - Arc reference to self (needed for the PooledBuffer to return the buffer)
    ///
    /// # Returns
    ///
    /// A `PooledBuffer` containing a `BytesMut` ready for use.
    pub fn acquire(pool: &Arc<BufferPool>) -> PooledBuffer {
        let buffer = pool.queue.pop().unwrap_or_else(|| {
            // Fallback: allocate new buffer if pool is exhausted
            let mut buffer = BytesMut::with_capacity(pool.buffer_size);
            buffer.resize(pool.buffer_size, 0);
            buffer
        });

        PooledBuffer {
            buffer: Some(buffer),
            pool: pool.clone(),
        }
    }

    /// Returns a buffer to the pool.
    ///
    /// The buffer is cleared before being returned to prevent data leakage.
    /// If the pool is full, the buffer is simply dropped.
    fn return_buffer(&self, mut buffer: BytesMut) {
        // Clear and resize to ensure consistent state
        buffer.clear();
        buffer.resize(self.buffer_size, 0);

        // Return to pool (ignore if full - buffer will be dropped)
        let _ = self.queue.push(buffer);
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(DEFAULT_POOL_CAPACITY, NETWORK_FRAME)
    }
}

/// A wrapper around a pooled buffer that returns it to the pool on drop.
///
/// This struct provides RAII-style buffer management. When dropped, the
/// underlying buffer is automatically returned to the pool for reuse.
///
/// # Usage
///
/// ```rust,ignore
/// use telepathy_audio::internal::buffer_pool::{BufferPool, PooledBuffer};
/// use std::sync::Arc;
///
/// let pool = Arc::new(BufferPool::new(16, 960));
/// let mut pooled = BufferPool::acquire(&pool);
///
/// // Write data to the buffer
/// pooled.as_mut()[0..4].copy_from_slice(&[1, 2, 3, 4]);
///
/// // Convert to PooledBytes for sending
/// let bytes = pooled.freeze();
/// // When `bytes` is dropped, buffer returns to pool
/// ```
pub struct PooledBuffer {
    /// The underlying buffer (Option to allow taking during freeze)
    buffer: Option<BytesMut>,
    /// Reference to the pool for returning the buffer
    pool: Arc<BufferPool>,
}

impl PooledBuffer {
    /// Returns a mutable reference to the underlying buffer.
    ///
    /// # Panics
    ///
    /// Panics if called after `freeze()` has been called.
    pub fn inner_mut(&mut self) -> &mut BytesMut {
        self.buffer.as_mut().expect("buffer already consumed")
    }

    /// Returns a new BytesMut cloned from the inner buffer
    ///
    /// This method is specifically used for looping back input
    /// frames into the output stack while returning the original
    /// buffer to the pool.
    ///
    /// # Panics
    ///
    /// Panics if called after `freeze()` has been called.
    pub fn clone_inner(&self) -> BytesMut {
        self.buffer.clone().expect("buffer already consumed")
    }

    /// Freezes the buffer into immutable `PooledBytes`.
    ///
    /// After calling this method, the buffer is wrapped in a `PooledBytes`
    /// that will attempt to return the buffer to the pool when dropped.
    /// The returned `PooledBytes` can be sent through channels and implements
    /// `Deref<Target=Bytes>` for transparent access.
    ///
    /// # Returns
    ///
    /// The buffer contents wrapped in `PooledBytes`.
    pub fn freeze(mut self) -> PooledBytes {
        let bytes = self
            .buffer
            .take()
            .expect("buffer already consumed")
            .freeze();
        PooledBytes {
            inner: Some(bytes),
            pool: self.pool.clone(),
        }
    }
}

impl AsRef<[u8]> for PooledBuffer {
    fn as_ref(&self) -> &[u8] {
        self.buffer.as_ref().expect("buffer already consumed")
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        // If buffer wasn't consumed by freeze(), return it to the pool
        if let Some(buffer) = self.buffer.take() {
            self.pool.return_buffer(buffer);
        }
    }
}

/// A wrapper around `Bytes` that returns the buffer to the pool on drop.
///
/// This struct provides the key mechanism for buffer reuse. When dropped,
/// it attempts to recover the underlying `BytesMut` using `Bytes::try_mut()`.
/// If the reference count is 1 (no other references exist), the buffer is
/// cleared and returned to the pool. If the `Bytes` was cloned or is still
/// shared, the buffer is simply dropped.
///
/// ## Deref Coercion
///
/// `PooledBytes` implements `Deref<Target=Bytes>`, allowing it to be used
/// transparently wherever `Bytes` is expected (via deref coercion).
///
/// ## Usage
///
/// ```rust,ignore
/// use telepathy_audio::internal::buffer_pool::{BufferPool, PooledBytes};
/// use std::sync::Arc;
///
/// let pool = Arc::new(BufferPool::new(16, 960));
/// let mut pooled = BufferPool::acquire(&pool);
///
/// // Write data to the buffer
/// pooled.as_mut()[0..4].copy_from_slice(&[1, 2, 3, 4]);
///
/// // Convert to PooledBytes for sending
/// let bytes = pooled.freeze();
/// assert_eq!(bytes.len(), 960);
///
/// // When `bytes` is dropped, buffer returns to pool (if refcount == 1)
/// ```
pub struct PooledBytes {
    /// The underlying Bytes (Option to allow taking during drop)
    inner: Option<Bytes>,
    /// Reference to the pool for returning the buffer
    pool: Arc<BufferPool>,
}

impl Deref for PooledBytes {
    type Target = Bytes;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("PooledBytes already consumed")
    }
}

impl AsRef<Bytes> for PooledBytes {
    fn as_ref(&self) -> &Bytes {
        self.deref()
    }
}

impl AsRef<[u8]> for PooledBytes {
    fn as_ref(&self) -> &[u8] {
        self.deref().as_ref()
    }
}

impl Drop for PooledBytes {
    fn drop(&mut self) {
        // Try to recover the buffer and return it to the pool
        if let Some(bytes) = self.inner.take() {
            // try_into_mut() succeeds only if refcount == 1 (we're the only owner)
            if let Ok(mut buffer) = bytes.try_into_mut() {
                // Clear the buffer to avoid data leakage, then return to pool
                buffer.clear();
                buffer.resize(self.pool.buffer_size, 0);
                // Return to pool (ignore if full - buffer will be dropped)
                let _ = self.pool.queue.push(buffer);
            }
            // If try_into_mut() fails, the Bytes was cloned/shared, just drop it
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let pool = Arc::new(BufferPool::new(16, 960));
        // Pool should have 16 buffers available
        assert_eq!(pool.queue.len(), 16);
    }

    #[test]
    fn test_acquire_and_return() {
        let pool = Arc::new(BufferPool::new(4, 960));
        assert_eq!(pool.queue.len(), 4);

        // Acquire a buffer
        let pooled = BufferPool::acquire(&pool);
        assert_eq!(pool.queue.len(), 3);

        // Drop the pooled buffer - it should return to pool
        drop(pooled);
        assert_eq!(pool.queue.len(), 4);
    }

    #[test]
    fn test_freeze_returns_on_drop() {
        let pool = Arc::new(BufferPool::new(4, 960));

        let pooled = BufferPool::acquire(&pool);
        assert_eq!(pool.queue.len(), 3);

        // Freeze wraps the buffer in PooledBytes
        let bytes = pooled.freeze();
        assert_eq!(pool.queue.len(), 3);

        // When PooledBytes is dropped, buffer should be returned
        drop(bytes);
        assert_eq!(pool.queue.len(), 4);
    }

    #[test]
    fn test_freeze_no_return_when_cloned() {
        let pool = Arc::new(BufferPool::new(4, 960));

        let pooled = BufferPool::acquire(&pool);
        assert_eq!(pool.queue.len(), 3);

        let bytes = pooled.freeze();
        // Clone the inner Bytes to increase refcount
        let _cloned = bytes.clone();

        // When PooledBytes is dropped, buffer should NOT be returned
        // because the inner Bytes was cloned (refcount > 1)
        drop(bytes);
        assert_eq!(pool.queue.len(), 3);

        // After the clone is dropped, we can't recover (original PooledBytes is gone)
        drop(_cloned);
        assert_eq!(pool.queue.len(), 3);
    }

    #[test]
    fn test_fallback_allocation() {
        let pool = Arc::new(BufferPool::new(2, 960));

        // Exhaust the pool
        let _b1 = BufferPool::acquire(&pool);
        let _b2 = BufferPool::acquire(&pool);
        assert_eq!(pool.queue.len(), 0);

        // This should still work (fallback allocation)
        let mut b3 = BufferPool::acquire(&pool);
        assert_eq!(b3.inner_mut().len(), 960);
    }

    #[test]
    fn test_buffer_clearing_on_return() {
        let pool = Arc::new(BufferPool::new(1, 16));

        // Acquire and modify buffer
        let mut pooled = BufferPool::acquire(&pool);
        pooled.inner_mut()[0..4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        drop(pooled);

        // Acquire again - buffer should be cleared
        let pooled = BufferPool::acquire(&pool);
        assert_eq!(&pooled.buffer.as_ref().unwrap()[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let pool = Arc::new(BufferPool::new(64, 960));
        let mut handles = vec![];

        for _ in 0..8 {
            let pool_clone = pool.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let mut pooled = BufferPool::acquire(&pool_clone);
                    // Simulate some work
                    pooled.inner_mut()[0] = 42;
                    // Sometimes freeze (returns on drop), sometimes return directly
                    if pooled.inner_mut()[0] % 2 == 0 {
                        let bytes = pooled.freeze();
                        // PooledBytes will return buffer to pool when dropped
                        drop(bytes);
                    }
                    // else: PooledBuffer returns buffer when dropped
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // After all threads complete, pool should be back to full capacity
        // (all buffers returned, though some may have been fallback allocations)
    }

    #[test]
    fn test_pooled_bytes_deref() {
        let pool = Arc::new(BufferPool::new(4, 16));

        let mut pooled = BufferPool::acquire(&pool);
        pooled.inner_mut()[0..4].copy_from_slice(&[1, 2, 3, 4]);

        let bytes = pooled.freeze();

        // Test Deref<Target=Bytes>
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[0..4], &[1, 2, 3, 4]);

        // Test AsRef<[u8]>
        let slice: &[u8] = bytes.as_ref();
        assert_eq!(&slice[0..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn test_pooled_bytes_clearing() {
        let pool = Arc::new(BufferPool::new(1, 16));

        // Acquire and write data
        let mut pooled = BufferPool::acquire(&pool);
        pooled.inner_mut()[0..4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);

        // Freeze and drop - buffer should be cleared before returning
        let bytes = pooled.freeze();
        drop(bytes);

        // Acquire again - buffer should be zeroed
        let pooled = BufferPool::acquire(&pool);
        assert_eq!(&pooled.buffer.as_ref().unwrap()[0..4], &[0, 0, 0, 0]);
    }
}
