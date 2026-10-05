//! Pairing two computers with a short one-time code.
//!
//! # Message flow
//!
//! ```text
//! client                                                      host
//!   | -- 1  SPAKE2 message A  ---------------------------------> |
//!   | <- 2  SPAKE2 message B  ---------------------------------- |   both now hold K
//!   | -- 3  Noise XX msg 1:  e ---------------------------------> |   (equal only if the
//!   | <- 4  Noise XX msg 2:  e, ee, s, es ---------------------- |    codes were equal)
//!   | -- 5  Noise XX msg 3:  s, se, psk(K) ---------------------> |   host checks K here
//!   | == 6  transport: Hello{device name} ======================> |
//!   |                         (host asks its owner, if it can)   |
//!   | <= 7  transport: Welcome{host name, verifying key} ======= |
//! ```
//!
//! * SPAKE2 (`spake2` crate, Ed25519 group) turns the six secret symbols of the
//!   code into a 32-byte key `K`. An observer, the relay included, learns
//!   nothing that lets it test guesses offline; an active attacker gets one
//!   online guess per attempt.
//! * `K` is the pre-shared key of a Noise `XXpsk3` handshake, which transmits
//!   and authenticates both static keys. A wrong code makes step 5 fail on the
//!   host, which counts the failure.
//! * Steps 6 and 7 travel under keys that depend on `K`, so a client trusts
//!   the host (and the host the client) only if the other knew the code.
//!   The client checks that the host's id is the hash of the verifying key it
//!   announces.
//!
//! The host's [`PairingOffer`] lives ten minutes, serves one success and burns
//! after five failed attempts.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use snow::HandshakeState;
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::channel::{LinkError, SecureChannel};
use crate::code::PairingCode;
use crate::identity::{DeviceId, Identity};
use crate::pipe::Pipe;
use crate::session::{Session, PAIR_PATTERN};
use leon_wire::HostId;

/// How long a code lives.
pub const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// How many failed attempts burn a code.
pub const MAX_FAILED_ATTEMPTS: u32 = 5;
/// How long one step of pairing may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// Marks a plain refusal sent instead of the SPAKE2 answer.
const REFUSAL_TAG: u8 = 0xE0;

const SPAKE_CLIENT: &[u8] = b"leon-pair-client";
const SPAKE_HOST: &[u8] = b"leon-pair-host";

/// Why pairing failed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PairError {
    /// The code is older than ten minutes.
    #[error("the code has expired")]
    Expired,
    /// Too many wrong attempts: the code is burned.
    #[error("the code was burned by too many failed attempts")]
    Burned,
    /// The code was already used.
    #[error("the code was already used")]
    AlreadyUsed,
    /// The host did not accept the code (wrong, or the host went away).
    #[error("the code was not accepted")]
    NotAccepted,
    /// The host's owner said no.
    #[error("the owner of the other computer declined")]
    Denied,
    /// Nothing arrived in time.
    #[error("the other computer did not answer in time")]
    Timeout,
    /// The connection ended.
    #[error("the connection closed")]
    Closed,
    /// The peer broke the protocol.
    #[error("the other side did not follow the protocol")]
    Protocol,
    /// Another failure in the secure channel.
    #[error("secure channel: {0}")]
    Link(String),
}

impl From<LinkError> for PairError {
    fn from(error: LinkError) -> Self {
        match error {
            LinkError::Closed => PairError::Closed,
            LinkError::Timeout => PairError::Timeout,
            other => PairError::Link(other.to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum PairMsg {
    Hello {
        device_name: String,
    },
    Welcome {
        host_name: String,
        verifying_key: [u8; 32],
    },
    Denied,
}

// ----- the host's offer ---------------------------------------------------------------

/// Where an offer stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferStatus {
    /// Usable; this many attempts remain.
    Open {
        /// Failed attempts still allowed.
        attempts_left: u32,
    },
    /// Past its lifetime.
    Expired,
    /// Burned by failures.
    Burned,
    /// Used successfully.
    Used,
}

#[derive(Debug, Default)]
struct OfferState {
    failures: u32,
    in_flight: u32,
    used: bool,
}

/// A code the host is offering: single use, expiring, attempt-limited.
#[derive(Debug)]
pub struct PairingOffer {
    code: PairingCode,
    created: Instant,
    lifetime: Duration,
    max_failures: u32,
    state: Mutex<OfferState>,
}

/// One attempt in progress. Dropping it without calling
/// [`AttemptTicket::succeed`] counts a failure.
#[derive(Debug)]
pub struct AttemptTicket<'a> {
    offer: &'a PairingOffer,
    done: bool,
}

impl PairingOffer {
    /// A new offer with the default lifetime and attempt limit.
    pub fn new(code: PairingCode) -> Self {
        Self::with_limits(code, Instant::now(), CODE_LIFETIME, MAX_FAILED_ATTEMPTS)
    }

    /// An offer with explicit limits and creation time.
    pub fn with_limits(
        code: PairingCode,
        created: Instant,
        lifetime: Duration,
        max_failures: u32,
    ) -> Self {
        Self {
            code,
            created,
            lifetime,
            max_failures,
            state: Mutex::new(OfferState::default()),
        }
    }

    /// The code to show.
    pub fn code(&self) -> &PairingCode {
        &self.code
    }

    /// Time left before expiry.
    pub fn remaining(&self, now: Instant) -> Duration {
        (self.created + self.lifetime).saturating_duration_since(now)
    }

    /// Where the offer stands at `now`.
    pub fn status(&self, now: Instant) -> OfferStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.used {
            OfferStatus::Used
        } else if state.failures >= self.max_failures {
            OfferStatus::Burned
        } else if now >= self.created + self.lifetime {
            OfferStatus::Expired
        } else {
            OfferStatus::Open {
                attempts_left: self.max_failures - state.failures,
            }
        }
    }

    /// Starts an attempt. Concurrent attempts together never exceed the
    /// failure budget.
    pub fn try_begin_at(&self, now: Instant) -> Result<AttemptTicket<'_>, PairError> {
        match self.status(now) {
            OfferStatus::Used => return Err(PairError::AlreadyUsed),
            OfferStatus::Burned => return Err(PairError::Burned),
            OfferStatus::Expired => return Err(PairError::Expired),
            OfferStatus::Open { .. } => {}
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.failures + state.in_flight >= self.max_failures {
            return Err(PairError::Burned);
        }
        state.in_flight += 1;
        Ok(AttemptTicket {
            offer: self,
            done: false,
        })
    }

    /// [`Self::try_begin_at`] now.
    pub fn try_begin(&self) -> Result<AttemptTicket<'_>, PairError> {
        self.try_begin_at(Instant::now())
    }
}

impl AttemptTicket<'_> {
    /// Marks the attempt as the one success; the code is spent.
    pub fn succeed(mut self) {
        self.done = true;
        let mut state = self.offer.state.lock().unwrap_or_else(|e| e.into_inner());
        state.in_flight = state.in_flight.saturating_sub(1);
        state.used = true;
    }
}

impl Drop for AttemptTicket<'_> {
    fn drop(&mut self) {
        if !self.done {
            let mut state = self.offer.state.lock().unwrap_or_else(|e| e.into_inner());
            state.in_flight = state.in_flight.saturating_sub(1);
            state.failures += 1;
        }
    }
}

// ----- the sans-IO steps ----------------------------------------------------------------

fn prologue(room: &str) -> Vec<u8> {
    let mut p = b"leon/1/pair/".to_vec();
    p.extend_from_slice(room.as_bytes());
    p
}

fn noise(
    secret: &[u8; 32],
    key: &[u8; 32],
    room: &str,
    initiator: bool,
) -> Result<HandshakeState, PairError> {
    let params: snow::params::NoiseParams =
        PAIR_PATTERN.parse().map_err(|_| PairError::Protocol)?;
    let context = prologue(room);
    let builder = snow::Builder::new(params)
        .prologue(&context)
        .and_then(|b| b.local_private_key(secret))
        .and_then(|b| b.psk(3, key))
        .map_err(|_| PairError::Protocol)?;
    if initiator {
        builder.build_initiator()
    } else {
        builder.build_responder()
    }
    .map_err(|_| PairError::Protocol)
}

/// The 32-byte key SPAKE2 derives, or a refusal when the peer's message is
/// not a valid group element.
fn shared_key(pake: Spake2<Ed25519Group>, peer: &[u8]) -> Result<Zeroizing<[u8; 32]>, PairError> {
    let raw = Zeroizing::new(pake.finish(peer).map_err(|_| PairError::NotAccepted)?);
    let key: [u8; 32] = raw.as_slice().try_into().map_err(|_| PairError::Protocol)?;
    Ok(Zeroizing::new(key))
}

struct ClientSteps {
    pake: Option<Spake2<Ed25519Group>>,
    noise: Option<HandshakeState>,
    secret: Zeroizing<[u8; 32]>,
    room: String,
}

impl ClientSteps {
    fn start(code: &PairingCode, identity: &Identity) -> (Self, Vec<u8>) {
        let (pake, message) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.password()),
            &SpakeIdentity::new(SPAKE_CLIENT),
            &SpakeIdentity::new(SPAKE_HOST),
        );
        (
            Self {
                pake: Some(pake),
                noise: None,
                secret: Zeroizing::new(*identity.static_secret()),
                room: code.room(),
            },
            message,
        )
    }

    fn on_pake(&mut self, reply: &[u8]) -> Result<Vec<u8>, PairError> {
        let pake = self.pake.take().ok_or(PairError::Protocol)?;
        let key = shared_key(pake, reply)?;
        let mut state = noise(&self.secret, &key, &self.room, true)?;
        let mut out = vec![0u8; 256];
        let n = state
            .write_message(&[], &mut out)
            .map_err(|_| PairError::Protocol)?;
        out.truncate(n);
        self.noise = Some(state);
        Ok(out)
    }

    fn on_noise2(&mut self, message: &[u8]) -> Result<Vec<u8>, PairError> {
        let state = self.noise.as_mut().ok_or(PairError::Protocol)?;
        let mut scratch = vec![0u8; 512];
        state
            .read_message(message, &mut scratch)
            .map_err(|_| PairError::NotAccepted)?;
        let mut out = vec![0u8; 512];
        let n = state
            .write_message(&[], &mut out)
            .map_err(|_| PairError::Protocol)?;
        out.truncate(n);
        Ok(out)
    }

    fn finish(self) -> Result<Session, PairError> {
        let state = self.noise.ok_or(PairError::Protocol)?;
        let remote: [u8; 32] = state
            .get_remote_static()
            .and_then(|k| k.try_into().ok())
            .ok_or(PairError::Protocol)?;
        let transport = state
            .into_transport_mode()
            .map_err(|_| PairError::Protocol)?;
        Ok(Session::from_transport(transport, remote))
    }
}

struct HostSteps {
    pake: Option<Spake2<Ed25519Group>>,
    noise: Option<HandshakeState>,
    secret: Zeroizing<[u8; 32]>,
    room: String,
}

impl HostSteps {
    fn start(code: &PairingCode, identity: &Identity) -> (Self, Vec<u8>) {
        let (pake, message) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.password()),
            &SpakeIdentity::new(SPAKE_CLIENT),
            &SpakeIdentity::new(SPAKE_HOST),
        );
        (
            Self {
                pake: Some(pake),
                noise: None,
                secret: Zeroizing::new(*identity.static_secret()),
                room: code.room(),
            },
            message,
        )
    }

    fn on_pake(&mut self, first: &[u8]) -> Result<(), PairError> {
        let pake = self.pake.take().ok_or(PairError::Protocol)?;
        let key = shared_key(pake, first)?;
        self.noise = Some(noise(&self.secret, &key, &self.room, false)?);
        Ok(())
    }

    fn on_noise1(&mut self, message: &[u8]) -> Result<Vec<u8>, PairError> {
        let state = self.noise.as_mut().ok_or(PairError::Protocol)?;
        let mut scratch = vec![0u8; 512];
        state
            .read_message(message, &mut scratch)
            .map_err(|_| PairError::NotAccepted)?;
        let mut out = vec![0u8; 512];
        let n = state
            .write_message(&[], &mut out)
            .map_err(|_| PairError::Protocol)?;
        out.truncate(n);
        Ok(out)
    }

    fn on_noise3(&mut self, message: &[u8]) -> Result<Session, PairError> {
        let mut state = self.noise.take().ok_or(PairError::Protocol)?;
        let mut scratch = vec![0u8; 512];
        // The pre-shared key is checked here: a wrong code fails this read.
        state
            .read_message(message, &mut scratch)
            .map_err(|_| PairError::NotAccepted)?;
        let remote: [u8; 32] = state
            .get_remote_static()
            .and_then(|k| k.try_into().ok())
            .ok_or(PairError::Protocol)?;
        let transport = state
            .into_transport_mode()
            .map_err(|_| PairError::Protocol)?;
        Ok(Session::from_transport(transport, remote))
    }
}

// ----- the async drivers -----------------------------------------------------------------

async fn next(pipe: &mut Pipe) -> Result<Vec<u8>, PairError> {
    match tokio::time::timeout(STEP_TIMEOUT, pipe.recv()).await {
        Err(_) => Err(PairError::Timeout),
        Ok(None) => Err(PairError::Closed),
        Ok(Some(bytes)) => Ok(bytes),
    }
}

async fn next_secure(channel: &mut SecureChannel) -> Result<PairMsg, PairError> {
    let bytes = match tokio::time::timeout(STEP_TIMEOUT, channel.recv_bytes()).await {
        Err(_) => return Err(PairError::Timeout),
        Ok(Err(_)) => return Err(PairError::NotAccepted),
        Ok(Ok(None)) => return Err(PairError::NotAccepted),
        Ok(Ok(Some(bytes))) => bytes,
    };
    postcard::from_bytes(&bytes).map_err(|_| PairError::Protocol)
}

/// What the client learns about the host it paired with.
#[derive(Clone, PartialEq, Eq)]
pub struct PairedHost {
    /// The host's id (a hash of its verifying key).
    pub host_id: HostId,
    /// The host's static X25519 key, to pin.
    pub host_key: [u8; 32],
    /// The host's Ed25519 verifying key.
    pub verifying_key: [u8; 32],
    /// The name the host gave itself.
    pub host_name: String,
}

impl std::fmt::Debug for PairedHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedHost")
            .field("host_id", &self.host_id)
            .field("host_name", &self.host_name)
            .finish_non_exhaustive()
    }
}

/// What a host asks its owner before accepting a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairRequest {
    /// The name the device gave itself.
    pub device_name: String,
    /// Its fingerprint, for the owner to compare.
    pub device_id: DeviceId,
}

/// A device the host has paired with.
#[derive(Clone, PartialEq, Eq)]
pub struct PairedDevice {
    /// Its static X25519 key.
    pub device_key: [u8; 32],
    /// The name it gave itself.
    pub device_name: String,
}

impl std::fmt::Debug for PairedDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedDevice")
            .field("device_id", &DeviceId::of(&self.device_key))
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

/// Pairs this device with the host that shows `code`.
pub async fn pair_as_client(
    mut pipe: Pipe,
    identity: &Identity,
    code: &PairingCode,
    device_name: &str,
) -> Result<PairedHost, PairError> {
    let (mut steps, first) = ClientSteps::start(code, identity);
    pipe.send(first).await.map_err(|_| PairError::Closed)?;
    let second = next(&mut pipe).await?;
    if second.len() == 2 && second[0] == REFUSAL_TAG {
        return Err(match second[1] {
            1 => PairError::Expired,
            2 => PairError::Burned,
            3 => PairError::AlreadyUsed,
            _ => PairError::NotAccepted,
        });
    }
    let n1 = steps.on_pake(&second)?;
    pipe.send(n1).await.map_err(|_| PairError::Closed)?;
    let n2 = next(&mut pipe).await?;
    let n3 = steps.on_noise2(&n2)?;
    pipe.send(n3).await.map_err(|_| PairError::Closed)?;
    let session = steps.finish()?;
    let host_key = session.remote_static();
    let mut channel = SecureChannel::new(pipe, session);
    let hello = postcard::to_allocvec(&PairMsg::Hello {
        device_name: device_name.chars().take(64).collect(),
    })
    .map_err(|_| PairError::Protocol)?;
    channel.send_bytes(&hello).await?;
    match next_secure(&mut channel).await? {
        PairMsg::Welcome {
            host_name,
            verifying_key,
        } => Ok(PairedHost {
            host_id: HostId::from_verifying_key(&verifying_key),
            host_key,
            verifying_key,
            host_name: host_name.chars().take(64).collect(),
        }),
        PairMsg::Denied => Err(PairError::Denied),
        PairMsg::Hello { .. } => Err(PairError::Protocol),
    }
}

/// Serves one pairing attempt against `offer`. `approve` decides, usually by
/// asking the person at the host; headless hosts approve whatever knows the
/// code.
pub async fn pair_as_host<F, Fut>(
    mut pipe: Pipe,
    identity: &Identity,
    offer: &PairingOffer,
    host_name: &str,
    approve: F,
) -> Result<PairedDevice, PairError>
where
    F: FnOnce(PairRequest) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let first = next(&mut pipe).await?;
    let ticket = match offer.try_begin() {
        Ok(ticket) => ticket,
        Err(error) => {
            let reason = match error {
                PairError::Expired => 1,
                PairError::Burned => 2,
                _ => 3,
            };
            let _ = pipe.send(vec![REFUSAL_TAG, reason]).await;
            return Err(error);
        }
    };
    let (mut steps, reply) = HostSteps::start(offer.code(), identity);
    steps.on_pake(&first)?;
    pipe.send(reply).await.map_err(|_| PairError::Closed)?;
    let n1 = next(&mut pipe).await?;
    let n2 = steps.on_noise1(&n1)?;
    pipe.send(n2).await.map_err(|_| PairError::Closed)?;
    let n3 = next(&mut pipe).await?;
    let session = steps.on_noise3(&n3)?;
    let device_key = session.remote_static();
    let mut channel = SecureChannel::new(pipe, session);
    let PairMsg::Hello { device_name } = next_secure(&mut channel).await? else {
        return Err(PairError::Protocol);
    };
    let device_name: String = device_name.chars().take(64).collect();
    let request = PairRequest {
        device_name: device_name.clone(),
        device_id: DeviceId::of(&device_key),
    };
    if !approve(request).await {
        let denied = postcard::to_allocvec(&PairMsg::Denied).map_err(|_| PairError::Protocol)?;
        let _ = channel.send_bytes(&denied).await;
        return Err(PairError::Denied);
    }
    let welcome = postcard::to_allocvec(&PairMsg::Welcome {
        host_name: host_name.chars().take(64).collect(),
        verifying_key: identity.verifying_key(),
    })
    .map_err(|_| PairError::Protocol)?;
    channel.send_bytes(&welcome).await?;
    ticket.succeed();
    Ok(PairedDevice {
        device_key,
        device_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> PairingCode {
        PairingCode::parse("ABCD-EFG-HJK").unwrap()
    }

    async fn pair(
        client: &Identity,
        host: &Identity,
        offer: &PairingOffer,
        typed: &PairingCode,
        approve: bool,
    ) -> (
        Result<PairedHost, PairError>,
        Result<PairedDevice, PairError>,
    ) {
        let (a, b) = Pipe::pair();
        tokio::join!(
            pair_as_client(a, client, typed, "Ana's laptop"),
            pair_as_host(b, host, offer, "build-box", |_| async move { approve })
        )
    }

    #[tokio::test]
    async fn pairing_with_the_right_code_exchanges_identities_both_ways() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        let (c, h) = pair(&client, &host, &offer, &code(), true).await;
        let paired_host = c.unwrap();
        let paired_device = h.unwrap();
        assert_eq!(paired_host.host_id, host.host_id());
        assert_eq!(paired_host.host_key, host.static_public());
        assert_eq!(paired_host.verifying_key, host.verifying_key());
        assert_eq!(paired_host.host_name, "build-box");
        assert_eq!(paired_device.device_key, client.static_public());
        assert_eq!(paired_device.device_name, "Ana's laptop");
    }

    #[tokio::test]
    async fn a_wrong_code_fails_on_both_sides_and_counts_a_failure() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        let wrong = PairingCode::parse("ABCD-EFG-HJM").unwrap();
        let (c, h) = pair(&client, &host, &offer, &wrong, true).await;
        assert!(c.is_err());
        assert_eq!(h.unwrap_err(), PairError::NotAccepted);
        assert_eq!(
            offer.status(Instant::now()),
            OfferStatus::Open { attempts_left: 4 }
        );
    }

    #[tokio::test]
    async fn five_wrong_attempts_burn_the_code_even_for_the_right_one() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        let wrong = PairingCode::parse("ABCD-EFG-HJM").unwrap();
        for _ in 0..MAX_FAILED_ATTEMPTS {
            let (c, _) = pair(&client, &host, &offer, &wrong, true).await;
            assert!(c.is_err());
        }
        assert_eq!(offer.status(Instant::now()), OfferStatus::Burned);
        let (c, h) = pair(&client, &host, &offer, &code(), true).await;
        assert_eq!(c.unwrap_err(), PairError::Burned);
        assert_eq!(h.unwrap_err(), PairError::Burned);
    }

    #[tokio::test]
    async fn a_code_works_once() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        assert!(pair(&client, &host, &offer, &code(), true).await.0.is_ok());
        let (c, _) = pair(&Identity::generate(), &host, &offer, &code(), true).await;
        assert_eq!(c.unwrap_err(), PairError::AlreadyUsed);
    }

    #[test]
    fn a_code_expires_after_its_lifetime() {
        let start = Instant::now();
        let offer = PairingOffer::with_limits(code(), start, CODE_LIFETIME, 5);
        assert!(offer.try_begin_at(start + Duration::from_secs(599)).is_ok());
        assert_eq!(
            offer
                .try_begin_at(start + Duration::from_secs(601))
                .unwrap_err(),
            PairError::Expired
        );
        assert_eq!(
            offer.remaining(start + Duration::from_secs(700)),
            Duration::ZERO
        );
        assert_eq!(
            offer.remaining(start + Duration::from_secs(100)),
            Duration::from_secs(500)
        );
    }

    #[test]
    fn concurrent_attempts_together_cannot_exceed_the_failure_budget() {
        let offer = PairingOffer::new(code());
        let tickets: Vec<_> = (0..5).map(|_| offer.try_begin().unwrap()).collect();
        assert_eq!(offer.try_begin().unwrap_err(), PairError::Burned);
        drop(tickets);
        assert_eq!(offer.status(Instant::now()), OfferStatus::Burned);
    }

    #[tokio::test]
    async fn the_owner_can_decline_and_the_client_is_told() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        let (c, h) = pair(&client, &host, &offer, &code(), false).await;
        assert_eq!(c.unwrap_err(), PairError::Denied);
        assert_eq!(h.unwrap_err(), PairError::Denied);
    }

    #[tokio::test]
    async fn the_owner_is_shown_who_is_asking() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        let (a, b) = Pipe::pair();
        let expected = client.device_id();
        let typed = code();
        let (_, h) = tokio::join!(
            pair_as_client(a, &client, &typed, "Ana's laptop"),
            pair_as_host(b, &host, &offer, "box", |request| async move {
                assert_eq!(request.device_name, "Ana's laptop");
                assert_eq!(request.device_id, expected);
                true
            })
        );
        h.unwrap();
    }

    #[tokio::test]
    async fn an_eavesdropper_recording_the_pairing_learns_neither_code_nor_keys() {
        let (client, host) = (Identity::generate(), Identity::generate());
        let offer = PairingOffer::new(code());
        let (a, b, transcript) = crate::test_support::tap();
        let typed = code();
        let (c, h) = tokio::join!(
            pair_as_client(a, &client, &typed, "Ana's laptop"),
            pair_as_host(b, &host, &offer, "build-box", |_| async { true })
        );
        c.unwrap();
        h.unwrap();
        let all = transcript.messages().concat();
        for secret in [&b"EFGHJK"[..], &b"build-box"[..], &b"Ana's laptop"[..]] {
            assert!(!all.windows(secret.len()).any(|w| w == secret));
        }
        assert!(!all.windows(8).any(|w| w == &client.static_public()[..8]));
        assert!(!all.windows(8).any(|w| w == &host.static_public()[..8]));
    }

    #[tokio::test]
    async fn a_man_in_the_middle_without_the_code_cannot_finish() {
        // The attacker answers the client with a SPAKE2 run on a guessed code.
        let client = Identity::generate();
        let (a, mut attacker) = Pipe::pair();
        let guess = PairingCode::parse("ABCD-222-222").unwrap();
        let attack = tokio::spawn(async move {
            let host = Identity::generate();
            let first = attacker.recv().await.unwrap();
            let (mut steps, reply) = HostSteps::start(&guess, &host);
            steps.on_pake(&first).unwrap();
            attacker.send(reply).await.unwrap();
            let n1 = attacker.recv().await.unwrap();
            let n2 = steps.on_noise1(&n1).unwrap();
            attacker.send(n2).await.unwrap();
            let n3 = attacker.recv().await.unwrap();
            steps.on_noise3(&n3).is_err()
        });
        let result = pair_as_client(a, &client, &code(), "x").await;
        assert!(result.is_err());
        assert!(
            attack.await.unwrap(),
            "the host-side check fails for a wrong code"
        );
    }

    #[test]
    fn debug_output_of_paired_parties_hides_keys() {
        let paired = PairedDevice {
            device_key: [7; 32],
            device_name: "x".into(),
        };
        assert!(!format!("{paired:?}").contains("[7"));
    }
}
