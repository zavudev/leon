//! Terminals on machines reached through a relay.
//!
//! [`RoutingBackend`] is the terminal backend of the application: a command
//! with no route starts in a pseudo-terminal on this computer, exactly as
//! before; one with a route (see `leon_remote::relay::route`) becomes a
//! terminal whose program runs on the other computer. The emulator, the view,
//! selection, search and everything else are the same; only the two ends
//! differ:
//!
//! * what the other computer prints is fed to the emulator
//!   ([`RemoteFeed`](leon_term::RemoteFeed)), in order and without duplicates,
//!   also across a reconnection: the client re-attaches from the last offset it
//!   saw;
//! * keystrokes, resizes and a hang-up go to the host. Dropping the terminal
//!   (the window closes, the application quits) only detaches: the program
//!   keeps running on the host and can be attached to again.
//!
//! The same machinery serves the keeper of durable local sessions
//! (`durable.rs`): the holder is a process on this computer, reached through
//! a socket instead of a relay, and the terminal is local in every way that
//! matters (see [`Pump::Keeper`]). What differs is only what is said when the
//! connection drops, and that the keeper is asked now and then who is in
//! front of the terminal, since this process no longer holds the
//! pseudo-terminal to look at itself.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use leon_link::client::{Client, ConnState, PtyEvent, PtyStream};
use leon_pty::{GridSize, SpawnSpec};
use leon_remote::RelayHub;
use leon_term::{
    Backend, HeldForeground, Pty, RemoteFeed, RemoteLink, SpawnError, Terminal, TerminalTheme, Wake,
};
use leon_wire::{ExecSpec, Grid};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

/// How often a keeper is asked who is in front of a terminal.
const PROBE_EVERY: Duration = Duration::from_millis(500);
/// Failed attempts to reach the keeper after which it is taken to be gone
/// (about three seconds with the local client's quick retries).
const KEEPER_GONE_AFTER: u32 = 4;

pub(crate) enum Op {
    Write(Vec<u8>),
    Resize(GridSize),
    Close,
    Detach,
    Terminate,
}

/// Where the program of a terminal is held, for [`pump`].
#[derive(Clone)]
pub(crate) enum Pump {
    /// On another computer, reached through a relay.
    Relay,
    /// By the keeper on this computer.
    Keeper(KeeperPump),
}

/// What a terminal held by the keeper needs besides the connection.
#[derive(Clone)]
pub(crate) struct KeeperPump {
    /// Where the answers to "who is in front" are left for the terminal.
    pub(crate) front: Arc<Mutex<Option<HeldForeground>>>,
    /// Whether the keeper is gone for good: its socket no longer answers. A
    /// keeper that is slow, full or restarting is not; a terminal is never
    /// declared lost on a connection that merely dropped.
    pub(crate) gone: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Hang-ups asked for and not yet handed to the connection.
    pub(crate) closing: Arc<AtomicUsize>,
}

struct Link(mpsc::UnboundedSender<Op>);

impl RemoteLink for Link {
    fn write(&self, bytes: Vec<u8>) {
        let _ = self.0.send(Op::Write(bytes));
    }
    fn resize(&self, size: GridSize) {
        let _ = self.0.send(Op::Resize(size));
    }
    fn close(&self) {
        let _ = self.0.send(Op::Close);
    }
    fn detach(&self) {
        let _ = self.0.send(Op::Detach);
    }
}

/// The link of a terminal the keeper holds: the same ops, and the answers
/// the keeper gave about the foreground.
pub(crate) struct HeldLink {
    pub(crate) ops: mpsc::UnboundedSender<Op>,
    pub(crate) front: Arc<Mutex<Option<HeldForeground>>>,
    /// Counts the hang-ups queued, so a quit can wait until they have been
    /// handed to the connection.
    pub(crate) closing: Arc<AtomicUsize>,
}

impl RemoteLink for HeldLink {
    fn write(&self, bytes: Vec<u8>) {
        let _ = self.ops.send(Op::Write(bytes));
    }
    fn resize(&self, size: GridSize) {
        let _ = self.ops.send(Op::Resize(size));
    }
    fn close(&self) {
        self.closing.fetch_add(1, Ordering::SeqCst);
        if self.ops.send(Op::Close).is_err() {
            self.closing.fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn detach(&self) {
        let _ = self.ops.send(Op::Detach);
    }
    fn is_local(&self) -> bool {
        true
    }
    fn foreground(&self) -> Option<HeldForeground> {
        self.front
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn terminate_foreground(&self) -> bool {
        self.ops.send(Op::Terminate).is_ok()
    }
}

pub(crate) fn grid(size: GridSize) -> Grid {
    Grid {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

/// A short label the host stores with the terminal, so the terminals still
/// running there can be recognised later.
fn label(spec: &SpawnSpec) -> String {
    let program = spec
        .program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&spec.program);
    match &spec.cwd {
        Some(cwd) => format!("{program} @ {cwd}"),
        None => program.to_owned(),
    }
}

/// Everything the window needs to reach other computers and to be reached.
pub struct Remote {
    /// The durable connections to paired hosts.
    pub hub: Arc<RelayHub>,
    /// Sharing this computer.
    pub share: Arc<crate::share::ShareService>,
    /// The engine's runtime, where connections run.
    pub handle: Handle,
}

/// Terminals here and on relay machines.
pub struct RoutingBackend {
    local: Pty,
    hub: Arc<RelayHub>,
    handle: Handle,
}

impl RoutingBackend {
    /// A backend whose relay terminals run on `handle`.
    pub fn new(hub: Arc<RelayHub>, handle: Handle) -> Self {
        Self {
            local: Pty::default(),
            hub,
            handle,
        }
    }
}

impl Backend for RoutingBackend {
    fn spawn(
        &self,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
    ) -> Result<Terminal, SpawnError> {
        let Some(route) = &spec.route else {
            return self.local.spawn(spec, size, theme, wake);
        };
        let client = self
            .hub
            .ensure_route(route)
            .map_err(|error| SpawnError(error.to_string()))?;
        let (ops_tx, ops_rx) = mpsc::unbounded_channel();
        let (terminal, feed) = Terminal::remote(spec, size, theme, wake, Arc::new(Link(ops_tx)));
        self.handle
            .spawn(drive(client, spec.clone(), size, feed, ops_rx));
        Ok(terminal)
    }
}

/// Opens the terminal on the host and connects it to the emulator until it
/// ends or is let go of.
async fn drive(
    client: Client,
    spec: SpawnSpec,
    size: GridSize,
    feed: RemoteFeed,
    ops: mpsc::UnboundedReceiver<Op>,
) {
    let wire = ExecSpec {
        program: spec.program.clone(),
        args: spec.args.clone(),
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
        stdin: None,
    };
    let stream = match client.pty_open(wire, grid(size), &label(&spec)).await {
        Ok(stream) => stream,
        Err(error) => {
            feed.notice(&format!(
                "Cannot start this on the other computer: {error}\n"
            ));
            feed.exit(1, None);
            return;
        }
    };
    pump(client, stream, feed, ops, Pump::Relay).await;
}

/// Whether the keeper is gone: the connection has been down for several
/// attempts **and** the socket no longer answers (or the connection was
/// closed). A connection that dropped while the keeper still answers is the
/// client's to mend, and its terminals are not declared lost on it.
fn keeper_gone(client: &Client, keeper: &KeeperPump) -> bool {
    match client.state() {
        ConnState::Closed => true,
        ConnState::Offline { attempt, .. } => attempt >= KEEPER_GONE_AFTER && (keeper.gone)(),
        _ => false,
    }
}

/// A keeper that is gone took its terminals with it: the terminal says so and
/// ends, marked so that what the window remembers keeps it (see
/// `crate::durable::KEEPER_LOST`).
fn lost_keeper(feed: &RemoteFeed) {
    feed.notice("\n[The keeper of the durable sessions ended, and this terminal with it.]\n");
    feed.exit(1, Some(crate::durable::KEEPER_LOST.to_owned()));
}

/// Connects an open terminal to the emulator until it ends or is let go of.
///
/// What the holder sent before the terminal was attached (`stream.attach`
/// says how much) is replayed into the screen without its bells; what comes
/// after is live. Output is in order and without duplicates also across a
/// reconnection: the client re-attaches from the last offset it saw.
pub(crate) async fn pump(
    client: Client,
    stream: PtyStream,
    feed: RemoteFeed,
    mut ops: mpsc::UnboundedReceiver<Op>,
    kind: Pump,
) {
    let handle = stream.handle();
    let mut replay_left = stream
        .attach
        .as_ref()
        .map_or(0, |a| a.end_offset.saturating_sub(a.replay_from))
        as usize;
    let mut events = stream.events;
    let place = match kind {
        Pump::Relay => "the other computer",
        Pump::Keeper(_) => "the keeper",
    };
    let mut tick = tokio::time::interval(PROBE_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut lost = false;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(PtyEvent::Data(bytes)) => {
                    // The first bytes after an attach are the replay.
                    let past = replay_left.min(bytes.len());
                    if past > 0 {
                        feed.replay(&bytes[..past]);
                        replay_left -= past;
                    }
                    if past < bytes.len() {
                        feed.data(&bytes[past..]);
                    }
                }
                Some(PtyEvent::Gap) => feed.gap(),
                Some(PtyEvent::Exited(exit)) => {
                    feed.exit(exit.code, exit.signal);
                    return;
                }
                Some(PtyEvent::Gone) => {
                    feed.notice(&format!("\n[This terminal no longer exists on {place}.]\n"));
                    // For a keeper it is remembered as lost, so that the next
                    // start says it was no longer held.
                    feed.exit(
                        1,
                        matches!(kind, Pump::Keeper(_))
                            .then(|| crate::durable::KEEPER_LOST.to_owned()),
                    );
                    return;
                }
                Some(PtyEvent::Disconnected) => {
                    lost = true;
                    feed.notice(&match kind {
                        Pump::Relay => "\n[Connection lost. Reconnecting; the program keeps running on the other computer.]\n".to_owned(),
                        Pump::Keeper(_) => "\n[Connection to the keeper lost. Reconnecting.]\n".to_owned(),
                    });
                }
                Some(PtyEvent::Reconnected) => {
                    lost = false;
                    feed.notice("[Reconnected.]\n");
                }
                Some(PtyEvent::Foreground(report)) => {
                    if let Pump::Keeper(KeeperPump { front, .. }) = &kind {
                        let now = Some(HeldForeground {
                            pid: report.pid,
                            shell_in_front: report.shell_in_front,
                            command: report.command,
                        });
                        let mut known = front
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        // Nothing prints when an agent starts or returns to
                        // the shell: the window is woken to look again.
                        if *known != now {
                            *known = now;
                            drop(known);
                            feed.changed();
                        }
                    }
                }
                // The connection was closed under this terminal: for a keeper
                // that is as good as gone.
                None if matches!(kind, Pump::Keeper(_)) => {
                    lost_keeper(&feed);
                    return;
                }
                None => return,
            },
            op = ops.recv() => match op {
                // Keystrokes and hang-ups wait for room in the connection's
                // queue instead of being dropped when it is full.
                Some(Op::Write(bytes)) => {
                    handle.write_wait(bytes).await;
                }
                Some(Op::Resize(size)) => handle.resize(grid(size)),
                Some(Op::Close) => {
                    handle.close_wait().await;
                    if let Pump::Keeper(keeper) = &kind {
                        keeper.closing.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                Some(Op::Terminate) => {
                    handle.terminate_foreground_wait().await;
                }
                Some(Op::Detach) | None => return,
            },
            _ = tick.tick(), if matches!(kind, Pump::Keeper(_)) => {
                if lost {
                    // A keeper that does not come back took its terminals
                    // with it (it was killed, or the computer restarted).
                    if let Pump::Keeper(keeper) = &kind {
                        if keeper_gone(&client, keeper) {
                            lost_keeper(&feed);
                            return;
                        }
                    }
                } else {
                    handle.probe();
                }
            }
        }
    }
}

#[cfg(all(test, leon_posix_tests))]
mod tests {
    use super::*;
    use leon_core::{Machine, MachineId, MachineKind};
    use leon_host::{Host, HostConfig, RelayState};
    use leon_link::relay_client::{dial, Target};
    use leon_link::test_support::TestRelay;
    use leon_link::{pair_as_client, Identity, PairingCode};
    use leon_remote::{interactive_on, SshOptions};
    use std::time::{Duration, Instant};

    fn theme() -> TerminalTheme {
        use leon_term::colors::from_rgb8;
        TerminalTheme {
            foreground: from_rgb8(250, 250, 250),
            background: from_rgb8(0, 0, 0),
            cursor: from_rgb8(97, 95, 255),
            selection: from_rgb8(30, 30, 90),
            find_match: from_rgb8(60, 60, 20),
            find_match_current: from_rgb8(250, 200, 0),
            ansi: std::array::from_fn(|i| from_rgb8(i as u8 * 16, 0, 0)),
        }
    }

    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_terminal_on_a_relay_machine_runs_on_the_host_and_survives_being_let_go_of() {
        let relay = TestRelay::start().await;
        let url = relay.url();
        let host = Host::start(
            HostConfig::new(&url, "box"),
            Arc::new(Identity::generate()),
            None,
        )
        .unwrap();
        let mut state = host.watch_relay();
        while *state.borrow_and_update() != RelayState::Online {
            state.changed().await.unwrap();
        }
        let me = Arc::new(Identity::generate());
        let info = host.new_pairing_code().await;
        let pipe = dial(&url, &Target::Room(info.room), None).await.unwrap();
        let paired = pair_as_client(pipe, &me, &PairingCode::parse(&info.code).unwrap(), "me")
            .await
            .unwrap();
        let hub = RelayHub::new(me, "me", Handle::current());
        let machine = Machine {
            id: MachineId::from_string("m"),
            name: "box".into(),
            kind: MachineKind::Relay {
                host_id: paired.host_id.to_string(),
                host_key: leon_remote::relay::key_hex(&paired.host_key),
                relay_url: url,
                name: "box".into(),
            },
        };
        let backend = RoutingBackend::new(hub, Handle::current());

        // The same call the application makes for an SSH machine's terminal.
        let command = leon_remote::CommandSpec {
            program: "/bin/sh".into(),
            env: vec![
                ("PS1".into(), "REMOTE> ".into()),
                ("ENV".into(), "/dev/null".into()),
            ],
            ..Default::default()
        };
        let placed = interactive_on(&machine, &command, &SshOptions::without_multiplexing());
        let spec = crate::launch::spawn_spec(placed);
        let terminal = backend
            .spawn(&spec, GridSize::new(80, 24), theme(), Box::new(|| {}))
            .unwrap();
        assert!(terminal.is_remote());
        until("the prompt", || terminal.screen_text().contains("REMOTE>")).await;
        terminal.write(&b"echo done-1\n"[..]);
        until("the output", || terminal.screen_text().contains("done-1")).await;
        assert_eq!(host.status().terminals, 1);

        // Letting go (the window closes) does not end the program on the host.
        drop(terminal);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(host.status().terminals, 1);
        until("the host to list it as running", || {
            host.status().terminals == 1
        })
        .await;
    }

    #[tokio::test]
    async fn a_command_with_no_route_starts_here() {
        let hub = RelayHub::new(Arc::new(Identity::generate()), "me", Handle::current());
        let backend = RoutingBackend::new(hub, Handle::current());
        let spec = SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo local-output".into()],
            ..SpawnSpec::default()
        };
        let terminal = backend
            .spawn(&spec, GridSize::new(80, 24), theme(), Box::new(|| {}))
            .unwrap();
        assert!(!terminal.is_remote());
        until("local output", || {
            terminal.screen_text().contains("local-output")
        })
        .await;
    }
}

/// The decision that a keeper is gone, with a real client that cannot connect.
#[cfg(all(test, unix))]
mod keeper_tests {
    use super::*;
    use leon_link::client::ClientConfig;
    use leon_link::local::UnixDialer;

    fn pump(gone: bool) -> KeeperPump {
        KeeperPump {
            front: Arc::default(),
            gone: Arc::new(move || gone),
            closing: Arc::default(),
        }
    }

    #[tokio::test]
    async fn a_terminal_is_not_declared_lost_on_a_connection_that_merely_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = ClientConfig::local(
            Arc::new(UnixDialer {
                path: dir.path().join("nothing.sock"),
            }),
            "t",
        );
        config.backoff = (Duration::from_millis(5), Duration::from_millis(10));
        let client = Client::start(config);
        // Several failed attempts in a row.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !matches!(client.state(), ConnState::Offline { attempt, .. } if attempt >= KEEPER_GONE_AFTER)
        {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // The keeper still answers on its socket (it is slow, or full): not gone.
        assert!(!keeper_gone(&client, &pump(false)));
        // Its socket refuses: gone.
        assert!(keeper_gone(&client, &pump(true)));
        // A connection closed under the terminal is gone too.
        client.close();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while client.state() != ConnState::Closed {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(keeper_gone(&client, &pump(false)));
    }
}
