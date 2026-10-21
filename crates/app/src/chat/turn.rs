//! One turn of a conversation, run on a pipe: the agent's process, its
//! protocol, and what goes between them.
//!
//! [`Turn`] holds the [`AgentSession`] of the unified protocol and the
//! [`Transport`] to the process. Lines the agent prints go in
//! ([`Turn::output`]) and events come out; whatever the session wants written
//! on the way (the next step of a handshake, the refusal of a request nobody
//! here serves) is written at once. On a pipe a line written is a line
//! delivered, so every message is acknowledged to the session as soon as it
//! is written.
//!
//! [`Spawner`] starts the process; [`Piped`] is the real one, a child with
//! its three streams piped, in the login shell so that `PATH` is the one of a
//! terminal. The tests put a scripted transport in its place.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};

use leon_core::Machine;
use leon_turn::{
    object_on, Action, Adapter, AgentCommand, AgentSession, Lines, Outgoing, Step, TurnError,
    TurnEvent, TurnSpec,
};

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The machine to start it on.
    pub machine: Machine,
    /// The argument vector, program first.
    pub argv: Vec<String>,
    /// The folder to start it in, on that machine.
    pub cwd: String,
    /// Variables added to its environment.
    pub env: Vec<(String, String)>,
}

/// What a process says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// One line of its standard output, without escape sequences.
    Line(String),
    /// One line of its standard error.
    Noise(String),
    /// It ended with this code. Nothing follows.
    Exited(u32),
}

/// The way to a running process.
pub trait Transport: Send {
    /// Writes one line to its input.
    fn write(&mut self, line: &str) -> io::Result<()>;
    /// Closes its input: there is no more to read.
    fn close_input(&mut self);
    /// Ends it now.
    fn kill(&mut self);
    /// Its process id on this computer, when it runs here.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn pid(&self) -> Option<u32> {
        None
    }
}

/// Starts processes.
pub trait Spawner: Send + Sync {
    /// Starts `start` and returns the way to it and what it says.
    fn spawn(&self, start: Start) -> io::Result<(Box<dyn Transport>, flume::Receiver<Output>)>;
}

/// A turn in flight.
pub struct Turn {
    session: Box<dyn AgentSession>,
    transport: Box<dyn Transport>,
    input_closed: bool,
    exited: bool,
    /// What the agent listed as its commands and nobody took yet.
    commands: Option<Vec<AgentCommand>>,
}

impl Turn {
    /// Starts a turn of `adapter`'s agent: the process, then whatever its
    /// protocol says it must read first.
    pub fn start(
        adapter: &Adapter,
        spec: &TurnSpec<'_>,
        start: Start,
        unique: u64,
        spawner: &dyn Spawner,
    ) -> io::Result<(Self, flume::Receiver<Output>, Vec<TurnEvent>)> {
        let mut session = (adapter.session)(unique);
        let first = session.first(spec);
        let (transport, output) = spawner.spawn(start)?;
        let mut turn = Self {
            session,
            transport,
            input_closed: false,
            exited: false,
            commands: None,
        };
        let mut events = Vec::new();
        turn.send(first, &mut events);
        turn.settle();
        Ok((turn, output, events))
    }

    /// Something the process said. After [`Output::Exited`] the turn is over.
    pub fn output(&mut self, output: Output) -> Vec<TurnEvent> {
        let mut events = Vec::new();
        match output {
            Output::Line(line) => match object_on(&line) {
                Some(value) => {
                    let mut step = Step::default();
                    self.session.line(&value, &mut step);
                    events.append(&mut step.events);
                    if step.commands.is_some() {
                        self.commands = step.commands.take();
                    }
                    self.send(step.send, &mut events);
                }
                None if line.trim().is_empty() => {}
                None => self.session.noise(&line),
            },
            Output::Noise(line) => {
                if !line.trim().is_empty() {
                    self.session.noise(&line);
                }
            }
            Output::Exited(code) => {
                self.exited = true;
                events = self.session.exited(code);
            }
        }
        self.settle();
        events
    }

    /// What the agent just listed as its commands, once: `None` when it
    /// said nothing of them since this was last asked.
    pub fn take_commands(&mut self) -> Option<Vec<AgentCommand>> {
        self.commands.take()
    }

    /// The person did something.
    pub fn act(&mut self, action: Action) -> Result<Vec<TurnEvent>, TurnError> {
        if self.exited {
            return Err(TurnError::TurnEnded);
        }
        let outgoing = self.session.encode(action)?;
        let mut events = Vec::new();
        self.send(outgoing, &mut events);
        self.settle();
        Ok(events)
    }

    /// The agent's process id on this computer, when it runs here.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn pid(&self) -> Option<u32> {
        self.transport.pid()
    }

    /// Ends the process now, whatever it is doing.
    pub fn kill(&mut self) {
        self.transport.kill();
    }

    /// Writes messages and tells the session each arrived, which may in turn
    /// make it want more written.
    fn send(&mut self, outgoing: Vec<Outgoing>, events: &mut Vec<TurnEvent>) {
        let mut waiting: std::collections::VecDeque<Outgoing> = outgoing.into();
        while let Some(message) = waiting.pop_front() {
            if self.input_closed || self.transport.write(&message.line).is_err() {
                // The process is gone or going: its exit says the rest.
                continue;
            }
            let mut step = Step::default();
            self.session.delivered(&message.tag, &mut step);
            events.append(&mut step.events);
            waiting.extend(step.send);
        }
    }

    /// An agent with nothing more to do waits for input that will not come:
    /// its input is closed, which ends it.
    fn settle(&mut self) {
        if !self.input_closed && !self.exited && self.session.idle() {
            self.input_closed = true;
            self.transport.close_input();
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if !self.exited {
            self.transport.kill();
        }
    }
}

/// Processes of this computer, each in the login shell with its streams
/// piped.
pub struct Piped;

struct PipedTransport {
    input: Option<ChildStdin>,
    child: Arc<Mutex<Child>>,
}

impl Transport for PipedTransport {
    fn pid(&self) -> Option<u32> {
        self.child.lock().ok().map(|child| child.id())
    }

    fn write(&mut self, line: &str) -> io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::BrokenPipe))?;
        input.write_all(line.as_bytes())?;
        input.write_all(b"\n")?;
        input.flush()
    }

    fn close_input(&mut self) {
        self.input = None;
    }

    fn kill(&mut self) {
        self.input = None;
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        // The agent starts programs of its own: the whole group goes.
        #[cfg(unix)]
        if matches!(child.try_wait(), Ok(None)) {
            // SAFETY: `kill` takes plain integers; the group is the one this
            // child was made the leader of, and it has not been waited for.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGTERM);
            }
        }
        let _ = child.kill();
    }
}

impl Spawner for Piped {
    fn spawn(&self, start: Start) -> io::Result<(Box<dyn Transport>, flume::Receiver<Output>)> {
        let mut argv = leon_turn::shell::login_argv(start.argv).into_iter();
        let mut command = leon_remote::spawn::std_child(argv.next().unwrap_or_default());
        command.args(argv).current_dir(&start.cwd).envs(start.env);
        piped(command)
    }
}

/// Starts `command` with its three streams piped: the way to it, and what
/// it says line by line until it has exited.
pub(super) fn piped(
    mut command: std::process::Command,
) -> io::Result<(Box<dyn Transport>, flume::Receiver<Output>)> {
    {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let (sender, receiver) = flume::unbounded();
        if let Some(stderr) = stderr {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if sender.send(Output::Noise(line)).is_err() {
                        break;
                    }
                }
            });
        }
        let waited = child.clone();
        std::thread::spawn(move || {
            if let Some(mut stdout) = stdout {
                let mut lines = Lines::default();
                let mut buffer = [0_u8; 16 * 1024];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            for line in lines.push(&buffer[..read]) {
                                if sender.send(Output::Line(line)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            // Its output ended: it has exited or is about to. The lock is
            // taken only to ask, so that `kill` is never kept waiting.
            let code = loop {
                let status = waited.lock().map(|mut child| child.try_wait());
                match status {
                    Ok(Ok(Some(status))) => break exit_code(status),
                    Ok(Ok(None)) => std::thread::sleep(std::time::Duration::from_millis(20)),
                    _ => break 1,
                }
            };
            let _ = sender.send(Output::Exited(code));
        });
        Ok((Box::new(PipedTransport { input, child }), receiver))
    }
}

/// The exit code of a process; one ended by a signal reads as a shell says
/// it, 128 and the signal.
fn exit_code(status: std::process::ExitStatus) -> u32 {
    if let Some(code) = status.code() {
        return code as u32;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return 128 + signal as u32;
        }
    }
    1
}

#[cfg(test)]
pub mod testing {
    //! A process that is a script: what was written to it is kept, and the
    //! test says what it prints.

    use super::*;

    /// What a scripted process was told.
    #[derive(Default)]
    pub struct Told {
        /// The lines written to it.
        pub written: Vec<String>,
        /// Its input was closed.
        pub closed: bool,
        /// It was killed.
        pub killed: bool,
        /// What it was started with.
        pub started: Vec<Start>,
        /// What the next process started will say.
        pub outputs: Vec<flume::Sender<Output>>,
    }

    /// Starts scripted processes.
    #[derive(Clone, Default)]
    pub struct Scripted(pub Arc<Mutex<Told>>);

    struct ScriptedTransport(Arc<Mutex<Told>>);

    impl Transport for ScriptedTransport {
        fn write(&mut self, line: &str) -> io::Result<()> {
            self.0.lock().unwrap().written.push(line.to_owned());
            Ok(())
        }
        fn close_input(&mut self) {
            self.0.lock().unwrap().closed = true;
        }
        fn kill(&mut self) {
            self.0.lock().unwrap().killed = true;
        }
    }

    impl Spawner for Scripted {
        fn spawn(&self, start: Start) -> io::Result<(Box<dyn Transport>, flume::Receiver<Output>)> {
            let (sender, receiver) = flume::unbounded();
            let mut told = self.0.lock().unwrap();
            told.started.push(start);
            told.outputs.push(sender);
            told.closed = false;
            Ok((Box::new(ScriptedTransport(self.0.clone())), receiver))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Scripted;
    use super::*;

    fn here() -> Machine {
        Machine {
            id: leon_core::MachineId::local(),
            name: "This computer".into(),
            kind: leon_core::MachineKind::Local,
        }
    }

    fn start() -> Start {
        Start {
            machine: here(),
            argv: vec!["claude".into()],
            cwd: "/work".into(),
            env: Vec::new(),
        }
    }

    fn claude(prompt: &str) -> (Turn, Scripted, Vec<TurnEvent>) {
        let scripted = Scripted::default();
        let adapter = leon_turn::adapter("claude").unwrap();
        let spec = TurnSpec {
            program: "claude",
            prompt,
            ..TurnSpec::default()
        };
        let (turn, _output, events) = Turn::start(adapter, &spec, start(), 1, &scripted).unwrap();
        (turn, scripted, events)
    }

    #[test]
    fn a_turn_writes_its_handshake_and_its_prompt_as_soon_as_it_starts() {
        let (_turn, scripted, _) = claude("say hi");
        let told = scripted.0.lock().unwrap();
        assert!(told.written[0].contains("initialize"), "{:?}", told.written);
        assert!(
            told.written.iter().any(|line| line.contains("say hi")),
            "{:?}",
            told.written
        );
        assert!(!told.closed);
    }

    #[test]
    fn the_agents_lines_become_events_and_its_end_closes_its_input() {
        let (mut turn, scripted, _) = claude("say hi");
        let started = turn.output(Output::Line(
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"m"}"#.into(),
        ));
        assert_eq!(
            started,
            [TurnEvent::Started {
                session_id: "s1".into(),
                model: Some("m".into())
            }]
        );
        // What a shell prints around it is not an event.
        assert!(turn.output(Output::Line("motd".into())).is_empty());
        assert!(turn.output(Output::Noise("warning".into())).is_empty());
        let done = turn.output(Output::Line(
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","queued_turn_count":0}"#.into(),
        ));
        assert!(
            matches!(done.last(), Some(TurnEvent::Finished { ok: true, .. })),
            "{done:?}"
        );
        assert!(scripted.0.lock().unwrap().closed);
        assert_eq!(
            turn.output(Output::Exited(0)),
            [TurnEvent::Exited { code: 0 }]
        );
        assert_eq!(turn.act(Action::Interrupt), Err(TurnError::TurnEnded));
    }

    #[test]
    fn a_question_is_answered_on_the_pipe_and_resolved_at_once() {
        let (mut turn, scripted, _) = claude("list");
        let asked = turn.output(Output::Line(
            r#"{"type":"control_request","request_id":"q1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"t1"}}"#.into(),
        ));
        let Some(TurnEvent::Question { id, .. }) = asked.last() else {
            panic!("{asked:?}");
        };
        let answered = turn
            .act(Action::Answer(
                id.clone(),
                leon_turn::Answer::Permission {
                    choice_id: "once".into(),
                    reason: None,
                },
            ))
            .unwrap();
        assert_eq!(answered, [TurnEvent::QuestionResolved { id: "q1".into() }]);
        let told = scripted.0.lock().unwrap();
        assert!(told.written.last().unwrap().contains("control_response"));
    }

    #[test]
    fn a_turn_dropped_while_it_runs_ends_its_process() {
        let (turn, scripted, _) = claude("go");
        drop(turn);
        assert!(scripted.0.lock().unwrap().killed);
    }

    /// The real thing: a process of this computer, read line by line and
    /// told when there is no more input.
    #[cfg(unix)]
    #[test]
    fn a_piped_process_is_read_line_by_line_until_it_ends() {
        let (mut transport, output) = Piped
            .spawn(Start {
                machine: here(),
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "read line; echo \"got $line\"; echo oops >&2; cat >/dev/null; exit 3".into(),
                ],
                cwd: "/".into(),
                env: vec![("SHELL".into(), "/bin/sh".into())],
            })
            .unwrap();
        transport.write("hello").unwrap();
        transport.close_input();
        let mut seen = Vec::new();
        while let Ok(said) = output.recv_timeout(std::time::Duration::from_secs(20)) {
            let last = matches!(said, Output::Exited(_));
            seen.push(said);
            if last {
                break;
            }
        }
        assert!(seen.contains(&Output::Line("got hello".into())), "{seen:?}");
        assert!(seen.contains(&Output::Noise("oops".into())), "{seen:?}");
        assert_eq!(seen.last(), Some(&Output::Exited(3)), "{seen:?}");
    }
}
