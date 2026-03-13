use bytes::Bytes;
use std::sync::mpsc;
use telepathy_audio::adapters::{MpscSink, MpscSource};
use telepathy_audio::devices::AudioHost;
use telepathy_audio::internal::buffer_pool::PooledBuffer;
use telepathy_audio::io::{AudioInputBuilder, AudioOutputBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = AudioHost::new();

    // std::sync::mpsc is a simple, dependency-free option for native apps.
    // For higher throughput or advanced channel features, consider crossbeam/flume/etc.

    // Input: deliver processed frames into an mpsc channel via the built-in adapter.
    let (in_tx, in_rx) = mpsc::channel::<PooledBuffer>();
    let _input = AudioInputBuilder::new()
        .sink(MpscSink::new(in_tx))
        .build(&host)?;

    std::thread::spawn(move || {
        while let Ok(buf) = in_rx.recv() {
            println!("input frame: {} bytes", buf.as_ref().len());
        }
    });

    // Output: receive frames from an mpsc channel via the built-in adapter.
    let (out_tx, out_rx) = mpsc::channel::<Bytes>();
    let _output = AudioOutputBuilder::new()
        .sample_rate(48_000)
        .source(MpscSource::new(out_rx))
        .build(&host)?;

    // In a real app, feed `Bytes` frames (raw i16 bytes or codec frames, matching config).
    let _ = out_tx;

    std::thread::park();
    Ok(())
}
