//! The client side of a session: one durable connection to a host.
//!
//! A [`Client`] owns a background task that dials the host (through any
//! [`Dialer`]), runs the secure handshake, keeps the connection alive,
//! reconnects with backoff when it drops, and multiplexes requests:
//!
//! * [`Client::exec`] runs a command and waits for its output;
//! * [`Client::pty_open`] / [`Client::pty_attach`] give a [`PtyStream`]: the
//!   terminal's output in order and without duplicates. When the connection
//!   drops and comes back, every live stream is re-attached from the last
//!   offset it saw, so the consumer simply keeps receiving; a
//!   [`PtyEvent::Gap`] says the host's buffer no longer reached back that far.
//!
//! The connection state is observable ([`Client::watch`]) and, when it is not
//! `Online`, says why ([`Failure`]) in terms a person can act on.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use leon_wire::relay::RelayErrorCode;
use leon_wire::{
    ErrorCode, ExecOutput, ExecSpec, Exit, Grid, HostId, Message, PtyInfo, WireError,
    PROTOCOL_VERSION,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};

use crate::channel::{connect, LinkError, SecureChannel};
use crate::identity::Identity;
use crate::pipe::Pipe;
use crate::relay_client::{dial, DialError, Target};

/// A boxed future yielding a pipe to the host.
pub type DialFuture = Pin<Box<dyn Future<Output = Result<Pipe, DialError>> + Send>>;

/// Where pipes to the host come from.
pub trait Dialer: Send + Sync + 'static {
    /// Opens a new pipe to the host.
    fn dial(&self) -> DialFuture;
}

/// Reaches a host through a relay.
#[derive(Debug, Clone)]
pub struct RelayDialer {
    /// The relay (`wss://…`).
    pub url: String,
    /// The host to reach.
    pub host_id: HostId,
    /// An opaque token for a relay that requires one.
    pub token: Option<Vec<u8>>,
}

impl Dialer for RelayDialer {
    fn dial(&self) -> DialFuture {
        let (url, id, token) = (self.url.clone(), self.host_id, self.token.clone());
        Box::pin(async move { dial(&url, &Target::Host(id), token).await })
    }
}

/// Why the client is not online, in terms a person can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The relay cannot be reached (network, DNS, TLS, the service is down).
    RelayUnreachable,
    /// The relay is fine but the host is not connected to it: the other
    /// computer is off, asleep, or not sharing.
    HostOffline,
    /// The host refused this device: it was revoked or never paired.
    Revoked,
    /// The host speaks another protocol version.
    VersionMismatch,
    /// Anything else (a broken connection, a failed handshake).
    Other,
}

/// The state of the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// Connecting for the first time.
    Connecting,
    /// Connected and authenticated.
    Online,
    /// Not connected; trying again (or, for [`Failure::Revoked`] and
    /// [`Failure::VersionMismatch`], waiting to be told to).
    Offline {
        /// What went wrong.
        failure: Failure,
        /// A sentence for a person.
        reason: String,
        /// Failed attempts in a row.
        attempt: u32,
    },
    /// Closed by the application.
    Closed,
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ClientError {
    /// Not connected, and not connecting soon enough.
    #[error("not connected: {0}")]
    Offline(String),
    /// The connection dropped while the request was waiting.
    #[error("the connection dropped")]
    Disconnected,
    /// The host answered with an error.
    #[error("{}", .0.message)]
    Remote(WireError),
    /// The host answered with something unexpected.
    #[error("unexpected answer from the host")]
    Protocol,
    /// The client was closed.
    #[error("closed")]
    Closed,
}

/// What a terminal stream delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// Output bytes, in order, each exactly once.
    Data(Vec<u8>),
    /// The host no longer had bytes this client missed: the screen may be
    /// incomplete; start from a clean slate.
    Gap,
    /// The program ended.
    Exited(Exit),
    /// The terminal no longer exists on the host (reaped, or the host was
    /// restarted).
    Gone,
    /// The connection dropped; the client is reconnecting and will re-attach.
    Disconnected,
    /// The connection is back and the terminal was re-attached.
    Reconnected,
}

/// How a client connects.
pub struct ClientConfig {
    /// This device's keys.
    pub identity: Arc<Identity>,
    /// The host's pinned static key.
    pub host_key: [u8; 32],
    /// Where pipes come from.
    pub dialer: Arc<dyn Dialer>,
    /// The name shown on the host.
    pub device_name: String,
    /// Reconnection delays: first and longest.
    pub backoff: (Duration, Duration),
    /// How often to probe an idle connection.
    pub ping_every: Duration,
    /// An opaque token for a host or relay that wants one.
    pub token: Option<Vec<u8>>,
}

impl ClientConfig {
    /// Defaults around the required parts.
    pub fn new(
        identity: Arc<Identity>,
        host_key: [u8; 32],
        dialer: Arc<dyn Dialer>,
        device_name: impl Into<String>,
    ) -> Self {
        Self {
            identity,
            host_key,
            dialer,
            device_name: device_name.into(),
            backoff: (Duration::from_secs(1), Duration::from_secs(30)),
            ping_every: Duration::from_secs(20),
            token: None,
        }
    }
}

type Reply = oneshot::Sender<Result<Message, ClientError>>;

enum Command {
    Request {
        id: u64,
        message: Message,
        stream: Option<mpsc::UnboundedSender<PtyEvent>>,
        reply: Reply,
    },
    Send(Message),
    Reconnect,
    Close,
}

/// A live terminal on the host.
#[derive(Debug)]
pub struct PtyStream {
    /// The terminal's id on the host.
    pub pty: u64,
    /// Its output and lifecycle.
    pub events: mpsc::UnboundedReceiver<PtyEvent>,
    commands: mpsc::Sender<Command>,
}

impl PtyStream {
    /// Sends keystrokes.
    pub fn write(&self, bytes: Vec<u8>) {
        for chunk in bytes.chunks(leon_wire::MAX_PTY_CHUNK) {
            let _ = self.commands.try_send(Command::Send(Message::PtyData {
                pty: self.pty,
                offset: 0,
                bytes: chunk.to_vec(),
            }));
        }
    }

    /// Resizes the terminal.
    pub fn resize(&self, size: Grid) {
        let _ = self.commands.try_send(Command::Send(Message::PtyResize {
            pty: self.pty,
            size,
        }));
    }

    /// Hangs the program up.
    pub fn close(&self) {
        let _ = self
            .commands
            .try_send(Command::Send(Message::PtyClose { pty: self.pty }));
    }

    /// A handle that can send while the events are read elsewhere.
    pub fn handle(&self) -> PtyHandle {
        PtyHandle {
            pty: self.pty,
            commands: self.commands.clone(),
        }
    }
}

/// The sending half of a [`PtyStream`].
#[derive(Debug, Clone)]
pub struct PtyHandle {
    pty: u64,
    commands: mpsc::Sender<Command>,
}

impl PtyHandle {
    /// Sends keystrokes.
    pub fn write(&self, bytes: Vec<u8>) {
        for chunk in bytes.chunks(leon_wire::MAX_PTY_CHUNK) {
            let _ = self.commands.try_send(Command::Send(Message::PtyData {
                pty: self.pty,
                offset: 0,
                bytes: chunk.to_vec(),
            }));
        }
    }

    /// Resizes the terminal.
    pub fn resize(&self, size: Grid) {
        let _ = self.commands.try_send(Command::Send(Message::PtyResize {
            pty: self.pty,
            size,
        }));
    }

    /// Hangs the program up.
    pub fn close(&self) {
        let _ = self
            .commands
            .try_send(Command::Send(Message::PtyClose { pty: self.pty }));
    }
}

/// A durable connection to one host.
#[derive(Clone)]
pub struct Client {
    commands: mpsc::Sender<Command>,
    state: watch::Receiver<ConnState>,
    ids: Arc<AtomicU64>,
    host_name: watch::Receiver<Option<String>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("state", &*self.state.borrow())
            .finish()
    }
}

struct LivePty {
    tx: mpsc::UnboundedSender<PtyEvent>,
    next_offset: u64,
    exited: bool,
}

impl Client {
    /// Starts connecting in the background.
    pub fn start(config: ClientConfig) -> Self {
        let (commands, command_rx) = mpsc::channel(256);
        let (state_tx, state) = watch::channel(ConnState::Connecting);
        let (name_tx, host_name) = watch::channel(None);
        tokio::spawn(actor(config, command_rx, state_tx, name_tx));
        Self {
            commands,
            state,
            ids: Arc::new(AtomicU64::new(1)),
            host_name,
        }
    }

    /// The connection state.
    pub fn state(&self) -> ConnState {
        self.state.borrow().clone()
    }

    /// The connection state, as changes.
    pub fn watch(&self) -> watch::Receiver<ConnState> {
        self.state.clone()
    }

    /// The name the host gave itself, once connected.
    pub fn host_name(&self) -> Option<String> {
        self.host_name.borrow().clone()
    }

    /// Waits until online, up to `limit`.
    pub async fn wait_online(&self, limit: Duration) -> Result<(), ClientError> {
        let mut state = self.state.clone();
        let wait = async {
            loop {
                match &*state.borrow_and_update() {
                    ConnState::Online => return Ok(()),
                    ConnState::Closed => return Err(ClientError::Closed),
                    ConnState::Offline {
                        reason,
                        failure: Failure::Revoked | Failure::VersionMismatch,
                        ..
                    } => return Err(ClientError::Offline(reason.clone())),
                    _ => {}
                }
                if state.changed().await.is_err() {
                    return Err(ClientError::Closed);
                }
            }
        };
        match tokio::time::timeout(limit, wait).await {
            Ok(result) => result,
            Err(_) => Err(ClientError::Offline(match self.state() {
                ConnState::Offline { reason, .. } => reason,
                _ => "still connecting".into(),
            })),
        }
    }

    async fn request(
        &self,
        make: impl FnOnce(u64) -> Message,
        stream: Option<mpsc::UnboundedSender<PtyEvent>>,
    ) -> Result<Message, ClientError> {
        self.wait_online(Duration::from_secs(15)).await?;
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Request {
                id,
                message: make(id),
                stream,
                reply,
            })
            .await
            .map_err(|_| ClientError::Closed)?;
        match answer.await {
            Ok(Ok(Message::Error { error, .. })) => Err(ClientError::Remote(error)),
            Ok(result) => result,
            Err(_) => Err(ClientError::Disconnected),
        }
    }

    /// Runs a command on the host.
    pub async fn exec(
        &self,
        spec: ExecSpec,
        timeout_ms: Option<u32>,
    ) -> Result<ExecOutput, ClientError> {
        match self
            .request(
                |id| Message::Exec {
                    id,
                    spec,
                    timeout_ms,
                },
                None,
            )
            .await?
        {
            Message::ExecOutput { output, .. } => Ok(output),
            _ => Err(ClientError::Protocol),
        }
    }

    /// Opens a terminal on the host.
    pub async fn pty_open(
        &self,
        spec: ExecSpec,
        size: Grid,
        label: &str,
    ) -> Result<PtyStream, ClientError> {
        let (tx, events) = mpsc::unbounded_channel();
        let label = label.to_owned();
        match self
            .request(
                |id| Message::PtyOpen {
                    id,
                    spec,
                    size,
                    label,
                },
                Some(tx),
            )
            .await?
        {
            Message::PtyOpened { pty, .. } => Ok(PtyStream {
                pty,
                events,
                commands: self.commands.clone(),
            }),
            _ => Err(ClientError::Protocol),
        }
    }

    /// Attaches to a terminal that is already running on the host, receiving
    /// its output from `from_offset` on.
    pub async fn pty_attach(&self, pty: u64, from_offset: u64) -> Result<PtyStream, ClientError> {
        let (tx, events) = mpsc::unbounded_channel();
        match self
            .request(
                |id| Message::PtyAttach {
                    id,
                    pty,
                    from_offset,
                },
                Some(tx),
            )
            .await?
        {
            Message::PtyAttached { pty, .. } => Ok(PtyStream {
                pty,
                events,
                commands: self.commands.clone(),
            }),
            _ => Err(ClientError::Protocol),
        }
    }

    /// The host's terminals.
    pub async fn pty_list(&self) -> Result<Vec<PtyInfo>, ClientError> {
        match self.request(|id| Message::PtyList { id }, None).await? {
            Message::PtyListing { ptys, .. } => Ok(ptys),
            _ => Err(ClientError::Protocol),
        }
    }

    /// Tries again now, skipping any backoff or terminal failure.
    pub fn reconnect_now(&self) {
        let _ = self.commands.try_send(Command::Reconnect);
    }

    /// Closes the connection for good.
    pub fn close(&self) {
        let _ = self.commands.try_send(Command::Close);
    }
}

fn classify(error: &LinkError) -> (Failure, String) {
    match error {
        LinkError::Rejected => (
            Failure::Revoked,
            "the other computer does not accept this device (it was revoked, or the pairing was removed there)".into(),
        ),
        LinkError::Timeout => (Failure::Other, "the other computer did not answer in time".into()),
        other => (Failure::Other, other.to_string()),
    }
}

fn classify_dial(error: &DialError) -> (Failure, String) {
    match error {
        DialError::Unreachable(detail) => (Failure::RelayUnreachable, detail.clone()),
        DialError::Refused(e) if e.code == RelayErrorCode::HostNotFound => (
            Failure::HostOffline,
            "the other computer is not connected to the relay (it is off, asleep, or not sharing)"
                .into(),
        ),
        DialError::Refused(e) => (Failure::Other, e.message.clone()),
        DialError::Protocol => (
            Failure::Other,
            "the relay did not follow its protocol".into(),
        ),
    }
}

async fn establish(config: &ClientConfig) -> Result<(SecureChannel, String), (Failure, String)> {
    let pipe = config.dialer.dial().await.map_err(|e| classify_dial(&e))?;
    let mut channel = connect(pipe, &config.identity, &config.host_key)
        .await
        .map_err(|e| classify(&e))?;
    channel
        .send(&Message::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: env!("CARGO_PKG_VERSION").into(),
            device_name: config.device_name.clone(),
            token: config.token.clone(),
        })
        .await
        .map_err(|e| classify(&e))?;
    match tokio::time::timeout(Duration::from_secs(10), channel.recv()).await {
        Ok(Ok(Some(Message::Hello {
            protocol,
            device_name,
            ..
        }))) if protocol == PROTOCOL_VERSION => Ok((channel, device_name)),
        Ok(Ok(Some(Message::Hello { .. }))) => Err((
            Failure::VersionMismatch,
            "the other computer runs an incompatible version of Leon".into(),
        )),
        Ok(Ok(Some(Message::Error { error, .. }))) if error.code == ErrorCode::Unsupported => {
            Err((
                Failure::VersionMismatch,
                "the other computer runs an incompatible version of Leon".into(),
            ))
        }
        Ok(Ok(Some(Message::Error { error, .. }))) if error.code == ErrorCode::Forbidden => {
            Err((Failure::Revoked, error.message))
        }
        // The host closes without answering a device it does not know.
        Ok(Ok(None)) => Err((
            Failure::Revoked,
            "the other computer closed the connection (it may have revoked this device)".into(),
        )),
        _ => Err((Failure::Other, "the other computer did not greet us".into())),
    }
}

async fn actor(
    config: ClientConfig,
    mut commands: mpsc::Receiver<Command>,
    state: watch::Sender<ConnState>,
    host_name: watch::Sender<Option<String>>,
) {
    let mut ptys: HashMap<u64, LivePty> = HashMap::new();
    let mut attempt = 0u32;
    let mut delay = config.backoff.0;
    let mut halted = false;
    loop {
        if !halted {
            let established = tokio::select! {
                result = establish(&config) => Some(result),
                command = commands.recv() => match command {
                    None | Some(Command::Close) => break,
                    Some(Command::Request { reply, .. }) => {
                        let _ = reply.send(Err(ClientError::Disconnected));
                        None
                    }
                    Some(_) => None,
                },
            };
            match established {
                Some(Ok((channel, name))) => {
                    attempt = 0;
                    delay = config.backoff.0;
                    host_name.send_replace(Some(name));
                    state.send_replace(ConnState::Online);
                    match run_connection(&config, channel, &mut commands, &mut ptys).await {
                        Ended::Closed => break,
                        Ended::Dropped(failure, reason) => {
                            attempt += 1;
                            halted = matches!(failure, Failure::Revoked | Failure::VersionMismatch);
                            state.send_replace(ConnState::Offline {
                                failure,
                                reason,
                                attempt,
                            });
                        }
                    }
                }
                Some(Err((failure, reason))) => {
                    attempt += 1;
                    halted = matches!(failure, Failure::Revoked | Failure::VersionMismatch);
                    state.send_replace(ConnState::Offline {
                        failure,
                        reason,
                        attempt,
                    });
                }
                None => continue,
            }
        }
        // Wait out the backoff (or, when halted, a nudge) while answering
        // commands that cannot be served.
        let sleep = tokio::time::sleep(if halted {
            Duration::from_secs(86_400)
        } else {
            delay
        });
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                command = commands.recv() => match command {
                    None | Some(Command::Close) => {
                        state.send_replace(ConnState::Closed);
                        return;
                    }
                    Some(Command::Reconnect) => { halted = false; break; }
                    Some(Command::Request { reply, .. }) => {
                        let _ = reply.send(Err(ClientError::Disconnected));
                    }
                    Some(Command::Send(_)) => {}
                },
            }
        }
        delay = (delay * 2).min(config.backoff.1);
    }
    state.send_replace(ConnState::Closed);
}

enum Ended {
    Closed,
    Dropped(Failure, String),
}

async fn run_connection(
    config: &ClientConfig,
    mut channel: SecureChannel,
    commands: &mut mpsc::Receiver<Command>,
    ptys: &mut HashMap<u64, LivePty>,
) -> Ended {
    let mut pending: HashMap<u64, (Reply, Option<mpsc::UnboundedSender<PtyEvent>>)> =
        HashMap::new();
    let mut reattach: HashMap<u64, u64> = HashMap::new();
    let mut next_internal = u64::MAX / 2;
    // Re-attach what was live before the connection dropped.
    for (pty, live) in ptys.iter().filter(|(_, l)| !l.exited) {
        next_internal += 1;
        reattach.insert(next_internal, *pty);
        let attach = Message::PtyAttach {
            id: next_internal,
            pty: *pty,
            from_offset: live.next_offset,
        };
        if channel.send(&attach).await.is_err() {
            return Ended::Dropped(Failure::Other, "the connection dropped".into());
        }
    }
    let mut ping = tokio::time::interval(config.ping_every);
    ping.tick().await;
    let mut waiting_pong = false;
    let ended = loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Close) => break Ended::Closed,
                Some(Command::Reconnect) => break Ended::Dropped(Failure::Other, "reconnecting".into()),
                Some(Command::Send(message)) => {
                    if channel.send(&message).await.is_err() {
                        break Ended::Dropped(Failure::Other, "the connection dropped".into());
                    }
                }
                Some(Command::Request { id, message, stream, reply }) => {
                    pending.insert(id, (reply, stream));
                    if channel.send(&message).await.is_err() {
                        break Ended::Dropped(Failure::Other, "the connection dropped".into());
                    }
                }
            },
            incoming = channel.recv() => {
                let message = match incoming {
                    Ok(Some(message)) => message,
                    Ok(None) => break Ended::Dropped(Failure::Other, "the other computer closed the connection".into()),
                    Err(error) => break Ended::Dropped(Failure::Other, error.to_string()),
                };
                waiting_pong = false;
                if let Some(ended) = on_message(message, &mut pending, &mut reattach, ptys) {
                    break ended;
                }
            }
            _ = ping.tick() => {
                if waiting_pong {
                    break Ended::Dropped(Failure::Other, "the connection went quiet".into());
                }
                waiting_pong = true;
                if channel.send(&Message::Ping { nonce: 0 }).await.is_err() {
                    break Ended::Dropped(Failure::Other, "the connection dropped".into());
                }
            }
        }
    };
    for (_, (reply, _)) in pending.drain() {
        let _ = reply.send(Err(ClientError::Disconnected));
    }
    if matches!(ended, Ended::Dropped(..)) {
        ptys.retain(|_, live| live.tx.send(PtyEvent::Disconnected).is_ok() || live.exited);
    }
    ended
}

fn on_message(
    message: Message,
    pending: &mut HashMap<u64, (Reply, Option<mpsc::UnboundedSender<PtyEvent>>)>,
    reattach: &mut HashMap<u64, u64>,
    ptys: &mut HashMap<u64, LivePty>,
) -> Option<Ended> {
    match message {
        Message::PtyData { pty, offset, bytes } => {
            if let Some(live) = ptys.get_mut(&pty) {
                let end = offset + bytes.len() as u64;
                if end <= live.next_offset {
                    return None; // already seen
                }
                if offset > live.next_offset {
                    let _ = live.tx.send(PtyEvent::Gap);
                }
                let skip = live.next_offset.saturating_sub(offset) as usize;
                live.next_offset = end;
                let _ = live
                    .tx
                    .send(PtyEvent::Data(bytes[skip.min(bytes.len())..].to_vec()));
            }
        }
        Message::PtyExited { pty, exit } => {
            if let Some(live) = ptys.get_mut(&pty) {
                live.exited = true;
                let _ = live.tx.send(PtyEvent::Exited(exit));
            }
        }
        Message::PtyOpened { id, pty } => {
            if let Some((reply, stream)) = pending.remove(&id) {
                if let Some(tx) = stream {
                    ptys.insert(
                        pty,
                        LivePty {
                            tx,
                            next_offset: 0,
                            exited: false,
                        },
                    );
                }
                let _ = reply.send(Ok(Message::PtyOpened { id, pty }));
            }
        }
        Message::PtyAttached {
            id,
            pty,
            replay_from,
            gap,
            end_offset,
            size,
            exit,
        } => {
            if let Some(pty_id) = reattach.remove(&id) {
                if let Some(live) = ptys.get_mut(&pty_id) {
                    if gap {
                        let _ = live.tx.send(PtyEvent::Gap);
                    }
                    live.next_offset = replay_from;
                    let _ = live.tx.send(PtyEvent::Reconnected);
                }
            } else if let Some((reply, stream)) = pending.remove(&id) {
                if let Some(tx) = stream {
                    if gap {
                        let _ = tx.send(PtyEvent::Gap);
                    }
                    ptys.insert(
                        pty,
                        LivePty {
                            tx,
                            next_offset: replay_from,
                            exited: false,
                        },
                    );
                }
                let _ = reply.send(Ok(Message::PtyAttached {
                    id,
                    pty,
                    replay_from,
                    gap,
                    end_offset,
                    size,
                    exit,
                }));
            }
        }
        Message::Error {
            id: Some(id),
            error,
        } => {
            if let Some(pty) = reattach.remove(&id) {
                if let Some(live) = ptys.remove(&pty) {
                    let _ = live.tx.send(PtyEvent::Gone);
                }
            } else if let Some((reply, _)) = pending.remove(&id) {
                let _ = reply.send(Ok(Message::Error {
                    id: Some(id),
                    error,
                }));
            }
        }
        Message::Error { id: None, error } => {
            // A session-level refusal: revoked, or too slow to keep up.
            return Some(match error.code {
                ErrorCode::Forbidden => Ended::Dropped(Failure::Revoked, error.message),
                ErrorCode::Unsupported => Ended::Dropped(Failure::VersionMismatch, error.message),
                _ => Ended::Dropped(Failure::Other, error.message),
            });
        }
        Message::ExecOutput { id, output } => {
            if let Some((reply, _)) = pending.remove(&id) {
                let _ = reply.send(Ok(Message::ExecOutput { id, output }));
            }
        }
        Message::PtyListing { id, ptys: list } => {
            if let Some((reply, _)) = pending.remove(&id) {
                let _ = reply.send(Ok(Message::PtyListing { id, ptys: list }));
            }
        }
        Message::Pong { .. } | Message::Ping { .. } => {}
        _ => {}
    }
    None
}
