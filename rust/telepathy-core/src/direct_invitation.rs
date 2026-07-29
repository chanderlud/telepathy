use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::{PublicKey, TransportAddr};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

const PREFIX: &str = "tp1:";
const VERSION: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DirectInvitationError {
    MissingInvitation,
    InvalidPrefix,
    InvalidBase64,
    InvalidPayload,
    UnsupportedVersion,
    EmptyAddresses,
    RelayAddresses,
    PeerMismatch,
    NonCanonical,
}

impl Display for DirectInvitationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingInvitation => "A direct invitation is required",
            Self::InvalidPrefix => "Direct invitation must start with tp1:",
            Self::InvalidBase64
            | Self::InvalidPayload
            | Self::EmptyAddresses
            | Self::RelayAddresses => "Direct invitation is malformed",
            Self::UnsupportedVersion => "Direct invitation version is not supported",
            Self::PeerMismatch => "Direct invitation belongs to a different contact",
            Self::NonCanonical => "Direct invitation is not canonical",
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DirectInvitationPayload {
    version: u8,
    peer_id: String,
    addresses: Vec<TransportAddr>,
}

pub(crate) fn encode(peer_id: PublicKey, addresses: Vec<TransportAddr>) -> Option<String> {
    let addresses: Vec<_> = addresses
        .into_iter()
        .filter(|address| !address.is_relay())
        .collect();
    if addresses.is_empty() {
        return None;
    }

    let payload = DirectInvitationPayload {
        version: VERSION,
        peer_id: peer_id.to_string(),
        addresses,
    };
    let bytes = serde_json::to_vec(&payload).expect("direct invitation payload is serializable");
    Some(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub(crate) fn decode_for_peer(
    invitation: &str,
    expected_peer: PublicKey,
) -> Result<Vec<TransportAddr>, DirectInvitationError> {
    let encoded = invitation
        .strip_prefix(PREFIX)
        .ok_or(DirectInvitationError::InvalidPrefix)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| DirectInvitationError::InvalidBase64)?;
    let payload: DirectInvitationPayload =
        serde_json::from_slice(&bytes).map_err(|_| DirectInvitationError::InvalidPayload)?;

    if payload.version != VERSION {
        return Err(DirectInvitationError::UnsupportedVersion);
    }
    if payload.addresses.is_empty() {
        return Err(DirectInvitationError::EmptyAddresses);
    }
    if payload.addresses.iter().any(TransportAddr::is_relay) {
        return Err(DirectInvitationError::RelayAddresses);
    }
    let peer_id =
        PublicKey::from_str(&payload.peer_id).map_err(|_| DirectInvitationError::InvalidPayload)?;
    if peer_id != expected_peer {
        return Err(DirectInvitationError::PeerMismatch);
    }

    let canonical = encode(peer_id, payload.addresses.clone())
        .expect("validated direct invitation has addresses");
    if invitation != canonical {
        return Err(DirectInvitationError::NonCanonical);
    }

    Ok(payload.addresses)
}

pub(crate) fn canonicalize_for_peer(invitation: &str, expected_peer: PublicKey) -> Option<String> {
    if decode_for_peer(invitation, expected_peer).is_ok() {
        return Some(invitation.to_string());
    }

    let legacy_addresses: Vec<TransportAddr> = serde_json::from_str(invitation).ok()?;
    if legacy_addresses.iter().any(TransportAddr::is_relay) {
        return None;
    }
    encode(expected_peer, legacy_addresses)
}

#[cfg(test)]
mod tests {
    use super::{
        DirectInvitationError, DirectInvitationPayload, PREFIX, VERSION, canonicalize_for_peer,
        decode_for_peer, encode,
    };
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use iroh::{SecretKey, TransportAddr};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::str::FromStr;

    fn addresses() -> Vec<TransportAddr> {
        vec![TransportAddr::Ip(SocketAddr::from((
            Ipv4Addr::LOCALHOST,
            40142,
        )))]
    }

    fn relay_address() -> TransportAddr {
        TransportAddr::Relay(
            iroh::RelayUrl::from_str("https://relay.example/")
                .expect("test relay URL should parse"),
        )
    }

    fn token_for(payload: &DirectInvitationPayload) -> String {
        let bytes = serde_json::to_vec(payload).expect("test payload should serialize");
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
    }

    #[test]
    fn valid_invitation_round_trips_canonically() {
        let peer = SecretKey::generate().public();
        let invitation = encode(peer, addresses()).expect("nonempty addresses should encode");

        assert!(invitation.starts_with(PREFIX));
        assert!(!invitation.contains('='));
        assert_eq!(decode_for_peer(&invitation, peer), Ok(addresses()));
        assert_eq!(
            encode(peer, addresses()).as_deref(),
            Some(invitation.as_str())
        );
    }

    #[test]
    fn export_removes_relays_but_preserves_direct_addresses() {
        let peer = SecretKey::generate().public();
        let mut mixed = addresses();
        mixed.push(relay_address());

        let invitation = encode(peer, mixed).expect("direct address should remain exportable");

        assert_eq!(decode_for_peer(&invitation, peer), Ok(addresses()));
    }

    #[test]
    fn relay_only_export_has_no_invitation() {
        let peer = SecretKey::generate().public();

        assert_eq!(encode(peer, vec![relay_address()]), None);
    }

    #[test]
    fn malformed_invitations_are_rejected() {
        let peer = SecretKey::generate().public();

        assert_eq!(
            decode_for_peer("not-an-invitation", peer),
            Err(DirectInvitationError::InvalidPrefix)
        );
        assert_eq!(
            decode_for_peer("tp1:not+url/base64", peer),
            Err(DirectInvitationError::InvalidBase64)
        );
        let malformed_payload = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(b"not json"));
        assert_eq!(
            decode_for_peer(&malformed_payload, peer),
            Err(DirectInvitationError::InvalidPayload)
        );

        let padded = format!(
            "{}=",
            encode(peer, addresses()).expect("nonempty addresses should encode")
        );
        assert!(decode_for_peer(&padded, peer).is_err());
    }

    #[test]
    fn semantically_valid_noncanonical_payload_is_rejected() {
        let peer = SecretKey::generate().public();
        let canonical = encode(peer, addresses()).expect("nonempty addresses should encode");
        let encoded = canonical.strip_prefix(PREFIX).expect("encoder adds prefix");
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("encoder emits base64");
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).expect("encoder emits JSON");
        let reordered = serde_json::json!({
            "addresses": payload["addresses"],
            "peer_id": payload["peer_id"],
            "version": payload["version"],
        });
        let invitation = format!(
            "{PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&reordered).expect("test JSON serializes"))
        );

        assert_eq!(
            decode_for_peer(&invitation, peer),
            Err(DirectInvitationError::NonCanonical)
        );
    }

    #[test]
    fn unsupported_payload_version_is_rejected() {
        let peer = SecretKey::generate().public();
        let invitation = token_for(&DirectInvitationPayload {
            version: 2,
            peer_id: peer.to_string(),
            addresses: addresses(),
        });

        assert_eq!(
            decode_for_peer(&invitation, peer),
            Err(DirectInvitationError::UnsupportedVersion)
        );
    }

    #[test]
    fn empty_address_list_is_rejected() {
        let peer = SecretKey::generate().public();
        let invitation = token_for(&DirectInvitationPayload {
            version: 1,
            peer_id: peer.to_string(),
            addresses: Vec::new(),
        });

        assert_eq!(
            decode_for_peer(&invitation, peer),
            Err(DirectInvitationError::EmptyAddresses)
        );
        assert_eq!(encode(peer, Vec::new()), None);
    }

    #[test]
    fn relay_bearing_token_is_rejected_before_canonicality() {
        let peer = SecretKey::generate().public();
        let invitation = token_for(&DirectInvitationPayload {
            version: VERSION,
            peer_id: peer.to_string(),
            addresses: vec![relay_address()],
        });

        assert_eq!(
            decode_for_peer(&invitation, peer),
            Err(DirectInvitationError::RelayAddresses)
        );
    }

    #[test]
    fn contact_peer_mismatch_is_rejected() {
        let invited_peer = SecretKey::generate().public();
        let contact_peer = SecretKey::generate().public();
        let invitation =
            encode(invited_peer, addresses()).expect("nonempty addresses should encode");

        assert_eq!(
            decode_for_peer(&invitation, contact_peer),
            Err(DirectInvitationError::PeerMismatch)
        );
    }

    #[test]
    fn legacy_address_json_migrates_to_a_canonical_invitation() {
        let peer = SecretKey::generate().public();
        let legacy = serde_json::to_string(&addresses()).expect("legacy addresses serialize");

        let invitation =
            canonicalize_for_peer(&legacy, peer).expect("valid legacy addresses should migrate");

        assert!(invitation.starts_with(PREFIX));
        assert_eq!(decode_for_peer(&invitation, peer), Ok(addresses()));
    }

    #[test]
    fn legacy_relay_address_json_is_rejected() {
        let peer = SecretKey::generate().public();
        let legacy = serde_json::to_string(&vec![relay_address()]).expect("legacy serializes");

        assert_eq!(canonicalize_for_peer(&legacy, peer), None);
    }

    #[test]
    fn malformed_and_peer_mismatched_values_do_not_normalize() {
        let peer = SecretKey::generate().public();
        let other_peer = SecretKey::generate().public();

        assert_eq!(canonicalize_for_peer("not-json", peer), None);
        assert_eq!(
            canonicalize_for_peer(
                &encode(other_peer, addresses()).expect("test invitation encodes"),
                peer,
            ),
            None,
        );
    }
}
