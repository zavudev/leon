//! Dialling a relay over WebSocket and turning what comes out into [`Pipe`]s.
//!
//! * [`dial`]: a client reaches a host (by id, or by the room of a pairing
//!   code) and gets one [`Pipe`] whose messages are the opaque bytes the relay
//!   forwards. When the host leaves, or the relay does, the pipe closes.
//! * [`register`]: a host proves ownership of its id to the relay and gets a
//!   [`HostLink`]: a stream of arriving clients, each with its own [`Pipe`].
//!
//! The relay is untrusted; see `leon_wire::relay` for the rendezvous it
//! implements. Nothing here sees plaintext or keys beyond the host's Ed25519
//! signature over the relay's nonce.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use leon_wire::relay::{
    decode_client_side, decode_host_side, encode_client_data, encode_control, encode_host_data,
    join_path, pair_path, registration_message, ClientToRelay, HostToRelay, Inbound, RelayError,
    RelayLimits, RelayToClient, RelayToHost, PATH_HOST,
};
use leon_wire::HostId;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{protocol::WebSocketConfig, Message};

use crate::identity::Identity;
use crate::pipe::{Pipe, PIPE_CAPACITY};

/// How long connecting and the rendezvous may take.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(12);
/// How often a host pings the relay to keep proxies from idling it out.
const KEEPALIVE: Duration = Duration::from_secs(20);
/// The largest WebSocket message accepted from a relay.
const MAX_WS_MESSAGE: usize = 2 * 1024 * 1024;

/// Why a relay could not be used.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DialError {
    /// The relay could not be reached at all (DNS, refused, TLS, timeout).
    #[error("cannot reach the relay: {0}")]
    Unreachable(String),
    /// TLS could not be set up (no crypto provider, a failure inside the
    /// connection task). The connection is retried like any other failure.
    #[error("The secure connection could not be set up: {0}")]
    Secure(String),
    /// The relay answered but refused.
    #[error("the relay refused: {}", .0.message)]
    Refused(RelayError),
    /// The relay did not follow its protocol.
    #[error("the relay did not follow its protocol")]
    Protocol,
}

/// Whom a client wants to reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A paired host, by id.
    Host(HostId),
    /// The host showing a pairing code with this room.
    Room(String),
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn endpoint(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

async fn open_socket(url: &str) -> Result<Socket, DialError> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE))
        .max_frame_size(Some(MAX_WS_MESSAGE));
    let connector = if url.starts_with("wss://") {
        Some(crate::tls::connector().map_err(DialError::Secure)?)
    } else {
        None
    };
    let url = url.to_owned();
    // In a task of its own: a failure inside the connection (a panic of a TLS
    // library included) comes back here as an error to report and retry, and
    // never as a connection that stays "connecting" for ever.
    let task = tokio::spawn(async move {
        let connect = tokio_tungstenite::connect_async_tls_with_config(
            url.as_str(),
            Some(config),
            false,
            connector,
        );
        tokio::time::timeout(DIAL_TIMEOUT, connect).await
    });
    match task.await {
        Err(joined) => Err(DialError::Secure(if joined.is_panic() {
            "the TLS library failed".into()
        } else {
            "the connection was cancelled".into()
        })),
        Ok(Err(_)) => Err(DialError::Unreachable("timed out".into())),
        Ok(Ok(Err(error))) => Err(DialError::Unreachable(short_error(&error))),
        Ok(Ok(Ok((socket, _)))) => Ok(socket),
    }
}

/// A one-line, secret-free description of a connection error.
fn short_error(error: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    match error {
        Error::Io(io) => io.to_string(),
        Error::Http(response) => format!("HTTP {}", response.status()),
        Error::Tls(_) => "TLS error".into(),
        Error::Url(_) => "invalid relay address".into(),
        other => other.to_string(),
    }
}

async fn next_binary(socket: &mut Socket) -> Result<Vec<u8>, DialError> {
    loop {
        match tokio::time::timeout(DIAL_TIMEOUT, socket.next()).await {
            Err(_) => return Err(DialError::Unreachable("timed out".into())),
            Ok(None) | Ok(Some(Err(_))) => return Err(DialError::Protocol),
            Ok(Some(Ok(Message::Binary(bytes)))) => return Ok(bytes.to_vec()),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            Ok(Some(Ok(_))) => return Err(DialError::Protocol),
        }
    }
}

async fn send_binary(socket: &mut Socket, bytes: Vec<u8>) -> Result<(), DialError> {
    socket
        .send(Message::Binary(bytes.into()))
        .await
        .map_err(|error| DialError::Unreachable(short_error(&error)))
}

/// Connects to a host through the relay at `base_url` (`wss://…`).
pub async fn dial(
    base_url: &str,
    target: &Target,
    token: Option<Vec<u8>>,
) -> Result<Pipe, DialError> {
    let path = match target {
        Target::Host(id) => join_path(id),
        Target::Room(room) => pair_path(room),
    };
    let mut socket = open_socket(&endpoint(base_url, &path)).await?;
    send_binary(&mut socket, encode_control(&ClientToRelay::Join { token })).await?;
    let first = next_binary(&mut socket).await?;
    match decode_client_side::<RelayToClient>(&first) {
        Ok(Inbound::Control(RelayToClient::Joined { .. })) => {}
        Ok(Inbound::Control(RelayToClient::Error(error))) => return Err(DialError::Refused(error)),
        _ => return Err(DialError::Protocol),
    }
    let (to_peer_tx, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    let (from_peer_tx, from_peer_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    tokio::spawn(async move {
        let (mut sink, mut stream) = socket.split();
        loop {
            tokio::select! {
                outgoing = to_peer_rx.recv() => match outgoing {
                    Some(bytes) => {
                        if sink.send(Message::Binary(encode_client_data(&bytes).into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                },
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Binary(bytes))) => match decode_client_side::<RelayToClient>(&bytes) {
                        Ok(Inbound::Data { payload, .. }) => {
                            if from_peer_tx.send(payload).await.is_err() {
                                break;
                            }
                        }
                        // PeerLeft, an error or nonsense: the pipe ends.
                        _ => break,
                    },
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    _ => break,
                },
            }
        }
        let _ = sink.close().await;
    });
    Ok(Pipe::new(to_peer_tx, from_peer_rx))
}

/// A client that arrived at a host.
#[derive(Debug)]
pub struct Incoming {
    /// The relay's number for this client.
    pub channel: u32,
    /// Whether it came through a pairing room (so it should be served as a
    /// pairing attempt, not a session).
    pub pairing: bool,
    /// Its opaque message pipe.
    pub pipe: Pipe,
}

enum Command {
    OpenPairing(String, oneshot::Sender<Result<(), RelayError>>),
    ClosePairing(String),
}

/// A handle for pairing-room commands that can be cloned and used while the
/// [`HostLink`] itself is being read.
#[derive(Debug, Clone)]
pub struct LinkCommands {
    commands: mpsc::Sender<Command>,
}

impl LinkCommands {
    /// Announces a pairing room; waits for the relay's answer.
    pub async fn open_pairing(&self, room: &str) -> Result<(), DialError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::OpenPairing(room.to_owned(), reply))
            .await
            .map_err(|_| DialError::Unreachable("the relay connection ended".into()))?;
        match tokio::time::timeout(DIAL_TIMEOUT, answer).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(DialError::Refused(error)),
            _ => Err(DialError::Unreachable("no answer from the relay".into())),
        }
    }

    /// Withdraws a pairing room.
    pub async fn close_pairing(&self, room: &str) {
        let _ = self
            .commands
            .send(Command::ClosePairing(room.to_owned()))
            .await;
    }
}

/// A host's registration with a relay.
#[derive(Debug)]
pub struct HostLink {
    incoming: mpsc::Receiver<Incoming>,
    commands: mpsc::Sender<Command>,
    host_id: HostId,
    limits: RelayLimits,
}

impl HostLink {
    /// The id the relay confirmed.
    pub fn host_id(&self) -> HostId {
        self.host_id
    }

    /// The relay's advertised limits.
    pub fn limits(&self) -> &RelayLimits {
        &self.limits
    }

    /// The next client; `None` when the relay connection has ended.
    pub async fn accept(&mut self) -> Option<Incoming> {
        self.incoming.recv().await
    }

    /// A cloneable handle for the pairing commands.
    pub fn commands(&self) -> LinkCommands {
        LinkCommands {
            commands: self.commands.clone(),
        }
    }

    /// Announces a pairing room; waits for the relay's answer.
    pub async fn open_pairing(&self, room: &str) -> Result<(), DialError> {
        self.commands().open_pairing(room).await
    }

    /// Withdraws a pairing room.
    pub async fn close_pairing(&self, room: &str) {
        self.commands().close_pairing(room).await
    }
}

enum Out {
    Data(u32, Vec<u8>),
    CloseChannel(u32),
}

/// Registers this host with the relay at `base_url`.
pub async fn register(
    base_url: &str,
    identity: &Identity,
    token: Option<Vec<u8>>,
) -> Result<HostLink, DialError> {
    let mut socket = open_socket(&endpoint(base_url, PATH_HOST)).await?;
    let first = next_binary(&mut socket).await?;
    let (nonce, limits) = match decode_host_side::<RelayToHost>(&first) {
        Ok(Inbound::Control(RelayToHost::Challenge { nonce, limits })) => (nonce, limits),
        Ok(Inbound::Control(RelayToHost::Error(error))) => return Err(DialError::Refused(error)),
        _ => return Err(DialError::Protocol),
    };
    let verifying_key = identity.verifying_key();
    let signature = identity.sign(&registration_message(&nonce, &verifying_key));
    send_binary(
        &mut socket,
        encode_control(&HostToRelay::Register {
            verifying_key,
            signature,
            token,
        }),
    )
    .await?;
    let reply = next_binary(&mut socket).await?;
    let host_id = match decode_host_side::<RelayToHost>(&reply) {
        Ok(Inbound::Control(RelayToHost::Registered { host_id })) => host_id,
        Ok(Inbound::Control(RelayToHost::Error(error))) => return Err(DialError::Refused(error)),
        _ => return Err(DialError::Protocol),
    };
    if host_id != identity.host_id() {
        return Err(DialError::Protocol);
    }

    let (incoming_tx, incoming_rx) = mpsc::channel::<Incoming>(16);
    let (command_tx, mut command_rx) = mpsc::channel::<Command>(8);
    tokio::spawn(async move {
        let (mut sink, mut stream) = socket.split();
        let (out_tx, mut out_rx) = mpsc::channel::<Out>(PIPE_CAPACITY * 4);
        let mut channels: HashMap<u32, mpsc::Sender<Vec<u8>>> = HashMap::new();
        let mut pending: HashMap<String, oneshot::Sender<Result<(), RelayError>>> = HashMap::new();
        let mut keepalive = tokio::time::interval(KEEPALIVE);
        keepalive.tick().await;
        loop {
            tokio::select! {
                _ = keepalive.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                command = command_rx.recv() => match command {
                    Some(Command::OpenPairing(room, reply)) => {
                        pending.insert(room.clone(), reply);
                        let frame = encode_control(&HostToRelay::OpenPairing { room });
                        if sink.send(Message::Binary(frame.into())).await.is_err() { break; }
                    }
                    Some(Command::ClosePairing(room)) => {
                        let frame = encode_control(&HostToRelay::ClosePairing { room });
                        if sink.send(Message::Binary(frame.into())).await.is_err() { break; }
                    }
                    None => break,
                },
                out = out_rx.recv() => {
                    let frame = match out {
                        Some(Out::Data(channel, bytes)) => encode_host_data(channel, &bytes),
                        Some(Out::CloseChannel(channel)) => {
                            channels.remove(&channel);
                            encode_control(&HostToRelay::CloseChannel { channel })
                        }
                        None => break,
                    };
                    if sink.send(Message::Binary(frame.into())).await.is_err() { break; }
                }
                received = stream.next() => {
                    let Some(Ok(message)) = received else { break };
                    let bytes = match message {
                        Message::Binary(bytes) => bytes,
                        Message::Ping(_) | Message::Pong(_) => continue,
                        _ => break,
                    };
                    match decode_host_side::<RelayToHost>(&bytes) {
                        Ok(Inbound::Data { channel, payload }) => {
                            if let Some(tx) = channels.get(&channel) {
                                // A consumer that does not keep up loses its channel
                                // rather than stalling every other client.
                                if tx.try_send(payload).is_err() {
                                    channels.remove(&channel);
                                    let _ = out_tx.send(Out::CloseChannel(channel)).await;
                                }
                            }
                        }
                        Ok(Inbound::Control(RelayToHost::ChannelOpened { channel, pairing })) => {
                            let (to_peer_tx, mut to_peer_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
                            let (from_peer_tx, from_peer_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
                            channels.insert(channel, from_peer_tx);
                            let out = out_tx.clone();
                            tokio::spawn(async move {
                                while let Some(bytes) = to_peer_rx.recv().await {
                                    if out.send(Out::Data(channel, bytes)).await.is_err() {
                                        return;
                                    }
                                }
                                let _ = out.send(Out::CloseChannel(channel)).await;
                            });
                            let incoming = Incoming { channel, pairing, pipe: Pipe::new(to_peer_tx, from_peer_rx) };
                            if incoming_tx.send(incoming).await.is_err() { break; }
                        }
                        Ok(Inbound::Control(RelayToHost::ChannelClosed { channel })) => {
                            channels.remove(&channel);
                        }
                        Ok(Inbound::Control(RelayToHost::PairingOpened { room })) => {
                            if let Some(reply) = pending.remove(&room) {
                                let _ = reply.send(Ok(()));
                            }
                        }
                        Ok(Inbound::Control(RelayToHost::Error(error))) => {
                            // Errors after registration answer a pairing request.
                            if let Some(room) = pending.keys().next().cloned() {
                                if let Some(reply) = pending.remove(&room) {
                                    let _ = reply.send(Err(error));
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
        let _ = sink.close().await;
        // Dropping `channels` and `incoming_tx` tells every pipe and the
        // caller that the relay connection ended.
    });
    Ok(HostLink {
        incoming: incoming_rx,
        commands: command_tx,
        host_id,
        limits,
    })
}
