//! The Noise session used for every connection after pairing.
//!
//! The pattern is `Noise_IK_25519_ChaChaPoly_BLAKE2s`: the client already knows
//! the host's static key (it was pinned during pairing), sends its own static
//! key encrypted in the first message, and the host can refuse an unknown or
//! revoked key *before* it answers. Both sides are authenticated by their
//! static keys; the ephemeral keys give forward secrecy.
//!
//! Each application message is one or more Noise transport messages. A Noise
//! message holds at most 65535 bytes, so larger payloads are fragmented; every
//! transport message starts (inside the encryption) with a flag byte: `MORE`
//! when more fragments follow and `REKEY` when the sender switches to a new
//! key right after this message. Rekeying happens after a volume or time
//! threshold ([`RekeyPolicy`]); because the pipe is ordered, the receiver
//! rekeys at the same point.
//!
//! Any decryption failure poisons the session: it is closed and refuses
//! everything after, because a tampered stream cannot be trusted again.

use std::time::{Duration, Instant};

use snow::{HandshakeState, TransportState};
use thiserror::Error;

use crate::identity::Identity;

/// The Noise pattern of ordinary connections.
pub const IK_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// The Noise pattern of pairing (the pre-shared key is the SPAKE2 output).
pub const PAIR_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";

const PROLOGUE_IK: &[u8] = b"leon/1/session";
const MAX_NOISE: usize = 65_535;
const TAG: usize = 16;
/// The most plaintext one transport message carries (one byte is the flags).
const MAX_CHUNK: usize = MAX_NOISE - TAG - 1;
/// The most a reassembled message may be: the largest frame plus slack.
const MAX_REASSEMBLED: usize = leon_wire::MAX_FRAME_LEN + 64;

const FLAG_MORE: u8 = 1;
const FLAG_REKEY: u8 = 2;

/// Why the session failed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    /// The handshake did not complete (wrong key, tampering, wrong pattern).
    #[error("the secure handshake failed")]
    Handshake,
    /// A message did not authenticate: it was altered, replayed or reordered.
    #[error("a message failed authentication")]
    Decrypt,
    /// A peer sent something the protocol does not allow.
    #[error("protocol violation")]
    Protocol,
    /// A message grew beyond the limit.
    #[error("message too large")]
    TooLarge,
    /// The session was closed by an earlier failure.
    #[error("the session is closed")]
    Closed,
}

/// When to switch to fresh keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyPolicy {
    /// After this many messages sent.
    pub messages: u64,
    /// After this many plaintext bytes sent.
    pub bytes: u64,
    /// After this long since the last rekey.
    pub interval: Duration,
}

impl Default for RekeyPolicy {
    fn default() -> Self {
        Self {
            messages: 1 << 20,
            bytes: 1 << 30,
            interval: Duration::from_secs(3600),
        }
    }
}

/// An established, authenticated, encrypted session.
pub struct Session {
    transport: TransportState,
    remote_static: [u8; 32],
    partial: Vec<u8>,
    policy: RekeyPolicy,
    sent_messages: u64,
    sent_bytes: u64,
    keyed_at: Instant,
    rekeys_sent: u64,
    rekeys_received: u64,
    dead: bool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field(
                "remote",
                &crate::identity::DeviceId::of(&self.remote_static),
            )
            .field("closed", &self.dead)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub(crate) fn from_transport(transport: TransportState, remote_static: [u8; 32]) -> Self {
        Self {
            transport,
            remote_static,
            partial: Vec::new(),
            policy: RekeyPolicy::default(),
            sent_messages: 0,
            sent_bytes: 0,
            keyed_at: Instant::now(),
            rekeys_sent: 0,
            rekeys_received: 0,
            dead: false,
        }
    }

    /// Replaces the rekey thresholds (tests use tiny ones).
    pub fn set_rekey_policy(&mut self, policy: RekeyPolicy) {
        self.policy = policy;
    }

    /// The peer's static public key, authenticated by the handshake.
    pub fn remote_static(&self) -> [u8; 32] {
        self.remote_static
    }

    /// How many times each direction has rekeyed: `(sent, received)`.
    pub fn rekeys(&self) -> (u64, u64) {
        (self.rekeys_sent, self.rekeys_received)
    }

    /// Whether a failure closed the session.
    pub fn is_closed(&self) -> bool {
        self.dead
    }

    fn rekey_due(&self) -> bool {
        self.sent_messages >= self.policy.messages
            || self.sent_bytes >= self.policy.bytes
            || self.keyed_at.elapsed() >= self.policy.interval
    }

    /// Encrypts one application message into the transport messages that carry
    /// it, in order.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<Vec<u8>>, SessionError> {
        if self.dead {
            return Err(SessionError::Closed);
        }
        if plaintext.len() > MAX_REASSEMBLED {
            return Err(SessionError::TooLarge);
        }
        let chunks: Vec<&[u8]> = if plaintext.is_empty() {
            vec![&[][..]]
        } else {
            plaintext.chunks(MAX_CHUNK).collect()
        };
        let last = chunks.len() - 1;
        let mut out = Vec::with_capacity(chunks.len());
        for (index, chunk) in chunks.iter().enumerate() {
            let rekey = self.rekey_due();
            let mut flags = if index < last { FLAG_MORE } else { 0 };
            if rekey {
                flags |= FLAG_REKEY;
            }
            let mut clear = Vec::with_capacity(1 + chunk.len());
            clear.push(flags);
            clear.extend_from_slice(chunk);
            let mut sealed = vec![0u8; clear.len() + TAG];
            let n = self
                .transport
                .write_message(&clear, &mut sealed)
                .map_err(|_| {
                    self.dead = true;
                    SessionError::Closed
                })?;
            sealed.truncate(n);
            self.sent_messages += 1;
            self.sent_bytes += chunk.len() as u64;
            if rekey {
                self.transport.rekey_outgoing();
                self.rekeys_sent += 1;
                self.sent_messages = 0;
                self.sent_bytes = 0;
                self.keyed_at = Instant::now();
            }
            out.push(sealed);
        }
        Ok(out)
    }

    /// Decrypts one transport message. Returns the whole application message
    /// when this was its last fragment.
    pub fn open(&mut self, sealed: &[u8]) -> Result<Option<Vec<u8>>, SessionError> {
        if self.dead {
            return Err(SessionError::Closed);
        }
        if sealed.len() < TAG + 1 || sealed.len() > MAX_NOISE {
            self.dead = true;
            return Err(SessionError::Protocol);
        }
        let mut clear = vec![0u8; sealed.len()];
        let n = match self.transport.read_message(sealed, &mut clear) {
            Ok(n) => n,
            Err(_) => {
                self.dead = true;
                return Err(SessionError::Decrypt);
            }
        };
        clear.truncate(n);
        let flags = clear[0];
        if flags & !(FLAG_MORE | FLAG_REKEY) != 0 {
            self.dead = true;
            return Err(SessionError::Protocol);
        }
        if flags & FLAG_REKEY != 0 {
            self.transport.rekey_incoming();
            self.rekeys_received += 1;
        }
        if self.partial.len() + clear.len() - 1 > MAX_REASSEMBLED {
            self.dead = true;
            return Err(SessionError::TooLarge);
        }
        self.partial.extend_from_slice(&clear[1..]);
        if flags & FLAG_MORE != 0 {
            return Ok(None);
        }
        Ok(Some(std::mem::take(&mut self.partial)))
    }
}

fn params(pattern: &str) -> snow::params::NoiseParams {
    pattern.parse().expect("the patterns are constants")
}

/// The client's half of the `IK` handshake.
pub struct InitiatorHandshake {
    state: HandshakeState,
    host_key: [u8; 32],
}

impl InitiatorHandshake {
    /// Starts a handshake to the host whose pinned static key is `host_key`.
    /// Returns the first message to send.
    pub fn start(
        identity: &Identity,
        host_key: &[u8; 32],
    ) -> Result<(Self, Vec<u8>), SessionError> {
        let mut state = snow::Builder::new(params(IK_PATTERN))
            .prologue(PROLOGUE_IK)
            .and_then(|b| b.local_private_key(identity.static_secret()))
            .and_then(|b| b.remote_public_key(host_key))
            .and_then(|b| b.build_initiator())
            .map_err(|_| SessionError::Handshake)?;
        let mut message = vec![0u8; 256];
        let n = state
            .write_message(&[], &mut message)
            .map_err(|_| SessionError::Handshake)?;
        message.truncate(n);
        Ok((
            Self {
                state,
                host_key: *host_key,
            },
            message,
        ))
    }

    /// Completes the handshake with the host's answer.
    pub fn finish(mut self, reply: &[u8]) -> Result<Session, SessionError> {
        let mut scratch = vec![0u8; 256];
        self.state
            .read_message(reply, &mut scratch)
            .map_err(|_| SessionError::Handshake)?;
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|_| SessionError::Handshake)?;
        Ok(Session::from_transport(transport, self.host_key))
    }
}

/// The host's half of the `IK` handshake.
pub struct ResponderHandshake {
    state: HandshakeState,
    remote: [u8; 32],
}

impl ResponderHandshake {
    /// Reads the client's first message and learns its static key, which the
    /// caller must check against its authorised devices before going on.
    pub fn read_first(identity: &Identity, first: &[u8]) -> Result<Self, SessionError> {
        let mut state = snow::Builder::new(params(IK_PATTERN))
            .prologue(PROLOGUE_IK)
            .and_then(|b| b.local_private_key(identity.static_secret()))
            .and_then(|b| b.build_responder())
            .map_err(|_| SessionError::Handshake)?;
        let mut scratch = vec![0u8; 256];
        state
            .read_message(first, &mut scratch)
            .map_err(|_| SessionError::Handshake)?;
        let remote: [u8; 32] = state
            .get_remote_static()
            .and_then(|key| key.try_into().ok())
            .ok_or(SessionError::Handshake)?;
        Ok(Self { state, remote })
    }

    /// The client's static key.
    pub fn remote_static(&self) -> [u8; 32] {
        self.remote
    }

    /// Answers and returns the session. Call only for an authorised client.
    pub fn finish(mut self) -> Result<(Vec<u8>, Session), SessionError> {
        let mut message = vec![0u8; 256];
        let n = self
            .state
            .write_message(&[], &mut message)
            .map_err(|_| SessionError::Handshake)?;
        message.truncate(n);
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|_| SessionError::Handshake)?;
        Ok((message, Session::from_transport(transport, self.remote)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn established() -> (Session, Session, Identity, Identity) {
        let client = Identity::generate();
        let host = Identity::generate();
        let (init, first) = InitiatorHandshake::start(&client, &host.static_public()).unwrap();
        let responder = ResponderHandshake::read_first(&host, &first).unwrap();
        assert_eq!(responder.remote_static(), client.static_public());
        let (reply, host_session) = responder.finish().unwrap();
        let client_session = init.finish(&reply).unwrap();
        (client_session, host_session, client, host)
    }

    fn deliver(from: &mut Session, to: &mut Session, plain: &[u8]) -> Vec<u8> {
        let mut result = None;
        for part in from.seal(plain).unwrap() {
            result = to.open(&part).unwrap();
        }
        result.expect("the last fragment completes the message")
    }

    #[test]
    fn a_handshake_with_the_right_key_gives_two_sessions_that_talk_both_ways() {
        let (mut client, mut host, c, h) = established();
        assert_eq!(client.remote_static(), h.static_public());
        assert_eq!(host.remote_static(), c.static_public());
        assert_eq!(deliver(&mut client, &mut host, b"ls"), b"ls");
        assert_eq!(deliver(&mut host, &mut client, b"files"), b"files");
        assert_eq!(deliver(&mut client, &mut host, b""), b"");
    }

    #[test]
    fn a_host_with_another_static_key_cannot_answer() {
        let client = Identity::generate();
        let expected = Identity::generate();
        let impostor = Identity::generate();
        let (init, first) = InitiatorHandshake::start(&client, &expected.static_public()).unwrap();
        // The impostor cannot even read the first message: it is encrypted to
        // the key the client pinned.
        assert!(ResponderHandshake::read_first(&impostor, &first).is_err());
        drop(init);
    }

    #[test]
    fn a_tampered_first_message_is_rejected() {
        let client = Identity::generate();
        let host = Identity::generate();
        let (_init, mut first) = InitiatorHandshake::start(&client, &host.static_public()).unwrap();
        let last = first.len() - 1;
        first[last] ^= 1;
        assert!(ResponderHandshake::read_first(&host, &first).is_err());
    }

    #[test]
    fn a_tampered_reply_is_rejected() {
        let client = Identity::generate();
        let host = Identity::generate();
        let (init, first) = InitiatorHandshake::start(&client, &host.static_public()).unwrap();
        let (mut reply, _) = ResponderHandshake::read_first(&host, &first)
            .unwrap()
            .finish()
            .unwrap();
        reply[10] ^= 0x40;
        assert!(init.finish(&reply).is_err());
    }

    #[test]
    fn a_large_message_is_fragmented_and_reassembled() {
        let (mut client, mut host, ..) = established();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let parts = client.seal(&big).unwrap();
        assert!(parts.len() >= 5);
        assert!(parts.iter().all(|p| p.len() <= MAX_NOISE));
        let mut got = None;
        for (i, part) in parts.iter().enumerate() {
            let r = host.open(part).unwrap();
            assert_eq!(r.is_some(), i == parts.len() - 1);
            got = r;
        }
        assert_eq!(got.unwrap(), big);
    }

    #[test]
    fn a_flipped_bit_closes_the_session_for_good() {
        let (mut client, mut host, ..) = established();
        let mut parts = client.seal(b"secret command").unwrap();
        parts[0][3] ^= 1;
        assert_eq!(host.open(&parts[0]), Err(SessionError::Decrypt));
        assert!(host.is_closed());
        let again = client.seal(b"next").unwrap();
        assert_eq!(host.open(&again[0]), Err(SessionError::Closed));
    }

    #[test]
    fn a_replayed_or_reordered_message_is_rejected() {
        let (mut client, mut host, ..) = established();
        let first = client.seal(b"one").unwrap().remove(0);
        let second = client.seal(b"two").unwrap().remove(0);
        // Out of order: the nonce does not match.
        assert!(host.open(&second).is_err());
        let (mut client, mut host, ..) = established();
        let first_again = client.seal(b"one").unwrap().remove(0);
        host.open(&first_again).unwrap();
        assert!(host.open(&first_again).is_err(), "a replay fails");
        drop(first);
    }

    #[test]
    fn rekeying_happens_after_the_threshold_and_both_sides_keep_talking() {
        let (mut client, mut host, ..) = established();
        client.set_rekey_policy(RekeyPolicy {
            messages: 3,
            bytes: u64::MAX,
            interval: Duration::from_secs(3600),
        });
        for n in 0..20u32 {
            let text = format!("message {n}").into_bytes();
            assert_eq!(deliver(&mut client, &mut host, &text), text);
            assert_eq!(deliver(&mut host, &mut client, &text), text);
        }
        let (sent, _) = client.rekeys();
        assert!(sent >= 5, "{sent}");
        assert_eq!(host.rekeys().1, sent);
    }

    #[test]
    fn a_time_threshold_also_triggers_a_rekey() {
        let (mut client, mut host, ..) = established();
        client.set_rekey_policy(RekeyPolicy {
            messages: u64::MAX,
            bytes: u64::MAX,
            interval: Duration::ZERO,
        });
        assert_eq!(deliver(&mut client, &mut host, b"x"), b"x");
        assert_eq!(client.rekeys().0, 1);
        assert_eq!(deliver(&mut client, &mut host, b"y"), b"y");
    }

    #[test]
    fn an_unknown_flag_bit_is_a_protocol_violation() {
        // A peer sealing a hostile flag byte through the same keys.
        let (mut client, mut host, ..) = established();
        let mut clear = vec![0x80u8, 1, 2];
        let mut sealed = vec![0u8; clear.len() + TAG];
        let n = client.transport.write_message(&clear, &mut sealed).unwrap();
        sealed.truncate(n);
        clear.clear();
        assert_eq!(host.open(&sealed), Err(SessionError::Protocol));
    }

    #[test]
    fn the_session_debug_output_has_no_key_material() {
        let (client, ..) = established();
        let text = format!("{client:?}");
        assert!(text.contains("Session") && text.contains("DeviceId"));
    }
}
