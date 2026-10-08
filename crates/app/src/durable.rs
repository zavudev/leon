//! Durable local sessions, the application's side.
//!
//! With the setting `durable_sessions` on, the terminals of the sessions this
//! window opens on this computer are held by the keeper (`leon keeper`, see
//! `leon_host::keeper`), not by this process, so they keep running when the
//! window closes, when the application quits and when it crashes. This module
//! is what the window needs of it:
//!
//! * [`Durable`]: the service the window is given (`Options::durable`). The
//!   real one is [`LocalKeeper`]; the window's tests hand in a scripted one.
//!   It opens a terminal, lists the terminals the keeper holds, attaches to
//!   one with what it printed while nobody looked, and ends them all.
//! * [`Held`]: a terminal the keeper lists, read from the wire's description
//!   of it ([`held_from`]). A terminal opened by this application carries a
//!   token as its label ([`label_of`]); only those are ours.
//! * [`wire_spec`]: the program, directory and environment a new terminal is
//!   asked for. The keeper does not use its own environment (it is whatever it
//!   was when the first terminal was asked for), so the whole environment this
//!   process would have given an in-process terminal travels with the request.
//! * [`DurableBackend`]: the terminal backend that opens or attaches one
//!   terminal through the service, so a durable terminal is built by the same
//!   `TerminalView::spawn_with` as every other.
//!
//! The terminal itself is the remote terminal of `remote.rs`: the same
//! emulator, fed by the keeper's output and sending the person's keystrokes
//! to it. Scrollback beyond what the keeper keeps for each terminal
//! ([`leon_wire::REPLAY_BUFFER_BYTES`], 2 MiB of output) is not restored.

// The pieces only the keeper's client (Unix) uses are dead code elsewhere.
#![cfg_attr(not(unix), allow(dead_code))]

use std::sync::{Arc, Mutex};

use leon_core::KeeperRef;
use leon_pty::{GridSize, SpawnSpec};
use leon_term::{Backend, SpawnError, Terminal, TerminalTheme, Wake};
use leon_wire::{ExecSpec, Exit, Grid, PtyInfo};

/// The start of the label a terminal of this application carries in the
/// keeper's list.
pub const LABEL_PREFIX: &str = "keep:";

/// What a terminal ends with when its keeper is gone (killed, or the computer
/// restarted) and not its program: the window keeps such a session in what it
/// remembers, so the next start can say that the keeper no longer held it.
pub const KEEPER_LOST: &str = "keeper lost";

/// The label a terminal opened under `token` is listed by.
pub fn label_of(token: &str) -> String {
    format!("{LABEL_PREFIX}{token}")
}

/// A token for a new terminal: unique among this application's terminals
/// (the time it was made, this process and a counter), and not secret.
pub fn new_token(nanos: u128, pid: u32, counter: u64) -> String {
    format!("{nanos:x}-{pid:x}-{counter:x}")
}

/// A terminal the keeper holds, as it lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// The keeper's number for it.
    pub pty: u64,
    /// The token it was opened under.
    pub token: String,
    /// Where it runs.
    pub cwd: Option<String>,
    /// Its size.
    pub size: Grid,
    /// When it started, in seconds since the epoch.
    pub started_unix: u64,
    /// How it ended; absent while it runs.
    pub exit: Option<Exit>,
    /// How many windows are attached to it now. At a start nothing of this
    /// process is, so one that is belongs to another Leon on the same data
    /// directory.
    pub attached: u32,
}

impl Held {
    /// What a saved terminal needs to find it again.
    pub fn reference(&self) -> KeeperRef {
        KeeperRef {
            pty: self.pty,
            token: self.token.clone(),
        }
    }
}

/// The terminals of this application in the keeper's list, oldest first. A
/// terminal with a label of somebody else's is left alone.
pub fn held_from(infos: &[PtyInfo]) -> Vec<Held> {
    infos
        .iter()
        .filter_map(|info| {
            Some(Held {
                pty: info.pty,
                token: info.label.strip_prefix(LABEL_PREFIX)?.to_owned(),
                cwd: info.cwd.clone(),
                size: info.size,
                started_unix: info.started_unix,
                exit: info.exit.clone(),
                attached: info.attached,
            })
        })
        .collect()
}

/// The terminal request for `spec`: its program and arguments, its directory
/// (this process's when it names none) and the whole environment a terminal
/// of this process would have: `base` (this process's) with the spec's on top.
/// `Err` says why the request cannot be made (it breaks a limit of the wire),
/// and the terminal is then started in this process instead.
pub fn wire_spec(
    spec: &SpawnSpec,
    base: impl IntoIterator<Item = (String, String)>,
    here: Option<String>,
) -> Result<ExecSpec, String> {
    // The wire's own limits: a request over them would be refused as a whole.
    const MAX_ITEMS: usize = 4096;
    const MAX_STRING: usize = 64 * 1024;
    let mut env: Vec<(String, String)> = Vec::new();
    // The terminal's own type is the application's to set, as it is for an
    // in-process terminal (`TERM` and `COLORTERM` of the program that started
    // Leon describe that program's terminal, not this one); the plan's own
    // value for either still wins.
    let base = base
        .into_iter()
        .filter(|(name, _)| name != "TERM" && name != "COLORTERM");
    for (name, value) in base.chain(spec.env.iter().cloned()) {
        match env.iter_mut().find(|(known, _)| *known == name) {
            Some(slot) => slot.1 = value,
            None => env.push((name, value)),
        }
    }
    let cwd = spec.cwd.clone().or(here);
    let strings = std::iter::once(&spec.program)
        .chain(spec.args.iter())
        .chain(cwd.iter())
        .chain(env.iter().flat_map(|(a, b)| [a, b]));
    if strings.into_iter().any(|text| text.len() > MAX_STRING)
        || spec.args.len() > MAX_ITEMS
        || env.len() > MAX_ITEMS
    {
        return Err("the environment of this process is too large to hand over".to_owned());
    }
    Ok(ExecSpec {
        program: spec.program.clone(),
        args: spec.args.clone(),
        env,
        cwd,
        stdin: None,
    })
}

/// This process's environment, as text: a variable that is not valid UTF-8 is
/// left out (the wire carries text).
pub fn this_environment() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// The keeper's reference to a terminal, which a new terminal does not have
/// until the keeper has opened it (the window shows the terminal at once and
/// the open happens behind it): shared between the session and the task that
/// opens the terminal.
#[derive(Clone, Debug, Default)]
pub struct KeeperSlot(Arc<Mutex<Option<KeeperRef>>>);

impl KeeperSlot {
    /// A slot that already knows its terminal.
    pub fn known(reference: KeeperRef) -> Self {
        Self(Arc::new(Mutex::new(Some(reference))))
    }

    /// The reference, once there is one.
    pub fn get(&self) -> Option<KeeperRef> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Fills the slot.
    pub fn set(&self, reference: KeeperRef) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reference);
    }
}

/// What the window needs of the keeper. Nothing here may hold the window up
/// for long: a new terminal appears at once and is opened behind it, and what
/// has to wait (listing at start, settling at quit) is bounded.
pub trait Durable: Send + Sync {
    /// The terminals the keeper holds now. Does not start a keeper: `Err`
    /// says why there is none to ask (none is running, or it did not answer
    /// within a second and a half).
    fn held(&self) -> Result<Vec<Held>, String>;

    /// Opens a terminal for `spec` under `token`, starting the keeper when it
    /// is not running. The terminal is returned at once and filled in as the
    /// keeper answers; its reference is in the slot once it has. `Err` says
    /// why it cannot even be tried (the keeper's folder is not safe to talk
    /// through, it did not come up or answer a moment ago, the environment is
    /// too large): the terminal is then started in this process. A failure
    /// after that is printed in the terminal.
    fn open(
        &self,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
        token: &str,
    ) -> Result<(Terminal, KeeperSlot), SpawnError>;

    /// Attaches to a terminal the keeper holds, with everything it printed
    /// that its buffer still has. Returned at once, like [`Durable::open`].
    fn attach(&self, held: &Held, theme: TerminalTheme, wake: Wake)
        -> Result<Terminal, SpawnError>;

    /// Hangs up the terminals this window opened or attached to (never another
    /// window's) and waits, a second and a half at most, until the keeper
    /// lists them as ended. `false` when it did not confirm.
    fn end_all(&self) -> bool;

    /// Waits, a second at most, until what was sent to the keeper (the
    /// hang-ups of terminals that were closed) has reached it. `false` when it
    /// could not be confirmed.
    fn settle(&self) -> bool;

    /// The keeper's process id, when one is running and answers as this
    /// user's: what it started is this application's own.
    fn keeper_pid(&self) -> Option<u32>;

    /// A token for a new terminal.
    fn token(&self) -> String;
}

/// Opens or attaches one terminal through a [`Durable`], as a terminal
/// backend, and keeps what it learned of the terminal.
pub struct DurableBackend {
    durable: Arc<dyn Durable>,
    how: How,
    opened: Mutex<Option<KeeperSlot>>,
}

/// What a [`DurableBackend`] does.
pub enum How {
    /// Opens a new terminal under this token.
    Open(String),
    /// Attaches to this one.
    Attach(Held),
}

impl DurableBackend {
    /// A backend for one terminal.
    pub fn new(durable: Arc<dyn Durable>, how: How) -> Self {
        Self {
            durable,
            how,
            opened: Mutex::new(None),
        }
    }

    /// The keeper's reference to the terminal that was spawned, once it was
    /// (a new terminal's is filled in when the keeper has opened it).
    pub fn slot(&self) -> Option<KeeperSlot> {
        self.opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Backend for DurableBackend {
    fn spawn(
        &self,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
    ) -> Result<Terminal, SpawnError> {
        let (terminal, slot) = match &self.how {
            How::Open(token) => self.durable.open(spec, size, theme, wake, token)?,
            How::Attach(held) => {
                let terminal = self.durable.attach(held, theme, wake)?;
                (terminal, KeeperSlot::known(held.reference()))
            }
        };
        *self
            .opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(slot);
        Ok(terminal)
    }
}

#[cfg(unix)]
pub use local::LocalKeeper;

/// The real keeper, on this computer's socket.
#[cfg(unix)]
mod local {
    use super::*;
    use crate::remote::{grid, pump, HeldLink, KeeperPump, Op, Pump};
    use leon_host::keeper::{self, check_private_dir, probe_socket, DirRefusal, KeeperPaths};
    use leon_link::client::{Client, ClientConfig, ConnState};
    use leon_link::local::UnixDialer;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tokio::runtime::Handle;
    use tokio::sync::mpsc;

    /// How long the keeper has to answer a request the window waits for.
    const ANSWER_WITHIN: Duration = Duration::from_millis(1_500);
    /// How long the keeper has to answer a request that only a terminal waits
    /// for (the window is not).
    const OPEN_WITHIN: Duration = Duration::from_secs(5);
    /// How long a keeper that was just started has to accept connections.
    const START_WITHIN: Duration = Duration::from_secs(4);
    /// How long after a keeper could not be started or did not answer it is
    /// not tried again.
    const RETRY_AFTER: Duration = Duration::from_secs(20);
    /// The longest a quit waits for the keeper, in all.
    const QUIT_WITHIN: Duration = Duration::from_millis(1_500);

    /// The program and arguments that run the keeper of `data_dir`: the same
    /// binary, a hidden subcommand. `exe` is the running binary; a binary an
    /// update replaced while it ran is named by the system with a
    /// ` (deleted)` suffix, which is not a path. `data_dir` is made absolute:
    /// the keeper runs in `/`, where a relative path names another folder (and
    /// another keeper).
    pub fn keeper_command(exe: &Path, data_dir: &Path) -> (PathBuf, Vec<String>) {
        let name = exe.to_string_lossy();
        let program = name
            .strip_suffix(" (deleted)")
            .map_or_else(|| exe.to_path_buf(), PathBuf::from);
        let data_dir = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        (
            program,
            vec![
                "keeper".to_owned(),
                "--data-dir".to_owned(),
                data_dir.to_string_lossy().into_owned(),
            ],
        )
    }

    /// Makes every descriptor above the standard three close when the keeper's
    /// program is started: whatever this process holds open (a connection to the
    /// display server, a database, a file) and a library did not mark itself
    /// must not be inherited by a process that outlives the window.
    ///
    /// # Safety
    /// Must be called from `pre_exec`: it only makes system calls that are
    /// safe between `fork` and `exec`.
    unsafe fn seal_descriptors() {
        // SAFETY: `sysconf` and `fcntl` are async-signal-safe.
        unsafe {
            let limit = libc::sysconf(libc::_SC_OPEN_MAX).clamp(64, 65_536) as libc::c_int;
            for fd in 3..limit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
        }
    }

    /// Detaches `command` from this process: a session of its own (so it is
    /// neither in the window's process group nor hung up with its terminal),
    /// nothing on its standard streams, no directory of ours, and no
    /// descriptor of ours.
    fn detach(command: &mut std::process::Command) {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir("/");
        // SAFETY: `setsid` and the descriptor sealing are async-signal-safe
        // and touch nothing of ours.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                seal_descriptors();
                Ok(())
            });
        }
    }

    /// Starts the keeper detached.
    fn start_process(exe: &Path, data_dir: &Path) -> std::io::Result<()> {
        let (program, arguments) = keeper_command(exe, data_dir);
        let mut command = leon_remote::spawn::std_child(program);
        command.args(arguments);
        detach(&mut command);
        let mut child = command.spawn()?;
        // Reaped when it ends, so it never lingers as a zombie of this
        // process; if this process ends first, the system adopts it.
        std::thread::Builder::new()
            .name("leon-keeper-reaper".to_owned())
            .spawn(move || {
                let _ = child.wait();
            })?;
        Ok(())
    }

    /// Everything the terminals' tasks share.
    struct Inner {
        paths: KeeperPaths,
        exe: PathBuf,
        data_dir: PathBuf,
        handle: Handle,
        /// The one connection. Held across the making of a new one, so two
        /// terminals opened together make one.
        client: tokio::sync::Mutex<Option<Client>>,
        counter: AtomicU64,
        /// When starting the keeper failed or it did not answer, and why: not
        /// tried again for a while, so a keeper that will not come up does not
        /// hold up every new session.
        failed: Mutex<Option<(Instant, String)>>,
        /// The terminals this window opened or attached to: the only ones it
        /// ends.
        mine: Mutex<HashSet<u64>>,
        /// Hang-ups asked for and not yet handed to the connection.
        closing: Arc<AtomicUsize>,
    }

    /// The keeper of one data directory, reached through its socket.
    pub struct LocalKeeper {
        inner: Arc<Inner>,
    }

    impl Inner {
        fn recent_failure(&self) -> Option<String> {
            let failed = self
                .failed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            failed
                .as_ref()
                .filter(|(at, _)| at.elapsed() < RETRY_AFTER)
                .map(|(_, why)| why.clone())
        }

        fn remember_failure(&self, why: &str) {
            *self
                .failed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((Instant::now(), why.to_owned()));
        }

        fn forget_failure(&self) {
            *self
                .failed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }

        /// What can be told without waiting for anything: the keeper's folder
        /// is one nobody else can have made (a folder that is not there yet
        /// is fine: the keeper makes it), and the keeper did not just fail.
        fn pre_check(&self) -> Result<(), String> {
            if let Some(dir) = self.paths.socket.parent() {
                match check_private_dir(dir, keeper::our_uid()) {
                    Ok(()) | Err(DirRefusal::NotThere) => {}
                    Err(why) => {
                        return Err(format!(
                            "the keeper's socket cannot be trusted ({why}); a program of another user may be listening there"
                        ))
                    }
                }
            }
            match self.recent_failure() {
                Some(why) => Err(why),
                None => Ok(()),
            }
        }

        /// The connection, making one (and, with `start`, the keeper) when
        /// there is none. A connection that is reconnecting is reused while
        /// the keeper still answers on its socket: it is shared by terminals
        /// that are waiting for it, and is never closed under them.
        async fn client(&self, start: bool, wait: Duration) -> Result<Client, String> {
            let mut slot = self.client.lock().await;
            if let Some(client) = slot.as_ref() {
                match client.state() {
                    ConnState::Online => return Ok(client.clone()),
                    ConnState::Closed => *slot = None,
                    _ if probe_socket(&self.paths.socket).is_ok() => {
                        return match client.wait_online(wait).await {
                            Ok(()) => Ok(client.clone()),
                            Err(_) => {
                                self.remember_failure("the keeper does not answer");
                                Err("the keeper does not answer".to_owned())
                            }
                        };
                    }
                    // The socket refuses: the keeper is gone. The old
                    // connection is left to the terminals that hold it.
                    _ => *slot = None,
                }
            }
            self.pre_check()?;
            if let Err(why) = probe_socket(&self.paths.socket) {
                if !start {
                    return Err(format!("no keeper is running ({why})"));
                }
                if let Err(error) = start_process(&self.exe, &self.data_dir) {
                    let why = format!("the keeper could not be started: {error}");
                    self.remember_failure(&why);
                    return Err(why);
                }
                let deadline = Instant::now() + START_WITHIN;
                while probe_socket(&self.paths.socket).is_err() {
                    if Instant::now() >= deadline {
                        let why = "the keeper did not come up";
                        self.remember_failure(why);
                        return Err(why.to_owned());
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            }
            let client = Client::start(ClientConfig::local(
                Arc::new(UnixDialer {
                    path: self.paths.socket.clone(),
                }),
                "leon",
            ));
            if let Err(error) = client.wait_online(wait).await {
                client.close();
                let why = format!("the keeper did not answer: {error}");
                self.remember_failure(&why);
                return Err(why);
            }
            self.forget_failure();
            *slot = Some(client.clone());
            Ok(client)
        }

        /// The terminal around a stream the keeper opened, driven until it
        /// ends.
        fn keeper_pump(&self, front: Arc<Mutex<Option<leon_term::HeldForeground>>>) -> Pump {
            let paths = self.paths.clone();
            Pump::Keeper(KeeperPump {
                front,
                gone: Arc::new(move || probe_socket(&paths.socket).is_err()),
                closing: self.closing.clone(),
            })
        }
    }

    /// Tells the person, in the terminal, that the keeper could not give it,
    /// and ends it. The next terminals fall back to the window for a while.
    fn fail(feed: &leon_term::RemoteFeed, what: &str, why: &str) {
        feed.notice(&format!(
            "\n[{what}: {why}. Sessions opened in the next few seconds start in the window instead.]\n"
        ));
        feed.exit(1, None);
    }

    impl LocalKeeper {
        /// The keeper of `data_dir`, started from `exe` when needed. Its
        /// connection runs on `handle`.
        pub fn new(paths: KeeperPaths, exe: PathBuf, data_dir: PathBuf, handle: Handle) -> Self {
            Self {
                inner: Arc::new(Inner {
                    paths,
                    exe,
                    data_dir: std::path::absolute(&data_dir).unwrap_or(data_dir),
                    handle,
                    client: tokio::sync::Mutex::new(None),
                    counter: AtomicU64::new(0),
                    failed: Mutex::new(None),
                    mine: Mutex::new(HashSet::new()),
                    closing: Arc::new(AtomicUsize::new(0)),
                }),
            }
        }

        /// The data directory the keeper is started for (always absolute).
        #[cfg(test)]
        pub fn data_dir(&self) -> &Path {
            &self.inner.data_dir
        }

        fn mine(&self, pty: u64) {
            self.inner
                .mine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(pty);
        }

        /// The links and the terminal around them.
        fn terminal(
            &self,
            spec: &SpawnSpec,
            size: GridSize,
            theme: TerminalTheme,
            wake: Wake,
        ) -> (
            Terminal,
            leon_term::RemoteFeed,
            mpsc::UnboundedReceiver<Op>,
            Arc<Mutex<Option<leon_term::HeldForeground>>>,
        ) {
            let (ops, ops_rx) = mpsc::unbounded_channel::<Op>();
            let front = Arc::new(Mutex::new(None));
            let link = Arc::new(HeldLink {
                ops,
                front: front.clone(),
                closing: self.inner.closing.clone(),
            });
            let (terminal, feed) = Terminal::remote(spec, size, theme, wake, link);
            (terminal, feed, ops_rx, front)
        }
    }

    impl Durable for LocalKeeper {
        fn held(&self) -> Result<Vec<Held>, String> {
            let inner = self.inner.clone();
            let listed = async move {
                let client = inner.client(false, ANSWER_WITHIN).await?;
                client.pty_list().await.map_err(|e| e.to_string())
            };
            match self
                .inner
                .handle
                .block_on(async move { tokio::time::timeout(ANSWER_WITHIN, listed).await })
            {
                Ok(Ok(infos)) => Ok(held_from(&infos)),
                Ok(Err(why)) => Err(why),
                Err(_) => {
                    self.inner.remember_failure("the keeper does not answer");
                    Err("the keeper did not answer in time".to_owned())
                }
            }
        }

        fn open(
            &self,
            spec: &SpawnSpec,
            size: GridSize,
            theme: TerminalTheme,
            wake: Wake,
            token: &str,
        ) -> Result<(Terminal, KeeperSlot), SpawnError> {
            let wire = wire_spec(
                spec,
                this_environment(),
                std::env::current_dir()
                    .ok()
                    .map(|dir| dir.to_string_lossy().into_owned()),
            )
            .map_err(SpawnError)?;
            self.inner.pre_check().map_err(SpawnError)?;
            let (terminal, feed, ops, front) = self.terminal(spec, size, theme, wake);
            let slot = KeeperSlot::default();
            let (inner, token, task_slot) = (self.inner.clone(), token.to_owned(), slot.clone());
            self.inner.handle.spawn(async move {
                let client = match inner.client(true, OPEN_WITHIN).await {
                    Ok(client) => client,
                    Err(why) => return fail(&feed, "Cannot start this under the keeper", &why),
                };
                let opened = tokio::time::timeout(
                    OPEN_WITHIN,
                    client.pty_open(wire, grid(size), &label_of(&token)),
                )
                .await;
                let stream = match opened {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(error)) => {
                        return fail(
                            &feed,
                            "Cannot start this under the keeper",
                            &error.to_string(),
                        )
                    }
                    Err(_) => {
                        inner.remember_failure("the keeper does not answer");
                        return fail(&feed, "The keeper did not answer", "it took too long");
                    }
                };
                task_slot.set(KeeperRef {
                    pty: stream.pty,
                    token,
                });
                // The layout the window remembers can name it from now on.
                feed.changed();
                inner
                    .mine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(stream.pty);
                pump(client, stream, feed, ops, inner.keeper_pump(front)).await;
            });
            Ok((terminal, slot))
        }

        fn attach(
            &self,
            held: &Held,
            theme: TerminalTheme,
            wake: Wake,
        ) -> Result<Terminal, SpawnError> {
            self.inner.pre_check().map_err(SpawnError)?;
            let spec = SpawnSpec {
                cwd: held.cwd.clone(),
                ..SpawnSpec::default()
            };
            // The size it has now: the screen is rebuilt at that size and
            // resized to the pane once it is drawn.
            let size = GridSize {
                cols: held.size.cols.max(2),
                rows: held.size.rows.max(1),
                cell_width: held.size.cell_width,
                cell_height: held.size.cell_height,
            };
            let (terminal, feed, ops, front) = self.terminal(&spec, size, theme, wake);
            self.mine(held.pty);
            let (inner, pty) = (self.inner.clone(), held.pty);
            self.inner.handle.spawn(async move {
                let client = match inner.client(false, OPEN_WITHIN).await {
                    Ok(client) => client,
                    Err(why) => return fail(&feed, "Cannot attach to this under the keeper", &why),
                };
                // From the start of what the keeper kept: the screen is
                // rebuilt from it.
                match tokio::time::timeout(OPEN_WITHIN, client.pty_attach(pty, 0)).await {
                    Ok(Ok(stream)) => {
                        pump(client, stream, feed, ops, inner.keeper_pump(front)).await
                    }
                    Ok(Err(error)) => fail(
                        &feed,
                        "Cannot attach to this under the keeper",
                        &error.to_string(),
                    ),
                    Err(_) => {
                        inner.remember_failure("the keeper does not answer");
                        fail(&feed, "The keeper did not answer", "it took too long")
                    }
                }
            });
            Ok(terminal)
        }

        fn end_all(&self) -> bool {
            let inner = self.inner.clone();
            let ended = async move {
                let Ok(client) = inner.client(false, QUIT_WITHIN).await else {
                    // Nobody to tell: nothing of this window runs.
                    return true;
                };
                let mine = inner
                    .mine
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let deadline = Instant::now() + QUIT_WITHIN;
                let mut asked = false;
                loop {
                    let Ok(infos) = client.pty_list().await else {
                        return false;
                    };
                    let running: Vec<u64> = infos
                        .iter()
                        .filter(|i| i.exit.is_none() && mine.contains(&i.pty))
                        .map(|i| i.pty)
                        .collect();
                    if running.is_empty() {
                        return true;
                    }
                    if Instant::now() >= deadline {
                        return false;
                    }
                    if !asked {
                        for pty in &running {
                            client.pty_close(*pty).await;
                        }
                        asked = true;
                    }
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            };
            self.inner
                .handle
                .block_on(async move { tokio::time::timeout(QUIT_WITHIN, ended).await })
                .unwrap_or(false)
        }

        fn settle(&self) -> bool {
            let inner = self.inner.clone();
            let settled = async move {
                let deadline = Instant::now() + QUIT_WITHIN;
                while inner.closing.load(Ordering::SeqCst) > 0 {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let Ok(client) = inner.client(false, QUIT_WITHIN).await else {
                    return true;
                };
                // Answered after everything sent before it, on one connection.
                client.pty_list().await.is_ok()
            };
            self.inner
                .handle
                .block_on(async move { tokio::time::timeout(QUIT_WITHIN, settled).await })
                .unwrap_or(false)
        }

        fn keeper_pid(&self) -> Option<u32> {
            probe_socket(&self.inner.paths.socket).ok()?;
            keeper::read_pid(&self.inner.paths.lock)
        }

        fn token(&self) -> String {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            new_token(
                nanos,
                std::process::id(),
                self.inner.counter.fetch_add(1, Ordering::Relaxed),
            )
        }
    }

    #[cfg(all(test, target_os = "linux", leon_posix_tests))]
    pub(super) mod sealing {
        use super::*;

        /// Whether a program started the way the keeper is sees a descriptor
        /// that this process opened without close-on-exec.
        pub(in crate::durable) fn child_sees_descriptor(seal: bool) -> bool {
            let mut fds = [0 as libc::c_int; 2];
            // SAFETY: `fds` has room for the two descriptors.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            let n = fds[0];
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", &format!("test -e /proc/self/fd/{n}")]);
            if seal {
                detach(&mut command);
            }
            let seen = command.status().unwrap().success();
            // SAFETY: the two descriptors are ours.
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            seen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Grid {
        Grid {
            cols: 100,
            rows: 30,
            cell_width: 8,
            cell_height: 16,
        }
    }

    fn info(pty: u64, label: &str) -> PtyInfo {
        PtyInfo {
            pty,
            label: label.to_owned(),
            program: "zsh".into(),
            cwd: Some("/srv/api".into()),
            size: grid(),
            started_unix: 1_700_000_000,
            exit: None,
            first_offset: 0,
            end_offset: 0,
            attached: 0,
        }
    }

    fn spec() -> SpawnSpec {
        SpawnSpec {
            program: "/bin/zsh".into(),
            args: vec!["-l".into()],
            env: vec![
                ("CLAUDE_CONFIG_DIR".into(), "/home/ana/.claude-work".into()),
                ("PATH".into(), "/spec/bin".into()),
            ],
            cwd: Some("/srv/api".into()),
            route: None,
        }
    }

    fn base() -> Vec<(String, String)> {
        vec![
            ("HOME".into(), "/home/ana".into()),
            ("PATH".into(), "/usr/bin".into()),
            ("LANG".into(), "en_US.UTF-8".into()),
        ]
    }

    #[test]
    fn a_terminal_of_ours_is_found_by_its_label_and_a_stranger_is_left_alone() {
        let infos = [
            info(1, &label_of("abc")),
            info(2, "claude @ /srv/api"),
            info(3, &label_of("def")),
        ];
        let held = held_from(&infos);
        assert_eq!(
            held.iter()
                .map(|h| (h.pty, h.token.as_str()))
                .collect::<Vec<_>>(),
            [(1, "abc"), (3, "def")]
        );
        assert_eq!(held[0].cwd.as_deref(), Some("/srv/api"));
        assert_eq!(held[0].size, grid());
        assert_eq!(
            held[0].reference(),
            KeeperRef {
                pty: 1,
                token: "abc".into()
            }
        );
    }

    #[test]
    fn tokens_differ_by_time_process_and_count() {
        let a = new_token(1, 2, 3);
        assert_ne!(a, new_token(2, 2, 3));
        assert_ne!(a, new_token(1, 3, 3));
        assert_ne!(a, new_token(1, 2, 4));
        assert!(!a.contains(char::is_whitespace));
    }

    #[test]
    fn what_leon_tells_a_terminal_of_the_shared_memory_travels_to_the_keeper() {
        // The plan's environment as `launch::plan_with` makes it: Leon's own
        // variables, then the user's, the last of a name being the one kept.
        let mut plan = spec();
        plan.env = vec![
            ("LEON_BIN".into(), "/opt/leon/leon".into()),
            ("LEON_MEMORY".into(), "/data/memory/api.md".into()),
            ("LEON_DATA_DIR".into(), "/data".into()),
            ("LEON_MEMORY".into(), "/my/own.md".into()),
        ];
        let wire = wire_spec(&plan, base(), None).unwrap();
        let get = |name: &str| {
            wire.env
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("LEON_BIN"), Some("/opt/leon/leon"));
        assert_eq!(get("LEON_DATA_DIR"), Some("/data"));
        assert_eq!(get("LEON_MEMORY"), Some("/my/own.md"), "the user's wins");
        assert_eq!(
            wire.env.iter().filter(|(n, _)| n == "LEON_MEMORY").count(),
            1
        );
    }

    #[test]
    fn the_request_carries_this_processs_environment_with_the_plans_on_top() {
        let wire = wire_spec(&spec(), base(), Some("/elsewhere".into())).unwrap();
        let get = |name: &str| {
            wire.env
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("HOME"), Some("/home/ana"));
        assert_eq!(get("LANG"), Some("en_US.UTF-8"));
        assert_eq!(get("PATH"), Some("/spec/bin"), "the plan wins");
        assert_eq!(
            get("CLAUDE_CONFIG_DIR"),
            Some("/home/ana/.claude-work"),
            "the account's variables travel"
        );
        assert_eq!(wire.env.iter().filter(|(n, _)| n == "PATH").count(), 1);
        assert_eq!(wire.program, "/bin/zsh");
        assert_eq!(wire.args, ["-l"]);
        assert_eq!(wire.cwd.as_deref(), Some("/srv/api"));
        assert!(wire.stdin.is_none());
    }

    #[test]
    fn the_terminal_type_of_the_program_that_started_leon_is_not_the_terminals() {
        let mut inherited = base();
        inherited.push(("TERM".into(), "xterm-kitty".into()));
        inherited.push(("COLORTERM".into(), "24bit".into()));
        let wire = wire_spec(&spec(), inherited.clone(), None).unwrap();
        assert!(
            wire.env
                .iter()
                .all(|(n, _)| n != "TERM" && n != "COLORTERM"),
            "the keeper sets the terminal's own, as an in-process terminal gets"
        );
        // What the plan itself sets is the plan's to say.
        let mut plan = spec();
        plan.env.push(("TERM".into(), "screen".into()));
        let wire = wire_spec(&plan, inherited, None).unwrap();
        assert!(wire.env.contains(&("TERM".into(), "screen".into())));
        assert!(wire.env.iter().all(|(n, _)| n != "COLORTERM"));
    }

    #[test]
    fn a_plan_with_no_folder_starts_where_this_process_is() {
        let mut plain = spec();
        plain.cwd = None;
        let wire = wire_spec(&plain, base(), Some("/here".into())).unwrap();
        assert_eq!(wire.cwd.as_deref(), Some("/here"));
        let none = wire_spec(&plain, base(), None).unwrap();
        assert_eq!(none.cwd, None);
    }

    #[test]
    fn an_environment_over_the_wires_limits_is_refused_so_the_terminal_starts_here_instead() {
        let many: Vec<(String, String)> =
            (0..4100).map(|i| (format!("V{i}"), "x".into())).collect();
        assert!(wire_spec(&spec(), many, None).is_err());
        let long = vec![("BIG".to_owned(), "x".repeat(70_000))];
        assert!(wire_spec(&spec(), long, None).is_err());
        // The limits are the wire's own: what passes here encodes there.
        let ok = wire_spec(&spec(), base(), None).unwrap();
        let message = leon_wire::Message::PtyOpen {
            id: 1,
            spec: ok,
            size: grid(),
            label: label_of("t"),
        };
        assert!(leon_wire::encode_frame(&message).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_keeper_is_the_same_binary_with_a_hidden_subcommand_and_the_data_directory() {
        let (program, args) = local::keeper_command(
            std::path::Path::new("/opt/leon/bin/leon"),
            std::path::Path::new("/home/ana/.local/share/leon"),
        );
        assert_eq!(program, std::path::Path::new("/opt/leon/bin/leon"));
        assert_eq!(
            args,
            ["keeper", "--data-dir", "/home/ana/.local/share/leon"]
        );
        // An update replaced the file under the running binary.
        let (program, _) = local::keeper_command(
            std::path::Path::new("/opt/leon/bin/leon (deleted)"),
            std::path::Path::new("/d"),
        );
        assert_eq!(program, std::path::Path::new("/opt/leon/bin/leon"));
        // A relative data directory is made absolute: the keeper runs in `/`.
        let (_, args) = local::keeper_command(
            std::path::Path::new("/opt/leon/bin/leon"),
            std::path::Path::new("rel/data"),
        );
        assert!(std::path::Path::new(&args[2]).is_absolute(), "{args:?}");
        assert!(args[2].ends_with("rel/data"), "{args:?}");
    }

    #[cfg(all(unix, target_os = "linux", leon_posix_tests))]
    #[test]
    fn the_keeper_does_not_inherit_what_this_process_holds_open() {
        // Without the sealing a descriptor opened without close-on-exec (by a
        // library, say) is seen by the program; with it, it is not.
        assert!(local::sealing::child_sees_descriptor(false));
        assert!(!local::sealing::child_sees_descriptor(true));
    }
}

/// The real keeper client against a real keeper (the one `leon keeper` runs)
/// on a real socket in a temporary directory, with scripted terminals: no
/// process is started, no shell runs, nothing of the user's is touched.
#[cfg(all(test, unix))]
mod real {
    use super::*;
    use leon_host::keeper::{self, Keeper, KeeperConfig, KeeperPaths};
    use leon_host::ptys::{Process, Spawner};
    use leon_pty::{event_channel, EventSender, Events, Foreground, PtyEvent, PtyExit};
    use leon_term::colors::from_rgb8;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    fn theme() -> TerminalTheme {
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

    fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    struct Scripted {
        events: EventSender,
        ended: AtomicBool,
    }

    impl Process for Scripted {
        fn write(&self, bytes: Vec<u8>) {
            let mut echo = b"echo:".to_vec();
            echo.extend(bytes);
            let _ = self.events.send(PtyEvent::Output(echo));
        }
        fn resize(&self, _: leon_pty::GridSize) {}
        fn kill(&self) {
            if !self.ended.swap(true, Ordering::SeqCst) {
                let _ = self.events.send(PtyEvent::Exited(PtyExit {
                    code: 1,
                    signal: Some("Hangup".into()),
                }));
            }
        }
        fn pid(&self) -> Option<u32> {
            Some(4242)
        }
        fn foreground(&self) -> Option<Foreground> {
            Some(Foreground {
                pid: 4242,
                shell_in_front: true,
                command: None,
            })
        }
        fn terminate_foreground(&self) -> bool {
            false
        }
    }

    struct Studio;

    impl Spawner for Studio {
        fn spawn(
            &self,
            _: &leon_pty::SpawnSpec,
            _: leon_pty::GridSize,
        ) -> Result<(Arc<dyn Process>, Events), String> {
            let (events, receiver) = event_channel();
            let _ = events.send(PtyEvent::Output(b"ready\r\n".to_vec()));
            Ok((
                Arc::new(Scripted {
                    events,
                    ended: AtomicBool::new(false),
                }),
                receiver,
            ))
        }
    }

    struct Rig {
        runtime: tokio::runtime::Runtime,
        dir: tempfile::TempDir,
        paths: KeeperPaths,
    }

    impl Rig {
        fn new() -> Self {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let paths = keeper::paths(
                Some(dir.path().to_path_buf()),
                keeper::our_uid(),
                dir.path(),
                &dir.path().join("data"),
            );
            let mut cfg = KeeperConfig::new(paths.clone());
            cfg.spawner = Arc::new(Studio);
            cfg.idle_after = Duration::from_secs(600);
            let bound = runtime.block_on(async { Keeper::bind(cfg) }).unwrap();
            runtime.spawn(bound.serve());
            Self {
                runtime,
                dir,
                paths,
            }
        }

        /// A window: its own connection to the keeper.
        fn window(&self) -> LocalKeeper {
            LocalKeeper::new(
                self.paths.clone(),
                self.dir.path().join("no-such-leon"),
                self.dir.path().join("data"),
                self.runtime.handle().clone(),
            )
        }
    }

    fn open(keeper: &LocalKeeper) -> (Terminal, KeeperSlot) {
        let spec = SpawnSpec {
            program: "zsh".into(),
            cwd: Some("/srv/api".into()),
            ..SpawnSpec::default()
        };
        keeper
            .open(
                &spec,
                GridSize::new(80, 24),
                theme(),
                Box::new(|| {}),
                &keeper.token(),
            )
            .unwrap()
    }

    #[test]
    fn a_terminal_opens_at_once_fills_in_and_is_listed_attached_and_ended() {
        let rig = Rig::new();
        let window = rig.window();
        assert_eq!(window.held().unwrap(), vec![]);
        let (terminal, slot) = open(&window);
        // Returned before the keeper answered; its reference follows.
        until("the reference", || slot.get().is_some());
        until("the first output", || {
            terminal.screen_text().contains("ready")
        });
        terminal.write(&b"hi"[..]);
        until("the echo", || terminal.screen_text().contains("echo:hi"));
        // The keeper answers who is in front, and the terminal reports it.
        until("the pid", || terminal.process_id() == Some(4242));
        assert_eq!(terminal.shell_is_foreground(), Some(true));
        assert!(terminal.is_held() && !terminal.is_remote());
        let reference = slot.get().unwrap();
        let listed = window.held().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reference(), reference);
        assert_eq!(listed[0].cwd.as_deref(), Some("/srv/api"));
        assert_eq!(listed[0].attached, 1, "this window is attached");

        // The window goes away: a new one finds it, attached to by no one,
        // and attaches with the replay.
        drop(terminal);
        drop(window);
        let next = rig.window();
        until("the old window to be gone", || {
            next.held().unwrap()[0].attached == 0
        });
        let held = next.held().unwrap().remove(0);
        let again = next.attach(&held, theme(), Box::new(|| {})).unwrap();
        until("the replay", || {
            let screen = again.screen_text();
            screen.contains("ready") && screen.contains("echo:hi")
        });
        assert!(next.end_all(), "the keeper confirmed");
        until("the program to be listed as ended", || {
            next.held().unwrap()[0].exit.is_some()
        });
    }

    #[test]
    fn ending_everything_ends_only_what_this_window_holds() {
        let rig = Rig::new();
        let (one, other) = (rig.window(), rig.window());
        let (_a, slot_a) = open(&one);
        let (_b, slot_b) = open(&other);
        until("both", || slot_a.get().is_some() && slot_b.get().is_some());
        assert!(one.end_all(), "confirmed");
        let listed = other.held().unwrap();
        let state = |slot: &KeeperSlot| {
            let pty = slot.get().unwrap().pty;
            listed.iter().find(|h| h.pty == pty).unwrap().exit.is_some()
        };
        assert!(state(&slot_a), "its own is ended");
        assert!(!state(&slot_b), "another window's is not");
    }

    #[test]
    fn settling_waits_for_a_hang_up_to_reach_the_keeper() {
        let rig = Rig::new();
        let window = rig.window();
        let (terminal, slot) = open(&window);
        until("the reference", || slot.get().is_some());
        let pty = slot.get().unwrap().pty;
        terminal.kill();
        assert!(window.settle(), "the keeper has been told");
        let held = window.held().unwrap();
        until("the program to end", || {
            window
                .held()
                .unwrap()
                .iter()
                .any(|h| h.pty == pty && h.exit.is_some())
        });
        let _ = held;
    }

    #[test]
    fn a_folder_that_is_a_link_or_open_to_others_is_never_talked_through() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let base = tempfile::tempdir().unwrap();
        let real = base.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let open_dir = base.path().join("open");
        std::fs::create_dir(&open_dir).unwrap();
        std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        for dir in [link, open_dir] {
            let paths = KeeperPaths {
                socket: dir.join("k.sock"),
                lock: dir.join("k.lock"),
                dir,
            };
            // A listener of someone else's would be here; nothing may reach it.
            let keeper = LocalKeeper::new(
                paths.clone(),
                base.path().join("no-such-leon"),
                base.path().join("data"),
                runtime.handle().clone(),
            );
            let spec = SpawnSpec::new("zsh");
            let refused = keeper.open(&spec, GridSize::new(80, 24), theme(), Box::new(|| {}), "t");
            let why = refused.err().expect("refused").0;
            assert!(why.contains("cannot be trusted"), "{why}");
            assert!(keeper.held().is_err());
            assert_eq!(keeper.keeper_pid(), None);
        }
    }

    #[test]
    fn a_keeper_that_cannot_be_started_is_said_in_the_terminal_and_not_tried_again_at_once() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let base = tempfile::tempdir().unwrap();
        let paths = keeper::paths(
            Some(base.path().to_path_buf()),
            keeper::our_uid(),
            base.path(),
            &base.path().join("data"),
        );
        let keeper = LocalKeeper::new(
            paths,
            base.path().join("no-such-leon"),
            base.path().join("data"),
            runtime.handle().clone(),
        );
        let spec = SpawnSpec::new("zsh");
        let (terminal, _slot) = keeper
            .open(&spec, GridSize::new(80, 24), theme(), Box::new(|| {}), "a")
            .unwrap();
        until("the terminal to say why", || terminal.exit_info().is_some());
        assert!(
            terminal
                .screen_text()
                .contains("Cannot start this under the keeper"),
            "{}",
            terminal.screen_text()
        );
        // The next one is refused at once, so it starts in the window.
        let again = keeper.open(&spec, GridSize::new(80, 24), theme(), Box::new(|| {}), "b");
        assert!(again.is_err());
    }

    #[test]
    fn a_relative_data_directory_names_the_same_keeper_from_anywhere() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let keeper = LocalKeeper::new(
            KeeperPaths {
                dir: "/nowhere".into(),
                socket: "/nowhere/s".into(),
                lock: "/nowhere/l".into(),
            },
            "/bin/leon".into(),
            "relative/data".into(),
            runtime.handle().clone(),
        );
        assert!(keeper.data_dir().is_absolute());
        assert!(keeper.data_dir().ends_with("relative/data"));
    }
}
