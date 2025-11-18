use crate::api::error::{Error, ErrorKind};
use crate::api::sockets::Transport;
use crate::api::telepathy::DeviceName;
use bincode::config::standard;
use bincode::{Decode, Encode, decode_from_slice, encode_to_vec};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, Host, Stream};
use flutter_rust_bridge::for_generated::futures::{Sink, SinkExt};
use libp2p::bytes::Bytes;
use libp2p::futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use tokio::io::{AsyncRead, AsyncWrite};

type Result<T> = std::result::Result<T, Error>;

/// wraps a cpal stream to unsafely make it send
pub(crate) struct SendStream {
    pub(crate) stream: Stream,
}

/// Safety: SendStream must not be used across awaits
unsafe impl Send for SendStream {}

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
        // TODO could decode from slice borrowed be used here to potentially avoid copying
        let (message, _) = decode_from_slice(&buffer[..], standard())?; // decode the message
        Ok(message)
    } else {
        Err(ErrorKind::TransportRecv.into())
    }
}

pub(crate) fn atomic_u32_serialize<S>(
    value: &Arc<AtomicU32>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let value = value.load(Relaxed);
    serializer.serialize_u32(value)
}

pub(crate) fn atomic_u32_deserialize<'de, D>(
    deserializer: D,
) -> std::result::Result<Arc<AtomicU32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    Ok(Arc::new(AtomicU32::new(value)))
}

pub(crate) mod rwlock_option_recording_config {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::api::flutter::RecordingConfig;

    pub fn serialize<S>(
        value: &Arc<RwLock<Option<RecordingConfig>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lock = value.blocking_read();
        lock.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Arc<RwLock<Option<RecordingConfig>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = Option::<RecordingConfig>::deserialize(deserializer)?;
        Ok(Arc::new(RwLock::new(inner)))
    }
}
