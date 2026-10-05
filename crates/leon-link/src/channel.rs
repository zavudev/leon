//! The encrypted message channel and the handshakes that open it.
//!
//! [`connect`] (client) and [`accept`] (host) run the `IK` handshake over a
//! [`Pipe`] and return a [`SecureChannel`]: a pipe of authenticated, encrypted
//! [`Message`]s. A host asks its device registry before it answers, so an
//! unknown or revoked key never gets a reply, let alone a command.

use std::time::Duration;

use leon_wire::{decode_frame, encode_frame, FrameError, Message};
use thiserror::Error;

use crate::identity::Identity;
use crate::pipe::{Pipe, PipeClosed};
use crate::session::{InitiatorHandshake, ResponderHandshake, Session, SessionError};

/// How long a handshake may take.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a channel failed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LinkError {
    /// The peer is gone.
    #[error("the connection closed")]
    Closed,
    /// Nothing arrived in time.
    #[error("the other side did not answer in time")]
    Timeout,
    /// The host does not know this device or has revoked it.
    #[error("the host refused this device")]
    Rejected,
    /// The secure session failed (including tampering).
    #[error("secure session failed: {0}")]
    Session(#[from] SessionError),
    /// A message was malformed.
    #[error("bad message: {0}")]
    Frame(#[from] FrameError),
}

impl From<PipeClosed> for LinkError {
    fn from(_: PipeClosed) -> Self {
        LinkError::Closed
    }
}

/// An encrypted pipe of protocol messages.
#[derive(Debug)]
pub struct SecureChannel {
    pipe: Pipe,
    session: Session,
}

impl SecureChannel {
    /// A channel over an established session.
    pub fn new(pipe: Pipe, session: Session) -> Self {
        Self { pipe, session }
    }

    /// The peer's authenticated static key.
    pub fn remote_static(&self) -> [u8; 32] {
        self.session.remote_static()
    }

    /// The session, for inspection.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The session, to tune it.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Sends raw application bytes.
    pub async fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        for part in self.session.seal(bytes)? {
            self.pipe.send(part).await?;
        }
        Ok(())
    }

    /// The next application message's bytes; `None` when the peer closed
    /// cleanly. Cancel-safe.
    pub async fn recv_bytes(&mut self) -> Result<Option<Vec<u8>>, LinkError> {
        loop {
            let Some(sealed) = self.pipe.recv().await else {
                return Ok(None);
            };
            if let Some(bytes) = self.session.open(&sealed)? {
                return Ok(Some(bytes));
            }
        }
    }

    /// Sends a protocol message.
    pub async fn send(&mut self, message: &Message) -> Result<(), LinkError> {
        let frame = encode_frame(message)?;
        self.send_bytes(&frame).await
    }

    /// The next protocol message; `None` when the peer closed cleanly.
    pub async fn recv(&mut self) -> Result<Option<Message>, LinkError> {
        match self.recv_bytes().await? {
            None => Ok(None),
            Some(bytes) => {
                let (message, used) = decode_frame(&bytes)?;
                if used != bytes.len() {
                    return Err(FrameError::Malformed("trailing bytes".into()).into());
                }
                Ok(Some(message))
            }
        }
    }
}

/// Why a host turns a client away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejection;

/// Connects to the host whose pinned static key is `host_key`.
pub async fn connect(
    mut pipe: Pipe,
    identity: &Identity,
    host_key: &[u8; 32],
) -> Result<SecureChannel, LinkError> {
    let (handshake, first) = InitiatorHandshake::start(identity, host_key)?;
    pipe.send(first).await?;
    let reply = match tokio::time::timeout(HANDSHAKE_TIMEOUT, pipe.recv()).await {
        Err(_) => return Err(LinkError::Timeout),
        // A host that refuses us closes without answering.
        Ok(None) => return Err(LinkError::Rejected),
        Ok(Some(reply)) => reply,
    };
    let session = handshake.finish(&reply)?;
    Ok(SecureChannel::new(pipe, session))
}

/// Accepts a client. `authorised` is asked about the client's static key
/// *before* anything is sent back; when it says no, nothing is.
pub async fn accept(
    mut pipe: Pipe,
    identity: &Identity,
    authorised: impl FnOnce(&[u8; 32]) -> bool,
) -> Result<SecureChannel, LinkError> {
    let first = match tokio::time::timeout(HANDSHAKE_TIMEOUT, pipe.recv()).await {
        Err(_) => return Err(LinkError::Timeout),
        Ok(None) => return Err(LinkError::Closed),
        Ok(Some(first)) => first,
    };
    let responder = ResponderHandshake::read_first(identity, &first)?;
    if !authorised(&responder.remote_static()) {
        return Err(LinkError::Rejected);
    }
    let (reply, session) = responder.finish()?;
    pipe.send(reply).await?;
    Ok(SecureChannel::new(pipe, session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tap;

    async fn linked() -> (SecureChannel, SecureChannel, Identity, Identity) {
        let client = Identity::generate();
        let host = Identity::generate();
        let (a, b) = Pipe::pair();
        let host_key = host.static_public();
        let client_key = client.static_public();
        let (c, h) = tokio::join!(
            connect(a, &client, &host_key),
            accept(b, &host, |k| *k == client_key)
        );
        (c.unwrap(), h.unwrap(), client, host)
    }

    #[tokio::test]
    async fn the_handshake_succeeds_and_messages_flow_both_ways() {
        let (mut client, mut host, ..) = linked().await;
        client.send(&Message::Ping { nonce: 5 }).await.unwrap();
        assert_eq!(host.recv().await.unwrap(), Some(Message::Ping { nonce: 5 }));
        host.send(&Message::Pong { nonce: 5 }).await.unwrap();
        assert_eq!(
            client.recv().await.unwrap(),
            Some(Message::Pong { nonce: 5 })
        );
    }

    #[tokio::test]
    async fn a_client_that_pinned_the_wrong_host_key_never_gets_a_session() {
        let client = Identity::generate();
        let host = Identity::generate();
        let wrong = Identity::generate().static_public();
        let (a, b) = Pipe::pair();
        let (c, h) = tokio::join!(connect(a, &client, &wrong), accept(b, &host, |_| true));
        assert!(c.is_err());
        assert!(h.is_err());
    }

    #[tokio::test]
    async fn an_unauthorised_device_is_refused_before_any_reply() {
        let client = Identity::generate();
        let host = Identity::generate();
        let (a, b) = Pipe::pair();
        let key = host.static_public();
        let (c, h) = tokio::join!(connect(a, &client, &key), accept(b, &host, |_| false));
        assert_eq!(c.unwrap_err(), LinkError::Rejected);
        assert_eq!(h.unwrap_err(), LinkError::Rejected);
    }

    #[tokio::test]
    async fn a_replayed_first_message_gets_no_session_and_no_command() {
        let client = Identity::generate();
        let host = Identity::generate();
        let (client_end, host_end, transcript) = tap();
        let key = host.static_public();
        let ckey = client.static_public();
        let (c, h) = tokio::join!(
            connect(client_end, &client, &key),
            accept(host_end, &host, |k| *k == ckey)
        );
        let mut honest_client = c.unwrap();
        let mut honest_host = h.unwrap();
        honest_client
            .send(&Message::Ping { nonce: 1 })
            .await
            .unwrap();
        assert!(honest_host.recv().await.unwrap().is_some());
        let first = transcript.messages()[0].clone();

        // An attacker replays the recorded first message to the host.
        let (attacker, host_end) = Pipe::pair();
        let accepting = tokio::spawn(async move { accept(host_end, &host, |k| *k == ckey).await });
        attacker.send(first).await.unwrap();
        let mut attacker = attacker;
        let _reply = attacker
            .recv()
            .await
            .expect("the host answers a valid first message");
        let mut replayed = accepting.await.unwrap().unwrap();
        // The attacker has no ephemeral key: whatever it sends fails to decrypt.
        attacker.send(vec![7; 64]).await.unwrap();
        assert!(matches!(
            replayed.recv().await,
            Err(LinkError::Session(SessionError::Decrypt))
        ));
    }

    #[tokio::test]
    async fn what_the_relay_forwards_contains_none_of_the_plaintext() {
        let client = Identity::generate();
        let host = Identity::generate();
        let (client_end, host_end, transcript) = tap();
        let key = host.static_public();
        let ckey = client.static_public();
        let (c, h) = tokio::join!(
            connect(client_end, &client, &key),
            accept(host_end, &host, |k| *k == ckey)
        );
        let (mut c, mut h) = (c.unwrap(), h.unwrap());
        let marker = b"PLAINTEXT-MARKER-4f9a1c";
        let spec = leon_wire::ExecSpec {
            program: String::from_utf8_lossy(marker).into_owned(),
            ..Default::default()
        };
        c.send(&Message::Exec {
            id: 1,
            spec,
            timeout_ms: None,
        })
        .await
        .unwrap();
        assert!(h.recv().await.unwrap().is_some());
        h.send(&Message::PtyData {
            pty: 1,
            offset: 0,
            bytes: marker.to_vec(),
        })
        .await
        .unwrap();
        assert!(c.recv().await.unwrap().is_some());
        let all: Vec<u8> = transcript.messages().concat();
        assert!(all.len() > 100);
        assert!(!all.windows(marker.len()).any(|w| w == marker));
        assert!(
            !all.windows(8).any(|w| w == &client.static_public()[..8]),
            "static keys are encrypted too"
        );
    }

    #[tokio::test]
    async fn tampered_ciphertext_is_rejected_and_the_channel_stays_closed() {
        let client = Identity::generate();
        let host = Identity::generate();
        let (client_end, host_end, transcript) = tap();
        let key = host.static_public();
        let ckey = client.static_public();
        let (c, h) = tokio::join!(
            connect(client_end, &client, &key),
            accept(host_end, &host, |k| *k == ckey)
        );
        let (mut c, mut h) = (c.unwrap(), h.unwrap());
        transcript.corrupt_next_from_client();
        c.send(&Message::Ping { nonce: 1 }).await.unwrap();
        assert!(matches!(
            h.recv().await,
            Err(LinkError::Session(SessionError::Decrypt))
        ));
        c.send(&Message::Ping { nonce: 2 }).await.unwrap();
        assert!(h.recv().await.is_err());
        assert!(h.session().is_closed());
    }

    #[tokio::test]
    async fn a_message_bigger_than_one_noise_message_crosses_intact() {
        let (mut client, mut host, ..) = linked().await;
        let output = leon_wire::ExecOutput {
            status: Some(0),
            stdout: vec![0xAB; 300_000],
            stderr: Vec::new(),
            truncated: false,
        };
        host.send(&Message::ExecOutput {
            id: 9,
            output: output.clone(),
        })
        .await
        .unwrap();
        assert_eq!(
            client.recv().await.unwrap(),
            Some(Message::ExecOutput { id: 9, output })
        );
    }

    #[tokio::test]
    async fn a_closed_peer_reads_as_a_clean_end() {
        let (client, mut host, ..) = linked().await;
        drop(client);
        assert_eq!(host.recv().await.unwrap(), None);
    }
}
