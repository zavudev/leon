//! The keeper: the process that owns the terminals of durable local sessions.
//!
//! With durable sessions on, the application does not hold the pseudo-terminals
//! of the sessions it opens on this computer: a separate process of the same
//! binary does (`leon keeper`, started on demand, detached from the window and
//! its process group). The terminals keep running when the window closes, when
//! the application quits and when it crashes; the next start lists them and
//! attaches again with the output that was missed. It is the same machinery as
//! a shared computer's terminals (`ptys`: the table and its replay ring) and the
//! same wire messages; only the way in differs:
//!
//! * the way in is a Unix domain socket, in a directory only this user can
//!   enter, named after a hash of the data directory ([`paths`]), so an
//!   instance started with `--data-dir` has a keeper of its own;
//! * nothing is encrypted and nothing is paired: the file permissions and a
//!   check of the user id of every connection ([`peer_allowed`]) are the
//!   authentication. Whoever connects gets a shell as the user, so a peer whose
//!   id cannot be read, or is not ours, is turned away before it says anything;
//! * one keeper per data directory: an exclusive lock on a file next to the
//!   socket is held for its whole life ([`Keeper::bind`]), so a stale socket of
//!   a keeper that died is replaced by the one that holds the lock, and never
//!   by a second keeper racing the first;
//! * it ends by itself, [`KeeperConfig::idle_after`] after its last program
//!   ended and its last client left ([`idle_expired`]).
//!
//! Terminals are started with the environment the client sends and nothing of
//! the keeper's own, which is whatever it was when the first terminal was
//! asked for, and stale after that.
//!
//! A keeper keeps running the binary it was started from. An update replaces
//! the file on disk, not the running process: the old keeper serves the old
//! build until its terminals end, and the next one started is the new build.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use leon_link::local::{pipe_over, PlainChannel};
use leon_wire::{ErrorCode, Message, WireError, PROTOCOL_VERSION, REPLAY_BUFFER_BYTES};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch, Notify};

use crate::ptys::{Outbox, PtyTable, RealSpawner, Spawner};
use crate::session::{is_pty_request, pty_request};

/// The longest socket path the operating systems accept is about a hundred
/// bytes (104 on macOS, 108 on Linux, with the terminating zero).
pub const SOCKET_BUDGET: usize = 100;
/// How long an exited terminal stays listed, for an app that comes back.
const RETENTION: Duration = Duration::from_secs(10 * 60);
/// Most terminals one keeper holds.
const MAX_PTYS: usize = 128;
/// Most clients at once: the window, and a second window of the same data
/// directory, with room to spare.
const MAX_CLIENTS: usize = 8;
/// Messages queued for one client.
const OUTBOX: usize = 1024;
/// How long a client may take to say hello.
const HELLO_WITHIN: Duration = Duration::from_secs(5);
/// How long the keeper waits, with no terminal running and no client, before
/// it ends.
pub const IDLE_AFTER: Duration = Duration::from_secs(30);

// ----- where it lives (pure) ----------------------------------------------------------------

/// Where a keeper lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeeperPaths {
    /// The directory, made for the user alone.
    pub dir: PathBuf,
    /// The socket clients connect to.
    pub socket: PathBuf,
    /// The lock that makes one process the keeper; it holds that process's pid.
    pub lock: PathBuf,
}

/// A stable name for a data directory: 64-bit FNV-1a of its bytes, in hex. It
/// has to be the same for every build (an updated application must find the
/// keeper an older one started), which rules out the standard hasher, and it
/// only tells directories apart: it is no secret and protects nothing.
pub fn data_dir_key(data_dir: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data_dir.as_os_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Where the keeper of `data_dir` lives: under the platform's runtime
/// directory (`XDG_RUNTIME_DIR`) when a socket path under it fits the limit,
/// else in a directory named after the user under `tmp`.
pub fn paths(runtime: Option<PathBuf>, uid: u32, tmp: &Path, data_dir: &Path) -> KeeperPaths {
    let key = data_dir_key(data_dir);
    let in_dir = |dir: PathBuf| KeeperPaths {
        socket: dir.join(format!("{key}.sock")),
        lock: dir.join(format!("{key}.lock")),
        dir,
    };
    if let Some(base) = runtime {
        let candidate = in_dir(base.join("leon-keeper"));
        if candidate.socket.as_os_str().len() <= SOCKET_BUDGET {
            return candidate;
        }
    }
    in_dir(tmp.join(format!("leon-keeper-{uid}")))
}

/// [`paths`] for this process: its user, its `XDG_RUNTIME_DIR` (when it is an
/// absolute path) and `/tmp`. `data_dir` is made absolute first, so a relative
/// `--data-dir` names the same keeper from wherever it is given.
pub fn paths_here(data_dir: &Path) -> KeeperPaths {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let data_dir = std::fs::canonicalize(data_dir)
        .or_else(|_| std::path::absolute(data_dir))
        .unwrap_or_else(|_| data_dir.to_path_buf());
    paths(runtime, our_uid(), Path::new("/tmp"), &data_dir)
}

pub use leon_link::local::our_uid;

pub use leon_link::local::{check_private_dir, judge_dir, peer_allowed, probe_socket, DirRefusal};

/// Whether the keeper has been idle long enough to end: nothing runs, nobody
/// is connected, and that has been so for `idle_after`.
pub fn idle_expired(busy: bool, idle_for: Duration, idle_after: Duration) -> bool {
    !busy && idle_for >= idle_after
}

/// Creates `path` for the user alone and checks that it is: a real directory
/// (not a link somebody else planted), owned by this user and closed to
/// everyone else (the same check a client makes before it dials, see
/// [`check_private_dir`]). A directory of ours that is open is closed; one
/// that is not ours is refused, never touched.
pub fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
    {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let refuse = |why: DirRefusal| std::io::Error::other(format!("{}: {why}", path.display()));
    match check_private_dir(path, our_uid()) {
        Err(DirRefusal::TooOpen) => {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            check_private_dir(path, our_uid()).map_err(refuse)
        }
        other => other.map_err(refuse),
    }
}

// ----- the keeper ---------------------------------------------------------------------------

/// How a keeper is set up.
#[derive(Clone)]
pub struct KeeperConfig {
    /// Where it lives.
    pub paths: KeeperPaths,
    /// The only user whose connections are served.
    pub uid: u32,
    /// Output kept per terminal.
    pub ring_bytes: usize,
    /// How long an exited terminal stays listed.
    pub retention: Duration,
    /// Most terminals.
    pub max_ptys: usize,
    /// How long it stays with nothing to keep and nobody to serve.
    pub idle_after: Duration,
    /// What starts the programs.
    pub spawner: Arc<dyn Spawner>,
    /// The number the first terminal gets.
    pub first_id: u64,
}

impl KeeperConfig {
    /// The defaults for a keeper at `paths`, for this user, starting real
    /// terminals with the environment each request carries. Its terminals are
    /// numbered from a random base.
    pub fn new(paths: KeeperPaths) -> Self {
        use rand_core::RngCore;
        let mut base = [0u8; 4];
        rand_core::OsRng.fill_bytes(&mut base);
        Self {
            paths,
            uid: our_uid(),
            ring_bytes: REPLAY_BUFFER_BYTES,
            retention: RETENTION,
            max_ptys: MAX_PTYS,
            idle_after: IDLE_AFTER,
            spawner: Arc::new(RealSpawner {
                own_environment: true,
            }),
            // Room for a billion terminals before the number wraps into the
            // next keeper's.
            first_id: u64::from(u32::from_le_bytes(base)) << 20 | 1,
        }
    }
}

/// Why a keeper did not start.
#[derive(Debug)]
pub enum KeeperError {
    /// Another keeper holds the lock for this data directory.
    AlreadyRunning,
    /// Something else failed, in a sentence.
    Failed(String),
}

impl std::fmt::Display for KeeperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeeperError::AlreadyRunning => f.write_str("a keeper is already running"),
            KeeperError::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for KeeperError {}

fn failed(what: &str, error: std::io::Error) -> KeeperError {
    KeeperError::Failed(format!("{what}: {error}"))
}

/// Takes the lock that makes this process the keeper, and writes its pid in it.
fn take_lock(path: &Path) -> Result<File, KeeperError> {
    use std::io::Write;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| failed("cannot open the lock", e))?;
    // SAFETY: `flock` on a descriptor this process owns.
    let locked = unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    if locked != 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
            KeeperError::AlreadyRunning
        } else {
            failed("cannot lock", error)
        });
    }
    file.set_len(0)
        .map_err(|e| failed("cannot write the lock", e))?;
    write!(file, "{}", std::process::id()).map_err(|e| failed("cannot write the lock", e))?;
    Ok(file)
}

/// The pid of the keeper holding `lock`, as it wrote it; `None` when there is
/// no lock file or it says nothing usable. Only a hint: the file outlives a
/// keeper that died.
pub fn read_pid(lock: &Path) -> Option<u32> {
    std::fs::read_to_string(lock).ok()?.trim().parse().ok()
}

/// A bound keeper, ready to [`serve`](Keeper::serve).
pub struct Keeper {
    cfg: KeeperConfig,
    listener: UnixListener,
    table: Arc<PtyTable>,
    clients: Arc<AtomicUsize>,
    next_session: Arc<AtomicU64>,
    stop: watch::Sender<bool>,
    // Held for the keeper's whole life: the lock goes with the process.
    _lock: File,
}

impl Keeper {
    /// Takes the lock and binds the socket. A socket left by a keeper that
    /// died is removed (this process holds the lock, so no live keeper owns
    /// it). Must be called inside a Tokio runtime.
    pub fn bind(cfg: KeeperConfig) -> Result<Self, KeeperError> {
        ensure_private_dir(&cfg.paths.dir).map_err(|e| failed("cannot make the directory", e))?;
        let lock = take_lock(&cfg.paths.lock)?;
        match std::fs::remove_file(&cfg.paths.socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(failed("cannot replace the old socket", error)),
        }
        let listener =
            UnixListener::bind(&cfg.paths.socket).map_err(|e| failed("cannot bind", e))?;
        // The directory already keeps others out; this is the second lock.
        std::fs::set_permissions(&cfg.paths.socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| failed("cannot protect the socket", e))?;
        let table = PtyTable::with_spawner(
            cfg.ring_bytes,
            cfg.retention,
            cfg.max_ptys,
            cfg.first_id,
            cfg.spawner.clone(),
        );
        let (stop, _) = watch::channel(false);
        Ok(Self {
            cfg,
            listener,
            table,
            clients: Arc::new(AtomicUsize::new(0)),
            next_session: Arc::new(AtomicU64::new(1)),
            stop,
            _lock: lock,
        })
    }

    /// Ends [`Keeper::serve`] from outside (a test, or a signal).
    pub fn stopper(&self) -> watch::Sender<bool> {
        self.stop.clone()
    }

    /// The terminals, for a test to look at.
    pub fn table(&self) -> &Arc<PtyTable> {
        &self.table
    }

    /// Serves until it has been idle for [`KeeperConfig::idle_after`] or is
    /// stopped, then removes its socket. Programs still running are not
    /// touched by that: the idle end is only reached when none runs, and a
    /// stop is the end of the process, which takes them with it.
    pub async fn serve(self) {
        let mut stop = self.stop.subscribe();
        let mut idle_since = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => self.accept(stream),
                    // Out of descriptors, or the like: waiting is better than
                    // spinning on the same error.
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                },
                _ = tick.tick() => {
                    self.table.reap(Instant::now());
                    let busy = self.clients.load(Ordering::Relaxed) > 0 || self.table.running() > 0;
                    if busy {
                        idle_since = Instant::now();
                    }
                    if idle_expired(busy, idle_since.elapsed(), self.cfg.idle_after) {
                        // A client that connected while this was decided is
                        // waiting in the backlog: serve it and stay, instead
                        // of removing the socket from under it.
                        if let Ok(Ok((stream, _))) =
                            tokio::time::timeout(Duration::ZERO, self.listener.accept()).await
                        {
                            self.accept(stream);
                            idle_since = Instant::now();
                            continue;
                        }
                        break;
                    }
                }
                _ = stop.changed() => break,
            }
        }
        let _ = std::fs::remove_file(&self.cfg.paths.socket);
    }

    /// Checks who connected and, when it is us, serves it.
    fn accept(&self, stream: UnixStream) {
        let peer = stream.peer_cred().ok().map(|cred| cred.uid());
        if !peer_allowed(peer, self.cfg.uid) {
            // Dropped without a word: it learns nothing about what is here.
            return;
        }
        let full = self.clients.load(Ordering::Relaxed) >= MAX_CLIENTS;
        let count = (!full).then(|| Counter::new(&self.clients));
        let table = self.table.clone();
        let session = self.next_session.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            let _count = count;
            serve_client(table, PlainChannel::new(pipe_over(stream)), session, full).await;
        });
    }
}

struct Counter(Arc<AtomicUsize>);

impl Counter {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter.clone())
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn error(id: Option<u64>, code: ErrorCode, message: &str) -> Message {
    Message::Error {
        id,
        error: WireError {
            code,
            message: message.into(),
        },
    }
}

/// One client: the greeting, then its requests about terminals until it goes.
/// Its terminals keep running when it does.
async fn serve_client(table: Arc<PtyTable>, mut channel: PlainChannel, session: u64, full: bool) {
    let hello = tokio::time::timeout(HELLO_WITHIN, channel.recv()).await;
    let Ok(Ok(Some(Message::Hello { protocol, .. }))) = hello else {
        return;
    };
    if full {
        // Said, not dropped: the other side shows why it is turned away.
        let _ = channel
            .send(&error(
                None,
                ErrorCode::Busy,
                "too many windows are connected to this keeper",
            ))
            .await;
        return;
    }
    if protocol != PROTOCOL_VERSION {
        let _ = channel
            .send(&error(
                None,
                ErrorCode::Unsupported,
                "unsupported protocol version",
            ))
            .await;
        return;
    }
    let ours = Message::Hello {
        protocol: PROTOCOL_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        device_name: "keeper".into(),
        token: None,
    };
    if channel.send(&ours).await.is_err() {
        return;
    }
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(OUTBOX);
    let lagged = Arc::new(Notify::new());
    let outbox = Outbox {
        session,
        tx: out_tx.clone(),
        lagged: lagged.clone(),
    };
    // Writing is a task of its own: this one never waits to write before it
    // reads, or a client that sends a flood while it is sent one would wait
    // for this end to read, and this end for it, for ever.
    let (sender, mut receiver) = channel.split();
    let mut writer = {
        let sender = sender.clone();
        tokio::spawn(async move {
            while let Some(message) = out_rx.recv().await {
                if sender.send(&message).await.is_err() {
                    break;
                }
            }
        })
    };
    'session: loop {
        tokio::select! {
            incoming = receiver.recv() => match incoming {
                Ok(Some(message)) => {
                    if !is_pty_request(&message) {
                        // Running commands and sharing history are for hosts
                        // that pair devices; the keeper only holds terminals.
                        let _ = out_tx.try_send(error(None, ErrorCode::BadRequest, "unexpected message"));
                    } else if !pty_request(&table, message, &outbox, &out_tx) {
                        break 'session;
                    }
                }
                Ok(None) | Err(_) => break 'session,
            },
            _ = &mut writer => break 'session,
            _ = lagged.notified() => {
                let _ = tokio::time::timeout(
                    Duration::from_secs(1),
                    sender.send(&error(None, ErrorCode::Busy, "the connection could not keep up; reconnect")),
                )
                .await;
                break 'session;
            }
        }
    }
    writer.abort();
    table.detach_session(session);
}

// ----- the command ---------------------------------------------------------------------------

/// `leon keeper --data-dir <path>`: runs the keeper of that data directory
/// until it is idle, and returns the process's exit status (0 for an ordinary
/// end or when another keeper already runs). Not for people: the application
/// starts it, detached, when a durable session is opened.
pub fn run(arguments: Vec<String>) -> i32 {
    let mut data_dir = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data-dir" => data_dir = arguments.next().map(PathBuf::from),
            _ => return 2,
        }
    }
    let Some(data_dir) = data_dir else { return 2 };
    // A keeper does not change its umask: the terminals it starts inherit it,
    // and a shell that makes files nobody else can read would be a surprise.
    // What it creates itself is closed explicitly.
    let _ = std::env::set_current_dir("/");
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("keeper")
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 1,
    };
    runtime.block_on(async {
        let paths = paths_here(&data_dir);
        // A keeper that is ending has removed its socket but may hold the lock
        // for a moment longer: that is not "a keeper is running". Only a
        // socket that answers is.
        let give_up = Instant::now() + Duration::from_secs(3);
        loop {
            match Keeper::bind(KeeperConfig::new(paths.clone())) {
                Ok(keeper) => {
                    keeper.serve().await;
                    return 0;
                }
                Err(KeeperError::AlreadyRunning) => {
                    if probe_socket(&paths.socket).is_ok() || Instant::now() >= give_up {
                        return 0;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(_) => return 1,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here(runtime: Option<&str>, data: &str) -> KeeperPaths {
        paths(
            runtime.map(PathBuf::from),
            1000,
            Path::new("/tmp"),
            Path::new(data),
        )
    }

    #[test]
    fn the_keeper_lives_under_the_runtime_directory_when_there_is_one() {
        let p = here(Some("/run/user/1000"), "/home/ana/.local/share/leon");
        assert_eq!(p.dir, Path::new("/run/user/1000/leon-keeper"));
        assert_eq!(
            p.socket,
            Path::new("/run/user/1000/leon-keeper").join(format!(
                "{}.sock",
                data_dir_key(Path::new("/home/ana/.local/share/leon"))
            ))
        );
        assert_eq!(p.lock.parent(), Some(p.dir.as_path()));
        assert!(p.lock.extension().is_some_and(|e| e == "lock"));
    }

    #[test]
    fn without_a_runtime_directory_the_user_gets_a_directory_of_their_own_in_tmp() {
        let p = here(None, "/home/ana/.local/share/leon");
        assert_eq!(p.dir, Path::new("/tmp/leon-keeper-1000"));
        assert!(p.socket.starts_with("/tmp/leon-keeper-1000"));
    }

    #[test]
    fn a_runtime_directory_that_makes_the_socket_path_too_long_is_not_used() {
        let long = "/var/folders/g4/0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdef/T";
        let p = here(Some(long), "/home/ana/.local/share/leon");
        assert_eq!(p.dir, Path::new("/tmp/leon-keeper-1000"));
        assert!(p.socket.as_os_str().len() <= SOCKET_BUDGET);
        // The short case is under the limit too.
        let short = here(Some("/run/user/1000"), "/x");
        assert!(short.socket.as_os_str().len() <= SOCKET_BUDGET);
    }

    #[test]
    fn each_data_directory_has_its_own_keeper_and_the_name_never_changes() {
        let a = here(Some("/run/user/1000"), "/home/ana/.local/share/leon");
        let b = here(Some("/run/user/1000"), "/tmp/other-instance");
        assert_ne!(a.socket, b.socket);
        assert_ne!(a.lock, b.lock);
        // Pinned: an updated build must find the keeper an older one started.
        assert_eq!(data_dir_key(Path::new("")), "cbf29ce484222325");
        assert_eq!(data_dir_key(Path::new("/a")), "07d66707b49cd92d");
        assert_eq!(data_dir_key(Path::new("/a")).len(), 16);
    }

    #[test]
    fn two_users_never_share_a_fallback_directory() {
        let one = paths(None, 1, Path::new("/tmp"), Path::new("/d"));
        let two = paths(None, 2, Path::new("/tmp"), Path::new("/d"));
        assert_ne!(one.dir, two.dir);
    }

    #[test]
    fn only_our_own_user_may_connect_and_an_unreadable_peer_is_refused() {
        assert!(peer_allowed(Some(1000), 1000));
        assert!(!peer_allowed(Some(0), 1000), "not even root");
        assert!(!peer_allowed(Some(1001), 1000));
        assert!(!peer_allowed(None, 1000), "an id that cannot be read");
        assert!(!peer_allowed(None, 0));
    }

    #[test]
    fn the_keeper_ends_only_when_nothing_runs_nobody_is_connected_and_it_has_been_so_a_while() {
        let wait = Duration::from_secs(30);
        assert!(!idle_expired(false, Duration::from_secs(29), wait));
        assert!(idle_expired(false, Duration::from_secs(30), wait));
        assert!(!idle_expired(true, Duration::from_secs(3600), wait));
    }

    #[test]
    fn the_private_directory_is_made_closed_and_an_open_one_is_closed() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("k");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn a_file_or_a_link_in_place_of_the_directory_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let file = base.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert!(ensure_private_dir(&file).is_err());
        let link = base.path().join("l");
        std::os::unix::fs::symlink(base.path(), &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());
    }

    #[test]
    fn a_second_process_cannot_take_the_lock_and_the_pid_is_readable() {
        let base = tempfile::tempdir().unwrap();
        let lock = base.path().join("k.lock");
        let held = take_lock(&lock).unwrap();
        assert_eq!(read_pid(&lock), Some(std::process::id()));
        // flock is per open file description, so a second open in the same
        // process stands for another process.
        assert!(matches!(take_lock(&lock), Err(KeeperError::AlreadyRunning)));
        drop(held);
        // The lock goes with its holder. A process another test forks at this
        // very moment shares the descriptor until it execs, so it may take a
        // moment.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while take_lock(&lock).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "the lock was not released"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_command_needs_its_data_directory_and_knows_no_other_argument() {
        assert_eq!(run(vec![]), 2);
        assert_eq!(run(vec!["--nope".into()]), 2);
    }
}
