use crate::types::DartError;
use flutter_rust_bridge::frb;
use iroh::{PublicKey, SecretKey};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;
#[cfg(not(target_family = "wasm"))]
use tokio::process::Command;

#[frb(sync)]
pub fn generate_keys() -> (String, Vec<u8>) {
    let pair = SecretKey::generate();

    let peer_id = pair.public();

    (peer_id.to_string(), pair.to_bytes().to_vec())
}

#[frb(sync)]
pub fn room_hash(peers: Vec<String>) -> Result<String, DartError> {
    let mut acc = 0;

    for peer in peers {
        if let Ok(peer) = PublicKey::from_str(&peer) {
            let mut hasher = DefaultHasher::new();
            peer.hash(&mut hasher);
            acc ^= hasher.finish();
        } else {
            return Err(DartError::from(peer));
        }
    }

    Ok(format!("room-{}", acc))
}

#[frb(sync)]
pub fn validate_peer_id(peer_id: String) -> bool {
    PublicKey::from_str(&peer_id).is_ok()
}

pub async fn screenshare_available() -> bool {
    #[cfg(target_family = "wasm")]
    return false;

    #[cfg(not(target_family = "wasm"))]
    if let Ok(status) = Command::new("ffmpeg").status().await {
        // ffmpeg with no arguments returns status 1
        status.code() == Some(1)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ignore]
    #[tokio::test]
    async fn screenshare_available_returns_true() {
        let ffmpeg_available = screenshare_available().await;
        assert!(ffmpeg_available);
    }
}
