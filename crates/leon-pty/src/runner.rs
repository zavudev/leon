//! A child in a pseudo-terminal, driven through channels.
//!
//! [`PtyProcess::spawn`] starts the program and returns the process handle and
//! a receiver of [`PtyEvent`]s: output chunks in order and, last, how the
//! child ended. The reader works on a bounded channel, so a child that floods
//! its terminal is slowed down by the PTY's own flow control instead of
//! filling memory.
//!
//! Dropping the handle hangs the child up and, if it ignores that, kills its
//! process group a moment later; a waiter thread reaps it.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use portable_pty::{ChildKiller, MasterPty};

use crate::size::GridSize;
use crate::spec::{command_builder_with, SpawnSpec};

/// How long a hung-up child has to exit before its process group is killed.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_millis(1_500);
/// How long, after the child exits, the last of its output may still arrive.
const DRAIN_GRACE: Duration = Duration::from_millis(250);
/// Chunks buffered between the reader thread and the consumer.
const CHUNKS: usize = 64;

/// How the child ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyExit {
    /// The exit code.
    pub code: u32,
    /// The signal that ended it, when one did.
    pub signal: Option<String>,
}

/// Who is in front of a terminal, as [`PtyProcess::foreground`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    /// The child's pid (the shell's).
    pub pid: u32,
    /// Whether the shell, and not a program it started, is in front.
    pub shell_in_front: bool,
    /// The program in front of the shell (the `comm` of its process group
    /// leader), where the system can name it.
    pub command: Option<String>,
}

/// The name of a process, where the system says it (Linux's `/proc`).
#[cfg(target_os = "linux")]
fn command_of(pid: u32) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn command_of(_pid: u32) -> Option<String> {
    None
}

/// What a PTY reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// Bytes the child wrote.
    Output(Vec<u8>),
    /// The child ended; the last event.
    Exited(PtyExit),
}

/// The receiving end of what a PTY reports.
pub type Events = flume::Receiver<PtyEvent>;
/// The sending end, for something that stands in for a PTY.
pub type EventSender = flume::Sender<PtyEvent>;

/// A channel of PTY events, for a scripted terminal: a test that decides what
/// a terminal prints and when it ends sends them here.
pub fn event_channel() -> (EventSender, Events) {
    flume::unbounded()
}

/// A running child in a pseudo-terminal.
pub struct PtyProcess {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: flume::Sender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    exited: Arc<AtomicBool>,
}

impl std::fmt::Debug for PtyProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyProcess")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

fn thread(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(error) = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
    {
        tracing::error!(%error, name, "could not start a terminal thread");
    }
}

impl PtyProcess {
    /// Starts `spec` in a new PTY of `size`, with this process's environment
    /// under the spec's.
    pub fn spawn(spec: &SpawnSpec, size: GridSize) -> Result<(Self, Events), String> {
        Self::spawn_with(spec, size, false)
    }

    /// [`PtyProcess::spawn`], with the choice of the environment:
    /// `own_environment` gives the child the spec's `env` and nothing of this
    /// process's (see [`command_builder_with`]).
    pub fn spawn_with(
        spec: &SpawnSpec,
        size: GridSize,
        own_environment: bool,
    ) -> Result<(Self, Events), String> {
        let pty = portable_pty::native_pty_system()
            .openpty(size.pty_size())
            .map_err(|error| format!("cannot open a terminal: {error}"))?;
        let mut child = pty
            .slave
            .spawn_command(command_builder_with(spec, own_environment))
            .map_err(|error| format!("cannot start {}: {error}", spec.program))?;
        drop(pty.slave);
        let reader = pty
            .master
            .try_clone_reader()
            .map_err(|error| format!("cannot read the terminal: {error}"))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|error| format!("cannot write to the terminal: {error}"))?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        let (events_tx, events_rx) = flume::bounded::<PtyEvent>(CHUNKS);
        let (input_tx, input_rx) = flume::unbounded::<Vec<u8>>();
        let reader_done = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));

        thread("leon-pty-writer", move || {
            let mut writer: Box<dyn Write + Send> = writer;
            while let Ok(bytes) = input_rx.recv() {
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        });
        {
            let (events, done) = (events_tx.clone(), reader_done.clone());
            thread("leon-pty-reader", move || {
                let mut reader: Box<dyn Read + Send> = reader;
                let mut buffer = vec![0u8; 32 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if events.send(PtyEvent::Output(buffer[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                    }
                }
                done.store(true, Ordering::Release);
            });
        }
        {
            let (done, exited) = (reader_done, exited.clone());
            thread("leon-pty-waiter", move || {
                let status = child.wait();
                let deadline = Instant::now() + DRAIN_GRACE;
                while !done.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let exit = match status {
                    Ok(status) => PtyExit {
                        code: status.exit_code(),
                        signal: status.signal().map(str::to_owned),
                    },
                    Err(_) => PtyExit {
                        code: 1,
                        signal: None,
                    },
                };
                exited.store(true, Ordering::Release);
                let _ = events_tx.send(PtyEvent::Exited(exit));
            });
        }
        Ok((
            Self {
                master: Mutex::new(pty.master),
                input: input_tx,
                killer: Mutex::new(killer),
                pid,
                exited,
            },
            events_rx,
        ))
    }

    /// Sends bytes to the child.
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(bytes);
    }

    /// Resizes the terminal; the child is told.
    pub fn resize(&self, size: GridSize) {
        let _ = self.master.lock().resize(size.pty_size());
    }

    /// The child's process id, where the system has one.
    pub fn process_id(&self) -> Option<u32> {
        self.pid
    }

    /// Who is in front of the terminal: whether the child (the shell) leads
    /// the terminal's foreground process group, which program does when it
    /// does not, and the child's pid. `None` where the system cannot say
    /// (Windows has no foreground process group) or the child is gone.
    pub fn foreground(&self) -> Option<Foreground> {
        #[cfg(unix)]
        {
            let pid = self.pid?;
            let leader = self.master.lock().process_group_leader()?;
            let shell_in_front = leader as u32 == pid;
            Some(Foreground {
                pid,
                shell_in_front,
                command: if shell_in_front {
                    None
                } else {
                    command_of(leader as u32)
                },
            })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Asks the program in front of the shell to end: SIGTERM to the
    /// terminal's foreground process group. `false` when nothing was sent
    /// (the shell is in front, the foreground cannot be told, or the signal
    /// failed).
    pub fn terminate_foreground(&self) -> bool {
        #[cfg(unix)]
        {
            let Some(pid) = self.pid else { return false };
            let Some(leader) = self.master.lock().process_group_leader() else {
                return false;
            };
            if leader as u32 == pid || leader <= 1 {
                return false;
            }
            // SAFETY: plain signal delivery to the terminal's foreground
            // process group, which the pseudo-terminal itself reported.
            unsafe { libc::kill(-leader, libc::SIGTERM) == 0 }
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Hangs the child up; its process group is killed after a grace period
    /// if it is still there.
    pub fn kill(&self) {
        if self.exited.load(Ordering::Acquire) {
            return;
        }
        let _ = self.killer.lock().kill();
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            let exited = self.exited.clone();
            thread("leon-pty-reaper", move || {
                let deadline = Instant::now() + KILL_GRACE;
                while Instant::now() < deadline {
                    if exited.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                if !exited.load(Ordering::Acquire) {
                    // SAFETY: plain signal delivery to a process group we started.
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
            });
        }
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> SpawnSpec {
        SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            ..SpawnSpec::default()
        }
    }

    fn collect(events: &flume::Receiver<PtyEvent>) -> (String, PtyExit) {
        let mut text = Vec::new();
        loop {
            match events
                .recv_timeout(Duration::from_secs(20))
                .expect("an event")
            {
                PtyEvent::Output(bytes) => text.extend(bytes),
                PtyEvent::Exited(exit) => {
                    return (String::from_utf8_lossy(&text).into_owned(), exit)
                }
            }
        }
    }

    #[test]
    fn the_childs_output_and_exit_status_arrive_in_order() {
        let (_pty, events) =
            PtyProcess::spawn(&sh("echo hello; exit 3"), GridSize::new(80, 24)).unwrap();
        let (text, exit) = collect(&events);
        assert!(text.contains("hello"), "{text:?}");
        assert_eq!(exit.code, 3);
    }

    #[test]
    fn input_reaches_the_child_and_the_resize_is_seen_by_it() {
        let spec = sh("stty size; read line; echo got:$line; stty size");
        let (pty, events) = PtyProcess::spawn(&spec, GridSize::new(80, 24)).unwrap();
        let mut seen = String::new();
        // Wait for the first size report before resizing.
        while !seen.contains("24 80") {
            if let PtyEvent::Output(bytes) = events.recv_timeout(Duration::from_secs(20)).unwrap() {
                seen.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        pty.resize(GridSize::new(100, 30));
        pty.write(b"ping\n".to_vec());
        let (rest, exit) = collect(&events);
        assert!(rest.contains("got:ping"), "{rest:?}");
        assert!(rest.contains("30 100"), "{rest:?}");
        assert_eq!(exit.code, 0);
    }

    #[test]
    fn killing_a_child_that_ignores_nothing_ends_it_with_a_signal() {
        let (pty, events) = PtyProcess::spawn(&sh("sleep 60"), GridSize::new(80, 24)).unwrap();
        pty.kill();
        let (_, exit) = collect(&events);
        assert!(exit.signal.is_some() || exit.code != 0, "{exit:?}");
    }

    fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_foreground_says_whether_the_shell_or_a_program_is_in_front() {
        let spec = SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-i".into()],
            env: vec![
                ("ENV".into(), "/dev/null".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("PS1".into(), "READY> ".into()),
            ],
            ..SpawnSpec::default()
        };
        let (pty, events) = PtyProcess::spawn(&spec, GridSize::new(80, 24)).unwrap();
        let mut seen = String::new();
        while !seen.contains("READY>") {
            if let PtyEvent::Output(bytes) = events.recv_timeout(Duration::from_secs(20)).unwrap() {
                seen.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        let shell = pty.foreground().expect("a foreground on this system");
        assert_eq!(Some(shell.pid), pty.process_id());
        assert!(shell.shell_in_front);
        assert_eq!(shell.command, None);
        assert!(!pty.terminate_foreground(), "nobody is in front to ask");
        pty.write(b"cat\n".to_vec());
        wait_for("cat to take the terminal", || {
            pty.foreground().is_some_and(|f| !f.shell_in_front)
        });
        #[cfg(target_os = "linux")]
        assert_eq!(pty.foreground().unwrap().command.as_deref(), Some("cat"));
        assert!(pty.terminate_foreground(), "SIGTERM reached cat");
        wait_for("the shell to get the terminal back", || {
            pty.foreground().is_some_and(|f| f.shell_in_front)
        });
    }

    #[test]
    fn an_unknown_program_is_an_error_not_a_hang() {
        let spec = SpawnSpec::new("/definitely/not/here");
        assert!(PtyProcess::spawn(&spec, GridSize::new(80, 24)).is_err());
    }
}
