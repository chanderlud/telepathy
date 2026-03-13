use bytes::Bytes;
use crossbeam::channel;
use telepathy_audio::devices::AudioHost;
use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::io::traits::ClosedOrFailed;
use telepathy_audio::io::{AudioDataSink, AudioDataSource, AudioInputBuilder, AudioOutputBuilder};

#[derive(Clone)]
struct CrossbeamSink(channel::Sender<PooledBuffer>);

impl AudioDataSink for CrossbeamSink {
    fn send(&self, data: PooledBuffer) -> Result<(), ClosedOrFailed> {
        self.0.send(data).map_err(|_| ClosedOrFailed::Closed)
    }
}

struct CrossbeamSource(channel::Receiver<Bytes>);

impl AudioDataSource for CrossbeamSource {
    fn recv(&self) -> Result<Bytes, ClosedOrFailed> {
        self.0.recv().map_err(|_| ClosedOrFailed::Closed)
    }

    fn try_recv(&self) -> Result<Option<Bytes>, ClosedOrFailed> {
        match self.0.try_recv() {
            Ok(b) => Ok(Some(b)),
            Err(channel::TryRecvError::Empty) => Ok(None),
            Err(channel::TryRecvError::Disconnected) => Err(ClosedOrFailed::Closed),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = AudioHost::new();

    // Input: deliver processed frames into a crossbeam channel.
    let (in_tx, in_rx) = channel::unbounded::<PooledBuffer>();
    let _input = AudioInputBuilder::new()
        .sink(CrossbeamSink(in_tx))
        .build(&host)?;

    std::thread::spawn(move || {
        while let Ok(buf) = in_rx.recv() {
            println!("input frame: {} bytes", buf.as_ref().len());
        }
    });

    // Output: receive frames from a crossbeam channel.
    let (out_tx, out_rx) = channel::unbounded::<Bytes>();
    let _output = AudioOutputBuilder::new()
        .sample_rate(48_000)
        .source(CrossbeamSource(out_rx))
        .build(&host)?;

    // In a real app, feed `Bytes` frames (raw i16 bytes or codec frames, matching config).
    let _ = out_tx;

    std::thread::park();
    Ok(())
}
