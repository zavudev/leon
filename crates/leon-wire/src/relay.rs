//! The rendezvous protocol between a peer and a relay server.
//!
//! This is a specification of what a relay must do; the relay service is
//! operated by Zavu and its server is not part of this repository. The relay
//! is **untrusted by design**: it pairs two peers and forwards opaque bytes.
//! Everything it forwards after the rendezvous is Noise ciphertext, so the
//! relay can neither read nor alter it undetected; the only thing it can do
//! to a connection is drop it.
//!
//! # Transport
//!
//! WebSocket (`wss://` in production). Every WebSocket message is binary and
//! starts with a tag byte:
//!
//! * [`TAG_CONTROL`] followed by a [`postcard`] control message;
//! * [`TAG_DATA`] followed, on the host's connection only, by a `u32`
//!   big-endian channel number, then the opaque payload. On a client's
//!   connection the payload follows the tag directly.
//!
//! # Endpoints
//!
//! * [`PATH_HOST`]: a host connects here. The relay sends
//!   [`RelayToHost::Challenge`] with a random nonce; the host answers
//!   [`HostToRelay::Register`] with its Ed25519 verifying key and a signature
//!   over [`registration_message`]. The relay derives the [`HostId`] from the
//!   key ([`HostId::from_verifying_key`]), verifies the signature, and answers
//!   [`RelayToHost::Registered`]. Nobody can take an id without its key. A
//!   second registration of the same id replaces the first.
//! * [`join_path`]: a client connects to `/v1/join/<HostId>` and sends
//!   [`ClientToRelay::Join`]; the relay tells the host
//!   [`RelayToHost::ChannelOpened`] with a fresh channel number and the client
//!   [`RelayToClient::Joined`]. Data then flows both ways until either side
//!   leaves ([`RelayToHost::ChannelClosed`] / [`RelayToClient::PeerLeft`]).
//! * [`pair_path`]: the same for a client that only has a pairing code: the
//!   host first announces the code's public room ([`HostToRelay::OpenPairing`]);
//!   a client joining `/v1/pair/<ROOM>` is bridged to that host with
//!   `pairing: true`. The room is not secret; the rest of the code never
//!   leaves the two computers.
//!
//! # What a relay may enforce
//!
//! Message size, rates and byte budgets, clients per host, idle time,
//! pairing attempts per host per window ([`RelayLimits`]), and, later, an
//! account or plan carried as the opaque `token` of the registration and join
//! messages. Hosts and clients treat a relay that refuses as unavailable.

use serde::{Deserialize, Serialize};

use crate::id::{unbase32, HostId};

/// The tag of a control message.
pub const TAG_CONTROL: u8 = 1;
/// The tag of a data message.
pub const TAG_DATA: u8 = 2;
/// Where hosts connect.
pub const PATH_HOST: &str = "/v1/host";
/// How many symbols a pairing room has.
pub const ROOM_LEN: usize = 4;

/// The path a client uses to reach `host`.
pub fn join_path(host: &HostId) -> String {
    format!("/v1/join/{host}")
}

/// The path a client uses to reach the host that announced `room`.
pub fn pair_path(room: &str) -> String {
    format!("/v1/pair/{room}")
}

/// Whether `room` is a well-formed pairing room.
pub fn valid_room(room: &str) -> bool {
    room.len() == ROOM_LEN
        && unbase32(room).is_some()
        && room.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The bytes a host signs to prove it owns its id: a domain tag, the relay's
/// nonce and the key being registered.
pub fn registration_message(nonce: &[u8; 32], verifying_key: &[u8; 32]) -> Vec<u8> {
    let mut message = Vec::with_capacity(22 + 64);
    message.extend_from_slice(b"leon-relay-register-v1");
    message.extend_from_slice(nonce);
    message.extend_from_slice(verifying_key);
    message
}

/// What a relay tells its peers about its limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayLimits {
    /// The largest WebSocket message it forwards.
    pub max_message_bytes: u32,
    /// How many clients one host may have at once.
    pub max_clients_per_host: u32,
    /// Seconds of silence after which it closes a connection.
    pub idle_timeout_secs: u32,
}

/// Why a relay refused or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayErrorCode {
    /// Malformed or unexpected message.
    BadRequest,
    /// The signature or token was not accepted.
    Unauthorized,
    /// No such host is connected.
    HostNotFound,
    /// The host has too many clients.
    HostBusy,
    /// The pairing room is taken.
    RoomInUse,
    /// A message over the limit.
    TooLarge,
    /// Too many messages or bytes, or too many pairing attempts.
    RateLimited,
    /// Closed for being idle.
    Idle,
    /// The relay failed.
    Internal,
}

/// A refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayError {
    /// What kind.
    pub code: RelayErrorCode,
    /// A sentence for a person.
    pub message: String,
}

/// Host to relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostToRelay {
    /// Proves ownership of the id derived from `verifying_key`.
    Register {
        /// The Ed25519 verifying key.
        verifying_key: [u8; 32],
        /// 64-byte Ed25519 signature over [`registration_message`].
        signature: Vec<u8>,
        /// Opaque, optional: an account or plan token a relay may require.
        token: Option<Vec<u8>>,
    },
    /// Announces a pairing room.
    OpenPairing {
        /// The room.
        room: String,
    },
    /// Withdraws a pairing room.
    ClosePairing {
        /// The room.
        room: String,
    },
    /// Hangs up one client.
    CloseChannel {
        /// The channel.
        channel: u32,
    },
}

/// Relay to host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayToHost {
    /// The nonce to sign.
    Challenge {
        /// Random, single use.
        nonce: [u8; 32],
        /// The relay's limits.
        limits: RelayLimits,
    },
    /// Registration accepted.
    Registered {
        /// The id the relay derived.
        host_id: HostId,
    },
    /// A pairing room was announced.
    PairingOpened {
        /// The room.
        room: String,
    },
    /// A client arrived.
    ChannelOpened {
        /// Its channel number.
        channel: u32,
        /// Whether it came through a pairing room.
        pairing: bool,
    },
    /// A client left.
    ChannelClosed {
        /// Its channel number.
        channel: u32,
    },
    /// A refusal.
    Error(RelayError),
}

/// Client to relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientToRelay {
    /// The first message of a client.
    Join {
        /// Opaque, optional: an account or plan token.
        token: Option<Vec<u8>>,
    },
}

/// Relay to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelayToClient {
    /// The host is there; data may flow.
    Joined {
        /// The relay's limits.
        limits: RelayLimits,
    },
    /// The host left.
    PeerLeft,
    /// A refusal.
    Error(RelayError),
}

/// A decoded WebSocket message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound<T> {
    /// A control message.
    Control(T),
    /// Opaque data; `channel` is 0 on a client's connection.
    Data {
        /// The channel (host connections only).
        channel: u32,
        /// The payload.
        payload: Vec<u8>,
    },
}

/// Why a WebSocket message was not understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadFrame;

/// Encodes a control message.
pub fn encode_control<T: Serialize>(message: &T) -> Vec<u8> {
    let mut out = vec![TAG_CONTROL];
    out.extend(postcard::to_allocvec(message).unwrap_or_default());
    out
}

/// Encodes data on a host's connection.
pub fn encode_host_data(channel: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(TAG_DATA);
    out.extend_from_slice(&channel.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Encodes data on a client's connection.
pub fn encode_client_data(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len());
    out.push(TAG_DATA);
    out.extend_from_slice(payload);
    out
}

/// Decodes a message received on a host's connection.
pub fn decode_host_side<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<Inbound<T>, BadFrame> {
    match bytes.split_first() {
        Some((&TAG_CONTROL, rest)) => postcard::from_bytes(rest)
            .map(Inbound::Control)
            .map_err(|_| BadFrame),
        Some((&TAG_DATA, rest)) if rest.len() >= 4 => Ok(Inbound::Data {
            channel: u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]),
            payload: rest[4..].to_vec(),
        }),
        _ => Err(BadFrame),
    }
}

/// Decodes a message received on a client's connection.
pub fn decode_client_side<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<Inbound<T>, BadFrame> {
    match bytes.split_first() {
        Some((&TAG_CONTROL, rest)) => postcard::from_bytes(rest)
            .map(Inbound::Control)
            .map_err(|_| BadFrame),
        Some((&TAG_DATA, rest)) => Ok(Inbound::Data {
            channel: 0,
            payload: rest.to_vec(),
        }),
        _ => Err(BadFrame),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_round_trip_on_both_sides() {
        let message = HostToRelay::Register {
            verifying_key: [3; 32],
            signature: vec![9; 64],
            token: Some(vec![1]),
        };
        let bytes = encode_control(&message);
        assert_eq!(
            decode_host_side::<HostToRelay>(&bytes),
            Ok(Inbound::Control(message))
        );
        let bytes = encode_control(&RelayToClient::PeerLeft);
        assert_eq!(
            decode_client_side::<RelayToClient>(&bytes),
            Ok(Inbound::Control(RelayToClient::PeerLeft))
        );
    }

    #[test]
    fn data_carries_its_channel_only_on_the_host_side() {
        let bytes = encode_host_data(7, b"abc");
        assert_eq!(
            decode_host_side::<RelayToHost>(&bytes),
            Ok(Inbound::Data {
                channel: 7,
                payload: b"abc".to_vec()
            })
        );
        let bytes = encode_client_data(b"abc");
        assert_eq!(
            decode_client_side::<RelayToClient>(&bytes),
            Ok(Inbound::Data {
                channel: 0,
                payload: b"abc".to_vec()
            })
        );
    }

    #[test]
    fn garbage_and_truncations_are_refused_not_panics() {
        for bad in [
            &[][..],
            &[0],
            &[9, 1, 2],
            &[TAG_DATA, 1, 2],
            &[TAG_CONTROL, 0xFF, 0xFF],
        ] {
            assert!(decode_host_side::<RelayToHost>(bad).is_err(), "{bad:?}");
        }
        assert!(decode_client_side::<RelayToClient>(&[]).is_err());
    }

    #[test]
    fn the_signed_registration_message_binds_the_nonce_and_the_key() {
        let a = registration_message(&[1; 32], &[2; 32]);
        assert_ne!(a, registration_message(&[9; 32], &[2; 32]));
        assert_ne!(a, registration_message(&[1; 32], &[9; 32]));
        assert!(a.starts_with(b"leon-relay-register-v1"));
    }

    #[test]
    fn rooms_have_four_unambiguous_symbols() {
        assert!(valid_room("AB23"));
        assert!(!valid_room("AB2"));
        assert!(!valid_room("AB2O"));
        assert!(!valid_room("AB2-"));
    }

    #[test]
    fn paths_name_the_target() {
        let id = HostId::from_bytes([5; 16]);
        assert_eq!(join_path(&id), format!("/v1/join/{id}"));
        assert_eq!(pair_path("AB23"), "/v1/pair/AB23");
    }
}
