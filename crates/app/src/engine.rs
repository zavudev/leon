//! The engine: everything that works in the background.
//!
//! The UI only ever reads the local store. The engine is what keeps the store
//! fresh: it imports the local agent history, discovers projects from the
//! folders the sessions ran in, asks git which worktrees each project has,
//! probes machines, checks a machine's folder and agent before a session is
//! resumed there and, on request, creates and removes worktrees. It writes the store (which announces each write as a
//! [`StoreChange`](leon_core::StoreChange)) and nothing else; the one thing it
//! keeps itself is a status line and the state of each machine, announced as
//! [`EngineEvent`]s, because neither belongs in a database.
//!
//! Nothing in here panics on a failing command or a missing machine. Every
//! failure becomes an error status the UI can show.
//!
//! The engine is not generic over the [`Runner`]: [`Exec`] is a small
//! object-safe wrapper, so the UI holds one concrete `Engine` while tests hand
//! it a scripted runner and an in-memory store.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use leon_core::icon::{IconImage, IconKind, NewIcon};
use leon_core::{
    AgentId, Machine, MachineId, MachineKind, NewWorktree, ProjectId, SessionFilter, Store,
    StoreError, WorktreeId,
};
use leon_history::{HistoryRoots, ImportReport, Importer};
use leon_remote::connect::{self, Checklist, Target as Login};
use leon_remote::{
    probe, CommandSpec, Git, GitError, Output, ProbeError, ProbeReport, RunError, Runner,
    SshOptions,
};
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, Semaphore};
use tokio::task::{JoinHandle, JoinSet};

use crate::address;
use crate::avatar::{Fetched, IconFetcher, NoFetch};
use crate::elsewhere::{self, Found};

/// How many folders are resolved with git at the same time while projects are
/// being discovered.
const DISCOVERY_PARALLELISM: usize = 8;

/// How many of a machine's sessions a scan reads to tie processes to them.
const SCAN_SESSIONS: usize = 50_000;

/// A boxed, sendable future.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An object-safe [`Runner`].
pub trait Exec: Send + Sync {
    /// Runs `spec` and waits for it to finish.
    fn exec<'a>(&'a self, spec: &'a CommandSpec) -> BoxFuture<'a, Result<Output, RunError>>;
}

impl<R: Runner> Exec for R {
    fn exec<'a>(&'a self, spec: &'a CommandSpec) -> BoxFuture<'a, Result<Output, RunError>> {
        Box::pin(self.run(spec))
    }
}

/// The runner the git and probe code is handed: a shared [`Exec`].
struct SharedRunner(Arc<dyn Exec>);

impl Runner for SharedRunner {
    async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
        self.0.exec(spec).await
    }
}

/// A job running in the background whose result is read once. Dropping it
/// stops the job: that is how a test of a connection or a search of a
/// machine's folders is cancelled.
pub struct Background<T> {
    rx: tokio::sync::oneshot::Receiver<T>,
    task: JoinHandle<()>,
}

impl<T> Background<T> {
    /// Waits for the result; `None` when the job was stopped.
    pub async fn finish(mut self) -> Option<T> {
        (&mut self.rx).await.ok()
    }
}

impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A test of a connection under way: the checklist as far as it has got, read
/// through `updates`. Dropping it cancels the test (the `ssh` it runs is
/// killed with it).
pub struct CheckRun {
    /// The latest checklist; changes after every step.
    pub updates: tokio::sync::watch::Receiver<Checklist>,
    task: JoinHandle<()>,
}

impl Drop for CheckRun {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What the engine can be asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Import the local agent history, discover projects from the folders the
    /// sessions ran in, sync every project's worktrees and probe the SSH
    /// machines. What happens at start and on "refresh".
    Refresh,
    /// Import the local agent history.
    #[allow(dead_code)] // As above: `Refresh` is what the UI asks for today.
    ImportHistory,
    /// Collect the history report of "Why is a session missing?".
    DiagnoseHistory,
    /// Import what changed in the agents' history since the last time, and
    /// say nothing unless a source cannot be read. What the window asks for
    /// after an agent went quiet or ended, when it regains the focus and on
    /// a timer.
    SyncHistory,
    /// Ask git for the worktrees of a project.
    #[allow(dead_code)] // The vocabulary of the engine; the UI asks through `Refresh` for now.
    SyncWorktrees(ProjectId),
    /// Inventory a machine.
    Probe(MachineId),
    /// Look for agent processes on a machine that hold history sessions: the
    /// sessions running in another terminal.
    Scan(MachineId),
    /// Save a project, then sync its worktrees.
    AddProject {
        /// The machine it lives on.
        machine: MachineId,
        /// Absolute path of its root on that machine.
        path: String,
        /// The name shown in the list.
        name: String,
    },
    /// Remove a project from the list and remember that it was removed, so
    /// that discovery does not bring it back.
    RemoveProject(ProjectId),
    /// Give an SSH machine another name.
    RenameMachine {
        /// The machine.
        machine: MachineId,
        /// Its new name.
        name: String,
    },
    /// Remove an SSH machine with its projects and sessions.
    RemoveMachine(MachineId),
    /// Remove one session from the history (the agent's own file is left
    /// alone).
    RemoveSession(leon_core::SessionId),
    /// Let discovery adopt a project root that was removed.
    RestoreRoot {
        /// The machine the root is on.
        machine: MachineId,
        /// The root.
        root: String,
    },
    /// Create a worktree on a new branch, then sync.
    AddWorktree {
        /// The project the worktree belongs to.
        project: ProjectId,
        /// The new branch.
        branch: String,
        /// What the branch starts from; `HEAD` when absent.
        base: Option<String>,
    },
    /// Remove a worktree, then sync.
    RemoveWorktree {
        /// The project the worktree belongs to.
        project: ProjectId,
        /// The worktree to remove.
        worktree: WorktreeId,
    },
    /// Look for the project's logo again (a file of the repository, else the
    /// owner's avatar).
    DetectIcon(ProjectId),
    /// Use an image file of this computer as the project's logo.
    SetIcon {
        /// The project.
        project: ProjectId,
        /// The file the user chose.
        path: std::path::PathBuf,
    },
    /// Forget the user's choice of logo; detection's shows again.
    ResetIcon(ProjectId),
    /// Read the usage limits of every machine, as the schedule does: a source
    /// that is backing off, or whose sign-in was refused, is left alone.
    CollectUsage,
    /// Read the usage limits now because the user asked, or because a setting
    /// changed: for these agents (every network source when empty) the
    /// back-off and the refusal remembered for the session are forgotten. A
    /// request that comes while a read is under way is run right after it.
    CollectUsageNow(Vec<leon_core::AgentId>),
    /// Delete the stored usage observations.
    ForgetUsageHistory,
}

/// Whether the status line reports something done, something under way or
/// something wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Something finished.
    Info,
    /// Work under way.
    Busy,
    /// A failure.
    Error,
}

/// The one line the UI shows about what the engine did last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// Whether it is good news.
    pub kind: StatusKind,
    /// What happened, in a short sentence.
    pub text: String,
}

/// What the engine knows of a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineState {
    /// Not probed yet.
    Unknown,
    /// A probe is running.
    Probing,
    /// Reachable. The report is absent for the local machine until it has
    /// been probed.
    Online(Option<ProbeReport>),
    /// Unreachable, with the reason.
    Offline(String),
}

/// What was found out about a machine before a session is resumed on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The machine answers, has the agent and has the folder.
    Ready,
    /// The machine did not answer; the text says why, with its name.
    Offline(String),
    /// The machine answers but the agent is not installed on it.
    AgentMissing,
    /// The machine answers but the folder is not there any more.
    FolderMissing,
}

/// What changed inside the engine: the UI reads it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineEvent {
    /// The status line changed.
    Status,
    /// A machine's state changed.
    Machines,
    /// What runs in other terminals changed.
    Elsewhere,
    /// A usage collection started or ended.
    Usage,
    /// The history report of "Why is a session missing?" changed.
    History,
}

/// Why an operation failed. The text is what the status line says.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The store refused.
    #[error("{0}")]
    Store(#[from] StoreError),
    /// Git failed.
    #[error("{0}")]
    Git(#[from] GitError),
    /// The probe failed.
    #[error("{0}")]
    Probe(#[from] ProbeError),
    /// The request was refused before anything ran.
    #[error("{0}")]
    Invalid(String),
    /// A background job did not finish.
    #[error("A background job failed: {0}")]
    Job(String),
}

/// What a scan of one machine found: its agent processes, and which stored
/// session each holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elsewhere {
    /// The agent processes, Leon's own among them (see `Found::leon_child`).
    pub found: Vec<Found>,
    /// How long the scan took, command and matching.
    pub took: std::time::Duration,
}

/// What collecting usage limits needs. Absent until the application supplies
/// it, so an engine in a test never reads a credential or calls a network.
struct UsageSetup {
    credentials: Arc<dyn leon_usage::network::Credentials>,
    http: Arc<dyn leon_usage::network::Http>,
    /// The time, in Unix seconds; injected so tests control it.
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// The back-off of each network source.
    throttle: HashMap<leon_core::AgentId, leon_usage::network::Throttle>,
    /// When each network source was last called (Unix seconds): a scheduled
    /// read never calls a vendor again within [`leon_usage::network::MIN_GAP`],
    /// whatever the refresh setting says.
    last_called: HashMap<leon_core::AgentId, i64>,
    /// Whether a collection is running: at most one request per source is in
    /// flight at any time.
    collecting: bool,
    /// Whether another collection was asked for while one was running.
    rerun: bool,
    /// The agents whose network source is being read right now.
    reading: std::collections::HashSet<leon_core::AgentId>,
    /// Why the last read of a source failed, until one succeeds.
    failed: HashMap<leon_core::AgentId, leon_usage::Reason>,
    /// The sources whose sign-in the system refused: not asked again this
    /// session until the user says so.
    refused: std::collections::HashSet<leon_core::AgentId>,
    /// Whether nothing is read yet: the application starts reading only once
    /// its window is up.
    deferred: bool,
}

/// What the engine knows of the last read of one agent's network source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceStatus {
    /// A read is under way.
    pub reading: bool,
    /// Why the last read failed, when it did.
    pub failed: Option<leon_usage::Reason>,
    /// When the source may be called again after a failure (Unix seconds).
    pub retry_at: Option<i64>,
    /// Whether the system refused the sign-in and the source waits for the
    /// user to try again.
    pub refused: bool,
}

/// Whether the network source of `agent` is switched on in `policy`.
fn asked_on(policy: &leon_usage::network::NetworkPolicy, agent: leon_core::AgentId) -> bool {
    policy.allows(agent)
}

/// A jitter of 0 to 10 percent, so that many installs do not call a vendor at
/// the same moment.
fn jitter_percent() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    i64::from(nanos % 11)
}

/// How processes are listed: through a runner, on this computer or over SSH.
struct Scanner {
    exec: Arc<dyn Exec>,
    /// This process's pid: what runs below it on this computer is Leon's own.
    leon_pid: Option<u32>,
}

struct State {
    /// The history report: `None` before it was asked, empty while it is
    /// being collected.
    history_report: Option<Vec<String>>,
    status: Option<StatusLine>,
    machines: HashMap<MachineId, MachineState>,
    elsewhere: HashMap<MachineId, Arc<Elsewhere>>,
}

/// What the user's settings change in the engine, live: how SSH is called,
/// where the history is read from, and which of the background jobs run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// Whether SSH connections are shared (when the platform has a directory
    /// for the sockets).
    pub ssh_multiplex: bool,
    /// How long an idle shared connection stays open, in minutes.
    pub ssh_persist_minutes: u32,
    /// How long `ssh` waits to connect, in seconds; none for ssh's own limit.
    pub ssh_connect_timeout: Option<u32>,
    /// Where each agent's history is read from.
    pub roots: HistoryRoots,
    /// Whether the folders of the sessions become projects.
    pub discover_projects: bool,
    /// Whether the repository is searched for a project's logo by itself.
    pub detect_logos: bool,
    /// Whether the owner's avatar may be downloaded from the Git host.
    pub fetch_avatars: bool,
}

struct Inner {
    store: Arc<Store>,
    runner: Arc<dyn Exec>,
    /// The directory of the shared connections' sockets, when there is one.
    ssh_dir: Option<std::path::PathBuf>,
    /// Where each agent's history is when the settings name no folder.
    default_roots: HistoryRoots,
    prefs: Mutex<Prefs>,
    handle: Handle,
    state: Mutex<State>,
    events: broadcast::Sender<EngineEvent>,
    fetcher: Mutex<Arc<dyn IconFetcher>>,
    scanner: Mutex<Option<Arc<Scanner>>>,
    usage: Mutex<Option<UsageSetup>>,
    /// What each history source looked like at the last incremental import.
    history_stamps: Mutex<HashMap<String, String>>,
    /// Whether the unreadable-layout notice was shown by an incremental import.
    unsupported_told: std::sync::atomic::AtomicBool,
    /// The home folder the history report writes as `~`.
    history_home: Mutex<Option<std::path::PathBuf>>,
    /// Which network sources are switched on.
    usage_policy: Mutex<leon_usage::network::NetworkPolicy>,
    /// How many jobs run on the blocking pool right now. They run on threads
    /// of their own, outside any scheduler a test controls, so a test waits for
    /// this to reach zero before it looks at what they stored.
    blocking: std::sync::atomic::AtomicUsize,
    /// Whether this computer has a POSIX shell of its own: false on Windows,
    /// where its usage is not collected and its processes are listed with
    /// PowerShell. Decided once from the platform; only tests change it.
    local_posix_shell: std::sync::atomic::AtomicBool,
}

/// The background half of the application. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// An engine over `store` that runs commands through `runner` and spawns
    /// its background jobs on `handle`.
    pub fn new<R: Runner + 'static>(
        store: Arc<Store>,
        runner: Arc<R>,
        ssh: SshOptions,
        roots: HistoryRoots,
        handle: Handle,
    ) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                store,
                runner,
                ssh_dir: ssh.control_dir.clone(),
                default_roots: roots.clone(),
                prefs: Mutex::new(Prefs {
                    ssh_multiplex: ssh.control_dir.is_some(),
                    ssh_persist_minutes: ssh.persist_minutes,
                    ssh_connect_timeout: ssh.connect_timeout,
                    roots,
                    discover_projects: false,
                    detect_logos: true,
                    fetch_avatars: true,
                }),
                handle,
                state: Mutex::new(State {
                    history_report: None,
                    status: None,
                    machines: HashMap::new(),
                    elsewhere: HashMap::new(),
                }),
                events,
                fetcher: Mutex::new(Arc::new(NoFetch)),
                scanner: Mutex::new(None),
                usage: Mutex::new(None),
                history_stamps: Mutex::new(HashMap::new()),
                unsupported_told: std::sync::atomic::AtomicBool::new(false),
                history_home: Mutex::new(leon_history::home_dir()),
                usage_policy: Mutex::new(leon_usage::network::NetworkPolicy::default()),
                blocking: std::sync::atomic::AtomicUsize::new(0),
                local_posix_shell: std::sync::atomic::AtomicBool::new(
                    crate::platform::local_has_posix_shell(),
                ),
            }),
        }
    }

    /// Runs `job` on the blocking pool and counts it while it runs.
    async fn blocking<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, EngineError> {
        use std::sync::atomic::Ordering;
        /// Takes the job off the count wherever the closure ends.
        struct Running(Arc<Inner>);
        impl Drop for Running {
            fn drop(&mut self) {
                self.0.blocking.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.inner.blocking.fetch_add(1, Ordering::SeqCst);
        let running = Running(self.inner.clone());
        tokio::task::spawn_blocking(move || {
            let _running = running;
            job()
        })
        .await
        .map_err(|error| EngineError::Job(error.to_string()))
    }

    /// How many blocking jobs are running (tests wait for zero).
    #[cfg(test)]
    pub(crate) fn blocking_jobs(&self) -> usize {
        self.inner
            .blocking
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Lets the engine download owner avatars with `fetcher`. Without one it
    /// never touches the network.
    pub fn set_icon_fetcher(&self, fetcher: Arc<dyn IconFetcher>) {
        *self
            .inner
            .fetcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = fetcher;
    }

    /// Lets the engine look for sessions that run in other terminals, through
    /// `runner`. Without one it never lists a process. `leon_pid` is this
    /// process: what runs below it is not "elsewhere".
    pub fn set_process_scanner<R: Runner + 'static>(&self, runner: Arc<R>, leon_pid: Option<u32>) {
        *self
            .inner
            .scanner
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(Scanner {
            exec: runner,
            leon_pid,
        }));
    }

    /// Whether processes are listed at all.
    pub fn scanning(&self) -> bool {
        self.scanner().is_some()
    }

    fn scanner(&self) -> Option<Arc<Scanner>> {
        self.inner
            .scanner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What the last scan of a machine found; `None` when there was none, or
    /// the last one could not be made (nothing is claimed then).
    pub fn elsewhere(&self, id: &MachineId) -> Option<Arc<Elsewhere>> {
        self.state().elsewhere.get(id).cloned()
    }

    /// Scans a machine now, in the background, and yields what it found
    /// (`None` when it could not be scanned).
    pub fn scan_elsewhere(&self, machine: MachineId) -> JoinHandle<Option<Arc<Elsewhere>>> {
        let engine = self.clone();
        self.inner
            .handle
            .spawn(async move { engine.scan_machine(&machine).await })
    }

    /// One command on the machine lists its processes; the result is tied to
    /// the stored sessions and kept. A scan that fails forgets what an earlier
    /// one found: nothing is claimed that is not known.
    async fn scan_machine(&self, id: &MachineId) -> Option<Arc<Elsewhere>> {
        let scanner = self.scanner()?;
        let machine = self.inner.store.machine(id).ok()?;
        let started = std::time::Instant::now();
        let local = machine.kind == MachineKind::Local;
        // PowerShell only for this computer when it is Windows; every remote
        // machine is POSIX wherever Leon runs.
        let spec = leon_remote::processes::scan_command(
            !local
                || self
                    .inner
                    .local_posix_shell
                    .load(std::sync::atomic::Ordering::Relaxed),
        );
        let runner = SharedRunner(scanner.exec.clone());
        let found = match runner
            .run(&leon_remote::run_on(&machine, &spec, &self.ssh()))
            .await
        {
            Ok(output) if output.success() => {
                let scan = leon_remote::processes::parse_scan(&output.stdout);
                let sessions = if scan.agents.is_empty() {
                    Vec::new()
                } else {
                    let filter = SessionFilter {
                        agent: None,
                        machine_id: Some(id.clone()),
                        project_id: None,
                    };
                    self.inner
                        .store
                        .recent_sessions(&filter, SCAN_SESSIONS)
                        .unwrap_or_default()
                };
                let leon_pid = scanner.leon_pid.filter(|_| local);
                Some(elsewhere::resolve(&scan, &sessions, leon_pid))
            }
            Ok(output) => {
                tracing::debug!(machine = %machine.name, status = ?output.status, "the process scan failed");
                None
            }
            Err(error) => {
                tracing::debug!(machine = %machine.name, %error, "the process scan could not run");
                None
            }
        };
        let report = found.map(|found| {
            Arc::new(Elsewhere {
                found,
                took: started.elapsed(),
            })
        });
        self.set_elsewhere(id, report.clone());
        report
    }

    fn set_elsewhere(&self, id: &MachineId, report: Option<Arc<Elsewhere>>) {
        let changed = {
            let mut state = self.state();
            let before = state.elsewhere.get(id).map(|r| r.found.clone());
            let after = report.as_ref().map(|r| r.found.clone());
            match report {
                Some(report) => state.elsewhere.insert(id.clone(), report),
                None => state.elsewhere.remove(id),
            };
            before != after
        };
        if changed {
            let _ = self.inner.events.send(EngineEvent::Elsewhere);
        }
    }

    // ----- usage limits ----------------------------------------------------

    /// Lets the engine collect usage limits: `credentials` and `http` serve
    /// the opt-in network sources, `clock` gives the time in Unix seconds.
    /// Without this the engine collects nothing.
    pub fn set_usage(
        &self,
        credentials: Arc<dyn leon_usage::network::Credentials>,
        http: Arc<dyn leon_usage::network::Http>,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) {
        *self.usage_setup() = Some(UsageSetup {
            credentials,
            http,
            clock,
            throttle: HashMap::new(),
            last_called: HashMap::new(),
            collecting: false,
            rerun: false,
            reading: Default::default(),
            failed: HashMap::new(),
            refused: Default::default(),
            deferred: false,
        });
    }

    /// Holds the reading back until [`Engine::start_usage`]: the application
    /// reads nothing (and so asks for no keychain access) before its window is
    /// up.
    pub fn defer_usage(&self) {
        if let Some(setup) = self.usage_setup().as_mut() {
            setup.deferred = true;
        }
    }

    /// Lets the reading begin. `true` when it had been held back.
    pub fn start_usage(&self) -> bool {
        self.usage_setup()
            .as_mut()
            .is_some_and(|setup| std::mem::take(&mut setup.deferred))
    }

    /// Which network sources are switched on.
    pub fn usage_policy(&self) -> leon_usage::network::NetworkPolicy {
        self.inner
            .usage_policy
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Changes which network sources are switched on, live: the next
    /// collection reads it. A source that was switched on or off starts clean:
    /// no back-off or refusal left from before is held against it.
    pub fn set_usage_policy(&self, policy: leon_usage::network::NetworkPolicy) {
        let before = std::mem::replace(
            &mut *self
                .inner
                .usage_policy
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            policy.clone(),
        );
        if let Some(setup) = self.usage_setup().as_mut() {
            for agent in leon_usage::network::switchable_agents() {
                if before.allows(agent) != policy.allows(agent) {
                    setup.throttle.remove(&agent);
                    setup.last_called.remove(&agent);
                    setup.refused.remove(&agent);
                    setup.failed.remove(&agent);
                }
            }
        }
    }

    /// What the engine knows of the last read of `agent`'s source.
    pub fn usage_status(&self, agent: leon_core::AgentId) -> SourceStatus {
        let usage = self.usage_setup();
        let Some(setup) = usage.as_ref() else {
            return SourceStatus::default();
        };
        SourceStatus {
            reading: setup.reading.contains(&agent),
            failed: setup.failed.get(&agent).copied(),
            retry_at: setup.throttle.get(&agent).and_then(|t| t.retry_at()),
            refused: setup.refused.contains(&agent),
        }
    }

    /// Makes the engine treat this computer as one with, or without, a POSIX
    /// shell of its own, whatever it is (tests only).
    #[cfg(test)]
    pub fn set_local_posix_shell(&self, posix: bool) {
        self.inner
            .local_posix_shell
            .store(posix, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether usage is collected at all.
    pub fn collects_usage(&self) -> bool {
        self.usage_setup().is_some()
    }

    /// Whether a collection is running right now.
    pub fn usage_collecting(&self) -> bool {
        self.usage_setup().as_ref().is_some_and(|u| u.collecting)
    }

    fn usage_setup(&self) -> std::sync::MutexGuard<'_, Option<UsageSetup>> {
        self.inner
            .usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn set_usage_collecting(&self, collecting: bool) {
        if let Some(setup) = self.usage_setup().as_mut() {
            setup.collecting = collecting;
        }
        let _ = self.inner.events.send(EngineEvent::Usage);
    }

    /// Takes the right to read: `false` when a read is already under way (so
    /// no source ever has two requests in flight). With `again`, the request
    /// is remembered and run when the one under way ends.
    fn begin_usage(&self, again: bool) -> bool {
        let mut usage = self.usage_setup();
        let Some(setup) = usage.as_mut() else {
            return false;
        };
        if setup.deferred {
            return false;
        }
        if setup.collecting {
            setup.rerun |= again;
            return false;
        }
        setup.collecting = true;
        true
    }

    /// Gives the right to read back.
    fn end_usage(&self) {
        self.set_usage_collecting(false);
    }

    /// Reads the usage limits of every machine that answers, a few at a time.
    async fn collect_usage(&self) {
        if !self.begin_usage(false) {
            return;
        }
        self.collect_usage_passes().await;
    }

    /// A read the user, or a changed setting, asked for.
    async fn collect_usage_now(&self, agents: &[leon_core::AgentId]) {
        {
            let mut usage = self.usage_setup();
            let Some(setup) = usage.as_mut() else {
                return;
            };
            let all = leon_usage::network::switchable_agents();
            for agent in if agents.is_empty() { &all[..] } else { agents } {
                setup.throttle.remove(agent);
                setup.last_called.remove(agent);
                setup.refused.remove(agent);
                setup.failed.remove(agent);
            }
        }
        if !self.begin_usage(true) {
            return;
        }
        self.collect_usage_passes().await;
    }

    /// Runs passes while another one is asked for; `collecting` is held.
    async fn collect_usage_passes(&self) {
        loop {
            let policy = self.usage_policy();
            {
                let mut usage = self.usage_setup();
                if let Some(setup) = usage.as_mut() {
                    setup.reading.clear();
                    for agent in policy.agents() {
                        if !setup.refused.contains(&agent) {
                            setup.reading.insert(agent);
                        }
                    }
                }
            }
            let _ = self.inner.events.send(EngineEvent::Usage);
            let machines = self.inner.store.machines().unwrap_or_default();
            let mut jobs = JoinSet::new();
            for machine in machines {
                let engine = self.clone();
                jobs.spawn_on(
                    async move { engine.collect_usage_machine(&machine).await },
                    &self.inner.handle,
                );
            }
            while jobs.join_next().await.is_some() {}
            let again = {
                let mut usage = self.usage_setup();
                usage.as_mut().is_some_and(|setup| {
                    setup.reading.clear();
                    std::mem::take(&mut setup.rerun)
                })
            };
            if !again {
                break;
            }
        }
        self.end_usage();
    }

    /// One bounded command on the machine, then the network sources that are
    /// on (this computer only). What came back is stored: the latest reading
    /// of each agent and the observations for the history.
    async fn collect_usage_machine(&self, machine: &Machine) {
        let asked = self.usage_policy();
        let (credentials, http, clock, held, refused) = {
            let usage = self.usage_setup();
            let Some(setup) = usage.as_ref() else {
                return;
            };
            if setup.deferred {
                return;
            }
            let now = (setup.clock)();
            let mut held: Vec<leon_core::AgentId> = setup
                .throttle
                .iter()
                .filter(|(_, throttle)| !throttle.allows(now))
                .map(|(agent, _)| *agent)
                .collect();
            held.extend(
                setup
                    .last_called
                    .iter()
                    .filter(|(_, at)| now - **at < leon_usage::network::MIN_GAP)
                    .map(|(agent, _)| *agent),
            );
            (
                setup.credentials.clone(),
                setup.http.clone(),
                setup.clock.clone(),
                held,
                setup.refused.clone(),
            )
        };
        // A source that is backing off, or whose sign-in was refused, is not
        // called again yet.
        let mut policy = asked.clone();
        let blocked = |agent: leon_core::AgentId| held.contains(&agent) || refused.contains(&agent);
        for agent in asked.agents() {
            if blocked(agent) {
                policy.set(agent, false);
            }
        }
        let now = clock();
        let runner = SharedRunner(self.inner.runner.clone());
        let mut collected = leon_usage::collect_machine_on(
            &runner,
            machine,
            self.inner
                .local_posix_shell
                .load(std::sync::atomic::Ordering::Relaxed),
            &self.ssh(),
            &policy,
            credentials.as_ref(),
            http.as_ref(),
            now,
        )
        .await;
        let previous: HashMap<leon_core::AgentId, leon_usage::AgentUsage> = self
            .inner
            .store
            .usage_readings()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.machine == machine.id)
            .filter_map(|row| {
                serde_json::from_str::<leon_usage::AgentUsage>(&row.payload)
                    .ok()
                    .map(|usage| (row.agent, usage))
            })
            .collect();
        let local = machine.kind == MachineKind::Local;
        // Back-off bookkeeping for the sources that were really called.
        if local {
            let mut usage = self.usage_setup();
            if let Some(setup) = usage.as_mut() {
                for (agent, reason) in collected.called.clone() {
                    setup.last_called.insert(agent, now);
                    match reason {
                        Some(reason) if reason.is_failure() => {
                            setup.failed.insert(agent, reason);
                            if reason == leon_usage::Reason::KeychainDenied {
                                // Asked once: not again this session, whatever
                                // the schedule says.
                                setup.refused.insert(agent);
                            } else {
                                // After a 429 the source rests at least five
                                // minutes, or what the vendor asked if longer.
                                let after = match reason {
                                    leon_usage::Reason::RateLimited(seconds) => Some(
                                        i64::from(seconds)
                                            .max(leon_usage::network::Throttle::RATE_LIMIT_MIN),
                                    ),
                                    _ => None,
                                };
                                setup.throttle.entry(agent).or_default().record_with(
                                    false,
                                    now,
                                    after,
                                    jitter_percent(),
                                );
                            }
                        }
                        _ => {
                            setup.failed.remove(&agent);
                            setup.throttle.entry(agent).or_default().record(true, now);
                        }
                    }
                }
            }
        }
        // A refused sign-in is said, not replaced by "off".
        if local {
            for reading in &mut collected.readings {
                if asked_on(&asked, reading.agent) && refused.contains(&reading.agent) {
                    *reading = leon_usage::AgentUsage::unknown(
                        reading.agent,
                        &reading.machine,
                        leon_usage::Reason::KeychainDenied,
                    );
                }
            }
        }
        let store = &self.inner.store;
        for (agent, reading) in crate::agent_usage::payloads(&collected, &previous) {
            // A source held back keeps what it showed, unless it is off.
            let reading = if asked_on(&asked, agent)
                && held.contains(&agent)
                && matches!(reading.state, leon_usage::State::Unknown { .. })
            {
                previous.get(&agent).cloned().unwrap_or(reading)
            } else {
                reading
            };
            if let Ok(json) = serde_json::to_string(&reading) {
                if let Err(error) = store.put_usage_reading(&machine.id, agent, &json, now) {
                    tracing::warn!(%error, "could not store a usage reading");
                }
            }
        }
        for (agent, account, points) in
            crate::agent_usage::history_points(&collected, machine.id.as_str())
        {
            if let Err(error) = store.record_usage_points(
                &machine.id,
                agent,
                &account,
                &points,
                now - crate::agent_usage::HISTORY_HORIZON,
            ) {
                tracing::warn!(%error, "could not store usage history");
            }
        }
    }

    fn fetcher(&self) -> Arc<dyn IconFetcher> {
        self.inner
            .fetcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The store this engine writes.
    pub fn store(&self) -> &Arc<Store> {
        &self.inner.store
    }

    /// How SSH commands are built: what live sessions use too.
    pub fn ssh(&self) -> SshOptions {
        let prefs = self.prefs();
        SshOptions {
            control_dir: self.inner.ssh_dir.clone().filter(|_| prefs.ssh_multiplex),
            persist_minutes: prefs.ssh_persist_minutes,
            connect_timeout: prefs.ssh_connect_timeout,
        }
    }

    /// Where each agent's history is when the settings name no folder.
    pub fn default_roots(&self) -> HistoryRoots {
        self.inner.default_roots.clone()
    }

    /// What the settings ask of the engine now.
    pub fn prefs(&self) -> Prefs {
        self.inner
            .prefs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Changes what the settings ask of the engine; the next job reads it.
    pub fn set_prefs(&self, prefs: Prefs) {
        // Another place to read: nothing is known of it yet.
        if self.prefs().roots != prefs.roots {
            self.inner
                .history_stamps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        *self
            .inner
            .prefs
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = prefs;
    }

    /// What the engine reported last.
    pub fn status(&self) -> Option<StatusLine> {
        self.state().status.clone()
    }

    /// What the engine knows of a machine. The local machine is online until
    /// a probe says otherwise.
    pub fn machine_state(&self, id: &MachineId) -> MachineState {
        match self.state().machines.get(id) {
            Some(state) => state.clone(),
            None if id.is_local() => MachineState::Online(None),
            None => MachineState::Unknown,
        }
    }

    /// Subscribes to the engine's own changes (the status line and the
    /// machines' states).
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    /// Starts `op` in the background and returns at once.
    pub fn submit(&self, op: Op) {
        let engine = self.clone();
        self.inner.handle.spawn(async move { engine.run(op).await });
    }

    /// Reports something the UI itself decided, such as an intent that cannot
    /// be carried out yet.
    pub fn report(&self, kind: StatusKind, text: impl Into<String>) {
        self.set_status(kind, text.into());
    }

    /// Does `op` and reports how it went. Failures become an error status.
    pub async fn run(&self, op: Op) {
        match self.execute(op).await {
            Ok(Some(text)) => self.set_status(StatusKind::Info, text),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "operation failed");
                self.set_status(StatusKind::Error, error.to_string());
            }
        }
    }

    async fn execute(&self, op: Op) -> Result<Option<String>, EngineError> {
        match op {
            Op::Refresh => self.refresh().await.map(Some),
            Op::ImportHistory => self.import_history().await.map(Some),
            Op::SyncHistory => {
                self.sync_history().await?;
                Ok(None)
            }
            Op::DiagnoseHistory => {
                self.diagnose_history().await?;
                Ok(None)
            }
            Op::SyncWorktrees(project) => {
                let count = self.sync_worktrees(&project).await?;
                let name = self.inner.store.project(&project)?.name;
                Ok(Some(format!(
                    "Synced {} of {name}.",
                    plural(count, "worktree")
                )))
            }
            Op::Probe(machine) => self.probe_machine(&machine).await,
            Op::Scan(machine) => {
                self.scan_machine(&machine).await;
                Ok(None)
            }
            Op::AddProject {
                machine,
                path,
                name,
            } => self.add_project(&machine, &path, &name).await.map(Some),
            Op::RemoveProject(project) => self.remove_project(&project).map(Some),
            Op::RenameMachine { machine, name } => self.rename_machine(&machine, &name).map(Some),
            Op::RemoveMachine(machine) => self.remove_machine(&machine).map(Some),
            Op::RemoveSession(session) => self.remove_session(&session).map(Some),
            Op::RestoreRoot { machine, root } => self.restore_root(&machine, &root).map(Some),
            Op::AddWorktree {
                project,
                branch,
                base,
            } => self
                .add_worktree(&project, &branch, base.as_deref())
                .await
                .map(Some),
            Op::RemoveWorktree { project, worktree } => {
                self.remove_worktree(&project, &worktree).await.map(Some)
            }
            Op::DetectIcon(project) => self.detect_icon_reported(&project).await.map(Some),
            Op::SetIcon { project, path } => self.set_icon(&project, path).await.map(Some),
            Op::ResetIcon(project) => self.reset_icon(&project).map(Some),
            Op::CollectUsage => {
                self.collect_usage().await;
                Ok(None)
            }
            Op::CollectUsageNow(agents) => {
                self.collect_usage_now(&agents).await;
                Ok(None)
            }
            Op::ForgetUsageHistory => {
                let removed = self.inner.store.forget_usage_history()?;
                Ok(Some(format!(
                    "Forgot {} of usage history.",
                    plural(removed, "observation")
                )))
            }
        }
    }

    // ----- history ---------------------------------------------------------

    async fn import_history(&self) -> Result<String, EngineError> {
        self.set_status(StatusKind::Busy, "Importing history...".to_owned());
        let store = self.inner.store.clone();
        let roots = self.prefs().roots;
        let report = self
            .blocking(move || Importer::run(&store, &MachineId::local(), &roots))
            .await?;
        Ok(describe_import(&report))
    }

    /// An incremental import that only speaks when something is wrong: a
    /// session that appears shows itself in the tree.
    async fn sync_history(&self) -> Result<(), EngineError> {
        let store = self.inner.store.clone();
        let roots = self.prefs().roots;
        let inner = self.inner.clone();
        let report = self
            .blocking(move || {
                let mut stamps = inner
                    .history_stamps
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                Importer::run_changed(&store, &MachineId::local(), &roots, &mut stamps)
            })
            .await?;
        if report.unsupported > 0
            && !self
                .inner
                .unsupported_told
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.set_status(StatusKind::Error, UNSUPPORTED_NOTICE.trim().to_owned());
        }
        Ok(())
    }

    /// The report of "Why is a session missing?": reads every history source
    /// off the UI thread and keeps the lines for the overlay.
    async fn diagnose_history(&self) -> Result<(), EngineError> {
        self.state().history_report = Some(Vec::new());
        let _ = self.inner.events.send(EngineEvent::History);
        let store = self.inner.store.clone();
        let roots = self.prefs().roots;
        let home = self
            .inner
            .history_home
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let lines = self
            .blocking(move || {
                let machine = MachineId::local();
                let stored = store
                    .history_overview(&machine)
                    .and_then(|overview| {
                        Ok(crate::history_report::Stored {
                            overview,
                            cursors: store.import_cursors(&machine)?,
                        })
                    })
                    .map_err(|error| error.to_string());
                let report = crate::history_report::collect(
                    &roots,
                    home.as_deref(),
                    &leon_history::env_variable,
                    stored,
                    None,
                );
                crate::history_report::render(&report, &crate::platform::describe())
            })
            .await?;
        self.state().history_report = Some(lines);
        let _ = self.inner.events.send(EngineEvent::History);
        Ok(())
    }

    /// The lines of the history report, once asked for.
    pub fn history_report(&self) -> Option<Vec<String>> {
        self.state().history_report.clone()
    }

    /// Where the history report says the home folder is. For tests.
    #[cfg(test)]
    pub(crate) fn set_history_home(&self, home: Option<std::path::PathBuf>) {
        *self
            .inner
            .history_home
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = home;
    }

    // ----- worktrees -------------------------------------------------------

    /// Makes the stored worktrees of a project match what git reports.
    /// Returns how many there are now.
    async fn sync_worktrees(&self, project_id: &ProjectId) -> Result<usize, EngineError> {
        let project = self.inner.store.project(project_id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let ssh = self.ssh();
        let git = Git::new(&runner, &machine, &ssh);
        let listed = new_worktrees(&git.list_worktrees(&project).await?);
        Ok(self
            .inner
            .store
            .replace_worktrees(project_id, listed)?
            .len())
    }

    async fn add_worktree(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<String, EngineError> {
        address::validate_branch(branch).map_err(|why| EngineError::Invalid(why.to_owned()))?;
        let project = self.inner.store.project(project_id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let path = address::worktree_path(&project.root, branch);
        let base = base
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .unwrap_or("HEAD");
        self.set_status(StatusKind::Busy, format!("Adding worktree {branch}..."));
        let runner = SharedRunner(self.inner.runner.clone());
        Git::new(&runner, &machine, &self.ssh())
            .add_worktree(&project, branch, &path, Some(base))
            .await?;
        self.sync_worktrees(project_id).await?;
        Ok(format!("Added worktree {branch} at {path}."))
    }

    async fn remove_worktree(
        &self,
        project_id: &ProjectId,
        worktree_id: &WorktreeId,
    ) -> Result<String, EngineError> {
        let project = self.inner.store.project(project_id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let worktree = self
            .inner
            .store
            .worktrees(project_id)?
            .into_iter()
            .find(|worktree| &worktree.id == worktree_id)
            .ok_or(StoreError::NotFound("worktree"))?;
        if worktree.is_main {
            return Err(EngineError::Invalid(
                "The main worktree cannot be removed.".to_owned(),
            ));
        }
        let runner = SharedRunner(self.inner.runner.clone());
        Git::new(&runner, &machine, &self.ssh())
            .remove_worktree(&project, &worktree.path, false)
            .await?;
        self.sync_worktrees(project_id).await?;
        Ok(format!("Removed worktree {}.", worktree.path))
    }

    // ----- machines and projects ------------------------------------------

    /// Saves an SSH machine from the Connect screen: a new one, or the one
    /// `id` names with everything replaced. Nothing is probed here; the
    /// caller marks it online with what a test found, or asks for a probe.
    pub fn save_machine(
        &self,
        id: Option<&MachineId>,
        name: &str,
        target: &Login,
    ) -> Result<Machine, EngineError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::Invalid("A machine needs a name.".to_owned()));
        }
        let host = target.host.trim();
        if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
            return Err(EngineError::Invalid(format!("{host:?} is not a host.")));
        }
        let kind = MachineKind::Ssh {
            host: host.to_owned(),
            user: target.user.clone(),
            port: target.port,
            identity_file: target.identity.clone(),
        };
        match id {
            None => Ok(self.inner.store.add_machine(name, kind)?),
            Some(id) => {
                let mut machine = self.inner.store.machine(id)?;
                machine.name = name.to_owned();
                machine.kind = kind;
                self.inner.store.update_machine(&machine)?;
                Ok(machine)
            }
        }
    }

    /// Saves a machine reached through a relay after pairing with a code. A
    /// machine already paired with the same host is updated, not duplicated.
    pub fn save_relay_machine(
        &self,
        name: &str,
        host_id: &str,
        host_key: &str,
        relay_url: &str,
        host_name: &str,
    ) -> Result<Machine, EngineError> {
        let name = name.trim();
        let name = if name.is_empty() {
            host_name.trim()
        } else {
            name
        };
        if name.is_empty() {
            return Err(EngineError::Invalid("A machine needs a name.".to_owned()));
        }
        let kind = MachineKind::Relay {
            host_id: host_id.to_owned(),
            host_key: host_key.to_owned(),
            relay_url: relay_url.to_owned(),
            name: host_name.to_owned(),
        };
        let existing =
            self.inner.store.machines()?.into_iter().find(
                |m| matches!(&m.kind, MachineKind::Relay { host_id: id, .. } if id == host_id),
            );
        match existing {
            Some(mut machine) => {
                machine.name = name.to_owned();
                machine.kind = kind;
                self.inner.store.update_machine(&machine)?;
                Ok(machine)
            }
            None => Ok(self.inner.store.add_machine(name, kind)?),
        }
    }

    /// Records that a machine answered, with what a test found out about it.
    pub fn mark_online(&self, id: &MachineId, report: ProbeReport) {
        self.set_machine(id, MachineState::Online(Some(report)));
    }

    /// Why a machine is offline, in the words the probe recorded.
    pub fn offline_reason(&self, id: &MachineId) -> Option<String> {
        match self.machine_state(id) {
            MachineState::Offline(why) => Some(why),
            _ => None,
        }
    }

    /// Tests the connection to `target` in the background, step by step.
    pub fn check_connection(&self, target: Login) -> CheckRun {
        let (sender, updates) = tokio::sync::watch::channel(Checklist::new());
        let runner = SharedRunner(self.inner.runner.clone());
        let task = self.inner.handle.spawn(async move {
            let done = connect::run_checks(&runner, &target, |list| {
                sender.send_replace(list.clone());
            })
            .await;
            sender.send_replace(done);
        });
        CheckRun { updates, task }
    }

    /// Looks for git repositories a few levels below `base` on a machine (its
    /// home folder when absent), in the background.
    pub fn find_repositories(
        &self,
        machine: MachineId,
        base: Option<String>,
    ) -> Background<Vec<String>> {
        let (sender, rx) = tokio::sync::oneshot::channel();
        let engine = self.clone();
        let task = self.inner.handle.spawn(async move {
            let found = match engine.inner.store.machine(&machine) {
                Ok(machine) => {
                    let runner = SharedRunner(engine.inner.runner.clone());
                    connect::find_repositories(&runner, &machine, &engine.ssh(), base.as_deref())
                        .await
                        .unwrap_or_default()
                }
                Err(_) => Vec::new(),
            };
            let _ = sender.send(found);
        });
        Background { rx, task }
    }

    async fn add_project(
        &self,
        machine: &MachineId,
        path: &str,
        name: &str,
    ) -> Result<String, EngineError> {
        let path = path.trim();
        if !address::is_absolute_path(path) {
            return Err(EngineError::Invalid(
                "A project path must be absolute.".to_owned(),
            ));
        }
        let name = match name.trim() {
            "" => address::default_project_name(path),
            name => name.to_owned(),
        };
        if name.is_empty() {
            return Err(EngineError::Invalid("A project needs a name.".to_owned()));
        }
        let project = self.inner.store.add_project(machine, &name, path)?;
        let synced = self.sync_worktrees(&project.id).await;
        self.detect_icon_quietly(&project.id).await;
        match synced {
            Ok(count) => Ok(format!(
                "Added project {name} with {}.",
                plural(count, "worktree")
            )),
            // The project is saved; "refresh" tries git again.
            Err(error) => Err(EngineError::Invalid(format!(
                "Added project {name}, but git failed: {error}"
            ))),
        }
    }

    fn remove_project(&self, id: &ProjectId) -> Result<String, EngineError> {
        let project = self.inner.store.project(id)?;
        self.inner.store.remove_project(id)?;
        Ok(format!("Removed project {}.", project.name))
    }

    fn rename_machine(&self, id: &MachineId, name: &str) -> Result<String, EngineError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::Invalid("A machine needs a name.".to_owned()));
        }
        let mut machine = self.inner.store.machine(id)?;
        let old = std::mem::replace(&mut machine.name, name.to_owned());
        self.inner.store.update_machine(&machine)?;
        Ok(format!("Renamed {old} to {name}."))
    }

    fn restore_root(&self, machine: &MachineId, root: &str) -> Result<String, EngineError> {
        if self.inner.store.restore_dismissed_root(machine, root)? {
            Ok(format!(
                "{root} may be found as a project again at the next refresh."
            ))
        } else {
            Ok(format!("{root} was not removed."))
        }
    }

    fn remove_machine(&self, id: &MachineId) -> Result<String, EngineError> {
        let machine = self.inner.store.machine(id)?;
        self.inner.store.remove_machine(id)?;
        self.state().machines.remove(id);
        self.set_elsewhere(id, None);
        Ok(format!("Removed machine {}.", machine.name))
    }

    fn remove_session(&self, id: &leon_core::SessionId) -> Result<String, EngineError> {
        let session = self.inner.store.session(id)?;
        self.inner.store.remove_session(id)?;
        Ok(format!("Removed \"{}\" from the history.", session.title))
    }

    /// Opens the folder `path` of a machine as a project: the repository it
    /// is in (from any folder of any of its worktrees) is added if it is not
    /// there yet, its worktrees are synced, and the status line says how it
    /// went. The handle yields the project, or `None` when it could not be
    /// opened.
    pub fn open_project(&self, machine: MachineId, path: String) -> JoinHandle<Option<ProjectId>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            match engine.open_repository(&machine, &path).await {
                Ok((id, text)) => {
                    engine.set_status(StatusKind::Info, text);
                    Some(id)
                }
                Err(error) => {
                    tracing::warn!(%error, "could not open the folder as a project");
                    engine.set_status(StatusKind::Error, error.to_string());
                    None
                }
            }
        })
    }

    async fn open_repository(
        &self,
        machine_id: &MachineId,
        path: &str,
    ) -> Result<(ProjectId, String), EngineError> {
        let path = path.trim();
        if !address::is_absolute_path(path) {
            return Err(EngineError::Invalid(
                "A project path must be absolute.".to_owned(),
            ));
        }
        let machine = self.inner.store.machine(machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let listing = Git::new(&runner, &machine, &self.ssh())
            .list_worktrees_at(path)
            .await
            .map_err(|error| {
                EngineError::Invalid(format!("{path} is not in a git repository ({error})."))
            })?;
        let root = match listing.first() {
            Some(main) if !main.is_bare => main.path.clone(),
            _ => {
                return Err(EngineError::Invalid(format!(
                    "{path} is in a bare repository, which has no checkout to open."
                )))
            }
        };
        let existing = self
            .inner
            .store
            .projects(Some(machine_id))?
            .into_iter()
            .find(|project| leon_core::path::key(&project.root) == leon_core::path::key(&root));
        let is_new = existing.is_none();
        let project = match existing {
            Some(project) => project,
            None => self.inner.store.add_project(
                machine_id,
                &address::default_project_name(&root),
                &root,
            )?,
        };
        let count = self
            .inner
            .store
            .replace_worktrees(&project.id, new_worktrees(&listing))?
            .len();
        if is_new {
            self.detect_icon_quietly(&project.id).await;
        }
        Ok((
            project.id,
            format!(
                "Opened project {} with {}.",
                project.name,
                plural(count, "worktree")
            ),
        ))
    }

    // ----- discovery -------------------------------------------------------

    /// Finds the projects the sessions of `machine` imply: the repositories
    /// of the folders no project contains yet. Each distinct folder is
    /// resolved once, a few at a time, with the one git command that names
    /// its repository's main worktree and lists the others. A folder that is
    /// not in a repository, or no longer exists, is skipped without a word;
    /// so is a project somebody removed.
    async fn discover_projects(&self, machine: &Machine) -> Result<Discovery, EngineError> {
        let folders = self.inner.store.session_cwds(&machine.id, true)?;
        if folders.is_empty() {
            return Ok(Discovery::default());
        }
        let gate = Arc::new(Semaphore::new(DISCOVERY_PARALLELISM));
        let mut jobs = JoinSet::new();
        for folder in folders {
            let (engine, machine, gate) = (self.clone(), machine.clone(), gate.clone());
            jobs.spawn_on(
                async move {
                    let _turn = gate.acquire_owned().await.ok()?;
                    let runner = SharedRunner(engine.inner.runner.clone());
                    let listing = Git::new(&runner, &machine, &engine.ssh())
                        .list_worktrees_at(&folder)
                        .await
                        .ok()?;
                    let main = listing.first().filter(|main| !main.is_bare)?;
                    Some((main.path.clone(), new_worktrees(&listing)))
                },
                &self.inner.handle,
            );
        }
        // Several folders lead to one repository: the first listing is kept.
        // Keyed by path identity, so that two spellings of one repository are
        // one entry; the first spelling seen is the one stored.
        let mut repositories: BTreeMap<String, (String, Vec<NewWorktree>)> = BTreeMap::new();
        while let Some(done) = jobs.join_next().await {
            if let Some((root, worktrees)) =
                done.map_err(|error| EngineError::Job(error.to_string()))?
            {
                repositories
                    .entry(leon_core::path::key(&root))
                    .or_insert((root, worktrees));
            }
        }

        let store = &self.inner.store;
        let dismissed: HashSet<String> = store
            .dismissed_roots(&machine.id)?
            .iter()
            .map(|root| leon_core::path::key(root))
            .collect();
        let mut known: HashMap<String, ProjectId> = store
            .projects(Some(&machine.id))?
            .into_iter()
            .map(|project| (leon_core::path::key(&project.root), project.id))
            .collect();
        let mut found = Discovery::default();
        for (key, (root, worktrees)) in repositories {
            let id = match known.get(&key) {
                Some(id) => id.clone(),
                None if dismissed.contains(&key) => continue,
                None => match store.add_project(
                    &machine.id,
                    &address::default_project_name(&root),
                    &root,
                ) {
                    Ok(project) => {
                        found.added += 1;
                        known.insert(key.clone(), project.id.clone());
                        project.id
                    }
                    Err(error) => {
                        tracing::warn!(%root, %error, "could not add a discovered project");
                        continue;
                    }
                },
            };
            match store.replace_worktrees(&id, worktrees) {
                Ok(_) => {
                    found.synced.insert(id);
                }
                Err(error) => tracing::warn!(%root, %error, "could not store its worktrees"),
            }
        }
        Ok(found)
    }

    /// Finds out, in the background, whether a session of `agent` can be
    /// resumed in `cwd` on an SSH `machine`: the machine is probed unless it
    /// was (the probe is kept for the next start), the agent must be in the
    /// report, and the folder is looked at through the same runner, by going
    /// into it. The local machine is answered by the UI from this computer's
    /// own file system and never asked here.
    pub fn check_target(
        &self,
        machine: MachineId,
        cwd: String,
        agent: AgentId,
    ) -> JoinHandle<Target> {
        let engine = self.clone();
        self.inner
            .handle
            .spawn(async move { engine.target_of(&machine, &cwd, agent).await })
    }

    async fn target_of(&self, id: &MachineId, cwd: &str, agent: AgentId) -> Target {
        let Ok(machine) = self.inner.store.machine(id) else {
            return Target::Offline("That machine is not known.".to_owned());
        };
        let known = match self.machine_state(id) {
            MachineState::Online(Some(report)) => Some(report),
            _ => None,
        };
        let report = match known {
            Some(report) => report,
            None => {
                if let Err(error) = self.probe_machine(id).await {
                    return Target::Offline(error.to_string());
                }
                match self.machine_state(id) {
                    MachineState::Online(Some(report)) => report,
                    _ => return Target::Offline(format!("{} did not report.", machine.name)),
                }
            }
        };
        let missing = agent.spec().is_some_and(|spec| {
            !spec.detect.is_empty() && crate::launch::probed(&report, spec).is_none()
        });
        if missing {
            return Target::AgentMissing;
        }
        // Going into the folder is all that is asked: `cd` fails when it is
        // gone, and ssh itself ends with 255 when the machine is not there.
        let enter = CommandSpec::new("true").cwd(cwd);
        let runner = SharedRunner(self.inner.runner.clone());
        match runner
            .run(&leon_remote::run_on(&machine, &enter, &self.ssh()))
            .await
        {
            Ok(output) if output.success() => Target::Ready,
            Ok(output) if output.status == Some(255) => Target::Offline(format!(
                "{} is offline: {}",
                machine.name,
                output.stderr.trim()
            )),
            Ok(_) => Target::FolderMissing,
            Err(error) => Target::Offline(format!("{} is offline: {error}", machine.name)),
        }
    }

    /// Probes a machine and records what it found.
    async fn probe_machine(&self, id: &MachineId) -> Result<Option<String>, EngineError> {
        let machine = self.inner.store.machine(id)?;
        self.set_machine(id, MachineState::Probing);
        let runner = SharedRunner(self.inner.runner.clone());
        match probe(&runner, &machine, &self.ssh()).await {
            Ok(report) => {
                let text = describe_probe(&machine, &report);
                self.set_machine(id, MachineState::Online(Some(report)));
                Ok(Some(text))
            }
            Err(error) => {
                self.set_machine(id, MachineState::Offline(error.to_string()));
                Err(EngineError::Invalid(format!(
                    "{} is offline: {error}",
                    machine.name
                )))
            }
        }
    }

    // ----- refresh ---------------------------------------------------------

    /// Imports the local history, then brings every machine up to date. The
    /// machines are worked on side by side, so one that does not answer costs
    /// its own time limit and nobody else's; within a machine, an SSH one is
    /// probed first and its projects are skipped when it is offline.
    async fn refresh(&self) -> Result<String, EngineError> {
        let started = std::time::Instant::now();
        let imported = self.import_history().await?;
        let machines = self.inner.store.machines()?;
        let mut jobs = JoinSet::new();
        for machine in machines {
            let engine = self.clone();
            jobs.spawn_on(
                async move { engine.refresh_machine(&machine).await },
                &self.inner.handle,
            );
        }
        let mut total = MachineRefresh::default();
        while let Some(done) = jobs.join_next().await {
            match done {
                Ok(done) => total.add(done),
                Err(error) => return Err(EngineError::Job(error.to_string())),
            }
        }
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            found = total.found,
            synced = total.synced,
            failed = total.failed,
            "refreshed"
        );
        let mut tail = String::new();
        if total.found > 0 {
            tail.push_str(&format!("found {}, ", plural(total.found, "project")));
        }
        tail.push_str(&format!("synced {}", plural(total.synced, "project")));
        if total.failed > 0 {
            tail.push_str(&format!(", {} failed", total.failed));
        }
        if total.offline > 0 {
            tail.push_str(&format!(", {} offline", plural(total.offline, "machine")));
        }
        Ok(format!("{}; {tail}.", imported.trim_end_matches('.')))
    }

    async fn refresh_machine(&self, machine: &Machine) -> MachineRefresh {
        let mut done = MachineRefresh::default();
        let store = &self.inner.store;
        if matches!(machine.kind, MachineKind::Ssh { .. })
            && self.probe_machine(&machine.id).await.is_err()
        {
            done.offline = 1;
            done.failed = store.projects(Some(&machine.id)).unwrap_or_default().len();
            // Nothing is known of what runs there while it does not answer.
            self.set_elsewhere(&machine.id, None);
            return done;
        }
        // The machine answers: what runs in other terminals, in one command,
        // and how much of each agent's limits is left, in another.
        self.scan_machine(&machine.id).await;
        if self.collects_usage() {
            // This computer's reading may call a network source: never while a
            // read is already under way.
            if machine.kind != MachineKind::Local {
                self.collect_usage_machine(machine).await;
            } else if self.begin_usage(false) {
                self.collect_usage_machine(machine).await;
                self.end_usage();
            }
        }
        let discovery = if !self.prefs().discover_projects {
            Ok(Discovery::default())
        } else {
            self.discover_projects(machine).await
        }
        .unwrap_or_else(|error| {
            tracing::warn!(machine = %machine.name, %error, "project discovery failed");
            Discovery::default()
        });
        done.found = discovery.added;
        let projects = store.projects(Some(&machine.id)).unwrap_or_default();
        for project in &projects {
            // Discovery has just listed the worktrees of these.
            if discovery.synced.contains(&project.id) {
                done.synced += 1;
                continue;
            }
            match self.sync_worktrees(&project.id).await {
                Ok(_) => done.synced += 1,
                Err(error) => {
                    tracing::warn!(project = %project.name, %error, "worktree sync failed");
                    done.failed += 1;
                }
            }
        }
        // Projects that never had their logo looked for: the ones just found,
        // and those from before logos existed.
        self.detect_missing_icons(machine).await;
        done
    }

    // ----- logos -----------------------------------------------------------

    /// Detects the logo of every project of `machine` that has none looked up
    /// yet, a few at a time. A failure leaves the project for the next
    /// refresh.
    async fn detect_missing_icons(&self, machine: &Machine) {
        if !self.prefs().detect_logos {
            return;
        }
        let Ok(pending) = self.inner.store.projects_without_detected_icon() else {
            return;
        };
        let mine: HashSet<ProjectId> = self
            .inner
            .store
            .projects(Some(&machine.id))
            .unwrap_or_default()
            .into_iter()
            .map(|project| project.id)
            .collect();
        let gate = Arc::new(Semaphore::new(DISCOVERY_PARALLELISM));
        let mut jobs = JoinSet::new();
        for id in pending.into_iter().filter(|id| mine.contains(id)) {
            let (engine, gate) = (self.clone(), gate.clone());
            jobs.spawn_on(
                async move {
                    let _turn = gate.acquire_owned().await.ok();
                    engine.detect_icon_quietly(&id).await;
                },
                &self.inner.handle,
            );
        }
        while jobs.join_next().await.is_some() {}
    }

    /// Detects a project's logo and stores it; a failure is logged and
    /// changes nothing.
    async fn detect_icon_quietly(&self, id: &ProjectId) {
        if !self.prefs().detect_logos {
            return;
        }
        if let Err(error) = self.detect_icon(id).await {
            tracing::debug!(%error, "could not detect the project's logo");
        }
    }

    async fn detect_icon_reported(&self, id: &ProjectId) -> Result<String, EngineError> {
        let name = self.inner.store.project(id)?.name;
        self.set_status(
            StatusKind::Busy,
            format!("Looking for the logo of {name}..."),
        );
        let described = self.detect_icon(id).await?;
        Ok(format!("Logo of {name}: {described}."))
    }

    /// Looks for the logo of a project and stores what it found: a file of
    /// the repository (one command on the project's machine, see
    /// `leon_remote::icon`), else the owner's avatar when the origin is on
    /// GitHub, else nothing. Returns what it chose, in words.
    pub(crate) async fn detect_icon(&self, id: &ProjectId) -> Result<String, EngineError> {
        let started = std::time::Instant::now();
        let project = self.inner.store.project(id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let detection = leon_remote::icon::detect(&runner, &machine, &self.ssh(), &project.root)
            .await
            .map_err(|error| EngineError::Invalid(error.to_string()))?;
        let remote_key = detection.remote.as_ref().map(|remote| remote.key());

        let (icon, described) = if let Some(found) = detection.icon {
            let described = format!("{} ({})", found.path, found.rule);
            (
                NewIcon {
                    kind: IconKind::Detected,
                    source: found.source(),
                    image: Some(found.image),
                },
                described,
            )
        } else {
            // The one call to the network: the settings can forbid it.
            let may_fetch = self.prefs().fetch_avatars;
            let avatar = match detection
                .remote
                .as_ref()
                .filter(|_| may_fetch)
                .and_then(|r| r.avatar_url().map(|u| (r, u)))
            {
                Some((remote, url)) => match self.fetcher().fetch(&url).await {
                    Fetched::Bytes(bytes) => IconImage::from_bytes(bytes)
                        .map(|image| (format!("{}/{}", remote.host, remote.owner), image)),
                    Fetched::Missing => None,
                    // Offline: nothing is stored, the next refresh asks again.
                    Fetched::Unreachable => {
                        return Ok("no image found, the avatar could not be fetched".to_owned());
                    }
                },
                None => None,
            };
            match avatar {
                Some((source, image)) => (
                    NewIcon {
                        kind: IconKind::Avatar,
                        source: source.clone(),
                        image: Some(image),
                    },
                    format!("the avatar of {source}"),
                ),
                None => (
                    NewIcon {
                        kind: IconKind::Folder,
                        source: String::new(),
                        image: None,
                    },
                    "the folder glyph".to_owned(),
                ),
            }
        };
        self.inner
            .store
            .set_detected_icon(id, &icon, remote_key.as_deref())?;
        tracing::info!(
            project = %project.name,
            kind = icon.kind.as_str(),
            source = %icon.source,
            round_trips = detection.round_trips,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "detected the project's logo"
        );
        Ok(described)
    }

    async fn set_icon(
        &self,
        id: &ProjectId,
        path: std::path::PathBuf,
    ) -> Result<String, EngineError> {
        let name = self.inner.store.project(id)?.name;
        let file = path.clone();
        let bytes = self
            .blocking(move || {
                let size = std::fs::metadata(&file)?.len();
                if size > leon_core::icon::MAX_ICON_BYTES as u64 {
                    return Err(std::io::Error::other("that image is larger than 256 KiB"));
                }
                std::fs::read(&file)
            })
            .await?
            .map_err(|error| {
                EngineError::Invalid(format!("cannot use {}: {error}", path.display()))
            })?;
        let image = IconImage::from_bytes(bytes).ok_or_else(|| {
            EngineError::Invalid(format!(
                "{} is not a PNG, WebP, ICO, JPEG or SVG image.",
                path.display()
            ))
        })?;
        let source = path
            .file_name()
            .map_or_else(|| "custom".to_owned(), |n| n.to_string_lossy().into_owned());
        self.inner.store.set_custom_icon(id, image, &source)?;
        Ok(format!("Logo of {name} is now {source}."))
    }

    fn reset_icon(&self, id: &ProjectId) -> Result<String, EngineError> {
        let name = self.inner.store.project(id)?.name;
        self.inner.store.clear_custom_icon(id)?;
        Ok(format!("Logo of {name} is back to what was detected."))
    }

    // ----- state -----------------------------------------------------------

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn set_status(&self, kind: StatusKind, text: String) {
        self.state().status = Some(StatusLine { kind, text });
        let _ = self.inner.events.send(EngineEvent::Status);
    }

    fn set_machine(&self, id: &MachineId, state: MachineState) {
        self.state().machines.insert(id.clone(), state);
        let _ = self.inner.events.send(EngineEvent::Machines);
    }
}

/// What discovering the projects of one machine found.
#[derive(Debug, Default)]
struct Discovery {
    /// How many projects were new.
    added: usize,
    /// The projects whose worktrees were stored from the same listing.
    synced: HashSet<ProjectId>,
}

/// What refreshing one machine did.
#[derive(Debug, Default)]
struct MachineRefresh {
    found: usize,
    synced: usize,
    failed: usize,
    offline: usize,
}

impl MachineRefresh {
    fn add(&mut self, other: Self) {
        self.found += other.found;
        self.synced += other.synced;
        self.failed += other.failed;
        self.offline += other.offline;
    }
}

/// The worktrees git listed, in the store's shape. Bare entries are left out,
/// and so are the prunable ones: their folder is gone, though git still lists
/// them until it is told to prune.
fn new_worktrees(listing: &[leon_remote::GitWorktree]) -> Vec<NewWorktree> {
    listing
        .iter()
        .filter(|worktree| !worktree.is_bare && worktree.prunable.is_none())
        .map(|worktree| worktree.to_new_worktree())
        .collect()
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// What the import's line adds when a history source has a layout Leon cannot
/// read: that is never reported as "no sessions".
const UNSUPPORTED_NOTICE: &str = " A history source has a layout Leon cannot read; run \"Why is a session missing?\" from the palette.";

fn describe_import(report: &ImportReport) -> String {
    let mut text = format!(
        "Imported {} ({} unchanged",
        plural(report.imported, "session"),
        report.skipped_unchanged
    );
    if report.failed > 0 {
        text.push_str(&format!(", {} failed", report.failed));
    }
    text.push_str(").");
    if report.unsupported > 0 {
        text.push_str(UNSUPPORTED_NOTICE);
    }
    text
}

fn describe_probe(machine: &Machine, report: &ProbeReport) -> String {
    let agents: Vec<&str> = leon_core::agent::all()
        .into_iter()
        .filter(|spec| crate::launch::probed(report, spec).is_some())
        .map(|spec| spec.name.as_str())
        .collect();
    let agents = if agents.is_empty() {
        "no agents found".to_owned()
    } else {
        agents.join(", ")
    };
    format!("{} is online: {agents}.", machine.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::Project;
    use leon_remote::ScriptedRunner;

    const PROJECT_ROOT: &str = "/srv/api";

    const LISTING: &str = "\
worktree /srv/api
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /srv/api-worktrees/feature-login
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature/login
";

    /// What the icon command prints for a repository with no image and no remote.
    const NO_ICON: &str = "LEON-ICON 1\nPKG .\n";

    const PROBE_OUTPUT: &str = "os=Linux\narch=x86_64\nhome=/home/dev\ntool=git=/usr/bin/git\ntool=claude=/home/dev/.local/bin/claude\n";

    struct Rig<R = ScriptedRunner> {
        engine: Engine,
        runner: Arc<R>,
        store: Arc<Store>,
    }

    fn rig(runner: ScriptedRunner) -> Rig {
        rig_with(runner)
    }

    fn rig_with<R: Runner + 'static>(runner: R) -> Rig<R> {
        let store = Store::open_in_memory().unwrap();
        let runner = Arc::new(runner);
        let engine = Engine::new(
            store.clone(),
            runner.clone(),
            SshOptions::without_multiplexing(),
            HistoryRoots::default(),
            Handle::current(),
        );
        Rig {
            engine,
            runner,
            store,
        }
    }

    /// A rig with project discovery on, which is off until somebody asks.
    fn discovery_rig<R: Runner + 'static>(runner: R) -> Rig<R> {
        let rig = rig_with(runner);
        let mut prefs = rig.engine.prefs();
        prefs.discover_projects = true;
        rig.engine.set_prefs(prefs);
        rig
    }

    fn local_project(store: &Store) -> Project {
        store
            .add_project(&MachineId::local(), "api", PROJECT_ROOT)
            .unwrap()
    }

    fn status(engine: &Engine) -> StatusLine {
        engine.status().expect("a status was reported")
    }

    #[tokio::test]
    async fn syncing_stores_the_worktrees_git_reports() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(LISTING)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let worktrees = rig.store.worktrees(&project.id).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature/login"));
        assert_eq!(status(&rig.engine).kind, StatusKind::Info);
        let call = &rig.runner.calls()[0];
        assert_eq!(call.program, "git");
        assert_eq!(call.cwd.as_deref(), Some(PROJECT_ROOT));
    }

    #[tokio::test]
    async fn a_bare_repository_entry_is_not_a_worktree() {
        let listing =
            "worktree /srv/api.git\nbare\n\nworktree /srv/api\nHEAD abc\nbranch refs/heads/main\n";
        let rig = rig(ScriptedRunner::new().reply(Output::ok(listing)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let worktrees = rig.store.worktrees(&project.id).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, "/srv/api");
    }

    #[tokio::test]
    async fn a_git_failure_becomes_an_error_status_and_leaves_the_store_alone() {
        let rig =
            rig(ScriptedRunner::new().reply(Output::failed(128, "fatal: not a git repository")));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("not a git repository"), "{}", line.text);
        assert!(rig.store.worktrees(&project.id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_command_that_cannot_start_becomes_an_error_status() {
        let rig = rig(ScriptedRunner::new());
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id)).await;
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
    }

    #[tokio::test]
    async fn an_unknown_project_is_an_error_status_not_a_panic() {
        let rig = rig(ScriptedRunner::new());
        rig.engine
            .run(Op::SyncWorktrees(ProjectId::from_string("nope")))
            .await;
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
    }

    #[tokio::test]
    async fn adding_a_worktree_creates_the_branch_beside_the_project_and_syncs() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(LISTING)));
        let project = local_project(&rig.store);
        rig.engine
            .run(Op::AddWorktree {
                project: project.id.clone(),
                branch: "feature/login".into(),
                base: Some("main".into()),
            })
            .await;
        let calls = rig.runner.calls();
        assert_eq!(
            calls[0].args,
            [
                "worktree",
                "add",
                "-b",
                "feature/login",
                "/srv/api-worktrees/feature-login",
                "main"
            ]
        );
        assert_eq!(calls[1].args, ["worktree", "list", "--porcelain"]);
        assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 2);
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Info);
        assert!(line.text.contains("feature/login"));
    }

    #[tokio::test]
    async fn a_worktree_without_a_base_starts_from_head() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(LISTING)));
        let project = local_project(&rig.store);
        rig.engine
            .run(Op::AddWorktree {
                project: project.id,
                branch: "fix".into(),
                base: Some("  ".into()),
            })
            .await;
        assert_eq!(rig.runner.calls()[0].args.last().unwrap(), "HEAD");
    }

    #[tokio::test]
    async fn a_bad_branch_name_is_refused_before_anything_runs() {
        let rig = rig(ScriptedRunner::new());
        let project = local_project(&rig.store);
        rig.engine
            .run(Op::AddWorktree {
                project: project.id,
                branch: "-rf".into(),
                base: None,
            })
            .await;
        assert!(rig.runner.calls().is_empty());
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
    }

    #[tokio::test]
    async fn the_main_worktree_cannot_be_removed() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(LISTING)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let main = rig.store.worktrees(&project.id).unwrap()[0].clone();
        rig.engine
            .run(Op::RemoveWorktree {
                project: project.id,
                worktree: main.id,
            })
            .await;
        assert_eq!(rig.runner.calls().len(), 1, "only the sync ran");
        assert!(status(&rig.engine).text.contains("main worktree"));
    }

    #[tokio::test]
    async fn removing_a_worktree_runs_git_and_syncs() {
        let after = "worktree /srv/api\nHEAD abc\nbranch refs/heads/main\n";
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(""))
            .reply(Output::ok(after)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let extra = rig.store.worktrees(&project.id).unwrap()[1].clone();
        rig.engine
            .run(Op::RemoveWorktree {
                project: project.id.clone(),
                worktree: extra.id,
            })
            .await;
        assert_eq!(
            rig.runner.calls()[1].args,
            ["worktree", "remove", "/srv/api-worktrees/feature-login"]
        );
        assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_connect_screen_saves_a_machine_and_edits_it_in_place() {
        let rig = rig(ScriptedRunner::new());
        let login = Login {
            host: "build.example".into(),
            user: Some("dev".into()),
            port: Some(2222),
            identity: Some("/k/id".into()),
        };
        let saved = rig
            .engine
            .save_machine(None, " build box ", &login)
            .unwrap();
        assert_eq!(saved.name, "build box");
        assert_eq!(
            Login::of(&rig.store.machine(&saved.id).unwrap()),
            Some(login.clone())
        );

        let moved = Login {
            host: "other".into(),
            ..login
        };
        rig.engine
            .save_machine(Some(&saved.id), "renamed", &moved)
            .unwrap();
        let again = rig.store.machine(&saved.id).unwrap();
        assert_eq!(again.name, "renamed");
        assert_eq!(Login::of(&again).unwrap().host, "other");
        assert_eq!(
            rig.store.machines().unwrap().len(),
            2,
            "no duplicate was made"
        );
        assert!(rig.runner.calls().is_empty(), "saving runs nothing");
    }

    #[tokio::test]
    async fn the_connect_screen_refuses_what_cannot_be_a_machine() {
        let rig = rig(ScriptedRunner::new());
        let login = |host: &str| Login {
            host: host.into(),
            ..Login::default()
        };
        assert!(rig.engine.save_machine(None, "", &login("box")).is_err());
        assert!(rig.engine.save_machine(None, "x", &login("-oFoo")).is_err());
        assert!(rig.engine.save_machine(None, "x", &login("")).is_err());
        assert_eq!(rig.store.machines().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_connection_test_reports_each_step_and_ends_with_the_checklist() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(PROBE_OUTPUT)));
        let mut run = rig.engine.check_connection(Login {
            host: "box".into(),
            ..Login::default()
        });
        while run.updates.changed().await.is_ok() {}
        let done = run.updates.borrow().clone();
        assert!(done.connected());
        assert!(done.report.is_some());
    }

    #[tokio::test]
    async fn searching_a_machine_for_repositories_is_one_bounded_command() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok("/home/d/a/.git\n/home/d/b/.git\n")));
        let remote = rig
            .store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        let found = rig.engine.find_repositories(remote.id, None).finish().await;
        assert_eq!(
            found,
            Some(vec!["/home/d/a".to_owned(), "/home/d/b".to_owned()])
        );
        assert_eq!(rig.runner.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_dropped_search_is_cancelled() {
        let rig = rig(ScriptedRunner::new());
        let search = rig.engine.find_repositories(MachineId::local(), None);
        drop(search);
        tokio::task::yield_now().await;
        assert!(rig.runner.calls().len() <= 1);
    }

    #[tokio::test]
    async fn the_local_machine_is_online_and_others_unknown_until_probed() {
        let rig = rig(ScriptedRunner::new());
        assert_eq!(
            rig.engine.machine_state(&MachineId::local()),
            MachineState::Online(None)
        );
        assert_eq!(
            rig.engine.machine_state(&MachineId::from_string("x")),
            MachineState::Unknown
        );
    }

    #[tokio::test]
    async fn adding_a_project_saves_it_and_syncs_its_worktrees() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(LISTING)));
        rig.engine
            .run(Op::AddProject {
                machine: MachineId::local(),
                path: PROJECT_ROOT.into(),
                name: String::new(),
            })
            .await;
        let projects = rig.store.projects(None).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "api", "named after the folder");
        assert_eq!(rig.store.worktrees(&projects[0].id).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_relative_project_path_is_refused() {
        let rig = rig(ScriptedRunner::new());
        rig.engine
            .run(Op::AddProject {
                machine: MachineId::local(),
                path: "api".into(),
                name: "api".into(),
            })
            .await;
        assert!(rig.store.projects(None).unwrap().is_empty());
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
    }

    #[tokio::test]
    async fn a_project_that_git_cannot_read_is_still_saved() {
        let rig =
            rig(ScriptedRunner::new().reply(Output::failed(128, "fatal: not a git repository")));
        rig.engine
            .run(Op::AddProject {
                machine: MachineId::local(),
                path: PROJECT_ROOT.into(),
                name: "api".into(),
            })
            .await;
        assert_eq!(rig.store.projects(None).unwrap().len(), 1);
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("not a git repository"), "{}", line.text);
    }

    #[tokio::test]
    async fn importing_history_with_nothing_to_import_says_so() {
        let rig = rig(ScriptedRunner::new());
        rig.engine.run(Op::ImportHistory).await;
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Info);
        assert!(
            line.text.starts_with("Imported 0 sessions"),
            "{}",
            line.text
        );
    }

    fn odd_database_roots() -> (tempfile::TempDir, HistoryRoots) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        leon_core::rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE sessions_v2 (id TEXT);")
            .unwrap();
        let roots = HistoryRoots {
            opencode_db: Some(db),
            ..Default::default()
        };
        (dir, roots)
    }

    #[tokio::test]
    async fn a_history_source_of_an_unknown_layout_is_never_reported_as_no_sessions() {
        let rig = rig(ScriptedRunner::new());
        let (_dir, roots) = odd_database_roots();
        let mut prefs = rig.engine.prefs();
        prefs.roots = roots;
        rig.engine.set_prefs(prefs);
        rig.engine.run(Op::ImportHistory).await;
        let text = status(&rig.engine).text;
        assert!(text.contains("cannot read"), "{text}");
        assert!(text.contains("Why is a session missing?"), "{text}");
    }

    #[tokio::test]
    async fn the_history_report_is_collected_off_the_ui_thread_and_kept() {
        let rig = rig(ScriptedRunner::new());
        assert_eq!(rig.engine.history_report(), None);
        let (dir, roots) = odd_database_roots();
        rig.engine.set_history_home(Some(dir.path().to_path_buf()));
        let mut prefs = rig.engine.prefs();
        prefs.roots = roots;
        rig.engine.set_prefs(prefs);
        let mut events = rig.engine.subscribe();
        rig.engine.run(Op::DiagnoseHistory).await;
        let lines = rig.engine.history_report().unwrap();
        let text = lines.join("\n");
        assert!(
            text.contains("PROBLEM: ~/opencode.db: unsupported layout"),
            "{text}"
        );
        assert_eq!(events.try_recv().unwrap(), EngineEvent::History);
    }

    #[tokio::test]
    async fn refreshing_imports_and_syncs_every_project() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(LISTING)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::Refresh).await;
        assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 2);
        let text = status(&rig.engine).text;
        assert!(text.contains("Imported"), "{text}");
        assert!(text.contains("synced 1 project"), "{text}");
    }

    #[tokio::test]
    async fn refreshing_counts_the_projects_that_failed() {
        let rig = rig(ScriptedRunner::new().reply(Output::failed(1, "boom")));
        local_project(&rig.store);
        rig.engine.run(Op::Refresh).await;
        assert!(status(&rig.engine).text.contains("1 failed"));
    }

    fn ssh_machine(store: &Store) -> Machine {
        store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap()
    }

    #[tokio::test]
    async fn refreshing_probes_a_remote_machine_before_syncing_its_projects() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(PROBE_OUTPUT))
            .reply(Output::ok(LISTING))
            .reply(Output::ok(NO_ICON)));
        let machine = ssh_machine(&rig.store);
        let project = rig
            .store
            .add_project(&machine.id, "infra", "/opt/infra")
            .unwrap();
        rig.engine.run(Op::Refresh).await;
        let calls = rig.runner.calls();
        assert_eq!(calls.len(), 3, "the probe, git, and the one icon command");
        assert!(calls[2].args.last().unwrap().contains("LEON-ICON"));
        assert!(
            calls[0].args.last().unwrap().contains("uname"),
            "the probe came first"
        );
        assert!(calls[1].args.last().unwrap().contains("worktree"));
        assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 2);
        assert!(matches!(
            rig.engine.machine_state(&machine.id),
            MachineState::Online(Some(_))
        ));
    }

    #[tokio::test]
    async fn refreshing_skips_the_projects_of_a_machine_that_is_offline() {
        let rig = rig(ScriptedRunner::new().reply(Output::failed(255, "timed out")));
        let machine = ssh_machine(&rig.store);
        let project = rig
            .store
            .add_project(&machine.id, "infra", "/opt/infra")
            .unwrap();
        rig.engine.run(Op::Refresh).await;
        assert_eq!(rig.runner.calls().len(), 1, "only the probe ran");
        assert!(rig.store.worktrees(&project.id).unwrap().is_empty());
        let text = status(&rig.engine).text;
        assert!(text.contains("1 machine offline"), "{text}");
        assert!(matches!(
            rig.engine.machine_state(&machine.id),
            MachineState::Offline(_)
        ));
    }

    #[tokio::test]
    async fn changes_are_announced_to_subscribers() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(PROBE_OUTPUT)));
        let mut events = rig.engine.subscribe();
        rig.engine.run(Op::Probe(MachineId::local())).await;
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        assert!(seen.contains(&EngineEvent::Machines));
        assert!(seen.contains(&EngineEvent::Status));
    }

    #[tokio::test]
    async fn the_ui_can_report_its_own_status() {
        let rig = rig(ScriptedRunner::new());
        rig.engine.report(StatusKind::Info, "hello");
        assert_eq!(
            status(&rig.engine),
            StatusLine {
                kind: StatusKind::Info,
                text: "hello".into()
            }
        );
    }

    // ----- discovering projects ---------------------------------------------

    /// A runner that answers by looking at the command, so tests do not
    /// depend on the order folders are resolved in, and that notes how many
    /// commands were running at once.
    struct Answering<F> {
        answer: F,
        calls: Mutex<Vec<CommandSpec>>,
        running: std::sync::atomic::AtomicUsize,
        most_running: std::sync::atomic::AtomicUsize,
    }

    impl<F> Answering<F>
    where
        F: Fn(&CommandSpec) -> Result<Output, RunError> + Send + Sync,
    {
        fn new(answer: F) -> Self {
            Self {
                answer,
                calls: Mutex::new(Vec::new()),
                running: Default::default(),
                most_running: Default::default(),
            }
        }

        fn calls(&self) -> Vec<CommandSpec> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl<F> Runner for Answering<F>
    where
        F: Fn(&CommandSpec) -> Result<Output, RunError> + Send + Sync,
    {
        async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
            use std::sync::atomic::Ordering::SeqCst;
            self.calls.lock().unwrap().push(spec.clone());
            let now = self.running.fetch_add(1, SeqCst) + 1;
            self.most_running.fetch_max(now, SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            self.running.fetch_sub(1, SeqCst);
            (self.answer)(spec)
        }
    }

    /// Git answers `listing` for the folders in `known` and "not a
    /// repository" for any other; a folder that does not exist cannot even
    /// start the command.
    fn git_at(
        known: &'static [&'static str],
        listing: &'static str,
    ) -> impl Fn(&CommandSpec) -> Result<Output, RunError> + Send + Sync {
        move |spec| match spec.cwd.as_deref() {
            Some(cwd) if known.contains(&cwd) => Ok(Output::ok(listing)),
            Some("/gone") => Err(RunError::Spawn {
                program: "git".into(),
                source: std::io::Error::other("no such directory"),
            }),
            _ => Ok(Output::failed(128, "fatal: not a git repository")),
        }
    }

    fn session_in(store: &Store, machine: &MachineId, cwd: &str, id: &str) {
        let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        store
            .upsert_session(
                &leon_core::NewSession {
                    agent: leon_core::AgentId::CLAUDE,
                    external_id: id.to_owned(),
                    machine_id: machine.clone(),
                    cwd: cwd.to_owned(),
                    title: id.to_owned(),
                    model: None,
                    started_at: at,
                    updated_at: at,
                },
                &[],
            )
            .unwrap();
    }

    fn roots(store: &Store, machine: &MachineId) -> Vec<String> {
        store
            .projects(Some(machine))
            .unwrap()
            .into_iter()
            .map(|project| project.root)
            .collect()
    }

    const ALL_API_FOLDERS: &[&str] = &[
        "/srv/api",
        "/srv/api/src",
        "/srv/api-worktrees/feature-login",
    ];

    #[tokio::test]
    async fn a_repository_a_session_ran_in_becomes_a_project_with_its_worktrees() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        session_in(&rig.store, &MachineId::local(), "/srv/api/src", "one");
        rig.engine.run(Op::Refresh).await;

        let projects = rig.store.projects(Some(&MachineId::local())).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].root, "/srv/api");
        assert_eq!(projects[0].name, "api", "named after the folder");
        assert_eq!(rig.store.worktrees(&projects[0].id).unwrap().len(), 2);
        let session = &rig
            .store
            .recent_sessions(&leon_core::SessionFilter::default(), 10)
            .unwrap()[0];
        assert_eq!(session.project_id.as_ref(), Some(&projects[0].id));
        assert!(status(&rig.engine).text.contains("found 1 project"));
    }

    #[tokio::test]
    async fn a_fresh_install_adopts_no_project_from_the_history() {
        let rig = rig_with(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api/src", "one");
        rig.engine.run(Op::Refresh).await;
        assert!(roots(&rig.store, &local).is_empty());
        assert_eq!(
            rig.store
                .recent_sessions(&leon_core::SessionFilter::default(), 10)
                .unwrap()
                .len(),
            1,
            "the history is still there to be searched"
        );
        assert!(
            rig.runner.calls().is_empty(),
            "no folder of the history was looked at"
        );
    }

    #[tokio::test]
    async fn sessions_of_a_project_opened_by_hand_still_hang_under_it_without_discovery() {
        let rig = rig_with(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api/src", "one");
        session_in(&rig.store, &local, "/srv/other", "two");
        let opened = rig
            .engine
            .open_project(local.clone(), "/srv/api".into())
            .await
            .unwrap()
            .unwrap();
        let sessions = rig
            .store
            .recent_sessions(&leon_core::SessionFilter::default(), 10)
            .unwrap();
        let linked: Vec<_> = sessions
            .iter()
            .filter(|session| session.project_id.as_ref() == Some(&opened))
            .map(|session| session.external_id.as_str())
            .collect();
        assert_eq!(linked, ["one"]);
    }

    #[tokio::test]
    async fn a_session_started_in_a_linked_worktree_belongs_to_the_main_checkouts_project() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api-worktrees/feature-login", "a");
        session_in(&rig.store, &local, "/srv/api", "b");
        rig.engine.run(Op::Refresh).await;
        assert_eq!(
            roots(&rig.store, &local),
            ["/srv/api"],
            "one project, not two"
        );
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repository_or_is_gone_does_not_become_a_project() {
        let rig = discovery_rig(Answering::new(git_at(&[], LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/home/me/notes", "a");
        session_in(&rig.store, &local, "/gone", "b");
        rig.engine.run(Op::Refresh).await;
        assert!(roots(&rig.store, &local).is_empty());
        assert_ne!(
            status(&rig.engine).kind,
            StatusKind::Error,
            "that is not a failure, only nothing to find"
        );
    }

    #[tokio::test]
    async fn a_bare_repository_is_not_taken_for_a_project() {
        let rig = discovery_rig(Answering::new(git_at(
            &["/srv/api.git"],
            "worktree /srv/api.git\nbare\n",
        )));
        session_in(&rig.store, &MachineId::local(), "/srv/api.git", "a");
        rig.engine.run(Op::Refresh).await;
        assert!(roots(&rig.store, &MachineId::local()).is_empty());
    }

    #[tokio::test]
    async fn each_folder_is_resolved_once_however_many_sessions_ran_in_it() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        for id in ["a", "b", "c"] {
            session_in(&rig.store, &local, "/srv/api/src", id);
        }
        rig.engine.run(Op::Refresh).await;
        let asked: Vec<_> = rig
            .runner
            .calls()
            .into_iter()
            .filter_map(|call| call.cwd)
            .collect();
        assert_eq!(asked, ["/srv/api/src"]);
    }

    #[tokio::test]
    async fn folders_that_already_belong_to_a_project_are_not_resolved_again() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        local_project(&rig.store);
        session_in(&rig.store, &local, "/srv/api/src", "a");
        rig.engine.run(Op::Refresh).await;
        let asked: Vec<_> = rig
            .runner
            .calls()
            .into_iter()
            .filter_map(|call| call.cwd)
            .collect();
        assert_eq!(asked, ["/srv/api"], "only the project's own sync ran");
    }

    #[tokio::test]
    async fn many_folders_are_resolved_a_few_at_a_time() {
        const FOLDERS: &[&str] = &[
            "/r/0", "/r/1", "/r/2", "/r/3", "/r/4", "/r/5", "/r/6", "/r/7", "/r/8", "/r/9",
            "/r/10", "/r/11", "/r/12", "/r/13", "/r/14", "/r/15", "/r/16", "/r/17", "/r/18",
            "/r/19",
        ];
        let rig = discovery_rig(Answering::new(git_at(&[], "")));
        for (index, folder) in FOLDERS.iter().enumerate() {
            session_in(&rig.store, &MachineId::local(), folder, &index.to_string());
        }
        rig.engine.run(Op::Refresh).await;
        let most = rig
            .runner
            .most_running
            .load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(rig.runner.calls().len(), FOLDERS.len());
        assert!(most > 1, "they ran side by side");
        assert!(most <= DISCOVERY_PARALLELISM, "{most} at once");
    }

    #[tokio::test]
    async fn a_removed_project_is_not_rediscovered() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api", "a");
        rig.engine.run(Op::Refresh).await;
        let project = rig.store.projects(Some(&local)).unwrap().remove(0);

        rig.engine.run(Op::RemoveProject(project.id)).await;
        assert!(status(&rig.engine).text.contains("Removed project api"));
        rig.engine.run(Op::Refresh).await;

        assert!(roots(&rig.store, &local).is_empty(), "it stays removed");
    }

    #[tokio::test]
    async fn a_removed_project_comes_back_when_it_is_opened_by_hand() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api", "a");
        rig.engine.run(Op::Refresh).await;
        let project = rig.store.projects(Some(&local)).unwrap().remove(0);
        rig.engine.run(Op::RemoveProject(project.id)).await;

        let opened = rig
            .engine
            .open_project(local.clone(), "/srv/api".into())
            .await
            .unwrap();
        assert!(opened.is_some());
        assert_eq!(roots(&rig.store, &local), ["/srv/api"]);
    }

    #[tokio::test]
    async fn a_remote_machine_is_searched_for_projects_only_when_it_answers() {
        let answer = |spec: &CommandSpec| {
            let last = spec.args.last().cloned().unwrap_or_default();
            if last.contains("uname") {
                Ok(Output::ok(PROBE_OUTPUT))
            } else if last.contains("/opt/infra") {
                Ok(Output::ok(
                    "worktree /opt/infra\nHEAD abc\nbranch refs/heads/main\n",
                ))
            } else {
                Ok(Output::failed(128, "fatal: not a git repository"))
            }
        };
        let rig = discovery_rig(Answering::new(answer));
        let machine = ssh_machine(&rig.store);
        session_in(&rig.store, &machine.id, "/opt/infra/deploy", "a");
        rig.engine.run(Op::Refresh).await;
        assert_eq!(roots(&rig.store, &machine.id), ["/opt/infra"]);

        let offline = discovery_rig(Answering::new(|_: &CommandSpec| {
            Ok(Output::failed(255, "timed out"))
        }));
        let machine = ssh_machine(&offline.store);
        session_in(&offline.store, &machine.id, "/opt/infra/deploy", "a");
        offline.engine.run(Op::Refresh).await;
        assert_eq!(offline.runner.calls().len(), 1, "only the probe ran");
        assert!(roots(&offline.store, &machine.id).is_empty());
    }

    // ----- opening a folder -------------------------------------------------

    #[tokio::test]
    async fn opening_a_subfolder_adds_the_repository_it_is_in() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        let id = rig
            .engine
            .open_project(local.clone(), "/srv/api/src".into())
            .await
            .unwrap()
            .expect("it opened");
        let project = rig.store.project(&id).unwrap();
        assert_eq!(project.root, "/srv/api");
        assert_eq!(rig.store.worktrees(&id).unwrap().len(), 2);
        assert!(status(&rig.engine).text.contains("Opened project api"));

        let again = rig
            .engine
            .open_project(local.clone(), "/srv/api-worktrees/feature-login".into())
            .await
            .unwrap();
        assert_eq!(again, Some(id), "the same project, not a second one");
        assert_eq!(roots(&rig.store, &local), ["/srv/api"]);
    }

    #[tokio::test]
    async fn opening_a_folder_that_is_not_a_repository_says_so_and_adds_nothing() {
        let rig = discovery_rig(Answering::new(git_at(&[], LISTING)));
        let opened = rig
            .engine
            .open_project(MachineId::local(), "/home/me/notes".into())
            .await
            .unwrap();
        assert_eq!(opened, None);
        assert!(roots(&rig.store, &MachineId::local()).is_empty());
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("/home/me/notes"), "{}", line.text);
    }

    #[tokio::test]
    async fn opening_a_relative_path_is_refused_before_git_runs() {
        let rig = discovery_rig(Answering::new(git_at(&[], LISTING)));
        let opened = rig
            .engine
            .open_project(MachineId::local(), "notes".into())
            .await
            .unwrap();
        assert_eq!(opened, None);
        assert!(rig.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn a_machine_can_be_renamed_and_removed_but_not_the_local_one() {
        let rig = rig(ScriptedRunner::new());
        let machine = rig
            .store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        rig.engine
            .run(Op::RenameMachine {
                machine: machine.id.clone(),
                name: "  build box ".into(),
            })
            .await;
        assert_eq!(rig.store.machine(&machine.id).unwrap().name, "build box");
        assert!(status(&rig.engine)
            .text
            .contains("Renamed box to build box"));
        rig.engine
            .run(Op::RenameMachine {
                machine: machine.id.clone(),
                name: "   ".into(),
            })
            .await;
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
        rig.engine.run(Op::RemoveMachine(machine.id.clone())).await;
        assert!(rig.store.machine(&machine.id).is_err());
        rig.engine.run(Op::RemoveMachine(MachineId::local())).await;
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
        assert!(rig.store.machine(&MachineId::local()).is_ok());
    }

    #[tokio::test]
    async fn a_session_can_be_removed_from_the_history() {
        let rig = rig(ScriptedRunner::new());
        session_in(&rig.store, &MachineId::local(), "/srv/api", "gone soon");
        let id = rig
            .store
            .recent_sessions(&leon_core::SessionFilter::default(), 5)
            .unwrap()
            .remove(0)
            .id;
        rig.engine.run(Op::RemoveSession(id.clone())).await;
        assert!(status(&rig.engine).text.contains("from the history"));
        assert!(rig.store.session(&id).is_err());
    }

    // ----- logos ---------------------------------------------------------

    use crate::avatar::BoxFuture;

    /// Answers every download with one result and remembers what was asked.
    struct FakeFetcher {
        answer: Fetched,
        asked: Mutex<Vec<String>>,
    }

    impl FakeFetcher {
        fn new(answer: Fetched) -> Arc<Self> {
            Arc::new(Self {
                answer,
                asked: Mutex::new(Vec::new()),
            })
        }
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl IconFetcher for FakeFetcher {
        fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Fetched> {
            self.asked.lock().unwrap().push(url.to_owned());
            let answer = self.answer.clone();
            Box::pin(async move { answer })
        }
    }

    fn png_bytes() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(16u32.to_be_bytes());
        bytes.extend(16u32.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        bytes
    }

    fn b64(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..4 {
                out.push(if i <= chunk.len() {
                    A[(n >> (18 - 6 * i) & 63) as usize] as char
                } else {
                    '='
                });
            }
        }
        out
    }

    fn scan_with_logo() -> String {
        let png = png_bytes();
        format!(
            "LEON-ICON 1\nREMOTE git@github.com:zavu/api.git\nPKG .\nFILE I {} logo.png\n{}\n",
            png.len(),
            b64(&png)
        )
    }

    fn scan_with_remote_only() -> String {
        "LEON-ICON 1\nREMOTE https://github.com/zavu/api.git\nPKG .\n".to_owned()
    }

    #[tokio::test]
    async fn detecting_a_logo_stores_the_file_its_rule_and_the_remote() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(scan_with_logo())));
        let project = local_project(&rig.store);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        let icon = rig
            .store
            .project_icons()
            .unwrap()
            .remove(&project.id)
            .unwrap();
        assert_eq!(icon.kind, IconKind::Detected);
        assert_eq!(icon.source, "generic:logo.png");
        assert_eq!(icon.remote.as_deref(), Some("github.com/zavu/api"));
        assert!(status(&rig.engine).text.contains("logo.png"));
    }

    #[tokio::test]
    async fn without_a_file_the_owners_avatar_is_downloaded_once_and_stored() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(scan_with_remote_only())));
        let fetcher = FakeFetcher::new(Fetched::Bytes(png_bytes()));
        rig.engine.set_icon_fetcher(fetcher.clone());
        let project = local_project(&rig.store);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        let icon = rig
            .store
            .project_icons()
            .unwrap()
            .remove(&project.id)
            .unwrap();
        assert_eq!(
            (icon.kind, icon.source.as_str()),
            (IconKind::Avatar, "github.com/zavu")
        );
        assert_eq!(fetcher.asked(), ["https://github.com/zavu.png?size=64"]);
    }

    #[tokio::test]
    async fn a_host_that_has_no_such_avatar_is_remembered_as_the_folder_and_offline_is_not() {
        let gone = rig(ScriptedRunner::new().reply(Output::ok(scan_with_remote_only())));
        gone.engine
            .set_icon_fetcher(FakeFetcher::new(Fetched::Missing));
        let project = local_project(&gone.store);
        gone.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert_eq!(
            gone.store.project_icons().unwrap()[&project.id].kind,
            IconKind::Folder
        );

        let offline = rig(ScriptedRunner::new().reply(Output::ok(scan_with_remote_only())));
        offline
            .engine
            .set_icon_fetcher(FakeFetcher::new(Fetched::Unreachable));
        let project = local_project(&offline.store);
        offline.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert!(
            offline.store.project_icons().unwrap().is_empty(),
            "tried again at the next refresh"
        );
        assert_eq!(
            offline.store.projects_without_detected_icon().unwrap(),
            vec![project.id]
        );
    }

    #[tokio::test]
    async fn another_git_host_is_never_asked_for_an_avatar() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(
            "LEON-ICON 1\nREMOTE git@gitlab.com:zavu/api.git\nPKG .\n",
        )));
        let fetcher = FakeFetcher::new(Fetched::Bytes(png_bytes()));
        rig.engine.set_icon_fetcher(fetcher.clone());
        let project = local_project(&rig.store);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert!(fetcher.asked().is_empty());
        assert_eq!(
            rig.store.project_icons().unwrap()[&project.id].kind,
            IconKind::Folder
        );
    }

    #[tokio::test]
    async fn the_default_engine_never_touches_the_network() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(scan_with_remote_only())));
        let project = local_project(&rig.store);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert!(rig.store.project_icons().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refreshing_detects_a_logo_once_and_never_again() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(scan_with_logo())));
        let project = local_project(&rig.store);
        rig.engine.run(Op::Refresh).await;
        let first = rig.runner.calls().len();
        assert_eq!(
            rig.store.project_icons().unwrap()[&project.id].source,
            "generic:logo.png"
        );
        rig.engine.run(Op::Refresh).await;
        let again: Vec<_> = rig.runner.calls().into_iter().skip(first).collect();
        assert!(
            again
                .iter()
                .all(|call| !call.args.iter().any(|arg| arg.contains("LEON-ICON"))),
            "no second detection"
        );
    }

    #[tokio::test]
    async fn a_chosen_file_becomes_the_logo_and_resetting_returns_to_detection() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok(scan_with_logo())));
        let project = local_project(&rig.store);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mine.png");
        std::fs::write(&file, png_bytes()).unwrap();
        rig.engine
            .run(Op::SetIcon {
                project: project.id.clone(),
                path: file,
            })
            .await;
        let icon = rig
            .store
            .project_icons()
            .unwrap()
            .remove(&project.id)
            .unwrap();
        assert_eq!(
            (icon.kind, icon.source.as_str()),
            (IconKind::Custom, "mine.png")
        );
        rig.engine.run(Op::ResetIcon(project.id.clone())).await;
        assert_eq!(
            rig.store.project_icons().unwrap()[&project.id].kind,
            IconKind::Detected
        );
    }

    /// A remote machine in the store, and what is asked of it before a
    /// session is resumed there.
    fn remote_box(store: &Store) -> MachineId {
        store
            .add_machine(
                "build box",
                MachineKind::Ssh {
                    host: "build.example".into(),
                    user: Some("dev".into()),
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn a_remote_folder_that_exists_and_an_agent_that_is_there_make_a_session_resumable() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(PROBE_OUTPUT))
            .reply(Output::ok("")));
        let machine = remote_box(&rig.store);
        let found = rig
            .engine
            .check_target(machine.clone(), "/opt/infra".into(), AgentId::CLAUDE)
            .await
            .unwrap();
        assert_eq!(found, Target::Ready);
        let calls = rig.runner.calls();
        assert_eq!(calls.len(), 2, "the probe, then the folder");
        let folder = calls[1].args.last().unwrap();
        assert!(folder.starts_with("cd /opt/infra && exec "), "{folder}");
        assert!(
            matches!(
                rig.engine.machine_state(&machine),
                MachineState::Online(Some(_))
            ),
            "the probe is kept for the next start"
        );
    }

    #[tokio::test]
    async fn a_machine_already_probed_is_not_probed_again_before_the_folder_is_checked() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok("")));
        let machine = remote_box(&rig.store);
        let report = leon_remote::parse_probe(PROBE_OUTPUT).unwrap();
        rig.engine
            .set_machine(&machine, MachineState::Online(Some(report)));
        let found = rig
            .engine
            .check_target(machine, "/opt/infra".into(), AgentId::CLAUDE)
            .await
            .unwrap();
        assert_eq!(found, Target::Ready);
        assert_eq!(rig.runner.calls().len(), 1, "only the folder was asked for");
    }

    #[tokio::test]
    async fn a_remote_folder_that_is_gone_is_told_apart_from_a_machine_that_does_not_answer() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(PROBE_OUTPUT))
            .reply(Output::failed(
                1,
                "sh: cd: /opt/infra: No such file or directory",
            ))
            .reply(Output::failed(
                255,
                "ssh: connect to host build.example: timed out",
            )));
        let machine = remote_box(&rig.store);
        let missing = rig
            .engine
            .check_target(machine.clone(), "/opt/infra".into(), AgentId::CLAUDE)
            .await
            .unwrap();
        assert_eq!(missing, Target::FolderMissing);
        let silent = rig
            .engine
            .check_target(machine, "/opt/infra".into(), AgentId::CLAUDE)
            .await
            .unwrap();
        match silent {
            Target::Offline(why) => assert!(why.contains("timed out"), "{why}"),
            other => panic!("expected an offline machine, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_offline_machine_or_a_missing_agent_is_found_before_the_folder_is_asked_for() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::failed(255, "Connection refused"))
            .reply(Output::ok("os=Linux\nhome=/home/dev\n")));
        let machine = remote_box(&rig.store);
        let down = rig
            .engine
            .check_target(machine.clone(), "/opt/infra".into(), AgentId::CLAUDE)
            .await
            .unwrap();
        match down {
            Target::Offline(why) => assert!(why.contains("Connection refused"), "{why}"),
            other => panic!("expected an offline machine, got {other:?}"),
        }
        let bare = rig
            .engine
            .check_target(machine, "/opt/infra".into(), AgentId::CODEX)
            .await
            .unwrap();
        assert_eq!(bare, Target::AgentMissing);
        assert_eq!(rig.runner.calls().len(), 2, "no folder was asked for");
    }

    #[tokio::test]
    async fn a_file_that_is_not_an_image_or_is_too_big_is_refused_with_a_reason() {
        let rig = rig(ScriptedRunner::new());
        let project = local_project(&rig.store);
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.png");
        std::fs::write(&text, "hello").unwrap();
        rig.engine
            .run(Op::SetIcon {
                project: project.id.clone(),
                path: text,
            })
            .await;
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("not a PNG"), "{}", line.text);
        let big = dir.path().join("big.png");
        std::fs::write(&big, vec![0u8; leon_core::icon::MAX_ICON_BYTES + 1]).unwrap();
        rig.engine
            .run(Op::SetIcon {
                project: project.id.clone(),
                path: big,
            })
            .await;
        assert!(status(&rig.engine).text.contains("larger than 256 KiB"));
        assert!(rig.store.project_icons().unwrap().is_empty());
    }

    // ----- sessions running in another terminal ------------------------------

    const HELD: &str = "0a1b2c3d-1111-4222-8333-444455556666";

    fn scan_output(extra: &str) -> String {
        format!(
            "now=1000000\nT 1 0 /sbin/launchd\nT 100 1 leon\nT 101 100 sh\nT 102 101 claude\nT 200 1 zsh\nT 70645 200 claude\n\
             A 70645 200 ttys000 00:30 claude --resume {HELD}\n{extra}"
        )
    }

    fn scanning_rig(replies: Vec<Output>) -> Rig {
        let mut runner = ScriptedRunner::new();
        for reply in replies {
            runner = runner.reply(reply);
        }
        let rig = rig(runner);
        rig.engine
            .set_process_scanner(rig.runner.clone(), Some(100));
        rig
    }

    fn history(rig: &Rig) {
        session_in(&rig.store, &MachineId::local(), "/w", HELD);
    }

    #[tokio::test]
    async fn a_scan_ties_a_process_to_the_session_it_holds() {
        let rig = scanning_rig(vec![Output::ok(scan_output(""))]);
        history(&rig);
        rig.engine.run(Op::Scan(MachineId::local())).await;
        let report = rig.engine.elsewhere(&MachineId::local()).expect("a report");
        assert_eq!(report.found.len(), 1);
        let found = &report.found[0];
        assert_eq!(found.pid, 70645);
        assert!(found.is_certain());
        assert!(found.session.is_some());
        assert!(!found.leon_child);
        // This computer's scan is a shell script, and PowerShell on Windows.
        let program = if crate::platform::is_windows() {
            "powershell"
        } else {
            "sh"
        };
        assert_eq!(rig.runner.calls()[0].program, program);
    }

    #[tokio::test]
    async fn a_relay_machine_is_scanned_through_its_route() {
        let rig = scanning_rig(vec![Output::ok(scan_output(""))]);
        let machine = rig
            .store
            .add_machine(
                "relayed",
                MachineKind::Relay {
                    host_id: "ABCDEFGH".into(),
                    host_key: "00".repeat(32),
                    relay_url: "wss://relay.example".into(),
                    name: "their computer".into(),
                },
            )
            .unwrap();
        rig.engine.run(Op::Scan(machine.id.clone())).await;
        assert!(rig.engine.elsewhere(&machine.id).is_some());
        let call = &rig.runner.calls()[0];
        assert!(call.route.is_some(), "the scan must go through the relay");
        assert_eq!(call.program, "sh");
    }

    #[tokio::test]
    async fn what_runs_below_this_process_is_marked_as_leons_own() {
        let own = "A 102 101 ttys1 00:10 claude --resume 0a1b2c3d-7777-4888-9999-aaaabbbbcccc\n";
        let rig = scanning_rig(vec![Output::ok(scan_output(own))]);
        rig.engine.run(Op::Scan(MachineId::local())).await;
        let report = rig.engine.elsewhere(&MachineId::local()).unwrap();
        let mut marks: Vec<(u32, bool)> =
            report.found.iter().map(|f| (f.pid, f.leon_child)).collect();
        marks.sort_unstable();
        assert_eq!(marks, [(102, true), (70645, false)]);
    }

    #[tokio::test]
    async fn a_scan_that_cannot_be_made_forgets_what_was_known() {
        let rig = scanning_rig(vec![
            Output::ok(scan_output("")),
            Output::failed(3, "ps: not found"),
        ]);
        rig.engine.run(Op::Scan(MachineId::local())).await;
        assert!(rig.engine.elsewhere(&MachineId::local()).is_some());
        rig.engine.run(Op::Scan(MachineId::local())).await;
        assert!(rig.engine.elsewhere(&MachineId::local()).is_none());
    }

    #[tokio::test]
    async fn an_engine_without_a_scanner_lists_no_process() {
        let rig = rig(ScriptedRunner::new());
        assert!(!rig.engine.scanning());
        rig.engine.run(Op::Scan(MachineId::local())).await;
        assert!(rig.runner.calls().is_empty());
        assert!(rig.engine.elsewhere(&MachineId::local()).is_none());
    }

    #[tokio::test]
    async fn another_machine_is_scanned_through_ssh_in_one_command() {
        let rig = scanning_rig(vec![Output::ok(scan_output(""))]);
        let machine = rig
            .store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        rig.engine.run(Op::Scan(machine.id.clone())).await;
        let calls = rig.runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].program, "ssh");
        let report = rig.engine.elsewhere(&machine.id).unwrap();
        // Ancestry is meaningless there: nothing is "Leon's child".
        assert!(report.found.iter().all(|f| !f.leon_child));
    }

    #[tokio::test]
    async fn the_ui_is_told_when_what_runs_elsewhere_changes_and_only_then() {
        let rig = scanning_rig(vec![
            Output::ok(scan_output("")),
            Output::ok(scan_output("")),
            Output::ok("now=1000000\n"),
        ]);
        let mut events = rig.engine.subscribe();
        rig.engine.run(Op::Scan(MachineId::local())).await;
        assert_eq!(events.try_recv().unwrap(), EngineEvent::Elsewhere);
        rig.engine.run(Op::Scan(MachineId::local())).await;
        assert!(events.try_recv().is_err(), "nothing changed");
        rig.engine.run(Op::Scan(MachineId::local())).await;
        assert_eq!(events.try_recv().unwrap(), EngineEvent::Elsewhere);
    }

    #[tokio::test]
    async fn a_refresh_looks_for_sessions_elsewhere_too() {
        let rig = scanning_rig(vec![Output::ok(scan_output(""))]);
        rig.engine.run(Op::Refresh).await;
        assert!(rig.engine.elsewhere(&MachineId::local()).is_some());
    }

    // ----- what the settings change ----------------------------------------------

    fn prefs_of(engine: &Engine, change: impl FnOnce(&mut Prefs)) {
        let mut prefs = engine.prefs();
        change(&mut prefs);
        engine.set_prefs(prefs);
    }

    #[tokio::test]
    async fn disabling_avatar_fetch_stops_the_engine_from_calling_the_fetcher() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(scan_with_remote_only()))
            .reply(Output::ok(scan_with_remote_only())));
        let fetcher = FakeFetcher::new(Fetched::Bytes(png_bytes()));
        rig.engine.set_icon_fetcher(fetcher.clone());
        let project = local_project(&rig.store);
        prefs_of(&rig.engine, |prefs| prefs.fetch_avatars = false);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert!(fetcher.asked().is_empty(), "the network was not touched");
        assert_eq!(
            rig.store.project_icons().unwrap()[&project.id].kind,
            IconKind::Folder
        );
        prefs_of(&rig.engine, |prefs| prefs.fetch_avatars = true);
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert_eq!(fetcher.asked(), ["https://github.com/zavu.png?size=64"]);
    }

    #[tokio::test]
    async fn disabling_logo_detection_stops_the_automatic_search_but_not_the_one_asked_for() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(scan_with_logo())));
        let project = local_project(&rig.store);
        prefs_of(&rig.engine, |prefs| prefs.detect_logos = false);
        rig.engine.run(Op::Refresh).await;
        assert!(
            rig.store.project_icons().unwrap().is_empty(),
            "refresh looked for nothing"
        );
        assert_eq!(
            rig.runner.calls().len(),
            1,
            "only the worktree listing ran, no logo scan"
        );
        rig.engine.run(Op::DetectIcon(project.id.clone())).await;
        assert!(
            rig.store.project_icons().unwrap().contains_key(&project.id),
            "an explicit request still works"
        );
    }

    #[tokio::test]
    async fn disabling_discovery_leaves_the_session_folders_unadopted() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api", "a");
        prefs_of(&rig.engine, |prefs| prefs.discover_projects = false);
        rig.engine.run(Op::Refresh).await;
        assert!(roots(&rig.store, &local).is_empty());
        prefs_of(&rig.engine, |prefs| prefs.discover_projects = true);
        rig.engine.run(Op::Refresh).await;
        assert_eq!(roots(&rig.store, &local), ["/srv/api"]);
    }

    #[tokio::test]
    async fn the_ssh_settings_are_what_every_command_is_built_with() {
        let store = Store::open_in_memory().unwrap();
        let engine = Engine::new(
            store,
            Arc::new(ScriptedRunner::new()),
            SshOptions::new("/run/leon", true),
            HistoryRoots::default(),
            Handle::current(),
        );
        assert_eq!(
            engine.ssh().control_dir.as_deref(),
            Some(std::path::Path::new("/run/leon"))
        );
        prefs_of(&engine, |prefs| {
            prefs.ssh_persist_minutes = 30;
            prefs.ssh_connect_timeout = Some(9);
        });
        let ssh = engine.ssh();
        assert_eq!((ssh.persist_minutes, ssh.connect_timeout), (30, Some(9)));
        prefs_of(&engine, |prefs| prefs.ssh_multiplex = false);
        assert!(engine.ssh().control_dir.is_none(), "sharing is off");
        prefs_of(&engine, |prefs| prefs.ssh_multiplex = true);
        assert!(engine.ssh().control_dir.is_some(), "and on again");
    }

    #[tokio::test]
    async fn the_history_folders_are_where_an_import_reads() {
        let dir = tempfile::tempdir().unwrap();
        let rig = rig(ScriptedRunner::new());
        prefs_of(&rig.engine, |prefs| {
            prefs.roots.claude_projects = Some(dir.path().join("claude"));
        });
        assert_eq!(
            rig.engine.prefs().roots.claude_projects,
            Some(dir.path().join("claude"))
        );
        // An import over a folder that does not exist imports nothing and
        // does not fail.
        rig.engine.run(Op::ImportHistory).await;
        assert!(
            status(&rig.engine).kind != StatusKind::Error,
            "{:?}",
            status(&rig.engine)
        );
    }

    #[tokio::test]
    async fn restoring_a_dismissed_root_lets_discovery_adopt_it_at_the_next_refresh() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api", "a");
        rig.engine.run(Op::Refresh).await;
        let project = rig.store.projects(Some(&local)).unwrap().remove(0);
        rig.engine.run(Op::RemoveProject(project.id)).await;
        rig.engine.run(Op::Refresh).await;
        assert!(roots(&rig.store, &local).is_empty());
        rig.engine
            .run(Op::RestoreRoot {
                machine: local.clone(),
                root: "/srv/api".into(),
            })
            .await;
        assert!(status(&rig.engine).text.contains("again"));
        rig.engine.run(Op::Refresh).await;
        assert_eq!(roots(&rig.store, &local), ["/srv/api"]);
    }

    /// Git for `/srv/api` once its linked worktree has been removed: the
    /// removal deletes the folder, so asking git there cannot even start.
    fn git_that_forgets_a_removed_worktree(
    ) -> impl Fn(&CommandSpec) -> Result<Output, RunError> + Send + Sync {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let removed = AtomicBool::new(false);
        move |spec| {
            let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
            if args.starts_with(&["worktree", "remove"]) {
                removed.store(true, SeqCst);
                return Ok(Output::ok(""));
            }
            if !args.starts_with(&["worktree", "list"]) {
                return Ok(Output::failed(128, "fatal: not a git repository"));
            }
            match (spec.cwd.as_deref(), removed.load(SeqCst)) {
                (Some("/srv/api-worktrees/feature-login"), true) => Err(RunError::Spawn {
                    program: "git".into(),
                    source: std::io::Error::other("no such directory"),
                }),
                (Some("/srv/api"), true) => Ok(Output::ok(
                    "worktree /srv/api\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n",
                )),
                (Some("/srv/api" | "/srv/api-worktrees/feature-login"), false) => {
                    Ok(Output::ok(LISTING))
                }
                _ => Ok(Output::failed(128, "fatal: not a git repository")),
            }
        }
    }

    #[tokio::test]
    async fn a_worktree_whose_folder_was_deleted_behind_gits_back_is_not_kept() {
        let stale = format!("{LISTING}prunable gitdir file points to non-existent location\n");
        let rig = rig(ScriptedRunner::new().reply(Output::ok(stale)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let paths: Vec<_> = rig
            .store
            .worktrees(&project.id)
            .unwrap()
            .into_iter()
            .map(|worktree| worktree.path)
            .collect();
        assert_eq!(
            paths,
            ["/srv/api"],
            "git still lists it, but it is not there"
        );
    }

    #[tokio::test]
    async fn the_sessions_of_a_removed_worktree_stay_out_of_the_sidebar_at_every_refresh() {
        let rig = discovery_rig(Answering::new(git_that_forgets_a_removed_worktree()));
        let local = MachineId::local();
        session_in(&rig.store, &local, "/srv/api-worktrees/feature-login", "a");
        rig.engine.run(Op::Refresh).await;
        let project = rig.store.projects(Some(&local)).unwrap().remove(0);
        let extra = rig.store.worktrees(&project.id).unwrap()[1].clone();
        rig.engine
            .run(Op::RemoveWorktree {
                project: project.id.clone(),
                worktree: extra.id,
            })
            .await;
        for _ in 0..2 {
            rig.engine.run(Op::Refresh).await;
            assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 1);
            assert_eq!(roots(&rig.store, &local), ["/srv/api"]);
        }
        assert!(rig
            .store
            .dismissed_roots(&local)
            .unwrap()
            .contains("/srv/api-worktrees/feature-login"));
    }

    // ----- usage limits

    struct NoCredentials;
    impl leon_usage::network::Credentials for NoCredentials {
        fn claude(&self) -> leon_usage::network::Read<leon_usage::claude::Credential> {
            leon_usage::network::Read::Missing
        }
        fn opencode_go(&self) -> leon_usage::network::Read<leon_usage::secret::Secret> {
            leon_usage::network::Read::Missing
        }
    }

    const CODEX_EVENT: &str = r#"{"timestamp":"2026-10-04T18:59:06.753Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":42.0,"window_minutes":300,"resets_at":1791154068},"secondary":{"used_percent":15.0,"window_minutes":10080,"resets_at":1791678919},"plan_type":"plus"}}}"#;
    const USAGE_NOW: i64 = 1_791_140_000;

    fn usage_rig(runner: ScriptedRunner) -> (Rig, Arc<leon_usage::network::ScriptedHttp>) {
        let rig = rig(runner);
        // The scripted runner needs no shell: these tests read this computer
        // as one that has a POSIX shell, on every platform. Windows has its
        // own test below.
        rig.engine.set_local_posix_shell(true);
        let http = Arc::new(leon_usage::network::ScriptedHttp::new());
        rig.engine.set_usage(
            Arc::new(NoCredentials),
            http.clone(),
            Arc::new(|| USAGE_NOW),
        );
        (rig, http)
    }

    fn found_output() -> Output {
        Output::ok(format!("has=codex\nhas=claude\n@codex\n{CODEX_EVENT}\n"))
    }

    #[tokio::test]
    async fn collecting_stores_each_agents_latest_reading_and_the_history() {
        let (rig, http) = usage_rig(ScriptedRunner::new().reply(found_output()));
        rig.engine.run(Op::CollectUsage).await;
        let rows = rig.store.usage_readings().unwrap();
        assert_eq!(rows.len(), leon_usage::network::switchable_agents().len());
        let codex = rows
            .iter()
            .find(|r| r.agent == leon_core::AgentId::CODEX)
            .unwrap();
        let reading: leon_usage::AgentUsage = serde_json::from_str(&codex.payload).unwrap();
        assert!(matches!(reading.state, leon_usage::State::Known { .. }));
        let account = leon_usage::series_key("local", leon_core::AgentId::CODEX, Some("plus"));
        let history = rig
            .store
            .usage_history(
                &MachineId::local(),
                leon_core::AgentId::CODEX,
                &account,
                "five_hour",
                0,
            )
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].used_percent, 42.0);
        assert_eq!(http.calls().len(), 0);
        assert!(!rig.engine.usage_collecting());
    }

    #[tokio::test]
    async fn this_computer_without_a_posix_shell_is_unsupported_but_a_server_is_read() {
        let (rig, http) = usage_rig(ScriptedRunner::new().reply(found_output()));
        rig.engine.set_local_posix_shell(false);
        rig.store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        rig.engine.run(Op::CollectUsage).await;
        let calls = rig.runner.calls();
        assert_eq!(calls.len(), 1, "only the server is asked");
        assert_eq!(calls[0].program, "ssh");
        let rows = rig.store.usage_readings().unwrap();
        let of = |machine: &MachineId, agent| {
            rows.iter()
                .find(|r| &r.machine == machine && r.agent == agent)
                .map(|r| r.payload.clone())
                .unwrap_or_default()
        };
        let local = MachineId::local();
        assert!(of(&local, leon_core::AgentId::CODEX).contains("not_supported"));
        let server = rows
            .iter()
            .find(|r| r.machine != local)
            .unwrap()
            .machine
            .clone();
        assert!(of(&server, leon_core::AgentId::CODEX).contains("\"known\""));
        assert_eq!(http.calls().len(), 0);
    }

    #[tokio::test]
    async fn a_disabled_network_source_makes_no_call_through_the_engine() {
        let (rig, http) = usage_rig(ScriptedRunner::new().reply(found_output()));
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 0);
        let rows = rig.store.usage_readings().unwrap();
        let claude = rows
            .iter()
            .find(|r| r.agent == leon_core::AgentId::CLAUDE)
            .unwrap();
        assert!(claude.payload.contains("source_disabled"));
    }

    #[tokio::test]
    async fn an_engine_without_usage_setup_collects_nothing() {
        let rig = rig(ScriptedRunner::new());
        rig.engine.run(Op::CollectUsage).await;
        assert!(rig.runner.calls().is_empty());
        assert!(rig.store.usage_readings().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_machine_keeps_the_reading_it_had() {
        let (rig, _) = usage_rig(
            ScriptedRunner::new()
                .reply(found_output())
                .reply(Output::failed(255, "connection refused")),
        );
        rig.engine.run(Op::CollectUsage).await;
        rig.engine.run(Op::CollectUsage).await;
        let rows = rig.store.usage_readings().unwrap();
        let codex = rows
            .iter()
            .find(|r| r.agent == leon_core::AgentId::CODEX)
            .unwrap();
        assert!(codex.payload.contains("\"known\""));
    }

    #[tokio::test]
    async fn each_machine_is_collected_with_its_own_command() {
        let (rig, _) = usage_rig(
            ScriptedRunner::new()
                .reply(found_output())
                .reply(found_output()),
        );
        rig.store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        rig.engine.run(Op::CollectUsage).await;
        let programs: Vec<String> = rig.runner.calls().into_iter().map(|c| c.program).collect();
        assert_eq!(programs.len(), 2);
        assert!(programs.contains(&"ssh".to_owned()));
        let machines: std::collections::HashSet<_> = rig
            .store
            .usage_readings()
            .unwrap()
            .into_iter()
            .map(|r| r.machine)
            .collect();
        assert_eq!(machines.len(), 2);
    }

    #[tokio::test]
    async fn forgetting_the_history_reports_how_much_it_removed() {
        let (rig, _) = usage_rig(ScriptedRunner::new().reply(found_output()));
        rig.engine.run(Op::CollectUsage).await;
        rig.engine.run(Op::ForgetUsageHistory).await;
        assert!(status(&rig.engine).text.starts_with("Forgot "));
        let account = leon_usage::series_key("local", leon_core::AgentId::CODEX, Some("plus"));
        assert!(rig
            .store
            .usage_history(
                &MachineId::local(),
                leon_core::AgentId::CODEX,
                &account,
                "five_hour",
                0
            )
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn refreshing_collects_usage_when_it_is_set_up() {
        let (rig, _) = usage_rig(ScriptedRunner::new());
        rig.engine.run(Op::Refresh).await;
        // The scripted runner has nothing queued, so the collection failed to
        // run: every agent is unreachable, but it was attempted.
        let rows = rig.store.usage_readings().unwrap();
        assert_eq!(rows.len(), leon_usage::network::switchable_agents().len());
    }

    // ----- the network sources, live

    /// A credential reader that answers as told and counts its reads.
    struct Scripted {
        mode: Mutex<Mode>,
        reads: std::sync::atomic::AtomicUsize,
    }
    #[derive(Clone, Copy)]
    enum Mode {
        Found,
        Denied,
    }
    impl Scripted {
        fn new(mode: Mode) -> Arc<Self> {
            Arc::new(Self {
                mode: Mutex::new(mode),
                reads: Default::default(),
            })
        }
        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    impl leon_usage::network::Credentials for Scripted {
        fn claude(&self) -> leon_usage::network::Read<leon_usage::claude::Credential> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match *self.mode.lock().unwrap() {
                Mode::Found => leon_usage::claude::parse_credential(
                    r#"{"claudeAiOauth":{"accessToken":"tok","subscriptionType":"max"}}"#,
                )
                .map_or(
                    leon_usage::network::Read::Missing,
                    leon_usage::network::Read::Found,
                ),
                Mode::Denied => leon_usage::network::Read::Denied,
            }
        }
        fn opencode_go(&self) -> leon_usage::network::Read<leon_usage::secret::Secret> {
            leon_usage::network::Read::Missing
        }
    }

    const CLAUDE_ANSWER: &str = r#"{"five_hour":{"utilization":10,"resets_at":1791300000},"seven_day":{"utilization":91,"resets_at":1791300000}}"#;

    fn network_rig(
        mode: Mode,
        http: leon_usage::network::ScriptedHttp,
        passes: usize,
    ) -> (Rig, Arc<Scripted>, Arc<leon_usage::network::ScriptedHttp>) {
        let mut runner = ScriptedRunner::new();
        for _ in 0..passes {
            runner = runner.reply(found_output());
        }
        let rig = rig(runner);
        rig.engine.set_local_posix_shell(true);
        let credentials = Scripted::new(mode);
        let http = Arc::new(http);
        rig.engine
            .set_usage(credentials.clone(), http.clone(), Arc::new(|| USAGE_NOW));
        (rig, credentials, http)
    }

    fn claude_row(rig: &Rig) -> String {
        rig.store
            .usage_readings()
            .unwrap()
            .into_iter()
            .find(|r| r.agent == leon_core::AgentId::CLAUDE)
            .unwrap()
            .payload
    }

    fn on() -> leon_usage::network::NetworkPolicy {
        leon_usage::network::NetworkPolicy::none().with(leon_core::AgentId::CLAUDE)
    }

    #[tokio::test]
    async fn the_live_preference_reaches_the_collector() {
        let (rig, _, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new().reply(200, CLAUDE_ANSWER),
            2,
        );
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        assert_eq!(http.calls().len(), 0, "off: nothing is called");
        assert!(claude_row(&rig).contains("source_disabled"));
        rig.engine.set_usage_policy(on());
        rig.engine
            .run(Op::CollectUsageNow(vec![leon_core::AgentId::CLAUDE]))
            .await;
        assert_eq!(http.calls().len(), 1, "on: read at once");
        assert!(claude_row(&rig).contains("\"known\""));
    }

    #[tokio::test]
    async fn turning_a_source_off_stops_the_calls_and_replaces_the_numbers() {
        let (rig, _, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new().reply(200, CLAUDE_ANSWER),
            2,
        );
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        assert!(claude_row(&rig).contains("\"known\""));
        rig.engine
            .set_usage_policy(leon_usage::network::NetworkPolicy::default());
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 1);
        assert!(claude_row(&rig).contains("source_disabled"));
    }

    #[tokio::test]
    async fn a_rate_limit_waits_as_long_as_the_service_asked_and_a_manual_read_goes_through() {
        let (rig, _, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new()
                .reply_after(429, "{}", Some(300))
                .reply(200, CLAUDE_ANSWER),
            3,
        );
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 1);
        let status = rig.engine.usage_status(leon_core::AgentId::CLAUDE);
        assert_eq!(status.failed, Some(leon_usage::Reason::RateLimited(300)));
        let wait = status.retry_at.unwrap() - USAGE_NOW;
        assert!((300..=330).contains(&wait), "{wait}");
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 1, "the schedule keeps off the service");
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        assert_eq!(http.calls().len(), 2, "a manual read is not held back");
        assert!(rig
            .engine
            .usage_status(leon_core::AgentId::CLAUDE)
            .failed
            .is_none());
    }

    /// Gives the engine a clock the test moves.
    fn moving_clock(
        rig: &Rig,
        credentials: &Arc<Scripted>,
        http: &Arc<leon_usage::network::ScriptedHttp>,
    ) -> Arc<std::sync::atomic::AtomicI64> {
        let clock = Arc::new(std::sync::atomic::AtomicI64::new(USAGE_NOW));
        let read = clock.clone();
        rig.engine.set_usage(
            credentials.clone(),
            http.clone(),
            Arc::new(move || read.load(std::sync::atomic::Ordering::SeqCst)),
        );
        clock
    }

    #[tokio::test]
    async fn a_scheduled_read_never_calls_a_vendor_twice_within_a_minute() {
        let (rig, credentials, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new()
                .reply(200, CLAUDE_ANSWER)
                .reply(200, CLAUDE_ANSWER)
                .reply(200, CLAUDE_ANSWER),
            5,
        );
        let clock = moving_clock(&rig, &credentials, &http);
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 1);
        // A schedule set below a minute asks again after thirty seconds:
        // the vendor is not called.
        clock.fetch_add(30, std::sync::atomic::Ordering::SeqCst);
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 1, "within a minute of the last call");
        assert!(claude_row(&rig).contains("\"known\""), "the numbers stay");
        clock.fetch_add(31, std::sync::atomic::Ordering::SeqCst);
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 2, "a minute on, it may be called");
        // A manual refresh reads at once.
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        assert_eq!(http.calls().len(), 3);
    }

    #[tokio::test]
    async fn a_429_without_a_wait_rests_five_minutes_and_keeps_the_numbers() {
        let (rig, credentials, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new()
                .reply(200, CLAUDE_ANSWER)
                .reply(429, "{}")
                .reply(200, CLAUDE_ANSWER),
            5,
        );
        let clock = moving_clock(&rig, &credentials, &http);
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsage).await;
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        assert_eq!(http.calls().len(), 2);
        let status = rig.engine.usage_status(leon_core::AgentId::CLAUDE);
        assert_eq!(status.failed, Some(leon_usage::Reason::RateLimited(0)));
        let rest = status.retry_at.unwrap() - USAGE_NOW;
        assert!((300..=330).contains(&rest), "{rest}");
        assert!(claude_row(&rig).contains("\"known\""), "numbers kept");
        // Four minutes on the schedule still keeps off; after five it reads.
        clock.fetch_add(4 * 60, std::sync::atomic::Ordering::SeqCst);
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 2);
        clock.fetch_add(2 * 60, std::sync::atomic::Ordering::SeqCst);
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 3);
        assert!(rig
            .engine
            .usage_status(leon_core::AgentId::CLAUDE)
            .failed
            .is_none());
    }

    #[tokio::test]
    async fn an_expired_token_keeps_the_last_numbers_and_says_what_to_do() {
        let (rig, _, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new()
                .reply(200, CLAUDE_ANSWER)
                .reply(401, "{}"),
            3,
        );
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        rig.engine.run(Op::CollectUsageNow(vec![])).await;
        assert_eq!(http.calls().len(), 2);
        assert_eq!(
            rig.engine.usage_status(leon_core::AgentId::CLAUDE).failed,
            Some(leon_usage::Reason::SessionExpired)
        );
        let row = claude_row(&rig);
        assert!(row.contains("\"known\""), "the numbers stay: {row}");
        assert!(!row.contains("not_signed_in"), "{row}");
    }

    #[tokio::test]
    async fn a_source_switched_on_again_is_not_held_back_by_an_old_back_off() {
        let (rig, _, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new()
                .reply(500, "oops")
                .reply(200, CLAUDE_ANSWER),
            3,
        );
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsage).await;
        assert!(rig
            .engine
            .usage_status(leon_core::AgentId::CLAUDE)
            .retry_at
            .is_some());
        rig.engine
            .set_usage_policy(leon_usage::network::NetworkPolicy::default());
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 2);
    }

    #[tokio::test]
    async fn a_refused_keychain_is_asked_once_a_session_until_the_user_tries_again() {
        let (rig, credentials, http) = network_rig(
            Mode::Denied,
            leon_usage::network::ScriptedHttp::new().reply(200, CLAUDE_ANSWER),
            6,
        );
        rig.engine.set_usage_policy(on());
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(credentials.reads(), 1);
        assert!(claude_row(&rig).contains("keychain_denied"));
        assert!(rig.engine.usage_status(leon_core::AgentId::CLAUDE).refused);
        for _ in 0..3 {
            rig.engine.run(Op::CollectUsage).await;
        }
        assert_eq!(credentials.reads(), 1, "never asked again by the schedule");
        assert!(
            claude_row(&rig).contains("keychain_denied"),
            "still said, not 'off'"
        );
        *credentials.mode.lock().unwrap() = Mode::Found;
        rig.engine
            .run(Op::CollectUsageNow(vec![leon_core::AgentId::CLAUDE]))
            .await;
        assert_eq!(credentials.reads(), 2);
        assert_eq!(http.calls().len(), 1);
        assert!(claude_row(&rig).contains("\"known\""));
    }

    /// A client that takes a while to answer and notes how many calls overlap.
    #[derive(Default)]
    struct SlowHttp {
        now: std::sync::atomic::AtomicUsize,
        most: std::sync::atomic::AtomicUsize,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl leon_usage::network::Http for SlowHttp {
        fn get<'a>(
            &'a self,
            _: &'a leon_usage::network::Request<'a>,
        ) -> leon_usage::network::BoxFuture<
            'a,
            Result<leon_usage::network::Response, leon_usage::network::HttpError>,
        > {
            use std::sync::atomic::Ordering::SeqCst;
            Box::pin(async move {
                self.calls.fetch_add(1, SeqCst);
                let inside = self.now.fetch_add(1, SeqCst) + 1;
                self.most.fetch_max(inside, SeqCst);
                for _ in 0..50 {
                    tokio::task::yield_now().await;
                }
                self.now.fetch_sub(1, SeqCst);
                Ok(leon_usage::network::Response {
                    status: 200,
                    body: CLAUDE_ANSWER.to_owned(),
                    retry_after: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn at_most_one_request_per_source_is_in_flight() {
        use std::sync::atomic::Ordering::SeqCst;
        let mut runner = ScriptedRunner::new();
        for _ in 0..4 {
            runner = runner.reply(found_output());
        }
        let rig = rig(runner);
        rig.engine.set_local_posix_shell(true);
        let http = Arc::new(SlowHttp::default());
        rig.engine.set_usage(
            Scripted::new(Mode::Found),
            http.clone(),
            Arc::new(|| USAGE_NOW),
        );
        rig.engine.set_usage_policy(on());
        tokio::join!(
            rig.engine.run(Op::CollectUsage),
            rig.engine.run(Op::CollectUsage),
            rig.engine.run(Op::CollectUsageNow(vec![])),
            rig.engine.run(Op::Refresh),
        );
        assert_eq!(http.most.load(SeqCst), 1, "never two at once");
        assert!(http.calls.load(SeqCst) >= 1);
    }

    #[tokio::test]
    async fn nothing_is_read_while_the_application_defers_it() {
        let (rig, _, http) = network_rig(
            Mode::Found,
            leon_usage::network::ScriptedHttp::new().reply(200, CLAUDE_ANSWER),
            2,
        );
        rig.engine.set_usage_policy(on());
        rig.engine.defer_usage();
        rig.engine.run(Op::CollectUsage).await;
        rig.engine.run(Op::Refresh).await;
        assert_eq!(http.calls().len(), 0);
        assert!(rig.store.usage_readings().unwrap().is_empty());
        assert!(rig.engine.start_usage());
        assert!(!rig.engine.start_usage());
        rig.engine.run(Op::CollectUsage).await;
        assert_eq!(http.calls().len(), 1);
    }
}
