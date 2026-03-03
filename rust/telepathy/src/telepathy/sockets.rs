use crate::error::Error;
use crate::telepathy::KEEP_ALIVE;
use kanal::{AsyncReceiver, Sender};
use libp2p::Stream;
use libp2p::bytes::Bytes;
use libp2p::futures::stream::{SplitSink, SplitStream};
use libp2p::futures::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use telepathy_audio::internal::NETWORK_FRAME;
use telepathy_audio::internal::buffer_pool::BufferPool;
use telepathy_audio::{FRAME_SIZE, PooledBuffer, PooledBytes};
use tokio::select;
#[cfg(not(target_family = "wasm"))]
use tokio::time::{Instant, timeout};
use tokio_util::bytes::{Buf, BufMut};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::compat::Compat;
use tokio_util::sync::CancellationToken;
#[cfg(target_family = "wasm")]
use wasmtimer::{std::Instant, tokio::timeout};

pub(crate) type SharedSockets = Arc<Mutex<Vec<(AudioSocket, Instant)>>>;
pub(crate) type TransportStream = Compat<Stream>;
pub(crate) type Transport<T> = Framed<T, LengthDelimitedCodec>;
pub(crate) type AudioSocket = SplitSink<Transport<TransportStream>, Bytes>;

/// only packets younger than this are accepted, represents 2.5 seconds
const MAX_AGE: u32 = 250;

/// Pre-computed exact capacity for timestamp buffers (4 + FRAME_SIZE * 2)
pub(crate) const TIMESTAMP_BUFFER_CAPACITY: usize = 4 + NETWORK_FRAME;

/// Pool size for timestamp buffers - increased from 2 to 8 for better pipelining
const TIMESTAMP_POOL_SIZE: usize = 8;

pub(crate) trait SendingSocket {
    async fn send(&mut self, packet: &Bytes) -> usize;
}

pub(crate) struct ConstSocket {
    socket: AudioSocket,

    start: Instant,
    /// Reused buffers via `BufferPool` for zero-reallocation packet construction.
    /// Buffers are automatically returned to the pool when `PooledBytes` is dropped
    /// (if not cloned), leveraging `Bytes::try_into_mut()` for recovery.
    timestamp_buffers: Arc<BufferPool>,
}

impl ConstSocket {
    pub(crate) fn new(socket: AudioSocket) -> Self {
        Self {
            socket,
            start: Instant::now(),
            timestamp_buffers: Arc::new(BufferPool::new(
                TIMESTAMP_POOL_SIZE,
                TIMESTAMP_BUFFER_CAPACITY,
            )),
        }
    }
}

impl SendingSocket for ConstSocket {
    async fn send(&mut self, packet: &Bytes) -> usize {
        let pooled_buffer = BufferPool::acquire(&self.timestamp_buffers);
        let pooled_bytes = prepend_timestamp(pooled_buffer, packet, timestamp(&self.start));
        // Clone the inner Bytes (O(1) refcount increment) for send, allowing
        // automatic buffer recovery when pooled_bytes is dropped after send completes.
        let ok = self.socket.send((*pooled_bytes).clone()).await.is_ok();
        ok as usize
    }
}

pub(crate) struct SendingSockets {
    new_sockets: SharedSockets,

    sockets: Vec<(AudioSocket, Instant)>,
    /// Reused buffers via `BufferPool` for zero-reallocation packet construction.
    /// Buffers are automatically returned to the pool when `PooledBytes` is dropped.
    timestamp_buffers: Arc<BufferPool>,
}

impl SendingSocket for SendingSockets {
    async fn send(&mut self, packet: &Bytes) -> usize {
        // this unwrap is safe because holders of SharedSockets do not panic
        for pair in self.new_sockets.lock().unwrap().drain(..) {
            self.sockets.push(pair);
        }

        // send the bytes to all connections, dropping any that error
        let mut i = 0;
        let mut successful_sends = 0;

        while i < self.sockets.len() {
            let pooled_buffer = BufferPool::acquire(&self.timestamp_buffers);
            let send_result = {
                // limit the &mut borrow to this block
                let socket = &mut self.sockets[i];
                let now = timestamp(&socket.1);
                let pooled_bytes = prepend_timestamp(pooled_buffer, packet, now);
                // Clone for send (O(1)), buffer recovered automatically via Drop
                socket.0.send((*pooled_bytes).clone()).await
            };

            if send_result.is_err() {
                // remove this socket, do NOT increment i
                _ = self.sockets.remove(i);
                info!(
                    "audio_input dropping socket [remaining={}]",
                    self.sockets.len()
                );
            } else {
                successful_sends += 1;
                i += 1;
            }
        }

        successful_sends
    }
}

impl SendingSockets {
    pub(crate) fn new(new_sockets: SharedSockets) -> Self {
        Self {
            new_sockets,
            sockets: Vec::new(),
            timestamp_buffers: Arc::new(BufferPool::new(
                TIMESTAMP_POOL_SIZE,
                TIMESTAMP_BUFFER_CAPACITY,
            )),
        }
    }
}

/// Receives frames of audio data from the input processor and sends them to the socket
pub(crate) async fn audio_input<S: SendingSocket>(
    input_receiver: AsyncReceiver<PooledBuffer>,
    mut sockets: S,
    cancel: CancellationToken,
    bandwidth: Arc<AtomicUsize>,
) -> Result<(), Error> {
    // static signal bytes
    let keep_alive = Bytes::from_static(&[1]);

    loop {
        let message = select! {
            message = timeout(KEEP_ALIVE, input_receiver.recv()) => message,
            _ = cancel.cancelled() => {
                debug!("audio_input ended with cancellation");
                break Ok(());
            }
        };

        let (bytes_len, successful_sends) = match message {
            Ok(Ok(buffer)) => {
                // send the bytes to all connections, dropping any that error
                let bytes = buffer.freeze();
                (bytes.len(), sockets.send(bytes.as_ref()).await)
            }
            // shutdown
            Ok(_) => {
                debug!("audio_input ended with input shutdown");
                break Ok(());
            }
            // send keep alive during extended silence
            Err(_) => (1, sockets.send(&keep_alive).await),
        };

        // update bandwidth based on successful sends only
        if successful_sends > 0 {
            bandwidth.fetch_add(bytes_len * successful_sends, Relaxed);
        }
    }
}

/// Receives audio data from the socket and sends it to the output processor
pub(crate) async fn audio_output(
    sender: Sender<Bytes>,
    mut socket: SplitStream<Transport<TransportStream>>,
    cancel: CancellationToken,
    bandwidth: Arc<AtomicUsize>,
    loss: Arc<AtomicUsize>,
) -> Result<(), Error> {
    let started_at = Instant::now();

    loop {
        let message = select! {
            message = socket.next() => message,
            _ = cancel.cancelled() => {
                debug!("audio_output ended with cancellation");
                break Ok(());
            },
        };

        match message {
            Some(Ok(mut message)) => {
                let len = message.len();
                bandwidth.fetch_add(len, Relaxed);

                if len >= 16 {
                    // Use wrapping_sub for safe overflow handling
                    let packet_ts = message.get_u32();
                    let current_ts = timestamp(&started_at);
                    let age = packet_ts
                        .wrapping_sub(current_ts)
                        .min(current_ts.wrapping_sub(packet_ts));

                    if age < MAX_AGE {
                        if sender.try_send(message.freeze()).is_err() {
                            info!("audio_output ended with closed channel");
                            break Ok(());
                        }
                    } else {
                        loss.fetch_add(FRAME_SIZE, Relaxed);
                    }
                } else if len == 5 {
                    debug!("audio_output received keep alive");
                } else {
                    warn!("audio_output received unexpected message len={len}");
                }
            }
            Some(Err(error)) => {
                error!("audio_output error: {}", error);
                break Err(error.into());
            }
            None => {
                debug!("audio_output ended with None");
                break Ok(());
            }
        }
    }
}

/// Zero-reallocation timestamp prepending using pooled buffers.
/// Takes ownership of a `PooledBuffer`, writes timestamp + payload,
/// and returns a `PooledBytes` that will automatically return the
/// underlying buffer to the pool when dropped (if refcount allows).
fn prepend_timestamp(mut buffer: PooledBuffer, payload: &[u8], ts: u32) -> PooledBytes {
    let buf = buffer.inner_mut();
    buf.clear();

    let total_size = 4 + payload.len();

    // Fast-path: skip reserve if capacity is already sufficient
    if buf.capacity() < total_size {
        buf.reserve(total_size);
    }

    // Write timestamp (use native endian put_u32 for consistency with get_u32)
    buf.put_u32(ts);
    buf.extend_from_slice(payload);

    // Convert to PooledBytes for automatic buffer recovery via Drop
    buffer.freeze()
}

/// allows for ~12,000 hours per session before overflow
fn timestamp(start: &Instant) -> u32 {
    (start.elapsed().as_millis() / 10) as u32
}
