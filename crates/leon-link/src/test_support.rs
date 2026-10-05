//! Test doubles: a recording tap between two pipe ends and an in-process
//! relay. Only built for tests and with the `test-support` feature; never part
//! of a shipped binary.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::pipe::{Pipe, PIPE_CAPACITY};

#[derive(Default)]
struct TapState {
    messages: Vec<Vec<u8>>,
    corrupt_client_next: bool,
}

/// Everything that crossed a [`tap`].
#[derive(Clone, Default)]
pub struct Transcript(Arc<Mutex<TapState>>);

impl Transcript {
    /// Every message, both directions, in the order they were forwarded.
    pub fn messages(&self) -> Vec<Vec<u8>> {
        self.0.lock().unwrap().messages.clone()
    }

    /// Flips a bit in the next message the client end sends.
    pub fn corrupt_next_from_client(&self) {
        self.0.lock().unwrap().corrupt_client_next = true;
    }
}

/// Two pipe ends joined by a forwarder that records what it forwards, as a
/// relay could.
pub fn tap() -> (Pipe, Pipe, Transcript) {
    let transcript = Transcript::default();
    let (client_tx, mut from_client) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    let (to_client, client_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    let (host_tx, mut from_host) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    let (to_host, host_rx) = mpsc::channel::<Vec<u8>>(PIPE_CAPACITY);
    let log = transcript.clone();
    tokio::spawn(async move {
        while let Some(mut message) = from_client.recv().await {
            {
                let mut state = log.0.lock().unwrap();
                if state.corrupt_client_next && message.len() > 3 {
                    state.corrupt_client_next = false;
                    message[3] ^= 1;
                }
                state.messages.push(message.clone());
            }
            if to_host.send(message).await.is_err() {
                break;
            }
        }
    });
    let log = transcript.clone();
    tokio::spawn(async move {
        while let Some(message) = from_host.recv().await {
            log.0.lock().unwrap().messages.push(message.clone());
            if to_client.send(message).await.is_err() {
                break;
            }
        }
    });
    (
        Pipe::new(client_tx, client_rx),
        Pipe::new(host_tx, host_rx),
        transcript,
    )
}

pub use relay::TestRelay;

/// An in-process relay implementing the public rendezvous specification
/// (`leon_wire::relay`) and nothing more: no limits, no accounts, no logging.
/// It exists so the end-to-end tests of the crates that use this one have
/// something to rendezvous through; the real relay service is not part of
/// this repository.
mod relay {
    #![allow(clippy::result_large_err)]
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use futures_util::{SinkExt, StreamExt};
    use leon_wire::relay::{
        decode_client_side, decode_host_side, encode_client_data, encode_control, encode_host_data,
        registration_message, valid_room, ClientToRelay, HostToRelay, Inbound, RelayError,
        RelayErrorCode, RelayLimits, RelayToClient, RelayToHost,
    };
    use leon_wire::HostId;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::Message;

    enum ToHost {
        Opened { channel: u32, pairing: bool },
        Closed { channel: u32 },
        Data { channel: u32, bytes: Vec<u8> },
    }

    enum ToClient {
        Data(Vec<u8>),
        HostLeft,
    }

    struct HostEntry {
        generation: u64,
        to_host: mpsc::UnboundedSender<ToHost>,
        clients: HashMap<u32, mpsc::UnboundedSender<ToClient>>,
        next_channel: u32,
    }

    #[derive(Default)]
    struct State {
        hosts: HashMap<HostId, HostEntry>,
        rooms: HashMap<String, HostId>,
        generation: u64,
    }

    type Shared = Arc<Mutex<State>>;

    /// A running test relay on `127.0.0.1`.
    pub struct TestRelay {
        addr: SocketAddr,
        tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
        accept: JoinHandle<()>,
    }

    impl TestRelay {
        /// Starts on an ephemeral port.
        pub async fn start() -> Self {
            Self::start_on("127.0.0.1:0".parse().unwrap()).await
        }

        /// Starts on a given address (to come back after a stop).
        pub async fn start_on(addr: SocketAddr) -> Self {
            let listener = TcpListener::bind(addr).await.expect("bind the test relay");
            let addr = listener.local_addr().unwrap();
            let state: Shared = Arc::default();
            let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
            let spawned = tasks.clone();
            let accept = tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let handle = tokio::spawn(connection(stream, state.clone()));
                    spawned.lock().unwrap().push(handle);
                }
            });
            Self {
                addr,
                tasks,
                accept,
            }
        }

        /// The address it listens on.
        pub fn addr(&self) -> SocketAddr {
            self.addr
        }

        /// Its `ws://` base URL.
        pub fn url(&self) -> String {
            format!("ws://{}", self.addr)
        }

        /// Stops it and drops every connection, as a crash would.
        pub async fn stop(self) {
            self.accept.abort();
            let _ = self.accept.await;
            let handles: Vec<_> = self.tasks.lock().unwrap().drain(..).collect();
            for handle in &handles {
                handle.abort();
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }

    fn limits() -> RelayLimits {
        RelayLimits {
            max_message_bytes: 1 << 21,
            max_clients_per_host: u32::MAX,
            idle_timeout_secs: 3600,
        }
    }

    fn refuse(code: RelayErrorCode, text: &str) -> RelayError {
        RelayError {
            code,
            message: text.into(),
        }
    }

    async fn connection(stream: TcpStream, state: Shared) {
        let mut path = String::new();
        let callback =
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                path = request.uri().path().to_owned();
                Ok(response)
            };
        let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
            return;
        };
        if path == "/v1/host" {
            host_connection(socket, state).await;
        } else if let Some(id) = path.strip_prefix("/v1/join/") {
            if let Ok(id) = id.parse::<HostId>() {
                client_connection(socket, state, Some(id), None).await
            }
        } else if let Some(room) = path.strip_prefix("/v1/pair/") {
            if valid_room(room) {
                client_connection(socket, state, None, Some(room.to_owned())).await;
            }
        }
    }

    type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

    async fn host_connection(mut socket: Socket, state: Shared) {
        let nonce: [u8; 32] = {
            use rand_core::RngCore;
            let mut n = [0u8; 32];
            rand_core::OsRng.fill_bytes(&mut n);
            n
        };
        let challenge = encode_control(&RelayToHost::Challenge {
            nonce,
            limits: limits(),
        });
        if socket
            .send(Message::Binary(challenge.into()))
            .await
            .is_err()
        {
            return;
        }
        let Some(Ok(Message::Binary(reply))) = socket.next().await else {
            return;
        };
        let Ok(Inbound::Control(HostToRelay::Register {
            verifying_key,
            signature,
            ..
        })) = decode_host_side::<HostToRelay>(&reply)
        else {
            return;
        };
        if !crate::identity::verify(
            &verifying_key,
            &registration_message(&nonce, &verifying_key),
            &signature,
        ) {
            let error = encode_control(&RelayToHost::Error(refuse(
                RelayErrorCode::Unauthorized,
                "bad signature",
            )));
            let _ = socket.send(Message::Binary(error.into())).await;
            return;
        }
        let host_id = HostId::from_verifying_key(&verifying_key);
        let (to_host, mut from_peers) = mpsc::unbounded_channel::<ToHost>();
        let generation = {
            let mut guard = state.lock().unwrap();
            guard.generation += 1;
            let generation = guard.generation;
            // A newer registration replaces the older one.
            guard.hosts.insert(
                host_id,
                HostEntry {
                    generation,
                    to_host,
                    clients: HashMap::new(),
                    next_channel: 1,
                },
            );
            generation
        };
        let ok = encode_control(&RelayToHost::Registered { host_id });
        if socket.send(Message::Binary(ok.into())).await.is_err() {
            return;
        }
        let (mut sink, mut stream) = socket.split();
        loop {
            tokio::select! {
                peer = from_peers.recv() => {
                    let frame = match peer {
                        Some(ToHost::Opened { channel, pairing }) =>
                            encode_control(&RelayToHost::ChannelOpened { channel, pairing }),
                        Some(ToHost::Closed { channel }) =>
                            encode_control(&RelayToHost::ChannelClosed { channel }),
                        Some(ToHost::Data { channel, bytes }) => encode_host_data(channel, &bytes),
                        // Replaced by a newer registration.
                        None => break,
                    };
                    if sink.send(Message::Binary(frame.into())).await.is_err() { break; }
                }
                received = stream.next() => {
                    let Some(Ok(message)) = received else { break };
                    let Message::Binary(bytes) = message else { continue };
                    match decode_host_side::<HostToRelay>(&bytes) {
                        Ok(Inbound::Data { channel, payload }) => {
                            let guard = state.lock().unwrap();
                            if let Some(client) = guard.hosts.get(&host_id).and_then(|h| h.clients.get(&channel)) {
                                let _ = client.send(ToClient::Data(payload));
                            }
                        }
                        Ok(Inbound::Control(HostToRelay::OpenPairing { room })) => {
                            let reply = {
                                let mut guard = state.lock().unwrap();
                                if !valid_room(&room) {
                                    RelayToHost::Error(refuse(RelayErrorCode::BadRequest, "bad room"))
                                } else if guard.rooms.get(&room).is_some_and(|owner| *owner != host_id) {
                                    RelayToHost::Error(refuse(RelayErrorCode::RoomInUse, "room in use"))
                                } else {
                                    guard.rooms.insert(room.clone(), host_id);
                                    RelayToHost::PairingOpened { room }
                                }
                            };
                            if sink.send(Message::Binary(encode_control(&reply).into())).await.is_err() { break; }
                        }
                        Ok(Inbound::Control(HostToRelay::ClosePairing { room })) => {
                            let mut guard = state.lock().unwrap();
                            if guard.rooms.get(&room) == Some(&host_id) {
                                guard.rooms.remove(&room);
                            }
                        }
                        Ok(Inbound::Control(HostToRelay::CloseChannel { channel })) => {
                            let mut guard = state.lock().unwrap();
                            if let Some(client) = guard.hosts.get_mut(&host_id).and_then(|h| h.clients.remove(&channel)) {
                                let _ = client.send(ToClient::HostLeft);
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
        let mut guard = state.lock().unwrap();
        // Only remove our own registration, not a newer one that replaced it.
        if guard
            .hosts
            .get(&host_id)
            .is_some_and(|h| h.generation == generation)
        {
            if let Some(entry) = guard.hosts.remove(&host_id) {
                for client in entry.clients.values() {
                    let _ = client.send(ToClient::HostLeft);
                }
            }
            guard.rooms.retain(|_, owner| *owner != host_id);
        }
    }

    async fn client_connection(
        mut socket: Socket,
        state: Shared,
        host: Option<HostId>,
        room: Option<String>,
    ) {
        let Some(Ok(Message::Binary(first))) = socket.next().await else {
            return;
        };
        if !matches!(
            decode_client_side::<ClientToRelay>(&first),
            Ok(Inbound::Control(ClientToRelay::Join { .. }))
        ) {
            return;
        }
        let (to_client, mut from_host) = mpsc::unbounded_channel::<ToClient>();
        let joined = {
            let mut guard = state.lock().unwrap();
            let id = host.or_else(|| room.as_ref().and_then(|r| guard.rooms.get(r).copied()));
            id.and_then(|id| {
                guard.hosts.get_mut(&id).map(|entry| {
                    let channel = entry.next_channel;
                    entry.next_channel += 1;
                    entry.clients.insert(channel, to_client);
                    let _ = entry.to_host.send(ToHost::Opened {
                        channel,
                        pairing: room.is_some(),
                    });
                    (id, channel, entry.to_host.clone())
                })
            })
        };
        let Some((id, channel, to_host)) = joined else {
            let error = encode_control(&RelayToClient::Error(refuse(
                RelayErrorCode::HostNotFound,
                "host not connected",
            )));
            let _ = socket.send(Message::Binary(error.into())).await;
            return;
        };
        let ok = encode_control(&RelayToClient::Joined { limits: limits() });
        if socket.send(Message::Binary(ok.into())).await.is_err() {
            let _ = to_host.send(ToHost::Closed { channel });
            return;
        }
        let (mut sink, mut stream) = socket.split();
        loop {
            tokio::select! {
                from = from_host.recv() => match from {
                    Some(ToClient::Data(bytes)) => {
                        if sink.send(Message::Binary(encode_client_data(&bytes).into())).await.is_err() { break; }
                    }
                    Some(ToClient::HostLeft) | None => {
                        let left = encode_control(&RelayToClient::PeerLeft);
                        let _ = sink.send(Message::Binary(left.into())).await;
                        break;
                    }
                },
                received = stream.next() => {
                    let Some(Ok(message)) = received else { break };
                    if let Message::Binary(bytes) = message {
                        if let Ok(Inbound::Data { payload, .. }) = decode_client_side::<RelayToClient>(&bytes) {
                            if to_host.send(ToHost::Data { channel, bytes: payload }).is_err() { break; }
                        }
                    }
                }
            }
        }
        let _ = to_host.send(ToHost::Closed { channel });
        if let Some(entry) = state.lock().unwrap().hosts.get_mut(&id) {
            entry.clients.remove(&channel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{accept, connect};
    use crate::identity::Identity;
    use crate::relay_client::{dial, register, DialError, Target};
    use leon_wire::relay::RelayErrorCode;
    use leon_wire::Message as Wire;

    #[tokio::test]
    async fn a_client_reaches_a_registered_host_and_they_talk_end_to_end() {
        let relay = TestRelay::start().await;
        let (host, client) = (Identity::generate(), Identity::generate());
        let mut link = register(&relay.url(), &host, None).await.unwrap();
        assert_eq!(link.host_id(), host.host_id());

        let pipe = dial(&relay.url(), &Target::Host(host.host_id()), None)
            .await
            .unwrap();
        let host_key = host.static_public();
        let client_key = client.static_public();
        let incoming = link.accept().await.unwrap();
        assert!(!incoming.pairing);
        let (c, h) = tokio::join!(
            connect(pipe, &client, &host_key),
            accept(incoming.pipe, &host, |k| *k == client_key)
        );
        let (mut c, mut h) = (c.unwrap(), h.unwrap());
        c.send(&Wire::Ping { nonce: 3 }).await.unwrap();
        assert_eq!(h.recv().await.unwrap(), Some(Wire::Ping { nonce: 3 }));
        h.send(&Wire::Pong { nonce: 3 }).await.unwrap();
        assert_eq!(c.recv().await.unwrap(), Some(Wire::Pong { nonce: 3 }));
    }

    #[tokio::test]
    async fn dialling_an_absent_host_says_so() {
        let relay = TestRelay::start().await;
        let absent = Identity::generate().host_id();
        let error = dial(&relay.url(), &Target::Host(absent), None)
            .await
            .unwrap_err();
        assert!(matches!(error, DialError::Refused(e) if e.code == RelayErrorCode::HostNotFound));
    }

    #[tokio::test]
    async fn an_unreachable_relay_is_reported_as_unreachable() {
        // A port nothing listens on: bind and release one.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let error = dial(
            &format!("ws://127.0.0.1:{port}"),
            &Target::Room("AB23".into()),
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DialError::Unreachable(_)), "{error:?}");
    }

    #[tokio::test]
    async fn when_the_host_leaves_the_clients_pipe_closes() {
        let relay = TestRelay::start().await;
        let host = Identity::generate();
        let link = register(&relay.url(), &host, None).await.unwrap();
        let mut pipe = dial(&relay.url(), &Target::Host(host.host_id()), None)
            .await
            .unwrap();
        drop(link);
        assert_eq!(pipe.recv().await, None);
    }

    #[tokio::test]
    async fn the_relay_routes_a_pairing_room_to_the_host_that_announced_it() {
        let relay = TestRelay::start().await;
        let host = Identity::generate();
        let mut link = register(&relay.url(), &host, None).await.unwrap();
        link.open_pairing("AB23").await.unwrap();
        let _pipe = dial(&relay.url(), &Target::Room("AB23".into()), None)
            .await
            .unwrap();
        assert!(link.accept().await.unwrap().pairing);
        let other = Identity::generate();
        let second = register(&relay.url(), &other, None).await.unwrap();
        let error = second.open_pairing("AB23").await.unwrap_err();
        assert!(matches!(error, DialError::Refused(e) if e.code == RelayErrorCode::RoomInUse));
    }
}
