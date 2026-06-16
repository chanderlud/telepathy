use crate::internal::KEEP_ALIVE;
use crate::internal::error::Error;
use bytes::Bytes;
use iroh::endpoint::Connection;
use kanal::{AsyncReceiver, Sender};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use telepathy_audio::FRAME_SIZE;
use telepathy_audio::internal::NETWORK_FRAME;
use telepathy_audio::internal::buffer_pool::{BufferPool, PooledBuffer, PooledBytes};
use tokio::select;
#[cfg(not(target_family = "wasm"))]
use tokio::time::{Instant, timeout};
use tokio_util::bytes::{Buf, BufMut};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
#[cfg(target_family = "wasm")]
use wasmtimer::{std::Instant, tokio::timeout};

pub(crate) type SharedConnections = Arc<RoomConnectionRegistry>;

/// 4 bytes for sequence number
const HEADER_SIZE: usize = 4;
const JITTER_LATENCY_FRAMES: u32 = 5;
const MAX_BUFFERED_FRAMES: u32 = 32;
/// Pre-computed exact capacity for prepared audio packets
const PACKET_BUFFER_CAPACITY: usize = HEADER_SIZE + NETWORK_FRAME;
/// Pool size for timestamp buffers
const PACKET_POOL_SIZE: usize = 8;
const KEEP_ALIVE_TAG: u8 = 1;
const KEEP_ALIVE_PACKET_SIZE: usize = HEADER_SIZE + 1;

/// Tracks room audio transport connections for the shared uplink path.
///
/// Connections are keyed by [`Connection::stable_id`] so duplicate joins and
/// leaves can remove the exact socket even after a peer reconnects.
pub(crate) struct RoomConnectionRegistry {
    pending: Mutex<Vec<Connection>>,
    removed: Mutex<HashSet<usize>>,
}

impl Default for RoomConnectionRegistry {
    fn default() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            removed: Mutex::new(HashSet::new()),
        }
    }
}

impl RoomConnectionRegistry {
    pub(crate) fn push(&self, connection: Connection) {
        self.pending.lock().unwrap().push(connection);
    }

    pub(crate) fn remove(&self, connection: &Connection) {
        let connection_id = connection.stable_id();
        self.removed.lock().unwrap().insert(connection_id);
        self.pending
            .lock()
            .unwrap()
            .retain(|pending| pending.stable_id() != connection_id);
    }

    fn is_removed(&self, connection: &Connection) -> bool {
        self.removed
            .lock()
            .unwrap()
            .contains(&connection.stable_id())
    }

    fn drain_pending(&self) -> Vec<Connection> {
        self.pending.lock().unwrap().drain(..).collect()
    }
}

pub(crate) trait TelepathyConnection {
    fn send(&mut self, packet: &Bytes) -> usize;
}

pub(crate) struct ConstConnection {
    connection: Connection,

    /// Reused buffers via `BufferPool` for zero-reallocation packet construction.
    /// Buffers are automatically returned to the pool when `PooledBytes` is dropped
    /// (if not cloned), leveraging `Bytes::try_into_mut()` for recovery.
    packet_buffers: Arc<BufferPool>,

    sequence_number: u32,
}

impl ConstConnection {
    pub(crate) fn new(connection: Connection) -> Self {
        Self {
            connection,
            packet_buffers: Arc::new(BufferPool::new(PACKET_POOL_SIZE, PACKET_BUFFER_CAPACITY)),
            sequence_number: 0,
        }
    }
}

impl TelepathyConnection for ConstConnection {
    fn send(&mut self, packet: &Bytes) -> usize {
        let pooled_buffer = BufferPool::acquire(&self.packet_buffers);
        let is_keep_alive = is_keep_alive_payload(packet.as_ref());
        let sequence_number = self.sequence_number;
        let pooled_bytes = prepare_packet(
            pooled_buffer,
            packet.as_ref(),
            sequence_number,
            is_keep_alive,
        );
        // Clone the inner Bytes (O(1) refcount increment) for send, allowing
        // automatic buffer recovery when pooled_bytes is dropped after send completes.
        let ok = self
            .connection
            .send_datagram((*pooled_bytes).clone())
            .is_ok();

        if ok && !is_keep_alive {
            self.sequence_number = self.sequence_number.wrapping_add(1);
        }

        ok as usize
    }
}

pub(crate) struct DynamicConnection {
    new_connections: SharedConnections,

    connections: Vec<Connection>,

    /// Reused buffers via `BufferPool` for zero-reallocation packet construction.
    /// Buffers are automatically returned to the pool when `PooledBytes` is dropped.
    packet_buffers: Arc<BufferPool>,

    /// Single sequence space intentionally shared across all room peers so the
    /// sender only ever increments one counter regardless of how many peers are
    /// currently connected. This means a peer that joins mid-talkspurt will
    /// observe a large initial sequence gap (the counter has already advanced
    /// for any audio it missed). Newly-joined peers rely on
    /// [`AudioJitterBuffer`]'s first-packet talkspurt-start handling in
    /// `insert_audio` to anchor playout: the first packet it sees becomes the
    /// anchor, jump-starting playback at that point rather than waiting for
    /// the original sequence origin.
    sequence_number: u32,
}

impl TelepathyConnection for DynamicConnection {
    fn send(&mut self, packet: &Bytes) -> usize {
        for connection in self.new_connections.drain_pending() {
            if !self.new_connections.is_removed(&connection) {
                self.connections.push(connection);
            }
        }

        self.connections
            .retain(|connection| !self.new_connections.is_removed(connection));

        // send the bytes to all connections, dropping any that error
        let mut i = 0;
        let mut successful_sends = 0;
        let is_keep_alive = is_keep_alive_payload(packet.as_ref());
        let sequence_number = self.sequence_number;

        while i < self.connections.len() {
            let pooled_buffer = BufferPool::acquire(&self.packet_buffers);

            let send_result = {
                // limit the &mut borrow to this block
                let socket = &mut self.connections[i];

                let pooled_bytes = prepare_packet(
                    pooled_buffer,
                    packet.as_ref(),
                    sequence_number,
                    is_keep_alive,
                );
                // Clone for send (O(1)), buffer recovered automatically via Drop
                socket.send_datagram((*pooled_bytes).clone())
            };

            if send_result.is_err() {
                // remove this socket, do NOT increment i
                _ = self.connections.remove(i);
                info!(
                    remaining = self.connections.len(),
                    "audio_input dropping socket"
                );
            } else {
                successful_sends += 1;
                i += 1;
            }
        }

        if successful_sends > 0 && !is_keep_alive {
            self.sequence_number = self.sequence_number.wrapping_add(1);
        }
        successful_sends
    }
}

impl DynamicConnection {
    pub(crate) fn new(new_connections: SharedConnections) -> Self {
        Self {
            new_connections,
            connections: Vec::new(),
            packet_buffers: Arc::new(BufferPool::new(PACKET_POOL_SIZE, PACKET_BUFFER_CAPACITY)),
            sequence_number: 0,
        }
    }
}

#[derive(Default)]
struct AudioJitterBuffer {
    packets: BTreeMap<u32, Bytes>,

    /// Next audio sequence number we want to play.
    next_seq: Option<u32>,

    /// Sequence number used to map packet sequence to wall-clock playout time.
    anchor_seq: u32,

    /// Wall-clock time when `anchor_seq` should be played.
    anchor_deadline: Option<Instant>,

    /// Anything before this has been made obsolete by playback or keepalive.
    min_seq: Option<u32>,

    /// The sample rate of the audio source used to calculate frame durations
    sample_rate: u32,
}

impl AudioJitterBuffer {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            ..Default::default()
        }
    }

    fn advance_min_seq(&mut self, candidate: u32) {
        if self
            .min_seq
            .is_none_or(|min_seq| seq_before(min_seq, candidate))
        {
            self.min_seq = Some(candidate);
        }
    }

    fn reset_after_keepalive(&mut self, sequence_floor: u32) {
        // Ignore stale keepalives from a previous talkspurt.
        if let Some(next_seq) = self.next_seq
            && !seq_before(next_seq, sequence_floor)
        {
            return;
        }
        self.packets.clear();
        self.next_seq = None;
        self.anchor_deadline = None;

        // The keepalive carries the next audio sequence number, not its own sequence.
        self.advance_min_seq(sequence_floor);
    }

    fn insert_audio(&mut self, seq: u32, payload: Bytes, now: Instant) -> bool {
        if let Some(min_seq) = self.min_seq
            && seq_before(seq, min_seq)
        {
            return false;
        }

        // First packet in a talkspurt. Start playout after the jitter delay.
        if self.next_seq.is_none() {
            self.next_seq = Some(seq);
            self.anchor_seq = seq;
            self.anchor_deadline = Some(now + self.frame_duration(JITTER_LATENCY_FRAMES));
        }

        let next_seq = self.next_seq.unwrap();

        // Already played or skipped.
        if seq_before(seq, next_seq) {
            return false;
        }

        let ahead = seq.wrapping_sub(next_seq);

        // Huge jump usually means a silence gap, old talkspurt, or major loss.
        // Restart instead of letting one packet force hundreds of fake losses.
        if ahead > MAX_BUFFERED_FRAMES {
            self.packets.clear();
            self.next_seq = Some(seq);
            self.anchor_seq = seq;
            self.anchor_deadline = Some(now + self.frame_duration(JITTER_LATENCY_FRAMES));
        }

        self.packets.insert(seq, payload).is_none()
    }

    fn deadline_for(&self, seq: u32) -> Option<Instant> {
        let anchor_deadline = self.anchor_deadline?;
        let offset_frames = seq.wrapping_sub(self.anchor_seq);
        Some(anchor_deadline + self.frame_duration(offset_frames))
    }

    fn next_deadline(&self) -> Option<Instant> {
        // Important: do not keep advancing forever when the sender is silent.
        // Only run playout while we have something buffered ahead.
        if self.packets.is_empty() {
            return None;
        }

        let next_seq = self.next_seq?;
        self.deadline_for(next_seq)
    }

    /// Returns:
    /// - None: not time to play yet
    /// - Some(Some(payload)): play this packet
    /// - Some(None): packet was missing, count/drop it
    fn pop_due(&mut self, now: Instant) -> Option<Option<Bytes>> {
        let next_seq = self.next_seq?;
        let deadline = self.deadline_for(next_seq)?;

        if now < deadline {
            return None;
        }

        let payload = self.packets.remove(&next_seq);
        let following = next_seq.wrapping_add(1);

        self.next_seq = Some(following);
        self.advance_min_seq(following);

        // If the buffer is empty after a miss or the last emitted packet,
        // go idle. This avoids counting silence as infinite packet loss.
        if self.packets.is_empty() {
            self.next_seq = None;
            self.anchor_deadline = None;
        }

        Some(payload)
    }

    fn frame_duration(&self, frames: u32) -> Duration {
        let samples = frames as u128 * FRAME_SIZE as u128;
        let nanos = samples * 1_000_000_000u128 / self.sample_rate.max(1) as u128;
        Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)
    }
}

/// Receives frames of audio data from the input processor and sends them to the socket
pub(crate) async fn audio_input<S: TelepathyConnection>(
    input_receiver: AsyncReceiver<PooledBuffer>,
    mut sockets: S,
    cancel: CancellationToken,
) -> Result<(), Error> {
    // static signal bytes
    let keep_alive = Bytes::from_static(&[KEEP_ALIVE_TAG]);

    loop {
        let message = select! {
            message = timeout(KEEP_ALIVE, input_receiver.recv()) => message,
            _ = cancel.cancelled() => {
                debug!("audio_input ended with cancellation");
                break Ok(());
            }
        };

        match message {
            Ok(Ok(buffer)) => {
                // send the bytes to all connections, dropping any that error
                let bytes = buffer.freeze();
                sockets.send(bytes.as_ref())
            }
            // shutdown
            Ok(_) => {
                debug!("audio_input ended with input shutdown");
                break Ok(());
            }
            // send keep alive during extended silence
            Err(_) => sockets.send(&keep_alive),
        };
    }
}

/// Receives audio data from the socket and sends it to the output processor
pub(crate) async fn audio_output(
    sender: Sender<Bytes>,
    connection: Connection,
    cancel: CancellationToken,
    loss: Arc<AtomicUsize>,
    sample_rate: u32,
) -> Result<(), Error> {
    let mut jitter = AudioJitterBuffer::new(sample_rate);

    'outer: loop {
        // First, emit everything whose playout deadline has arrived.
        while let Some(payload) = jitter.pop_due(Instant::now()) {
            match payload {
                Some(payload) => {
                    if sender.try_send(payload).is_err() {
                        info!("audio_output ended with closed channel");
                        break 'outer Ok(());
                    }
                }
                None => {
                    // We know a packet was missing because a later packet was buffered.
                    loss.fetch_add(FRAME_SIZE, Relaxed);
                }
            }
        }

        let maybe_message = if let Some(deadline) = jitter.next_deadline() {
            let wait = deadline.saturating_duration_since(Instant::now());

            select! {
                _ = cancel.cancelled() => {
                    debug!("audio_output ended with cancellation");
                    break Ok(());
                },
                result = timeout(wait, connection.read_datagram()) => {
                    result.ok()
                }
            }
        } else {
            select! {
                _ = cancel.cancelled() => {
                    debug!("audio_output ended with cancellation");
                    break Ok(());
                },
                message = connection.read_datagram() => Some(message),
            }
        };

        let Some(message) = maybe_message else {
            continue;
        };

        match message {
            Ok(mut message) => {
                let len = message.len();

                // Real audio packets always exceed the size KEEP_ALIVE_PACKET_SIZE
                if len == KEEP_ALIVE_PACKET_SIZE && message[0] == KEEP_ALIVE_TAG {
                    message.advance(1);
                    let sequence_floor = message.get_u32();
                    debug!("audio_output received keep alive");
                    jitter.reset_after_keepalive(sequence_floor);
                } else if len < HEADER_SIZE {
                    warn!("audio_output received unexpected message len={len}");
                } else {
                    let sequence_number = message.get_u32();
                    let inserted = jitter.insert_audio(sequence_number, message, Instant::now());
                    if !inserted {
                        loss.fetch_add(FRAME_SIZE, Relaxed);
                    }
                }
            }
            Err(error) => {
                error!("audio_output error: {}", error);
                break Err(error.into());
            }
        }
    }
}

/// Zero-reallocation packet preparation using pooled buffers.
/// Takes ownership of a `PooledBuffer`, writes sequence metadata + payload,
/// and returns a `PooledBytes` that will automatically return the
/// underlying buffer to the pool when dropped (if refcount allows).
fn prepare_packet(
    mut buffer: PooledBuffer,
    payload: &[u8],
    sequence_number: u32,
    is_keep_alive: bool,
) -> PooledBytes {
    let buf = buffer.inner_mut();
    buf.clear();

    let total_size = if is_keep_alive {
        KEEP_ALIVE_PACKET_SIZE
    } else {
        HEADER_SIZE + payload.len()
    };

    if buf.capacity() < total_size {
        buf.reserve(total_size);
    }

    if is_keep_alive {
        // Keepalives do not consume a sequence number. They carry the current
        // next audio sequence as a floor so the receiver can drop stale buffered
        // packets from the previous talkspurt.
        buf.put_u8(KEEP_ALIVE_TAG);
        buf.put_u32(sequence_number);
    } else {
        // Write sequence number. bytes::BufMut::put_u32 uses big-endian/network order.
        buf.put_u32(sequence_number);
        buf.extend_from_slice(payload);
    }

    // Convert to PooledBytes for automatic buffer recovery via Drop
    buffer.freeze()
}

fn is_keep_alive_payload(payload: &[u8]) -> bool {
    payload == [KEEP_ALIVE_TAG]
}

/// True when `a` is older than `b` under u32 sequence-number wraparound.
fn seq_before(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) > 0x8000_0000
}
