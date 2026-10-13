//! A turn's process on the machine it belongs to: this computer, a machine
//! reached over SSH, or one reached through a relay.
//!
//! [`Reach`] is the [`Spawner`] of the application. A conversation says what
//! to start and on which machine ([`Start`]); what comes back is the same
//! everywhere, a way to write lines and the lines the agent prints:
//!
//! * on **this computer**, a child on a pipe ([`Piped`]);
//! * over **SSH**, the same child, which is `ssh` running the command on the
//!   other machine ([`over_ssh`]). `ssh` hands its own input and output to
//!   the agent, so nothing else changes. A connection that drops ends the
//!   turn: the conversation stays and the next prompt continues the agent's
//!   session;
//! * through a **relay**, a terminal of the other computer with the bridge
//!   of `leon_turn::bridge` in it, as the phone does ([`hosted`]). A relay
//!   carries terminals, not pipes, and a terminal outlives a dropped
//!   connection: the client re-attaches and the turn goes on. [`Link`] is
//!   the pure half of that: what to write and when, what was acknowledged,
//!   and the agent's lines out of the terminal's bytes.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use leon_core::MachineKind;
use leon_link::client::{Client, PtyEvent};
use leon_remote::{run_on, CommandSpec, RelayHub, SshOptions};
use leon_turn::bridge::{self, Mark, Marks};
use leon_turn::Lines;
use leon_wire::{ExecSpec, Grid};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use super::turn::{piped, Output, Piped, Spawner, Start, Transport};

/// How long a written message may stay unacknowledged before it is written
/// again.
const RESEND_AFTER: Duration = Duration::from_secs(5);
/// How long the other computer may take to be reachable before a turn is
/// given up.
const ONLINE_WITHIN: Duration = Duration::from_secs(20);
/// How long a turn that was told to end may take to say it has.
const END_WITHIN: Duration = Duration::from_secs(5);
/// The exit code of a turn ended from here whose end the host did not
/// report: what a shell says of a terminated program.
const ENDED_HERE: u32 = 143;
/// What the terminal of a turn is named on the host. Not the phone's name
/// for its own turns, which it looks for to take them up again.
const LABEL: &str = "leon-chat";
/// The terminal of a turn: wide, so that nothing the host's own programs
/// print is folded.
const GRID: Grid = Grid {
    cols: 2000,
    rows: 50,
    cell_width: 0,
    cell_height: 0,
};

/// Starts a turn's process on the machine it is for.
pub struct Reach {
    ssh: SshOptions,
    hub: Arc<RelayHub>,
    runtime: Handle,
}

impl Reach {
    /// With the options `ssh` is run with, the connections to paired
    /// computers and the runtime their traffic is carried on.
    pub fn new(ssh: SshOptions, hub: Arc<RelayHub>, runtime: Handle) -> Self {
        Self { ssh, hub, runtime }
    }
}

impl Spawner for Reach {
    fn spawn(&self, start: Start) -> io::Result<(Box<dyn Transport>, flume::Receiver<Output>)> {
        match &start.machine.kind {
            MachineKind::Local => Piped.spawn(start),
            MachineKind::Ssh { .. } => {
                let placed = over_ssh(&start, &self.ssh);
                let mut command = leon_remote::spawn::std_child(&placed.program);
                command.args(&placed.args);
                piped(command)
            }
            MachineKind::Relay {
                host_id,
                host_key,
                relay_url,
                ..
            } => {
                let route = leon_remote::relay::route(host_id, host_key, relay_url);
                let client = self
                    .hub
                    .ensure_route(&route)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let (ops, asked) = mpsc::unbounded_channel();
                let (said, output) = flume::unbounded();
                self.runtime
                    .spawn(drive(client, hosted(&start), asked, said));
                Ok((Box::new(Hosted(ops)), output))
            }
        }
    }
}

/// The process to start here so that `start` runs on its SSH machine, in the
/// login shell there, with this process's input and output as its own.
pub fn over_ssh(start: &Start, ssh: &SshOptions) -> CommandSpec {
    let mut argv = leon_turn::shell::login_argv(start.argv.clone()).into_iter();
    let mut command = CommandSpec::new(argv.next().unwrap_or_default())
        .args(argv)
        .cwd(&start.cwd);
    command.env = start.env.clone();
    run_on(&start.machine, &command, ssh)
}

/// What a paired computer is asked to run in a terminal for `start`: the
/// bridge, in the login shell, with the agent's command behind it.
pub fn hosted(start: &Start) -> ExecSpec {
    let mut argv = leon_turn::shell::login_argv(bridge::argv(&[], start.argv.clone())).into_iter();
    ExecSpec {
        program: argv.next().unwrap_or_default(),
        args: argv.collect(),
        env: start.env.clone(),
        cwd: Some(start.cwd.clone()),
        stdin: None,
    }
}

/// One message on its way to the agent.
#[derive(Debug)]
struct Queued {
    seq: u64,
    bytes: Vec<u8>,
}

/// The state of a turn behind the bridge: what is still to be written, and
/// what the terminal printed.
///
/// One message is on its way at a time. The bridge passes a number on once
/// and only a number above the last, so a message written again arrives
/// once, and none can overtake another.
#[derive(Debug)]
pub struct Link {
    /// The bridge said it reads.
    ready: bool,
    /// The connection to the host is up.
    online: bool,
    seq: u64,
    outbox: VecDeque<Queued>,
    /// When the first message of the outbox was last written.
    written: Option<Instant>,
    marks: Marks,
    lines: Lines,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            ready: false,
            online: true,
            seq: 0,
            outbox: VecDeque::new(),
            written: None,
            marks: Marks::default(),
            lines: Lines::default(),
        }
    }
}

impl Link {
    /// One line for the agent.
    pub fn say(&mut self, line: &str) {
        self.seq += 1;
        self.outbox.push_back(Queued {
            seq: self.seq,
            bytes: bridge::frame(self.seq, "m", line),
        });
    }

    /// No more for the agent: its input is closed once everything before
    /// this arrived.
    pub fn end(&mut self) {
        self.seq += 1;
        self.outbox.push_back(Queued {
            seq: self.seq,
            bytes: bridge::end(self.seq),
        });
    }

    /// What the terminal printed: the agent's lines, without the bridge's
    /// marks, which are taken in.
    pub fn heard(&mut self, bytes: &[u8]) -> Vec<String> {
        let (mut output, mut marks) = (Vec::new(), Vec::new());
        self.marks.push(bytes, &mut output, &mut marks);
        for mark in marks {
            match mark {
                Mark::Ready => self.ready = true,
                Mark::Ack { seq, .. } => {
                    if self.outbox.front().is_some_and(|first| first.seq == seq) {
                        self.outbox.pop_front();
                        self.written = None;
                    }
                }
                // It arrived broken: it is written again at once.
                Mark::Nak { seq } => {
                    if self.outbox.front().is_some_and(|first| first.seq == seq) {
                        self.written = None;
                    }
                }
            }
        }
        self.lines.push(&output)
    }

    /// The host no longer had some of what the terminal printed: whatever
    /// was cut in two is forgotten.
    pub fn gap(&mut self) {
        self.marks.reset();
        self.lines.reset();
    }

    /// The connection dropped. Nothing is written until it is back.
    pub fn lost(&mut self) {
        self.online = false;
    }

    /// The connection is back: what was on its way may not have arrived.
    pub fn back(&mut self) {
        self.online = true;
        self.written = None;
    }

    /// What to write to the terminal now, if anything.
    pub fn write(&mut self, now: Instant) -> Option<Vec<u8>> {
        if !self.ready || !self.online {
            return None;
        }
        let first = self.outbox.front()?;
        let due = self
            .written
            .is_none_or(|written| now.duration_since(written) >= RESEND_AFTER);
        if !due {
            return None;
        }
        self.written = Some(now);
        Some(first.bytes.clone())
    }

    /// How many messages the agent has not acknowledged.
    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.outbox.len()
    }
}

/// What a conversation asks of a hosted turn.
enum Op {
    Say(String),
    End,
    Kill,
}

/// The way to a turn in a terminal of a paired computer.
struct Hosted(mpsc::UnboundedSender<Op>);

impl Transport for Hosted {
    fn write(&mut self, line: &str) -> io::Result<()> {
        self.0
            .send(Op::Say(line.to_owned()))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn close_input(&mut self) {
        let _ = self.0.send(Op::End);
    }

    fn kill(&mut self) {
        let _ = self.0.send(Op::Kill);
    }
}

/// Opens the turn's terminal on the host and carries both directions until
/// the turn has ended.
async fn drive(
    client: Client,
    spec: ExecSpec,
    mut asked: mpsc::UnboundedReceiver<Op>,
    said: flume::Sender<Output>,
) {
    let refused = |why: String| {
        let _ = said.send(Output::Noise(format!(
            "Cannot start this on the other computer: {why}"
        )));
        let _ = said.send(Output::Exited(1));
    };
    if let Err(error) = client.wait_online(ONLINE_WITHIN).await {
        return refused(error.to_string());
    }
    let stream = match client.pty_open(spec, GRID, LABEL).await {
        Ok(stream) => stream,
        Err(error) => return refused(error.to_string()),
    };
    let handle = stream.handle();
    let mut events = stream.events;
    let mut link = Link::default();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Set once the turn was told to end: when to stop waiting for it.
    let mut ending: Option<Instant> = None;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(PtyEvent::Data(bytes)) => {
                    for line in link.heard(&bytes) {
                        if said.send(Output::Line(line)).is_err() {
                            // Nobody listens any more.
                            handle.close();
                            return;
                        }
                    }
                }
                Some(PtyEvent::Gap) => link.gap(),
                Some(PtyEvent::Exited(exit)) => {
                    let _ = said.send(Output::Exited(exit.code));
                    return;
                }
                Some(PtyEvent::Gone) | None => {
                    let _ = said.send(Output::Noise(
                        "This turn no longer exists on the other computer.".to_owned(),
                    ));
                    let _ = said.send(Output::Exited(1));
                    return;
                }
                Some(PtyEvent::Disconnected) => link.lost(),
                Some(PtyEvent::Reconnected) => link.back(),
                Some(PtyEvent::Foreground(_)) => {}
            },
            op = asked.recv(), if ending.is_none() => match op {
                Some(Op::Say(line)) => link.say(&line),
                Some(Op::End) => link.end(),
                // The conversation ended the turn, or is gone itself.
                Some(Op::Kill) | None => {
                    handle.close();
                    ending = Some(Instant::now() + END_WITHIN);
                }
            },
            _ = tick.tick() => {}
        }
        let now = Instant::now();
        if ending.is_some_and(|until| now >= until) {
            let _ = said.send(Output::Exited(ENDED_HERE));
            return;
        }
        if ending.is_none() {
            if let Some(bytes) = link.write(now) {
                handle.write(bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::{Machine, MachineId};

    fn start(kind: MachineKind) -> Start {
        Start {
            machine: Machine {
                id: MachineId::local(),
                name: "box".into(),
                kind,
            },
            argv: vec!["claude".into(), "-p".into(), "it's $HOME".into()],
            cwd: "/work/my app".into(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), "/home/me/.claude-work".into())],
        }
    }

    fn ssh() -> MachineKind {
        MachineKind::Ssh {
            host: "box.example".into(),
            user: Some("me".into()),
            port: None,
            identity_file: None,
        }
    }

    const READY: &[u8] = b"\x1b]5379;leon-ready\x07";

    fn ack(seq: u64) -> Vec<u8> {
        format!("\x1b]5379;leon-ack:{seq}:m\x07").into_bytes()
    }

    #[test]
    fn over_ssh_the_agent_runs_in_the_login_shell_of_the_other_machine() {
        let placed = over_ssh(&start(ssh()), &SshOptions::without_multiplexing());
        assert_eq!(placed.program, "ssh");
        // Read by Leon, so no terminal is asked for.
        assert!(!placed.args.contains(&"-t".to_owned()));
        assert!(placed.args.contains(&"BatchMode=yes".to_owned()));
        let remote = placed.args.last().unwrap();
        assert!(
            remote.starts_with("cd '/work/my app' && exec env "),
            "{remote}"
        );
        assert!(remote.contains("'CLAUDE_CONFIG_DIR=/home/me/.claude-work'"));
        // The command is arguments of the login shell, never text in it.
        assert!(
            remote.ends_with(r#"esac' leon claude -p 'it'\''s $HOME'"#),
            "{remote}"
        );
        assert_eq!(placed.cwd, None);
        assert!(placed.env.is_empty());
    }

    /// What `ssh` is given to run there, run here by a shell in its place:
    /// the folder, the variables and every argument arrive, and the process
    /// reads and writes as on a pipe.
    #[cfg(unix)]
    #[test]
    fn what_ssh_is_asked_to_run_is_a_command_a_shell_runs_whole() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("my app");
        std::fs::create_dir(&cwd).unwrap();
        let mut start = start(ssh());
        start.cwd = cwd.to_string_lossy().into_owned();
        start.argv = vec![
            "/bin/sh".into(),
            "-c".into(),
            r#"echo "$PWD|$CLAUDE_CONFIG_DIR|$1"; read line; echo "got $line"; exit 4"#.into(),
            "leon".into(),
            "it's $HOME".into(),
        ];
        let placed = over_ssh(&start, &SshOptions::without_multiplexing());
        let mut command = std::process::Command::new("/bin/sh");
        command
            .args(["-c", placed.args.last().unwrap()])
            .env("SHELL", "/bin/sh")
            .env("ENV", "/dev/null");
        let (mut transport, output) = piped(command).unwrap();
        let said = |output: &flume::Receiver<Output>| loop {
            match output.recv_timeout(Duration::from_secs(20)).unwrap() {
                Output::Noise(_) => {}
                other => return other,
            }
        };
        let first = said(&output);
        let Output::Line(first) = first else {
            panic!("{first:?}");
        };
        assert!(
            first.ends_with("my app|/home/me/.claude-work|it's $HOME"),
            "{first}"
        );
        transport.write("hello").unwrap();
        assert_eq!(said(&output), Output::Line("got hello".into()));
        assert_eq!(said(&output), Output::Exited(4));
    }

    #[test]
    fn through_a_relay_the_bridge_stands_before_the_agent() {
        let spec = hosted(&start(MachineKind::Local));
        assert_eq!(spec.program, "sh");
        assert_eq!(spec.args[1], leon_turn::shell::LOGIN_SHELL);
        assert_eq!(
            &spec.args[2..5],
            ["leon", "sh", "-c"].map(str::to_owned).as_slice()
        );
        assert_eq!(spec.args[5], bridge::BRIDGE);
        assert_eq!(
            &spec.args[6..],
            ["leon-bridge", "0", "claude", "-p", "it's $HOME"]
                .map(str::to_owned)
                .as_slice()
        );
        assert_eq!(spec.cwd.as_deref(), Some("/work/my app"));
        assert_eq!(spec.env.len(), 1);
    }

    #[test]
    fn nothing_is_written_before_the_bridge_reads() {
        let mut link = Link::default();
        let now = Instant::now();
        link.say("{\"a\":1}");
        assert_eq!(link.write(now), None);
        assert!(link.heard(READY).is_empty());
        assert_eq!(
            link.write(now)
                .map(|bytes| String::from_utf8(bytes).unwrap()),
            Some("B 1\n+ 1 {\"a\":1}\nE 1 1 m\n".to_owned())
        );
    }

    #[test]
    fn one_message_is_on_its_way_at_a_time_and_goes_when_acknowledged() {
        let mut link = Link::default();
        let now = Instant::now();
        link.heard(READY);
        link.say("one");
        link.say("two");
        assert!(link.write(now).is_some());
        // The first is still on its way.
        assert_eq!(link.write(now), None);
        link.heard(&ack(1));
        let second = String::from_utf8(link.write(now).unwrap()).unwrap();
        assert!(second.contains("+ 2 two"), "{second}");
        link.heard(&ack(2));
        assert_eq!(link.waiting(), 0);
        assert_eq!(link.write(now), None);
    }

    #[test]
    fn a_message_nobody_acknowledged_is_written_again() {
        let mut link = Link::default();
        let now = Instant::now();
        link.heard(READY);
        link.say("one");
        let first = link.write(now).unwrap();
        assert_eq!(link.write(now + Duration::from_secs(1)), None);
        assert_eq!(link.write(now + RESEND_AFTER), Some(first.clone()));
        // One that arrived broken is not waited for.
        link.heard(b"\x1b]5379;leon-nak:1\x07");
        assert_eq!(link.write(now + RESEND_AFTER), Some(first));
    }

    #[test]
    fn a_dropped_connection_holds_what_is_written_until_it_is_back() {
        let mut link = Link::default();
        let now = Instant::now();
        link.heard(READY);
        link.say("one");
        let first = link.write(now).unwrap();
        link.lost();
        assert_eq!(link.write(now + RESEND_AFTER), None);
        link.back();
        assert_eq!(link.write(now), Some(first));
        // An acknowledgement of something else changes nothing.
        link.heard(&ack(9));
        assert_eq!(link.waiting(), 1);
    }

    #[test]
    fn the_agents_lines_come_out_whole_and_without_the_marks() {
        let mut link = Link::default();
        assert_eq!(link.heard(b"{\"type\":\"sys"), Vec::<String>::new());
        let mut bytes = b"tem\"}\r\n{\"b\":".to_vec();
        bytes.extend_from_slice(READY);
        bytes.extend_from_slice(b"2}\r\n");
        assert_eq!(link.heard(&bytes), ["{\"type\":\"system\"}", "{\"b\":2}"]);
        assert!(link.ready);
    }

    #[test]
    fn the_end_of_input_waits_its_turn() {
        let mut link = Link::default();
        let now = Instant::now();
        link.heard(READY);
        link.say("one");
        link.end();
        link.write(now);
        link.heard(&ack(1));
        assert_eq!(link.write(now), Some(b"X 2\n".to_vec()));
        link.heard(b"\x1b]5379;leon-ack:2:end\x07");
        assert_eq!(link.waiting(), 0);
    }
}

/// A turn through a real relay to a real host on this computer, with a shell
/// script in the agent's place.
#[cfg(all(test, leon_posix_tests))]
mod hosted_tests {
    use super::*;
    use leon_core::{Machine, MachineId};
    use leon_host::{Host, HostConfig, RelayState};
    use leon_link::relay_client::{dial, Target};
    use leon_link::test_support::TestRelay;
    use leon_link::{pair_as_client, Identity, PairingCode};

    async fn paired() -> (TestRelay, Host, Reach, Machine) {
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
        let reach = Reach::new(SshOptions::without_multiplexing(), hub, Handle::current());
        (relay, host, reach, machine)
    }

    fn script(machine: Machine, text: &str) -> Start {
        Start {
            machine,
            argv: vec!["/bin/sh".into(), "-c".into(), text.into()],
            cwd: "/".into(),
            // A login shell that reads nobody's files.
            env: vec![
                ("SHELL".into(), "/bin/sh".into()),
                ("ENV".into(), "/dev/null".into()),
            ],
        }
    }

    async fn next(output: &flume::Receiver<Output>) -> Output {
        tokio::time::timeout(Duration::from_secs(30), output.recv_async())
            .await
            .expect("the turn said nothing in time")
            .expect("the turn ended without saying so")
    }

    #[tokio::test]
    async fn a_turn_on_a_paired_computer_is_written_to_and_read_like_a_pipe() {
        let (_relay, host, reach, machine) = paired().await;
        let long = "x".repeat(6000);
        let (mut transport, output) = reach
            .spawn(script(
                machine,
                r#"echo '{"up":1}'; read one; echo "got $one"; read two; echo "len ${#two}"; cat >/dev/null; exit 3"#,
            ))
            .unwrap();
        assert_eq!(next(&output).await, Output::Line("{\"up\":1}".into()));
        transport.write("{\"say\":\"it's $HOME\"}").unwrap();
        assert_eq!(
            next(&output).await,
            Output::Line("got {\"say\":\"it's $HOME\"}".into())
        );
        // Longer than a terminal hands over at once.
        transport.write(&long).unwrap();
        assert_eq!(next(&output).await, Output::Line("len 6000".into()));
        transport.close_input();
        assert_eq!(next(&output).await, Output::Exited(3));
        drop(host);
    }

    #[tokio::test]
    async fn ending_a_turn_ends_its_program_on_the_other_computer() {
        let (_relay, host, reach, machine) = paired().await;
        let (mut transport, output) = reach
            .spawn(script(machine, "echo $$; exec sleep 600"))
            .unwrap();
        let Output::Line(pid) = next(&output).await else {
            panic!("the program did not say its number");
        };
        let running = std::path::PathBuf::from(format!("/proc/{pid}"));
        assert_eq!(host.status().terminals, 1);
        if cfg!(target_os = "linux") {
            assert!(running.exists());
        }
        transport.kill();
        let ended = Instant::now();
        assert!(matches!(next(&output).await, Output::Exited(_)));
        // The host said so itself; nobody had to give up waiting.
        assert!(ended.elapsed() < END_WITHIN, "{:?}", ended.elapsed());
        let deadline = Instant::now() + Duration::from_secs(10);
        while cfg!(target_os = "linux") && running.exists() {
            assert!(Instant::now() < deadline, "the program still runs");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
