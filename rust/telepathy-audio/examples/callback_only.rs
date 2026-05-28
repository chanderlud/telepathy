use telepathy_audio::devices::CpalAudioHost;
use telepathy_audio::io::AudioInputBuilder;

fn main() -> Result<(), telepathy_audio::Error> {
    let host = CpalAudioHost::new();

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
