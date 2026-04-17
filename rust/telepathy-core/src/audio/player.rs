//! Flutter wrapper for the telepathy-audio player.
//!
//! This module provides Flutter-specific bindings for the audio player,
//! wrapping the framework-agnostic `telepathy-audio::AudioPlayer` with
//! Flutter Rust Bridge attributes for Dart interop.

use crate::error::DartError;
#[cfg(not(target_family = "wasm"))]
use std::path::Path;
use std::sync::Arc;
use telepathy_audio::Host;
use telepathy_audio::player::{AudioPlayer, SoundHandle, wav_to_sea};
#[cfg(not(target_family = "wasm"))]
use tokio::fs::File;
#[cfg(not(target_family = "wasm"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Flutter-compatible sound player wrapping the library's AudioPlayer.
///
/// This struct provides Flutter Rust Bridge attributes for seamless
/// Dart integration while delegating all audio functionality to
/// the `telepathy-audio` library.
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct SoundPlayer(AudioPlayer);

impl SoundPlayer {
    /// Creates a new sound player with the specified output volume.
    ///
    /// # Arguments
    ///
    /// * `output_volume` - Output volume in decibels. 0 dB is unity gain,
    ///   negative values attenuate, positive values amplify.
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn new(output_volume: f32) -> SoundPlayer {
        SoundPlayer(AudioPlayer::new(output_volume))
    }

    /// Returns a reference to the audio host.
    ///
    /// This can be used to enumerate devices or access other host functionality.
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn host(&self) -> Arc<Host> {
        self.0.host()
    }

    /// Plays audio from the provided bytes.
    ///
    /// Supports both WAV files (with standard 44-byte header) and SEA codec files.
    /// The format is auto-detected based on header validation.
    ///
    /// # Arguments
    ///
    /// * `bytes` - The audio file bytes (WAV or SEA format).
    ///
    /// # Returns
    ///
    /// A `FlutterSoundHandle` that can be used to cancel playback.
    ///
    /// # Errors
    ///
    /// Returns `DartError` if:
    /// - The file is too short (< 14 bytes)
    /// - No output device is available
    /// - Stream configuration cannot be obtained
    /// - Stream creation fails
    pub async fn play(&self, bytes: Vec<u8>) -> Result<FlutterSoundHandle, DartError> {
        let handle = self
            .0
            .play(bytes)
            .await
            .map_err(|e| DartError::from(e.to_string()))?;
        Ok(FlutterSoundHandle(handle))
    }

    /// Updates the output volume.
    ///
    /// # Arguments
    ///
    /// * `volume` - New volume in decibels.
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn update_output_volume(&self, volume: f32) {
        self.0.set_volume(volume);
    }

    /// Sets the output device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device ID string to use, or `None` for the default device.
    pub async fn update_output_device(&self, device_id: Option<String>) {
        let parsed_id = device_id.and_then(|id| id.parse().ok());
        self.0.set_output_device(parsed_id).await;
    }
}

/// Flutter-compatible handle for controlling active sound playback.
///
/// This handle wraps the library's `SoundHandle` and provides Flutter
/// Rust Bridge attributes for Dart interop.
#[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(opaque))]
pub struct FlutterSoundHandle(SoundHandle);

impl FlutterSoundHandle {
    /// Cancels the sound playback.
    ///
    /// This triggers a graceful fade-out to prevent audio pops/clicks.
    #[cfg_attr(feature = "flutter", flutter_rust_bridge::frb(sync))]
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// Loads a ringtone from a WAV file and converts it to SEA format.
///
/// This function reads a WAV file, encodes it to the SEA codec format,
/// and saves it as "ringtone.sea" for efficient playback.
///
/// # Arguments
///
/// * `path` - Path to the WAV file to convert.
///
/// # Platform Support
///
/// - **Native**: Performs the file I/O and encoding.
/// - **WASM**: No-op (returns Ok immediately).
#[cfg(not(target_family = "wasm"))]
pub async fn load_ringtone(path: String) -> Result<(), DartError> {
    let path = Path::new(&path);

    let mut input_file = File::open(path).await.map_err(|error| error.to_string())?;
    let mut output_file = File::create("ringtone.sea")
        .await
        .map_err(|error| error.to_string())?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("wav") => {
            let mut wav_bytes = Vec::new();
            input_file
                .read_to_end(&mut wav_bytes)
                .await
                .map_err(|error| error.to_string())?;
            let sea_bytes = wav_to_sea(wav_bytes, 5_f32)
                .await
                .map_err(|error| error.to_string())?;
            output_file
                .write_all(&sea_bytes)
                .await
                .map_err(|error| error.to_string())?;
        }
        _ => return Err("Unsupported file type".to_string().into()),
    }

    Ok(())
}

/// WASM stub for load_ringtone (no-op on web platform).
#[cfg(target_family = "wasm")]
pub async fn load_ringtone(_path: String) -> Result<(), DartError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::sleep;

    #[ignore]
    #[tokio::test]
    async fn test_player() {
        let mut sea_bytes = Vec::new();
        let mut sea_file = File::open("../../assets/sounds/mute.sea").await.unwrap();
        sea_file.read_to_end(&mut sea_bytes).await.unwrap();

        let player = SoundPlayer::new(0.1);
        let handle = player.play(sea_bytes).await.unwrap();
        handle.cancel();

        sleep(std::time::Duration::from_secs(2)).await;
        sleep(std::time::Duration::from_secs(1)).await;
    }

    #[ignore]
    #[tokio::test]
    async fn test_wav_to_sea() {
        let wav_files = vec![
            "../../assets/wav-sounds/call_ended.wav",
            "../../assets/wav-sounds/connected.wav",
            "../../assets/wav-sounds/deafen.wav",
            "../../assets/wav-sounds/disconnected.wav",
            "../../assets/wav-sounds/incoming.wav",
            "../../assets/wav-sounds/mute.wav",
            "../../assets/wav-sounds/outgoing.wav",
            "../../assets/wav-sounds/reconnected.wav",
            "../../assets/wav-sounds/undeafen.wav",
            "../../assets/wav-sounds/unmute.wav",
        ];

        for wav_file_str in wav_files {
            let mut wav_bytes = Vec::new();
            let mut wav_file = File::open(wav_file_str).await.unwrap();
            wav_file.read_to_end(&mut wav_bytes).await.unwrap();

            let now = Instant::now();
            let len = wav_bytes.len();
            let other_data = wav_to_sea(wav_bytes, 5.0).await.unwrap();
            println!("wav to sea took {:?}", now.elapsed());
            println!("{}%", other_data.len() as f32 / len as f32);

            let mut output_file = File::create(wav_file_str.replace(".wav", ".sea"))
                .await
                .unwrap();
            output_file.write_all(&other_data).await.unwrap();
        }
    }
}
