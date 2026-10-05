//! A relay on localhost, a host and clients, all in this process.
//!
//! The relay is the public test double of `leon-link` (a plain forwarder);
//! everything above it is the real code: pairing, the encrypted channel,
//! commands, terminals, re-attach, revocation, reconnection.

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use leon_host::{Backoff, Host, HostConfig, RelayState};
use leon_link::client::{
    Client, ClientConfig, ConnState, Failure, PtyEvent, PtyStream, RelayDialer,
};
use leon_link::relay_client::{dial, Target};
use leon_link::test_support::TestRelay;
use leon_link::{pair_as_client, Identity, PairError, PairingCode};
use leon_wire::{ExecSpec, Grid};

const WAIT: Duration = Duration::from_secs(30);

async fn until<F: FnMut() -> bool>(what: &str, mut condition: F) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn grid(cols: u16, rows: u16) -> Grid {
    Grid {
        cols,
        rows,
        cell_width: 0,
        cell_height: 0,
    }
}

fn shell() -> ExecSpec {
    ExecSpec {
        program: "/bin/sh".into(),
        args: vec![],
        env: vec![
            ("PS1".into(), "LEON> ".into()),
            ("ENV".into(), "/dev/null".into()),
        ],
        cwd: None,
    }
}

struct World {
    relay: Option<TestRelay>,
    relay_url: String,
    host: Host,
    host_identity: Arc<Identity>,
    client_identity: Arc<Identity>,
    client: Client,
    _dir: tempfile::TempDir,
}

fn host_config(url: &str) -> HostConfig {
    let mut config = HostConfig::new(url, "build-box");
    config.backoff = Backoff {
        initial: Duration::from_millis(20),
        max: Duration::from_millis(200),
    };
    config.registry_poll = Duration::from_millis(50);
    config
}

fn client_for(url: &str, host: &Identity, me: &Arc<Identity>) -> Client {
    let mut config = ClientConfig::new(
        me.clone(),
        host.static_public(),
        Arc::new(RelayDialer {
            url: url.to_owned(),
            host_id: host.host_id(),
            token: None,
        }),
        "Ana's laptop",
    );
    config.backoff = (Duration::from_millis(20), Duration::from_millis(200));
    Client::start(config)
}

/// A relay, a host, and a client that has paired with it with a code.
async fn paired() -> World {
    let relay = TestRelay::start().await;
    let url = relay.url();
    paired_on(relay, url).await
}

/// [`paired`] on a relay that is already running, at `url`.
async fn paired_on(relay: TestRelay, url: String) -> World {
    let dir = tempfile::tempdir().unwrap();
    let host_identity = Arc::new(Identity::load_or_create(&dir.path().join("host")).unwrap());
    let client_identity = Arc::new(Identity::generate());
    let host = Host::start(
        host_config(&url),
        host_identity.clone(),
        Some(dir.path().join("devices.json")),
    )
    .unwrap();
    let mut state = host.watch_relay();
    while *state.borrow_and_update() != RelayState::Online {
        state.changed().await.unwrap();
    }
    let info = host.new_pairing_code().await;
    let code = PairingCode::parse(&info.code).unwrap();
    let pipe = dial(&url, &Target::Room(info.room.clone()), None)
        .await
        .unwrap();
    let paired = pair_as_client(pipe, &client_identity, &code, "Ana's laptop")
        .await
        .unwrap();
    assert_eq!(paired.host_id, host_identity.host_id());
    assert_eq!(paired.host_key, host_identity.static_public());
    let client = client_for(&url, &host_identity, &client_identity);
    client.wait_online(WAIT).await.unwrap();
    World {
        relay: Some(relay),
        relay_url: url,
        host,
        host_identity,
        client_identity,
        client,
        _dir: dir,
    }
}

/// A terminal stream with a buffer that `expect` searches, so output that
/// arrives in the same chunk as a match is not lost to the next search.
struct Screen {
    stream: PtyStream,
    buf: String,
    pos: usize,
}

impl Screen {
    fn new(stream: PtyStream) -> Self {
        Self {
            stream,
            buf: String::new(),
            pos: 0,
        }
    }

    fn write(&self, text: &str) {
        self.stream.write(text.as_bytes().to_vec());
    }

    fn resize(&self, size: Grid) {
        self.stream.resize(size);
    }

    /// Bytes received so far (what a resuming client would pass as its offset).
    fn received(&self) -> u64 {
        self.buf.len() as u64
    }

    /// Waits for `needle` after the last match; returns the text up to and
    /// including it.
    async fn expect(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(at) = self.buf[self.pos..].find(needle) {
                let end = self.pos + at + needle.len();
                let text = self.buf[self.pos..end].to_owned();
                self.pos = end;
                return text;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.stream.events.recv()).await {
                Ok(Some(PtyEvent::Data(bytes))) => {
                    self.buf.push_str(&String::from_utf8_lossy(&bytes))
                }
                Ok(Some(_)) => {}
                _ => panic!(
                    "timed out waiting for {needle:?}; unread: {:?}",
                    &self.buf[self.pos..]
                ),
            }
        }
    }
}

async fn open(client: &Client, label: &str) -> Screen {
    Screen::new(client.pty_open(shell(), grid(80, 24), label).await.unwrap())
}

#[tokio::test]
async fn pairing_with_the_code_authorises_the_device_and_burns_the_code() {
    let world = paired().await;
    let devices = world.host.devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Ana's laptop");
    assert_eq!(
        devices[0].public_key(),
        world.client_identity.static_public()
    );
    // The code was single use.
    assert!(world.host.pairing().is_none());
    let info = world.host.new_pairing_code().await;
    assert_ne!(info.code, "");
}

#[tokio::test]
async fn a_wrong_code_is_refused_and_five_wrong_codes_burn_it() {
    let world = paired().await;
    let info = world.host.new_pairing_code().await;
    let stranger = Identity::generate();
    let wrong = PairingCode::generate();
    // Same room, wrong secret.
    let typed = PairingCode::parse(&format!(
        "{}{}",
        info.room,
        &wrong.display().replace('-', "")[4..]
    ))
    .unwrap();
    for _ in 0..5 {
        let pipe = dial(&world.relay_url, &Target::Room(info.room.clone()), None)
            .await
            .unwrap();
        assert!(pair_as_client(pipe, &stranger, &typed, "x").await.is_err());
    }
    let pipe = dial(&world.relay_url, &Target::Room(info.room.clone()), None)
        .await
        .unwrap();
    let real = PairingCode::parse(&info.code).unwrap();
    assert_eq!(
        pair_as_client(pipe, &stranger, &real, "x")
            .await
            .unwrap_err(),
        PairError::Burned
    );
    assert!(world.host.pairing().is_none());
    assert_eq!(world.host.devices().len(), 1, "the stranger never got in");
}

#[tokio::test]
async fn a_command_runs_on_the_host_and_its_output_comes_back() {
    let world = paired().await;
    let spec = ExecSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "echo $LEON_X; echo oops >&2; exit 2".into()],
        env: vec![("LEON_X".into(), "hello".into())],
        cwd: None,
    };
    let out = world.client.exec(spec, None).await.unwrap();
    assert_eq!(out.stdout, b"hello\n");
    assert_eq!(out.stderr, b"oops\n");
    assert_eq!(out.status, Some(2));
}

/// The whole path over TLS: a `wss://` relay with a throwaway certificate for
/// `localhost` (trusted through the test seam only), pairing with a code, a
/// command, and a terminal that echoes. The first real relay panicked here, in
/// the TLS setup, while every other test used `ws://`.
#[tokio::test]
async fn pairing_a_command_and_a_terminal_work_over_a_wss_relay() {
    let relay = TestRelay::start_tls_self_signed().await;
    let url = relay.tls_url();
    let world = paired_on(relay, url).await;
    assert!(world.relay_url.starts_with("wss://"));
    assert_eq!(world.host.devices().len(), 1);
    let out = world
        .client
        .exec(
            ExecSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "echo over-tls".into()],
                env: vec![],
                cwd: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(out.stdout, b"over-tls\n");
    let mut pty = open(&world.client, "shell").await;
    pty.expect("LEON> ").await;
    pty.write("echo tls-echo-$((20+22))\n");
    pty.expect("tls-echo-42").await;
}

#[tokio::test]
async fn a_terminal_takes_input_shows_output_follows_resizes_and_reports_the_exit() {
    let world = paired().await;
    let mut pty = open(&world.client, "shell").await;
    pty.expect("LEON> ").await;
    pty.write("stty size\n");
    pty.expect("24 80").await;
    pty.resize(grid(100, 30));
    pty.write("stty size\n");
    pty.expect("30 100").await;
    pty.write("exit 3\n");
    loop {
        let event = tokio::time::timeout(WAIT, pty.stream.events.recv())
            .await
            .unwrap()
            .unwrap();
        if let PtyEvent::Exited(exit) = event {
            assert_eq!(exit.code, 3);
            break;
        }
    }
}

#[tokio::test]
async fn a_terminal_survives_with_no_client_and_a_new_client_gets_the_missed_output_exactly_once() {
    let world = paired().await;
    let mut pty = open(&world.client, "agent").await;
    pty.expect("LEON> ").await;
    pty.write("echo ONE\n");
    pty.expect("\r\nONE\r\n").await;
    pty.expect("LEON> ").await;
    // A command that prints later; the client leaves right after typing it.
    pty.write("sleep 1; echo MISSED\n");
    pty.expect("echo MISSED\r\n").await;
    let pty_id = pty.stream.pty;
    let offset = pty.received();
    drop(pty);
    world.client.close();
    until("the session to end", || world.host.status().sessions == 0).await;
    assert_eq!(
        world.host.status().terminals,
        1,
        "the terminal keeps running with zero clients"
    );

    let again = client_for(
        &world.relay_url,
        &world.host_identity,
        &world.client_identity,
    );
    again.wait_online(WAIT).await.unwrap();
    let listed = again.pty_list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].label, "agent");
    let mut resumed = Screen::new(again.pty_attach(pty_id, offset).await.unwrap());
    let rest = resumed.expect("MISSED\r\n").await;
    assert!(
        !rest.contains("ONE"),
        "nothing before the offset is repeated: {rest:?}"
    );
    resumed.expect("LEON> ").await;
    resumed.write("echo TWO\n");
    let rest = resumed.expect("\r\nTWO\r\n").await;
    assert!(
        !rest.contains("MISSED"),
        "and the missed output is delivered once: {rest:?}"
    );
}

#[tokio::test]
async fn a_revoked_device_is_cut_off_mid_session_and_then_refused() {
    let world = paired().await;
    let mut pty = open(&world.client, "shell").await;
    pty.expect("LEON> ").await;
    world.host.revoke("Ana's laptop").unwrap();
    until("the client to learn it was revoked", || {
        matches!(
            world.client.state(),
            ConnState::Offline {
                failure: Failure::Revoked,
                ..
            }
        )
    })
    .await;
    let error = world
        .client
        .exec(ExecSpec::default(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, leon_link::client::ClientError::Offline(_)));
    // A brand-new connection from the same device is refused before any command.
    let fresh = client_for(
        &world.relay_url,
        &world.host_identity,
        &world.client_identity,
    );
    until("the fresh client to be refused", || {
        matches!(
            fresh.state(),
            ConnState::Offline {
                failure: Failure::Revoked,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn a_device_that_never_paired_is_refused() {
    let world = paired().await;
    let stranger = Arc::new(Identity::generate());
    let client = client_for(&world.relay_url, &world.host_identity, &stranger);
    until("the stranger to be refused", || {
        matches!(
            client.state(),
            ConnState::Offline {
                failure: Failure::Revoked,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn a_host_that_is_not_connected_is_reported_as_offline_not_as_a_relay_failure() {
    let relay = TestRelay::start().await;
    let (host, me) = (Identity::generate(), Arc::new(Identity::generate()));
    let client = client_for(&relay.url(), &host, &me);
    until("the offline report", || {
        matches!(
            client.state(),
            ConnState::Offline {
                failure: Failure::HostOffline,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn an_unreachable_relay_is_reported_with_its_kind() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (host, me) = (Identity::generate(), Arc::new(Identity::generate()));
    let client = client_for(&format!("ws://127.0.0.1:{port}"), &host, &me);
    until("the unreachable report", || {
        matches!(
            client.state(),
            ConnState::Offline {
                failure: Failure::RelayUnreachable,
                ..
            }
        )
    })
    .await;
}

#[tokio::test]
async fn both_sides_survive_a_relay_restart_and_the_terminal_keeps_streaming() {
    let mut world = paired().await;
    let mut pty = open(&world.client, "shell").await;
    pty.expect("LEON> ").await;
    let addr = world.relay.as_ref().unwrap().addr();
    world.relay.take().unwrap().stop().await;
    until("both sides to notice", || {
        !matches!(world.client.state(), ConnState::Online)
            && matches!(world.host.status().relay, RelayState::Offline { .. })
    })
    .await;
    world.relay = Some(TestRelay::start_on(addr).await);
    until("both sides to reconnect", || {
        matches!(world.client.state(), ConnState::Online)
            && world.host.status().relay == RelayState::Online
    })
    .await;
    // The same terminal, the same stream, no new open.
    pty.write("echo AFTER\n");
    pty.expect("AFTER\r\n").await;
    let out = world
        .client
        .exec(
            ExecSpec {
                program: "/bin/echo".into(),
                args: vec!["ok".into()],
                ..ExecSpec::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(out.stdout, b"ok\n");
}

#[tokio::test]
async fn stopping_the_host_hangs_up_its_terminals_and_ends_sessions() {
    let world = paired().await;
    let mut pty = open(&world.client, "shell").await;
    pty.expect("LEON> ").await;
    world.host.stop();
    until("the host to stop", || {
        world.host.status().relay == RelayState::Stopped
    })
    .await;
    until("the client to notice", || {
        !matches!(world.client.state(), ConnState::Online)
    })
    .await;
}
