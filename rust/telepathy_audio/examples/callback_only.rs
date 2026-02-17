use telepathy_audio::{AudioHost, AudioInputBuilder};

fn main() -> Result<(), telepathy_audio::Error> {
    let host = AudioHost::new();

    let _input = AudioInputBuilder::new()
        .volume(1.0)
        .callback(|data| {
            // Process/transmit encoded or raw audio, depending on builder config.
            println!("received {} bytes", data.as_ref().len());
        })
        .build(&host)?;

    // Keep the program alive while audio runs.
    std::thread::park();
    Ok(())
}
