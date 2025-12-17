use crate::error::Error;
use crate::telepathy::KEEP_ALIVE;
use kanal::{AsyncReceiver, AsyncSender};
use libp2p::Stream;
use libp2p::bytes::Bytes;
use libp2p::futures::stream::{SplitSink, SplitStream};
use libp2p::futures::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use nnnoiseless::FRAME_SIZE;
use sea_codec::ProcessorMessage;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::select;
#[cfg(not(target_family = "wasm"))]
use tokio::time::timeout;
use tokio_util::bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::compat::Compat;
use tokio_util::sync::CancellationToken;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::timeout;

pub(crate) type SharedSockets = Arc<Mutex<Vec<(AudioSocket, Instant)>>>;
pub(crate) type TransportStream = Compat<Stream>;
pub(crate) type Transport<T> = Framed<T, LengthDelimitedCodec>;
pub(crate) type AudioSocket = SplitSink<Transport<TransportStream>, Bytes>;

/// only packets younger than this are accepted, represents 2.5 seconds
const MAX_AGE: u32 = 250;

pub(crate) trait SendingSocket {
    async fn send(&mut self, packet: Bytes) -> usize;
}

pub(crate) struct ConstSocket {
    socket: AudioSocket,

    start: Instant,
}

impl ConstSocket {
    pub(crate) fn new(socket: AudioSocket) -> Self {
        Self {
            socket,
            start: Instant::now(),
        }
    }
}

impl SendingSocket for ConstSocket {
    async fn send(&mut self, packet: Bytes) -> usize {
        let prepared_packet = prepend_timestamp(&packet, timestamp(&self.start));
        if self.socket.send(prepared_packet).await.is_ok() {
            1
        } else {
            0
        }
    }
}

pub(crate) struct SendingSockets {
    new_sockets: SharedSockets,

    sockets: Vec<(AudioSocket, Instant)>,
}

impl SendingSocket for SendingSockets {
    async fn send(&mut self, packet: Bytes) -> usize {
        // this unwrap is safe because holders of SharedSockets do not panic
        for pair in self.new_sockets.lock().unwrap().drain(..) {
            self.sockets.push(pair);
        }

        // send the bytes to all connections, dropping any that error
        let mut i = 0;
        let mut successful_sends = 0;

        while i < self.sockets.len() {
            let send_result = {
                // limit the &mut borrow to this block
                let socket = &mut self.sockets[i];
                let now = timestamp(&socket.1);
                socket.0.send(prepend_timestamp(&packet, now)).await
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
        }
    }
}

/// Receives frames of audio data from the input processor and sends them to the socket
pub(crate) async fn audio_input<S: SendingSocket>(
    input_receiver: AsyncReceiver<ProcessorMessage>,
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

        let bytes = match message {
            Ok(Ok(ProcessorMessage::Data(bytes))) => bytes,
            // shutdown
            Ok(_) => {
                debug!("audio_input ended with input shutdown");
                break Ok(());
            }
            // send keep alive during extended silence
            Err(_) => keep_alive.clone(),
        };

        let bytes_len = bytes.len();
        // send the bytes to all connections, dropping any that error
        let successful_sends = sockets.send(bytes).await;
        // update bandwidth based on successful sends only
        if successful_sends > 0 {
            bandwidth.fetch_add(bytes_len * successful_sends, Relaxed);
        }
    }
}

/// Receives audio data from the socket and sends it to the output processor
pub(crate) async fn audio_output(
    sender: AsyncSender<ProcessorMessage>,
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
                    if message.get_u32().abs_diff(timestamp(&started_at)) < MAX_AGE {
                        if sender.try_send(ProcessorMessage::bytes(message.freeze())).is_err() {
                            info!("audio_output ended with closed channel");
                            break Ok(());
                        }
                    } else {
                        loss.fetch_add(FRAME_SIZE, Relaxed);
                    }
                } else if len == 1 {
                    debug!("audio_output received keep alive");
                } else {
                    warn!("audio_output received unexpected message");
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

fn prepend_timestamp(payload: &Bytes, ts: u32) -> Bytes {
    let mut buf = BytesMut::with_capacity(4 + payload.len());
    buf.put_u32(ts);
    buf.extend_from_slice(payload);
    buf.freeze()
}

/// allows for ~12,000 hours per session before overflow
fn timestamp(start: &Instant) -> u32 {
    start.elapsed().as_millis() as u32 / 10
}
