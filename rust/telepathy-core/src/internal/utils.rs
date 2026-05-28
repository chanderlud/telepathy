use crate::internal::callbacks::CoreStatisticsCallback;
use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::ProtocolMessage;
use crate::internal::state::StatisticsCollectorState;
use crate::overlay::{CONNECTED, LATENCY, LOSS};
use crate::types::Statistics;
use bytes::Bytes;
#[cfg(feature = "flutter")]
pub use flutter_rust_bridge::JoinHandle;
use flutter_rust_bridge::for_generated::futures::{SinkExt, StreamExt};
use iroh::endpoint::{RecvStream, SendStream};
use kanal::AsyncReceiver;
use speedy::{Readable, Writable};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Duration;
use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::io::traits::ClosedOrFailed;
use telepathy_audio::io::{AudioDataSink, AudioDataSource};
use tokio::select;
use tokio::sync::Notify;
#[cfg(all(feature = "native", not(feature = "flutter")))]
pub use tokio::task::JoinHandle;
#[cfg(not(target_family = "wasm"))]
use tokio::time::interval;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tracing::debug;
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::interval;

type Result<T> = std::result::Result<T, Error>;

/// An `AudioDataSink` backed by a `kanal` channel.
pub(crate) struct KanalSink {
    sender: kanal::Sender<PooledBuffer>,
}

impl KanalSink {
    pub fn new(sender: kanal::AsyncSender<PooledBuffer>) -> Self {
        Self {
            sender: sender.to_sync(),
        }
    }
}

impl AudioDataSink for KanalSink {
    fn send(&self, data: PooledBuffer) -> std::result::Result<(), ClosedOrFailed> {
        self.sender.send(data).map_err(|_| ClosedOrFailed::Closed)
    }
}

/// An `AudioDataSource` backed by a `kanal` channel.
pub(crate) struct KanalSource {
    receiver: kanal::Receiver<Bytes>,
}

impl KanalSource {
    pub(crate) fn new(receiver: kanal::Receiver<Bytes>) -> Self {
        Self { receiver }
    }
}

impl AudioDataSource for KanalSource {
    fn recv(&self) -> std::result::Result<Bytes, ClosedOrFailed> {
        self.receiver.recv().map_err(|_| ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> std::result::Result<Option<Bytes>, ClosedOrFailed> {
        self.receiver.try_recv().map_err(|_| ClosedOrFailed::Closed)
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

/// Writes a message to the stream
pub(crate) async fn write_message(
    transport: &mut FramedWrite<SendStream, LengthDelimitedCodec>,
    message: &ProtocolMessage,
) -> Result<()> {
    let buffer = message.write_to_vec()?;
    transport
        .send(Bytes::from(buffer))
        .await
        .map_err(|_| ErrorKind::TransportSend.into())
}

/// Reads a message from the stream
pub(crate) async fn read_message(
    transport: &mut FramedRead<RecvStream, LengthDelimitedCodec>,
) -> Result<ProtocolMessage> {
    if let Some(Ok(buffer)) = transport.next().await {
        let message = ProtocolMessage::read_from_buffer(&buffer[..])?;
        Ok(message)
    } else {
        Err(ErrorKind::TransportRecv.into())
    }
}

/// Collects statistics from throughout the application, processes them, and provides them to the frontend
pub(crate) async fn statistics_collector<C: CoreStatisticsCallback>(
    state: StatisticsCollectorState,
    callback: C,
    cancel: CancellationToken,
    efficient: bool,
    statistics_paused: Arc<AtomicBool>,
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
                // update overlay statistics
                LATENCY.store(latency, Relaxed);
                LOSS.store(loss, Relaxed);
                // only post statistics if not paused
                if !statistics_paused.load(Relaxed) {
                    callback.post(Statistics {
                        input_level: level_from_window(state.input_rms.swap(0_f32, Relaxed), &mut input_max),
                        output_level: level_from_window(state.output_rms.swap(0_f32, Relaxed), &mut output_max),
                        latency,
                        upload_bandwidth: state.upload_bandwidth.load(Relaxed),
                        download_bandwidth: state.download_bandwidth.load(Relaxed),
                        loss,
                    }).await;
                }
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
    input_receiver: AsyncReceiver<PooledBuffer>,
    output_sender: kanal::Sender<Bytes>,
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
                    if output_sender.try_send(message.clone_inner().freeze()).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            },
        }
    }
}

pub(crate) fn spawn_task<F, T>(future: F) -> JoinHandle<T>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(feature = "flutter")]
    {
        flutter_rust_bridge::spawn(future)
    }

    #[cfg(all(feature = "native", not(feature = "flutter")))]
    {
        tokio::spawn(future)
    }
}

#[cfg(target_os = "ios")]
pub(crate) fn configure_audio_session() {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use objc2_foundation::ns_string;

    unsafe {
        let av_audio_session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];

        // set category to `AVAudioSessionCategoryPlayAndRecord`
        let category = ns_string!("AVAudioSessionCategoryPlayAndRecord");
        let mode = ns_string!("AVAudioSessionModeDefault");
        let error: *mut AnyObject = std::ptr::null_mut();

        let success: Bool = msg_send![av_audio_session, setCategory: category,
            mode: mode,
            options: 0_u64,
            error: &error];

        if success == Bool::NO {
            tracing::error!("Failed to set AVAudioSession category.");
        }

        let override_output: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        let _: Bool = msg_send![override_output, overrideOutputAudioPort: 1_u64, error: &error];

        // Activate the audio session
        let success: Bool = msg_send![av_audio_session, setActive: Bool::YES, error: &error];

        if success == Bool::NO {
            tracing::error!("Failed to activate AVAudioSession.");
        }
    }
}

#[cfg(target_os = "ios")]
pub(crate) fn deactivate_audio_session() {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};

    unsafe {
        let av_audio_session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];

        let error: *mut AnyObject = std::ptr::null_mut();
        let success: Bool = msg_send![av_audio_session, setActive: Bool::NO, error: &error];

        if success == Bool::NO {
            tracing::error!("Failed to deactivate AVAudioSession.");
        }
    }
}
