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

use std::sync::Arc;

use leon_link::client::{Client, PtyEvent};
use leon_pty::{GridSize, SpawnSpec};
use leon_remote::RelayHub;
use leon_term::{Backend, Pty, RemoteFeed, RemoteLink, SpawnError, Terminal, TerminalTheme, Wake};
use leon_wire::{ExecSpec, Grid};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

enum Op {
    Write(Vec<u8>),
    Resize(GridSize),
    Close,
    Detach,
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

fn grid(size: GridSize) -> Grid {
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
    mut ops: mpsc::UnboundedReceiver<Op>,
) {
    let wire = ExecSpec {
        program: spec.program.clone(),
        args: spec.args.clone(),
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
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
    let handle = stream.handle();
    let mut events = stream.events;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(PtyEvent::Data(bytes)) => feed.data(&bytes),
                Some(PtyEvent::Gap) => feed.gap(),
                Some(PtyEvent::Exited(exit)) => {
                    feed.exit(exit.code, exit.signal);
                    return;
                }
                Some(PtyEvent::Gone) => {
                    feed.notice("\n[This terminal no longer exists on the other computer.]\n");
                    feed.exit(1, None);
                    return;
                }
                Some(PtyEvent::Disconnected) => {
                    feed.notice("\n[Connection lost. Reconnecting; the program keeps running on the other computer.]\n");
                }
                Some(PtyEvent::Reconnected) => feed.notice("[Reconnected.]\n"),
                None => return,
            },
            op = ops.recv() => match op {
                Some(Op::Write(bytes)) => handle.write(bytes),
                Some(Op::Resize(size)) => handle.resize(grid(size)),
                Some(Op::Close) => handle.close(),
                Some(Op::Detach) | None => return,
            },
        }
    }
}

#[cfg(all(test, unix))]
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
