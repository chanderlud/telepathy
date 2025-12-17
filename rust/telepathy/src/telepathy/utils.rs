use crate::error::{Error, ErrorKind};
use crate::flutter::Statistics;
use crate::flutter::callbacks::FrbStatisticsCallback;
use crate::overlay::{CONNECTED, LATENCY, LOSS};
use crate::telepathy::sockets::{Transport, TransportStream};
use crate::telepathy::{
    ConnectionState, DeviceName, StatisticsCollectorState, TRANSFER_BUFFER_SIZE,
};
use bincode::config::standard;
use bincode::{Decode, Encode, decode_from_slice, encode_to_vec};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, Host, Stream};
use flutter_rust_bridge::for_generated::futures::{Sink, SinkExt};
use kanal::{AsyncReceiver, AsyncSender};
use libp2p::bytes::Bytes;
use libp2p::futures::StreamExt;
use libp2p::swarm::ConnectionId;
use log::debug;
use sea_codec::ProcessorMessage;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::select;
use tokio::sync::Notify;
#[cfg(not(target_family = "wasm"))]
use tokio::time::interval;
use tokio_util::codec::LengthDelimitedCodec;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::interval;

type Result<T> = std::result::Result<T, Error>;

/// wraps a cpal stream to unsafely make it send
pub(crate) struct SendStream {
    pub(crate) stream: Stream,
}

/// Safety: SendStream must not be used across awaits
unsafe impl Send for SendStream {}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
enum Locality {
    Loopback = 0,
    Lan = 1,
    Public = 2,
    Unknown = 3,
}

/// converts a decibel value to a multiplier
pub(crate) fn db_to_multiplier(db: f32) -> f32 {
    10_f32.powf(db / 20_f32)
}

/// Gets the output device
pub(crate) async fn get_output_device(
    output_device: &DeviceName,
    host: &Arc<Host>,
) -> Result<Device> {
    match *output_device.lock().await {
        Some(ref name) => Ok(host
            .output_devices()?
            .find(|device| {
                if let Ok(ref device_name) = device.name() {
                    name == device_name
                } else {
                    false
                }
            })
            .unwrap_or(
                host.default_output_device()
                    .ok_or(ErrorKind::NoOutputDevice)?,
            )),
        None => host
            .default_output_device()
            .ok_or(ErrorKind::NoOutputDevice.into()),
    }
}

/// Returns the percentage of the max input volume in the window compared to the max volume
pub(crate) fn level_from_window(local_max: f32, max: &mut f32) -> f32 {
    *max = max.max(local_max);
    if *max != 0_f32 {
        let level = local_max / *max;
        if level < 0.01 { 0_f32 } else { level }
    } else {
        0_f32
    }
}

/// Writes a bincode message to the stream
pub(crate) async fn write_message<M: Encode, W>(
    transport: &mut Transport<W>,
    message: &M,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    Transport<W>: Sink<Bytes> + Unpin,
{
    let buffer = encode_to_vec(message, standard())?;

    transport
        .send(Bytes::from(buffer))
        .await
        .map_err(|_| ErrorKind::TransportSend)
        .map_err(Into::into)
}

/// Reads a bincode message from the stream
pub(crate) async fn read_message<M: Decode<()>, R: AsyncRead + Unpin>(
    transport: &mut Transport<R>,
) -> Result<M> {
    if let Some(Ok(buffer)) = transport.next().await {
        let (message, _) = decode_from_slice(&buffer[..], standard())?; // decode the message
        Ok(message)
    } else {
        Err(ErrorKind::TransportRecv.into())
    }
}

/// Collects statistics from throughout the application, processes them, and provides them to the frontend
pub(crate) async fn statistics_collector<C: FrbStatisticsCallback>(
    state: StatisticsCollectorState,
    callback: C,
    cancel: CancellationToken,
    efficient: bool,
) {
    // the interval for statistics updates
    let mut update_interval = interval(Duration::from_millis(if efficient { 500 } else { 100 }));
    // the interval for the input_max and output_max to decrease
    let mut reset_interval = interval(Duration::from_secs(5));
    // max input RMS
    let mut input_max = 0_f32;
    // max output RMS
    let mut output_max = 0_f32;

    loop {
        select! {
            _ = cancel.cancelled() => {
                break;
            }
            _ = update_interval.tick() => {
                let latency = state.latency.load(Relaxed);
                let loss = state.loss.swap(0, Relaxed);

                callback.post(Statistics {
                    input_level: level_from_window(state.input_rms.swap(0_f32, Relaxed), &mut input_max),
                    output_level: level_from_window(state.output_rms.swap(0_f32, Relaxed), &mut output_max),
                    latency,
                    upload_bandwidth: state.upload_bandwidth.load(Relaxed),
                    download_bandwidth: state.download_bandwidth.load(Relaxed),
                    loss,
                }).await;

                LATENCY.store(latency, Relaxed);
                LOSS.store(loss, Relaxed);
            }
            _ = reset_interval.tick() => {
                input_max /= 2_f32;
                output_max /= 2_f32;
            }
        }
    }

    // zero out the statistics when the collector ends
    callback.post(Statistics::default()).await;
    LATENCY.store(0, Relaxed);
    LOSS.store(0, Relaxed);
    CONNECTED.store(false, Relaxed);
    debug!("statistics collector returning");
}

/// Used for audio tests, plays the input into the output
pub(crate) async fn loopback(
    input_receiver: AsyncReceiver<ProcessorMessage>,
    output_sender: AsyncSender<ProcessorMessage>,
    cancel: &CancellationToken,
    end_call: &Arc<Notify>,
) {
    loop {
        select! {
            _ = end_call.notified() => {
                break;
            }
            _ = cancel.cancelled() => {
                break;
            },
            message = input_receiver.recv() => {
                if let Ok(message) = message {
                    if output_sender.try_send(message).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            },
        }
    }
}

pub(crate) fn stream_to_audio_transport(stream: libp2p::Stream) -> Transport<TransportStream> {
    LengthDelimitedCodec::builder()
        .max_frame_length(TRANSFER_BUFFER_SIZE)
        .length_field_type::<u16>()
        .new_framed(stream.compat())
}

fn classify_locality(addr: Option<IpAddr>) -> Locality {
    match addr {
        Some(IpAddr::V4(v4)) => {
            if v4.is_loopback() {
                Locality::Loopback
            } else if v4.is_private() || v4.is_link_local() {
                Locality::Lan
            } else {
                Locality::Public
            }
        }
        Some(IpAddr::V6(v6)) => {
            if v6.is_loopback() {
                Locality::Loopback
            } else if v6.is_unique_local() || v6.is_unicast_link_local() {
                Locality::Lan
            } else {
                Locality::Public
            }
        }
        None => Locality::Unknown,
    }
}

/// Pick the best connection according to:
/// - non-relayed > relayed, then loopback > lan > public > unknown, then lowest latency
/// - retries & ipv6 status are considered last
pub(crate) fn select_best_connection(
    conns: &HashMap<ConnectionId, ConnectionState>,
) -> Option<(ConnectionId, &ConnectionState)> {
    conns
        .iter()
        .min_by_key(|(id, s)| {
            // lower is better
            let relayed_rank = if s.relayed { 1 } else { 0 };
            let locality_rank = classify_locality(s.remote_address);
            let latency_rank = s.latency.unwrap_or(u128::MAX);
            let retries_rank = s.retries.load(Relaxed);
            let is_ipv6 = if s.remote_address.is_some_and(|a| a.is_ipv6()) {
                0
            } else {
                1
            };
            (
                relayed_rank,
                locality_rank,
                latency_rank,
                retries_rank,
                is_ipv6,
                **id,
            )
        })
        .map(|(id, s)| (*id, s))
}
