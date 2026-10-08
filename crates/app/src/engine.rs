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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::DateTime;
use leon_core::icon::{IconImage, IconKind, NewIcon};
use leon_core::{
    AgentId, Machine, MachineId, MachineKind, NewMessage, NewSession, NewWorktree, Project,
    ProjectId, SessionFilter, Store, StoreChange, StoreError, WorktreeId,
};
use leon_history::{HistoryRoots, ImportReport, Importer};
use leon_link::client::ConnState;
use leon_remote::connect::{self, Checklist, Target as Login};
use leon_remote::files as remote_files;
use leon_remote::search as remote_search;
use leon_remote::{
    github, probe, run_on, CommandSpec, Git, GitError, Github, Output, ProbeError, ProbeReport,
    RelayHub, RunError, Runner, SshOptions,
};
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, Semaphore};
use tokio::task::{JoinHandle, JoinSet};

use crate::address;
use crate::avatar::{Fetched, IconFetcher, NoFetch};
use crate::elsewhere::{self, Found};
use crate::files::{self, FileContent, FileEntry, FileError, FileRevision, GitMarks, WriteOutcome};
use crate::project::{self, ProjectState};

mod changes;
pub use changes::{CommitRequest, ShipReport, ShipRequest};

/// How many folders are resolved with git at the same time while projects are
/// being discovered.
const DISCOVERY_PARALLELISM: usize = 8;

/// How many of a machine's sessions a scan reads to tie processes to them.
const SCAN_SESSIONS: usize = 50_000;

/// The least time between two questions to GitHub about one project. The
/// answers are kept in the store, so asking again sooner would only spend the
/// person's rate limit.
const GITHUB_ASK_EVERY: Duration = Duration::from_secs(60);

/// How many open pull requests one question asks for.
const OPEN_PULL_REQUESTS: usize = 100;

/// Whether a project last asked GitHub at `last` may ask again at `now`.
fn github_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= GITHUB_ASK_EVERY)
}

/// How long a search of a machine's files may take before it is given up.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Pull what the paired host shares of its own Leon — its projects and
    /// the sessions of its history — into the store, so that machine's
    /// section of the sidebar looks like the host's own. Asked on a refresh,
    /// on the import timer and after a fresh pairing.
    SyncHost(MachineId),
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
    /// Clone a repository into a folder on a machine, then add it as a
    /// project. Any git URL git itself accepts works (HTTPS, SSH, a path to
    /// a local repository).
    CloneProject {
        /// The machine the clone happens on.
        machine: MachineId,
        /// The repository to clone.
        url: String,
        /// Absolute path of the folder the clone goes into.
        parent: String,
        /// The folder the clone takes (its name).
        name: String,
    },
    /// Create a brand-new git repository (empty, with one initial commit so
    /// worktrees have a branch to hang from), then add it as a project.
    CreateProject {
        /// The machine the repository is created on.
        machine: MachineId,
        /// Absolute path of the folder that holds it.
        parent: String,
        /// Its name, which is also its folder's.
        name: String,
    },
    /// Remove a project from the list and remember that it was removed, so
    /// that discovery does not bring it back.
    RemoveProject(ProjectId),
    /// Turn the shared memory on or off for a project of this computer: write
    /// the managed block into its instruction files, or take it out (what
    /// `leon memory enable` and `disable` do, without the MCP entry).
    ProjectMemory {
        /// The project.
        project: ProjectId,
        /// On, or off.
        on: bool,
    },
    /// "Never for this project": remember not to ask about its agent memory
    /// again.
    MemoryNever(ProjectId),
    /// Write every memory file again: the size they may have was changed in
    /// the settings. Says nothing.
    RewriteMemoryFiles,
    /// Write the memory file of a project again, so that the terminal just
    /// started in it finds the file its `LEON_MEMORY` names. Says nothing.
    WriteMemoryFile {
        /// The root of the project the terminal's folder belongs to.
        root: String,
    },
    /// Give a project another name.
    RenameProject {
        /// The project.
        project: ProjectId,
        /// Its new name.
        name: String,
    },
    /// Write the sidebar order of one machine's projects: first row first.
    ReorderProjects {
        /// The machine the projects live on.
        machine: MachineId,
        /// Every project of the machine in its new order.
        ordered: Vec<ProjectId>,
    },
    /// Write the sidebar order of one project's worktrees: first row first.
    ReorderWorktrees {
        /// The project the worktrees belong to.
        project: ProjectId,
        /// Every worktree of the project in its new order.
        ordered: Vec<WorktreeId>,
    },
    /// Pin sessions in this order, as the pinned order of their parent (the
    /// Pinned section of a machine); every other session of that parent goes
    /// back to automatic (by recency).
    PinSessions {
        /// The parent whose pinned order is set.
        parent: leon_core::SessionScope,
        /// The pinned sessions, first row first.
        pinned: Vec<leon_core::SessionId>,
        /// What the status line says once it is done.
        done: &'static str,
    },
    /// Put sessions on the sidebar's shelves, or take them off: the Settled
    /// shelf, the Snoozed shelf until a time, or neither (`None`). A session
    /// that is already as asked, or is gone, is left alone.
    SetShelves {
        /// The sessions and where each goes.
        changes: Vec<(leon_core::SessionId, Option<leon_core::Shelf>)>,
        /// What the status line says once it is done; nothing when `None`.
        done: Option<String>,
    },
    /// Give an SSH machine another name.
    RenameMachine {
        /// The machine.
        machine: MachineId,
        /// Its new name.
        name: String,
    },
    /// Remove an SSH machine with its projects and sessions.
    RemoveMachine(MachineId),
    /// Give a history session a name of Leon's own; it shows over the agent's
    /// title and survives imports (the agent's own file is left alone).
    RenameSession {
        /// The session.
        session: leon_core::SessionId,
        /// Its new name; blank gives the agent's own title back.
        name: String,
    },
    /// Remember that a history session ran with an account of its agent (an
    /// id), so that resuming it starts the same one. Says nothing.
    SetSessionAccount {
        /// The session.
        session: leon_core::SessionId,
        /// The account's id.
        account: String,
    },
    /// Remove one session from the history (the agent's own file is left
    /// alone).
    RemoveSession(leon_core::SessionId),
    /// Takes sessions out of the history without a word: they went with
    /// something that was closed, which says so itself. Ones the history no
    /// longer has are skipped.
    ForgetSessions(Vec<leon_core::SessionId>),
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
    /// Drop the readings and the history of every account of an agent whose id
    /// is not in `keep`: what a removed account leaves behind. The ids are
    /// sent with the request because the engine reads the settings only when
    /// the window has applied them.
    DropAccountUsage {
        /// The ids of the accounts that are still in the settings.
        keep: Vec<String>,
    },
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
    /// There is a question to ask about a project's agent memory
    /// ([`Engine::take_memory_offer`]).
    MemoryOffer,
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
    /// A file could not be read, saved or listed.
    #[error("{0}")]
    File(String),
    /// A background job did not finish.
    #[error("A background job failed: {0}")]
    Job(String),
}

/// What removing a worktree came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// It is gone; the words for the status line.
    Removed(String),
    /// git refused it because it holds modified or untracked files: only
    /// `--force` removes it, which deletes them, so the person is asked
    /// first. Nothing has been removed.
    NeedsForce,
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
    /// What each project's `leon.toml` said when it was last read.
    project_files: HashMap<ProjectId, ProjectState>,
    /// The questions about agent memory that wait for the window, oldest
    /// first: one for each project somebody added by hand and was not asked
    /// about yet.
    memory_offers: std::collections::VecDeque<crate::memory_offer::Offer>,
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
    /// Where a new worktree goes: the setting `worktree_location`, a template
    /// of the project's folder and the branch (see
    /// [`address::worktree_location`]).
    pub worktree_location: String,
    /// The user's accounts of the agents: whose limits are read beside the
    /// agents' own, and whose readings stay when they are in this list.
    pub accounts: Vec<leon_core::Account>,
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
    /// The keeper of durable local sessions: what runs below it is this
    /// Leon's own, like what runs below this process.
    keeper: Mutex<Option<Arc<dyn crate::durable::Durable>>>,
    usage: Mutex<Option<UsageSetup>>,
    /// The connections to the computers paired with this one. `None` in the
    /// parts of the engine's life before the window, and in tests without a
    /// relay: without it nothing is asked of the hosts.
    hub: Mutex<Option<Arc<RelayHub>>>,
    /// What each history source looked like at the last incremental import.
    history_stamps: Mutex<HashMap<String, String>>,
    /// Whether the unreadable-layout notice was shown by an incremental import.
    unsupported_told: std::sync::atomic::AtomicBool,
    /// The home folder the history report writes as `~`.
    history_home: Mutex<Option<std::path::PathBuf>>,
    /// Leon's data folder, where the memory files are written. `None` until
    /// `main` says, and in tests that do not: no memory file is written then.
    memory_dir: Mutex<Option<std::path::PathBuf>>,
    /// The size a memory file may have, in bytes: the setting, which the
    /// window tells the engine at the start and when it changes.
    memory_budget: std::sync::atomic::AtomicUsize,
    /// Whether a project added by hand is asked about: the setting. Off until
    /// the window says, so that nothing is asked where there is no window.
    memory_offer: std::sync::atomic::AtomicBool,
    /// The roots (by identity) asked about since Leon started: "not now" is
    /// not asked again in the same run.
    memory_asked: Mutex<std::collections::HashSet<String>>,
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
    /// When each project last asked GitHub about its pull requests, so that a
    /// refresh, the timer and a new worktree together ask once a minute.
    github_asked: Mutex<HashMap<ProjectId, Instant>>,
    /// The runner of commands that may take minutes (a commit with its hooks,
    /// a push, an agent asked for a message); the usual one stops a command
    /// after thirty seconds. The usual runner serves when none was set.
    slow_runner: Mutex<Option<Arc<dyn Exec>>>,
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
                    worktree_location: address::DEFAULT_WORKTREE_LOCATION.to_owned(),
                    accounts: Vec::new(),
                }),
                handle,
                state: Mutex::new(State {
                    history_report: None,
                    status: None,
                    machines: HashMap::new(),
                    elsewhere: HashMap::new(),
                    project_files: HashMap::new(),
                    memory_offers: std::collections::VecDeque::new(),
                }),
                events,
                fetcher: Mutex::new(Arc::new(NoFetch)),
                scanner: Mutex::new(None),
                keeper: Mutex::new(None),
                usage: Mutex::new(None),
                hub: Mutex::new(None),
                history_stamps: Mutex::new(HashMap::new()),
                unsupported_told: std::sync::atomic::AtomicBool::new(false),
                history_home: Mutex::new(leon_history::home_dir()),
                memory_dir: Mutex::new(None),
                memory_budget: std::sync::atomic::AtomicUsize::new(
                    leon_memory::render::DEFAULT_BUDGET,
                ),
                memory_offer: std::sync::atomic::AtomicBool::new(false),
                memory_asked: Mutex::new(std::collections::HashSet::new()),
                usage_policy: Mutex::new(leon_usage::network::NetworkPolicy::default()),
                blocking: std::sync::atomic::AtomicUsize::new(0),
                local_posix_shell: std::sync::atomic::AtomicBool::new(
                    crate::platform::local_has_posix_shell(),
                ),
                github_asked: Mutex::new(HashMap::new()),
                slow_runner: Mutex::new(None),
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

    /// Tells the engine where Leon's data folder is, for the memory files it
    /// writes when a terminal starts. Without it none is written.
    pub fn set_memory_dir(&self, data_dir: std::path::PathBuf) {
        *self
            .inner
            .memory_dir
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(data_dir);
    }

    /// Whether a project added by hand is asked about its agent memory.
    pub fn set_memory_offer(&self, on: bool) {
        self.inner
            .memory_offer
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// The oldest question about agent memory that waits, taken off the
    /// queue: the window shows it.
    pub fn take_memory_offer(&self) -> Option<crate::memory_offer::Offer> {
        self.state().memory_offers.pop_front()
    }

    /// What there is to say or ask about the agent memory of a project that
    /// somebody has just added by hand. The project's files are looked at on
    /// the blocking pool; a question is queued for the window
    /// ([`EngineEvent::MemoryOffer`]); and when the files hold the block
    /// already, the words for the status line come back. A project of
    /// another machine, and anything that goes wrong, is silence: this never
    /// fails the adding.
    async fn memory_note(&self, project: &leon_core::Project) -> Option<String> {
        use crate::memory_offer::{decide, Decision, Facts, Offer};
        if !project.machine_id.is_local() {
            return None;
        }
        let root = std::path::PathBuf::from(&project.root);
        let found = self
            .blocking(move || crate::memory::look(&root))
            .await
            .ok()?;
        let key = leon_core::path::key(&project.root);
        let facts = Facts {
            local: true,
            explicit: true,
            setting_on: self
                .inner
                .memory_offer
                .load(std::sync::atomic::Ordering::SeqCst),
            answered: self
                .inner
                .store
                .memory_choice(&project.root)
                .ok()?
                .map(|(choice, _)| choice),
            asked_this_run: self
                .inner
                .memory_asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&key),
            block_present: !leon_memory::files::holding(&found).is_empty(),
        };
        match decide(&facts) {
            Decision::Nothing => None,
            Decision::AlreadyOn => {
                self.record_memory_choice(&project.root, leon_core::MemoryChoice::On);
                Some(crate::memory_offer::already_on(&project.name))
            }
            Decision::Ask => {
                let offer = Offer::new(
                    project.id.clone(),
                    project.name.clone(),
                    project.root.clone(),
                    &leon_memory::files::enable(&found),
                );
                if !offer.writes_something() {
                    return None;
                }
                self.inner
                    .memory_asked
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key);
                self.state().memory_offers.push_back(offer);
                let _ = self.inner.events.send(EngineEvent::MemoryOffer);
                None
            }
        }
    }

    /// Remembers what was decided about a root. A store that refuses is a
    /// warning: the decision itself was carried out.
    fn record_memory_choice(&self, root: &str, choice: leon_core::MemoryChoice) {
        let at = crate::memory::now_millis();
        if let Err(error) = self.inner.store.set_memory_choice(root, choice, at) {
            tracing::warn!(%error, "the answer about agent memory was not kept");
        }
    }

    /// The size a memory file may have, in bytes.
    pub fn memory_budget(&self) -> usize {
        self.inner
            .memory_budget
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Tells the engine the size a memory file may have. The files are
    /// written again with [`Op::RewriteMemoryFiles`].
    pub fn set_memory_budget(&self, bytes: usize) {
        self.inner
            .memory_budget
            .store(bytes, std::sync::atomic::Ordering::SeqCst);
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

    /// Tells the engine which keeper holds this Leon's durable sessions, so
    /// the agents running in its terminals are not mistaken for sessions
    /// running somewhere else (and never offered to be taken over).
    pub fn set_keeper(&self, keeper: Option<Arc<dyn crate::durable::Durable>>) {
        *self
            .inner
            .keeper
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = keeper;
    }

    /// The pid of the keeper, when one runs.
    fn keeper_pid(&self) -> Option<u32> {
        let keeper = self
            .inner
            .keeper
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        keeper?.keeper_pid()
    }

    /// Gives the engine a runner with a longer time limit for the commands
    /// that may take minutes: committing (hooks run), pushing and asking an
    /// agent to word a message.
    pub fn set_slow_runner<R: Runner + 'static>(&self, runner: Arc<R>) {
        *self
            .inner
            .slow_runner
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(runner);
    }

    /// Lets the engine ask the computers paired with this one what they are
    /// sharing: their projects and history sessions, through `hub`. Without
    /// one nothing is asked and the mirrors stay as they were.
    pub fn set_relay_hub(&self, hub: Arc<RelayHub>) {
        *self
            .inner
            .hub
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(hub);
    }

    fn hub(&self) -> Option<Arc<RelayHub>> {
        self.inner
            .hub
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
                // This process and the keeper it started: what runs below
                // either is this Leon's own.
                let ours: Vec<u32> = if local {
                    scanner
                        .leon_pid
                        .into_iter()
                        .chain(self.keeper_pid())
                        .collect()
                } else {
                    Vec::new()
                };
                Some(elsewhere::resolve_with(&scan, &sessions, &ours))
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
        let local_posix = self
            .inner
            .local_posix_shell
            .load(std::sync::atomic::Ordering::Relaxed);
        let mut collected = leon_usage::collect_machine_on(
            &runner,
            machine,
            local_posix,
            &self.ssh(),
            &policy,
            credentials.as_ref(),
            http.as_ref(),
            now,
        )
        .await;
        // The user's other accounts of Claude Code and Codex, read from their
        // own folders on this computer, each as a reading of its own.
        let folders = crate::agent_usage::account_folders(&self.prefs().roots);
        let accounts = leon_usage::collect_accounts(
            &runner,
            machine,
            local_posix,
            &self.ssh(),
            &policy,
            credentials.as_ref(),
            http.as_ref(),
            &folders,
            now,
        )
        .await;
        let previous: HashMap<leon_core::AgentId, leon_usage::AgentUsage> = self
            .inner
            .store
            .usage_readings()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.machine == machine.id && row.account.is_empty())
            .filter_map(|row| {
                serde_json::from_str::<leon_usage::AgentUsage>(&row.payload)
                    .ok()
                    .map(|usage| (row.agent, usage))
            })
            .collect();
        let local = machine.kind == MachineKind::Local;
        // Back-off bookkeeping for the sources that were really called. An
        // account's read counts for its agent: one failure backs the agent's
        // source off, whichever account it was.
        if local {
            let mut usage = self.usage_setup();
            if let Some(setup) = usage.as_mut() {
                let called = crate::agent_usage::fold_called(
                    collected
                        .called
                        .iter()
                        .chain(accounts.iter().flat_map(|one| one.called.iter())),
                );
                for (agent, reason) in called {
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
        if local {
            self.store_account_usage(machine, &asked, &held, &refused, &accounts, now);
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

    /// Drops the latest reading and the stored history of every account that is
    /// not in `keep` (ids), on every machine. The history is stored under a
    /// hash of the machine, agent, plan and account, so the series dropped is
    /// the one of the plan the reading last had: points stored under an earlier
    /// plan are not shown any more and age out with the rest of the history.
    fn drop_removed_accounts(&self, keep: &[String]) {
        let store = &self.inner.store;
        let rows = store.usage_readings().unwrap_or_default();
        for row in rows
            .iter()
            .filter(|row| !row.account.is_empty() && !keep.contains(&row.account))
        {
            let plan = serde_json::from_str::<leon_usage::AgentUsage>(&row.payload)
                .ok()
                .and_then(|usage| usage.plan);
            let series = leon_usage::series_key_of(
                row.machine.as_str(),
                row.agent,
                plan.as_deref(),
                Some(&row.account),
            );
            if let Err(error) = store.forget_usage_series(&row.machine, row.agent, &series) {
                tracing::warn!(%error, "could not drop the history of a removed account");
            }
        }
        if let Err(error) = store.retain_usage_accounts(keep) {
            tracing::warn!(%error, "could not drop the usage of removed accounts");
        }
    }

    /// Stores the readings of the user's accounts of the agents the way the
    /// agents' own are stored (a source held back keeps what it showed, a
    /// refused sign-in is said), each under its account, with its history; and
    /// drops the readings of accounts that are not in the settings any more.
    fn store_account_usage(
        &self,
        machine: &Machine,
        asked: &leon_usage::network::NetworkPolicy,
        held: &[leon_core::AgentId],
        refused: &std::collections::HashSet<leon_core::AgentId>,
        accounts: &[leon_usage::MachineUsage],
        now: i64,
    ) {
        let store = &self.inner.store;
        let current: Vec<String> = self
            .prefs()
            .accounts
            .iter()
            .map(|account| account.id.clone())
            .collect();
        self.drop_removed_accounts(&current);
        let previous: HashMap<String, leon_usage::AgentUsage> = store
            .usage_readings()
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.machine == machine.id && !row.account.is_empty())
            .filter_map(|row| {
                serde_json::from_str::<leon_usage::AgentUsage>(&row.payload)
                    .ok()
                    .map(|usage| (row.account, usage))
            })
            .collect();
        for one in accounts {
            for reading in &one.readings {
                let Some(account) = reading.account.clone() else {
                    continue;
                };
                let agent = reading.agent;
                let mut reading = reading.clone();
                if asked_on(asked, agent) && refused.contains(&agent) {
                    reading = leon_usage::AgentUsage::unknown(
                        agent,
                        &reading.machine,
                        leon_usage::Reason::KeychainDenied,
                    )
                    .for_account(&account);
                }
                let merged = crate::agent_usage::merge(previous.get(&account).cloned(), reading);
                let merged = if asked_on(asked, agent)
                    && held.contains(&agent)
                    && matches!(merged.state, leon_usage::State::Unknown { .. })
                {
                    previous.get(&account).cloned().unwrap_or(merged)
                } else {
                    merged
                };
                if let Ok(json) = serde_json::to_string(&merged) {
                    if let Err(error) =
                        store.put_usage_reading_for(&machine.id, agent, &account, &json, now)
                    {
                        tracing::warn!(%error, "could not store an account's usage reading");
                    }
                }
            }
            for (agent, account, points) in
                crate::agent_usage::history_points(one, machine.id.as_str())
            {
                if let Err(error) = store.record_usage_points(
                    &machine.id,
                    agent,
                    &account,
                    &points,
                    now - crate::agent_usage::HISTORY_HORIZON,
                ) {
                    tracing::warn!(%error, "could not store an account's usage history");
                }
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
                self.sync_hosts_quietly().await;
                Ok(None)
            }
            Op::SyncHost(machine) => self.report_host_sync(&machine).await,
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
            Op::CloneProject {
                machine,
                url,
                parent,
                name,
            } => self
                .clone_project(&machine, &url, &parent, &name)
                .await
                .map(Some),
            Op::CreateProject {
                machine,
                parent,
                name,
            } => self
                .create_project(&machine, &parent, &name)
                .await
                .map(Some),
            Op::RemoveProject(project) => self.remove_project(&project).map(Some),
            Op::ProjectMemory { project, on } => self.project_memory(&project, on).await.map(Some),
            Op::WriteMemoryFile { root } => {
                self.write_memory_file(Some(root)).await?;
                Ok(None)
            }
            Op::MemoryNever(project) => {
                let project = self.inner.store.project(&project)?;
                self.inner.store.set_memory_choice(
                    &project.root,
                    leon_core::MemoryChoice::Never,
                    crate::memory::now_millis(),
                )?;
                Ok(Some(crate::memory_offer::never_line(&project.name)))
            }
            Op::RewriteMemoryFiles => {
                self.write_memory_file(None).await?;
                Ok(None)
            }
            Op::RenameProject { project, name } => self.rename_project(&project, &name).map(Some),
            Op::ReorderProjects { machine, ordered } => {
                self.reorder_projects(&machine, &ordered).map(Some)
            }
            Op::ReorderWorktrees { project, ordered } => {
                self.inner.store.reorder_worktrees(&project, &ordered)?;
                Ok(Some("Moved the worktree.".to_owned()))
            }
            Op::PinSessions {
                parent,
                pinned,
                done,
            } => {
                self.inner.store.pin_sessions(&parent, &pinned)?;
                Ok(Some(done.to_owned()))
            }
            Op::SetShelves { changes, done } => {
                // Nothing is said of a change that changed nothing.
                let changed = self.inner.store.set_shelves(&changes)?;
                Ok(done.filter(|_| changed))
            }
            Op::RenameMachine { machine, name } => self.rename_machine(&machine, &name).map(Some),
            Op::RemoveMachine(machine) => self.remove_machine(&machine).map(Some),
            Op::RenameSession { session, name } => {
                let name = name.trim();
                self.inner.store.rename_session(&session, Some(name))?;
                Ok(Some(if name.is_empty() {
                    "Gave the session its own title back.".to_owned()
                } else {
                    format!("Renamed the session to {name}.")
                }))
            }
            Op::SetSessionAccount { session, account } => {
                match self
                    .inner
                    .store
                    .set_session_account(&session, Some(&account))
                {
                    // A session the history no longer has needs no account.
                    Ok(()) | Err(StoreError::NotFound(_)) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            }
            Op::RemoveSession(session) => self.remove_session(&session).map(Some),
            Op::ForgetSessions(sessions) => self.forget_sessions(&sessions).map(|()| None),
            Op::RestoreRoot { machine, root } => self.restore_root(&machine, &root).map(Some),
            Op::AddWorktree {
                project,
                branch,
                base,
            } => self
                .add_worktree(&project, &branch, base.as_deref())
                .await
                .map(Some),
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
            Op::DropAccountUsage { keep } => {
                self.drop_removed_accounts(&keep);
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

    // ----- a paired host's own Leon ---------------------------------------

    /// How many transcripts one sync fetches at most; the rest wait for a
    /// later one, so a first connect does not pull a whole history at once.
    const MAX_TRANSCRIPTS: usize = 40;

    /// Syncs every machine that is a paired host, saying nothing. What a
    /// timer import does after the local one.
    async fn sync_hosts_quietly(&self) {
        let Ok(machines) = self.inner.store.machines() else {
            return;
        };
        for machine in machines {
            if !matches!(machine.kind, MachineKind::Relay { .. }) {
                continue;
            }
            if let Err(error) = self.sync_host(&machine.id).await {
                tracing::debug!(machine = %machine.name, %error, "could not sync what it shares");
            }
        }
    }

    /// Syncs one host and tells what it brought.
    async fn report_host_sync(&self, machine: &MachineId) -> Result<Option<String>, EngineError> {
        let name = self.inner.store.machine(machine)?.name;
        let synced = self.sync_host(machine).await?;
        match synced {
            _ if synced.is_empty() => Ok(None),
            _ => Ok(Some(format!("Synced {name}: {}", synced.describe()))),
        }
    }

    /// Pulls what `machine`'s host shares of its own Leon into the store. The
    /// host is asked while it is reachable (a connecting one is waited for,
    /// any other state is left for a later ask); its projects that are not
    /// here yet are added — never one this computer dismissed — and its
    /// history sessions are stored with their transcripts, newest first and
    /// a few at a time.
    async fn sync_host(&self, machine: &MachineId) -> Result<HostSync, EngineError> {
        let Some(hub) = self.hub() else {
            return Ok(HostSync::default());
        };
        let Ok(machine) = self.inner.store.machine(machine) else {
            return Ok(HostSync::default());
        };
        let MachineKind::Relay {
            host_id,
            host_key,
            relay_url,
            ..
        } = &machine.kind
        else {
            return Err(EngineError::Invalid(format!(
                "{} is not a machine paired with a code.",
                machine.name
            )));
        };
        let client = hub
            .ensure(host_id, host_key, relay_url)
            .map_err(|error| EngineError::Invalid(error.to_string()))?;
        match client.state() {
            ConnState::Online => {}
            ConnState::Connecting => {
                if client.wait_online(Duration::from_secs(20)).await.is_err() {
                    return Ok(HostSync::default());
                }
            }
            ConnState::Offline { .. } | ConnState::Closed => return Ok(HostSync::default()),
        }
        let (projects, sessions, _) = client
            .share_state()
            .await
            .map_err(|error| EngineError::Invalid(error.to_string()))?;

        // The projects first: a session is tied to a project by what the
        // store knows of the machine while it is stored, so the roots have to
        // be there before the sessions land.
        let mut added = 0usize;
        let mut fresh: Vec<ProjectId> = Vec::new();
        {
            let dismissed: HashSet<String> = self
                .inner
                .store
                .dismissed_roots(&machine.id)?
                .into_iter()
                .map(|root| leon_core::path::key(&root))
                .collect();
            let known: HashSet<String> = self
                .inner
                .store
                .projects(Some(&machine.id))?
                .into_iter()
                .map(|project| leon_core::path::key(&project.root))
                .collect();
            for shared in projects {
                let key = leon_core::path::key(&shared.root);
                if known.contains(&key) || dismissed.contains(&key) || shared.root.is_empty() {
                    continue;
                }
                let name = shared.name.trim();
                let name = if name.is_empty() {
                    address::default_project_name(&shared.root)
                } else {
                    name.to_owned()
                };
                match self
                    .inner
                    .store
                    .add_project(&machine.id, &name, &shared.root)
                {
                    Ok(project) => {
                        added += 1;
                        fresh.push(project.id);
                    }
                    Err(error) => {
                        tracing::warn!(root = %shared.root, %error, "could not mirror a project")
                    }
                }
            }
        }
        // A project mirrored into the list wants its worktree rows at once,
        // like one added by hand would have them. git is asked on the host;
        // a listing that fails leaves them for the next refresh.
        for id in &fresh {
            if let Err(error) = self.sync_worktrees(id).await {
                tracing::debug!(project = %id, %error, "could not sync the mirrored worktrees");
            }
        }

        // The history sessions: each is named by its agent and the agent's
        // own id, so the import cursors say which of them were already taken
        // into the store and only what changed since is asked for. A session
        // that came with a transcript beyond this Leon's reply budget was
        // taken as the prefix it could carry, and wants asking again only
        // when the host's own summary moved on.
        let cursors: HashMap<String, String> = self.inner.store.import_cursors(&machine.id)?;
        let mut wait: Vec<leon_wire::SharedSession> = Vec::new();
        for shared in sessions {
            if AgentId::parse(&shared.agent).is_none() {
                // An agent this Leon has no name for: its sessions stay
                // where they are (the host's, not ours).
                continue;
            };
            let key = share_source_key(&shared.agent, &shared.external_id);
            let taken = cursors
                .get(&key)
                .is_some_and(|done| *done == share_fingerprint(&shared));
            if taken {
                continue;
            }
            wait.push(shared);
        }

        let mut synced = HostSync {
            projects: added,
            ..HostSync::default()
        };
        let mut sessions: Vec<(NewSession, Vec<NewMessage>)> = Vec::with_capacity(wait.len());
        let (machine_id, mut cursors) = (machine.id.clone(), Vec::with_capacity(wait.len()));
        for shared in wait.iter().take(Self::MAX_TRANSCRIPTS) {
            let Some(agent) = AgentId::parse(&shared.agent) else {
                continue;
            };
            let fingerprint = share_fingerprint(shared);
            let transcript = match tokio::time::timeout(
                Duration::from_secs(30),
                client.share_transcript(&shared.agent, &shared.external_id),
            )
            .await
            {
                Ok(Ok(transcript)) => transcript,
                // The link went down or the answer did not come: what
                // shifted waits for the next sync. A host that holds no
                // such session any more answers with nothing; its cursor
                // advances all the same, so it is never asked again.
                Ok(Err(error)) => {
                    tracing::debug!(
                        external = %shared.external_id,
                        %error,
                        "could not read one shared transcript"
                    );
                    break;
                }
                Err(_) => {
                    tracing::debug!(
                        external = %shared.external_id,
                        "the shared transcript did not come in time"
                    );
                    break;
                }
            };
            cursors.push((
                share_source_key(&shared.agent, &shared.external_id),
                fingerprint,
            ));
            let Some(transcript) = transcript else {
                continue;
            };
            let started = DateTime::from_timestamp_millis(shared.started_ms)
                .unwrap_or(DateTime::<chrono::Utc>::UNIX_EPOCH);
            let updated = DateTime::from_timestamp_millis(shared.updated_ms)
                .unwrap_or(DateTime::<chrono::Utc>::UNIX_EPOCH);
            sessions.push((
                NewSession {
                    agent,
                    external_id: shared.external_id.clone(),
                    machine_id: machine_id.clone(),
                    cwd: shared.cwd.clone(),
                    title: shared.title.clone(),
                    model: shared.model.clone(),
                    started_at: started,
                    updated_at: updated,
                },
                transcript
                    .messages
                    .into_iter()
                    .filter_map(|entry| transcript_message(&entry.role, entry.text, entry.at_ms))
                    .collect(),
            ));
        }
        if !cursors.is_empty() {
            self.inner.store.write(StoreChange::Sessions, |tx| {
                for (session, messages) in &sessions {
                    leon_core::upsert_session_in(tx, session, messages)?;
                }
                for (key, fingerprint) in &cursors {
                    leon_core::set_import_cursor_in(tx, &machine_id, key, fingerprint)?;
                }
                Ok(())
            })?;
            synced.sessions = sessions.len() as u32;
        }
        Ok(synced)
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
        let count = self
            .inner
            .store
            .replace_worktrees(project_id, listed)?
            .len();
        // A sync is somebody asking about the project (opening it, refreshing
        // it, finding it again), so it is also when the sidebar gets to know
        // the state of each checkout and what GitHub says: the pull requests
        // included, which the timer asks for far less often.
        self.probe_github(&project).await?;
        Ok(count)
    }

    /// Asks git for a project's worktrees and writes them back only when
    /// something changed (a checkout, a new worktree). Runs quietly: it is
    /// called on a timer while a terminal has that project open, and a
    /// failure is not something to say unless the worktrees really change.
    pub fn check_worktrees(&self, project: ProjectId) -> JoinHandle<()> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            if let Err(error) = engine.sync_worktrees_if_changed(&project).await {
                tracing::debug!(%error, "could not check the worktrees");
            }
        })
    }

    async fn sync_worktrees_if_changed(&self, project_id: &ProjectId) -> Result<(), EngineError> {
        let project = self.inner.store.project(project_id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let ssh = self.ssh();
        let git = Git::new(&runner, &machine, &ssh);
        let listed = new_worktrees(&git.list_worktrees(&project).await?);
        let stored = self.inner.store.worktrees(project_id)?;
        let same = listed.len() == stored.len()
            && listed.iter().zip(&stored).all(|(new, old)| {
                trim(new.path.as_str()) == trim(old.path.as_str())
                    && new.branch == old.branch
                    && new.head == old.head
                    && new.is_main == old.is_main
            });
        if !same {
            self.inner.store.replace_worktrees(project_id, listed)?;
        }
        // The timer that watches a project's worktrees reads the checkouts
        // but does not also leave the machine for GitHub;
        // `check_pull_requests` does, on its own slower cadence.
        self.refresh_checkouts(&project).await?;
        self.probe_merged(&project, false).await
    }

    /// [`Self::refresh_checkouts`] for a refresh the person asked for: quiet,
    /// because the worktrees were just synced and a checkout that cannot be
    /// read is not worth a line in the status.
    async fn refresh_checkouts_of(&self, project: &ProjectId) {
        let read = match self.inner.store.project(project) {
            Ok(project) => self.refresh_checkouts(&project).await,
            Err(error) => Err(error.into()),
        };
        if let Err(error) = read {
            tracing::debug!(%error, "could not read the state of the checkouts");
        }
    }

    /// Reads, for every stored worktree of `project`, how many files changed
    /// and how far it is from its upstream, and stores it as the git half of
    /// its [`WorktreeStatus`]. A worktree git cannot answer for (its folder is
    /// gone, git is missing) keeps what it had: the rest still get theirs.
    async fn refresh_checkouts(&self, project: &Project) -> Result<(), EngineError> {
        let worktrees = self.inner.store.worktrees(&project.id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let ssh = self.ssh();
        let git = Git::new(&runner, &machine, &ssh);
        for worktree in &worktrees {
            match git.checkout_state(&worktree.path).await {
                Ok(state) => {
                    self.inner
                        .store
                        .update_worktree_status(&worktree.id, |status| {
                            status.changed = Some(state.changed);
                            status.divergence = state.divergence;
                        })?;
                }
                Err(error) => {
                    tracing::debug!(%error, path = %worktree.path, "could not read the state of a checkout");
                }
            }
        }
        Ok(())
    }

    /// Asks GitHub about the pull requests of `project`, merged and open,
    /// unless it was asked less than [`GITHUB_ASK_EVERY`] ago.
    async fn probe_github(&self, project: &Project) -> Result<(), EngineError> {
        let due = {
            let mut asked = self
                .inner
                .github_asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            let due = github_due(asked.get(&project.id).copied(), now);
            if due {
                asked.insert(project.id.clone(), now);
            }
            due
        };
        if !due {
            return Ok(());
        }
        // One reading of the remote serves both questions.
        let on_github = if self
            .inner
            .store
            .worktrees(&project.id)?
            .iter()
            .any(|w| w.branch.is_some())
        {
            let machine = self.inner.store.machine(&project.machine_id)?;
            let runner = SharedRunner(self.inner.runner.clone());
            Some(
                self.origin_is_github(&runner, &machine, &self.ssh(), &project.root)
                    .await,
            )
        } else {
            None
        };
        self.probe_merged_on(project, true, on_github).await?;
        self.probe_open_pull_requests(project, on_github).await
    }

    /// Asks GitHub for the open pull requests of the repository and stores,
    /// as the GitHub half of each worktree's status, the one of its branch
    /// (or none). One question for the whole project. When GitHub cannot be
    /// asked (not a GitHub repository, no `gh`, not signed in, offline) every
    /// worktree keeps what it had, and nothing is said.
    async fn probe_open_pull_requests(
        &self,
        project: &Project,
        on_github: Option<bool>,
    ) -> Result<(), EngineError> {
        let worktrees = self.inner.store.worktrees(&project.id)?;
        if worktrees.iter().all(|worktree| worktree.branch.is_none()) {
            return Ok(());
        }
        let machine = self.inner.store.machine(&project.machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let ssh = self.ssh();
        let on_github = match on_github {
            Some(known) => known,
            None => {
                self.origin_is_github(&runner, &machine, &ssh, &project.root)
                    .await
            }
        };
        if !on_github {
            return Ok(());
        }
        let open = match Github::new(&runner, &machine, &ssh)
            .open_pull_requests(&project.root, OPEN_PULL_REQUESTS)
            .await
        {
            Ok(Some(open)) => open,
            Ok(None) => return Ok(()),
            Err(error) => {
                tracing::debug!(%error, "could not read the open pull requests");
                return Ok(());
            }
        };
        for worktree in &worktrees {
            let found = worktree
                .branch
                .as_ref()
                .and_then(|branch| open.get(branch))
                .cloned();
            self.inner
                .store
                .update_worktree_status(&worktree.id, |status| status.pull_request = found)?;
        }
        Ok(())
    }

    /// Whether the `origin` of the repository at `root` is on GitHub.
    async fn origin_is_github(
        &self,
        runner: &SharedRunner,
        machine: &Machine,
        ssh: &SshOptions,
        root: &str,
    ) -> bool {
        match Git::new(runner, machine, ssh)
            .remote_url(root, "origin")
            .await
        {
            Ok(Some(url)) => github::is_github_url(&url),
            Ok(None) => false,
            Err(error) => {
                tracing::debug!(%error, "could not read the remote of the project");
                false
            }
        }
    }

    /// Asks GitHub what is merged and writes it on the worktrees whose answer
    /// changed. One question for the whole repository, and `pull_requests`
    /// says whether this is one of the moments worth leaving the machine for:
    /// GitHub is asked when somebody is looking at the project and on its own
    /// slower cadence, not on the timer that lists worktrees.
    ///
    /// Git is deliberately not asked: a branch inside the base looks the same
    /// whether its work landed there or it never had any, so the honest
    /// answer is only the one GitHub recorded.
    ///
    /// Quiet on failure: a project whose repository cannot be asked keeps what
    /// it already knew, and a worktree nobody could ask about stays "not
    /// known".
    async fn probe_merged(
        &self,
        project: &Project,
        pull_requests: bool,
    ) -> Result<(), EngineError> {
        self.probe_merged_on(project, pull_requests, None).await
    }

    /// [`Engine::probe_merged`] for a caller that already knows whether the
    /// origin is on GitHub (`Some`), so the remote is read once for both
    /// questions.
    async fn probe_merged_on(
        &self,
        project: &Project,
        pull_requests: bool,
        on_github: Option<bool>,
    ) -> Result<(), EngineError> {
        let worktrees = self.inner.store.worktrees(&project.id)?;
        // Nothing but the main worktree: there is no pull request that could
        // be merged, so GitHub is not asked about this project.
        if !pull_requests || worktrees.iter().all(|worktree| worktree.is_main) {
            return Ok(());
        }
        let machine = self.inner.store.machine(&project.machine_id)?;
        let runner = SharedRunner(self.inner.runner.clone());
        let ssh = self.ssh();
        let merged: Option<HashSet<String>> = self
            .merged_pull_requests(&runner, &machine, &ssh, &project.root, on_github)
            .await;

        for worktree in worktrees.iter().filter(|worktree| !worktree.is_main) {
            let Some(branch) = worktree.branch.clone() else {
                continue;
            };
            let merged = merged
                .as_ref()
                .map_or(worktree.merged_pull_request, |merged| {
                    Some(merged.contains(&branch))
                });
            self.inner.store.set_merged(&worktree.id, merged)?;
        }
        Ok(())
    }

    /// The branches whose pull request GitHub says is merged, or `None` when
    /// the repository is not on GitHub, `gh` is not there, or the answer says
    /// nothing about the branches.
    async fn merged_pull_requests(
        &self,
        runner: &SharedRunner,
        machine: &Machine,
        ssh: &SshOptions,
        root: &str,
        on_github: Option<bool>,
    ) -> Option<HashSet<String>> {
        let on_github = match on_github {
            Some(known) => known,
            None => self.origin_is_github(runner, machine, ssh, root).await,
        };
        if !on_github {
            return None;
        }
        match Github::new(runner, machine, ssh)
            .merged_pull_request_branches(root, github::DEFAULT_LIMIT)
            .await
        {
            Ok(Some(branches)) => Some(branches.into_iter().collect()),
            Ok(None) => None,
            Err(error) => {
                tracing::debug!(%error, "could not read the merged pull requests");
                None
            }
        }
    }

    /// Asks GitHub, for the pull requests of `project`, as often as a sidebar
    /// with live terminals is worth it. The same quiet rules as
    /// [`Engine::check_worktrees`].
    pub fn check_pull_requests(&self, project: ProjectId) -> JoinHandle<()> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let Ok(project) = engine.inner.store.project(&project) else {
                return;
            };
            if let Err(error) = engine.probe_github(&project).await {
                tracing::debug!(%error, "could not check the pull requests");
            }
        })
    }

    /// Asks git for the branches of `project` (local and remote-tracking),
    /// for a worktree's base to choose from. Quiet on failure: the field
    /// stays free text.
    pub fn base_refs(&self, project: ProjectId) -> JoinHandle<Option<Vec<String>>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let project = engine.inner.store.project(&project).ok()?;
            let machine = engine.inner.store.machine(&project.machine_id).ok()?;
            let runner = SharedRunner(engine.inner.runner.clone());
            let ssh = engine.ssh();
            Git::new(&runner, &machine, &ssh)
                .branches(&project.root)
                .await
                .ok()
        })
    }

    async fn add_worktree(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<String, EngineError> {
        let path = self.create_worktree(project_id, branch, base).await?;
        Ok(format!("Added worktree {branch} at {path}."))
    }

    /// Makes the worktree `branch` and lists the project's worktrees again.
    /// Answers where it is.
    async fn create_worktree(
        &self,
        project_id: &ProjectId,
        branch: &str,
        base: Option<&str>,
    ) -> Result<String, EngineError> {
        address::validate_branch(branch).map_err(|why| EngineError::Invalid(why.to_owned()))?;
        let project = self.inner.store.project(project_id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let path =
            address::worktree_location(&self.prefs().worktree_location, &project.root, branch)
                .map_err(EngineError::Invalid)?;
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
        Ok(path)
    }

    /// Makes several worktrees of `project`, one after the other (git takes
    /// the repository's lock for each), and answers how each one went, in the
    /// order of `branches`: where it is, or why it was not made. One that
    /// fails does not stop the rest.
    pub fn add_worktrees(
        &self,
        project: ProjectId,
        branches: Vec<String>,
        base: Option<String>,
    ) -> JoinHandle<Vec<Result<String, EngineError>>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let mut done = Vec::with_capacity(branches.len());
            for branch in &branches {
                done.push(
                    engine
                        .create_worktree(&project, branch, base.as_deref())
                        .await,
                );
            }
            done
        })
    }

    /// Reads the project's `leon.toml` where the project is (this computer,
    /// over SSH or through the relay), keeps what it says for
    /// [`Engine::project_state`] and answers it. A project without the file
    /// is [`ProjectState::Absent`], quietly. A file that is wrong, or that
    /// cannot be read, is told on the status line once, when what is known of
    /// it changes; so are the shortcuts that collide with a command of Leon.
    pub fn read_project_file(
        &self,
        project: ProjectId,
    ) -> JoinHandle<Result<ProjectState, EngineError>> {
        let engine = self.clone();
        self.inner
            .handle
            .spawn(async move { engine.read_project_file_now(&project).await })
    }

    /// What the last reading of the project's `leon.toml` found; `None` before
    /// the first.
    pub fn project_state(&self, project: &ProjectId) -> Option<ProjectState> {
        self.state().project_files.get(project).cloned()
    }

    async fn read_project_file_now(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectState, EngineError> {
        let project = self.inner.store.project(project_id)?;
        let path = address::join_path(&project.root, project::FILE_NAME);
        let state = match self
            .read_optional_file_now(&project.machine_id, &path)
            .await
        {
            Ok(None) => ProjectState::Absent,
            Ok(Some(FileContent::Text { text, .. })) => match project::parse(&text) {
                Ok(file) => ProjectState::Loaded(Arc::new(file)),
                Err(error) => ProjectState::Invalid(error),
            },
            Ok(Some(_)) => ProjectState::Invalid(project::ProjectError {
                line: 0,
                message: format!(
                    "It is not a text file of at most {} bytes.",
                    remote_files::MAX_FILE_BYTES
                ),
            }),
            // The machine is away or the folder unreadable: what was known
            // stays, and the reason goes to the log.
            Err(error) => {
                tracing::debug!(%error, "could not read the project file");
                return self.project_state(project_id).ok_or(error);
            }
        };
        let before = self
            .state()
            .project_files
            .insert(project_id.clone(), state.clone());
        if before.as_ref() != Some(&state) {
            match &state {
                ProjectState::Invalid(error) => {
                    self.set_status(StatusKind::Error, format!("{}: {error}", project.name));
                }
                ProjectState::Loaded(file) => {
                    if let Some(notice) = file.notices(crate::platform::is_mac()).into_iter().next()
                    {
                        self.set_status(StatusKind::Info, notice);
                    }
                }
                ProjectState::Absent => {}
            }
        }
        Ok(state)
    }

    /// [`Engine::read_file_now`] for a file that may not be there: `None`
    /// then, where that one is an error.
    async fn read_optional_file_now(
        &self,
        machine_id: &MachineId,
        path: &str,
    ) -> Result<Option<FileContent>, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let path = std::path::PathBuf::from(path);
            return match self.blocking(move || files::read(&path)).await? {
                Ok(content) => Ok(Some(content)),
                Err(FileError::Missing(_)) => Ok(None),
                Err(FileError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    Ok(None)
                }
                Err(error) => Err(error.into()),
            };
        }
        let output = self
            .run_file_command(&machine, remote_files::read_command(path))
            .await?;
        match remote_files::parse_read(&output.stdout) {
            Some(remote_files::ReadOutcome::Found(content)) => Ok(Some(content)),
            Some(remote_files::ReadOutcome::Missing) => Ok(None),
            Some(remote_files::ReadOutcome::NotAFile) => {
                Err(file_error(format!("{path} is not a file.")))
            }
            None => Err(file_error(unanswered("read", path, &output))),
        }
    }

    /// Removes a worktree on the machine that holds it, in the background.
    /// With `force`, the files it holds are deleted with it; without it, a
    /// worktree that git refuses for them answers [`Removal::NeedsForce`]
    /// and is left whole, so the person can be asked first.
    pub fn remove_worktree(
        &self,
        project: ProjectId,
        worktree: WorktreeId,
        force: bool,
    ) -> JoinHandle<Result<Removal, EngineError>> {
        let engine = self.clone();
        self.inner
            .handle
            .spawn(async move { engine.remove_worktree_now(&project, &worktree, force).await })
    }

    async fn remove_worktree_now(
        &self,
        project_id: &ProjectId,
        worktree_id: &WorktreeId,
        force: bool,
    ) -> Result<Removal, EngineError> {
        let project = self.inner.store.project(project_id)?;
        let machine = self.inner.store.machine(&project.machine_id)?;
        let Some(worktree) = self
            .inner
            .store
            .worktrees(project_id)?
            .into_iter()
            .find(|worktree| &worktree.id == worktree_id)
        else {
            // Another window (or a sync) already took it: the row the person
            // clicked is old. Listing again drops it, and saying it is gone
            // beats an error they cannot act on.
            self.sync_worktrees(project_id).await?;
            return Ok(Removal::Removed(
                "The worktree was already gone.".to_owned(),
            ));
        };
        if worktree.is_main {
            return Err(EngineError::Invalid(
                "The main worktree cannot be removed.".to_owned(),
            ));
        }
        let runner = SharedRunner(self.inner.runner.clone());
        let ssh = self.ssh();
        let git = Git::new(&runner, &machine, &ssh);
        if let Err(error) = git.remove_worktree(&project, &worktree.path, force).await {
            // git refuses a worktree with modified or untracked files
            // without `--force`: that is the one to ask about. Anything
            // else (a locked worktree, a path gone) is the error itself.
            if !force
                && git
                    .worktree_has_changes(&worktree.path)
                    .await
                    .unwrap_or(false)
            {
                return Ok(Removal::NeedsForce);
            }
            // It may not be git's worktree any more (removed from another
            // window or from a terminal behind Leon's back): then there is
            // nothing to remove, and listing again takes the row away
            // instead of keeping an error the person cannot act on.
            if self.sync_worktrees(project_id).await.is_ok()
                && self
                    .inner
                    .store
                    .worktrees(project_id)?
                    .iter()
                    .all(|stored| &stored.id != worktree_id)
            {
                return Ok(Removal::Removed(
                    "The worktree was already gone.".to_owned(),
                ));
            }
            return Err(error.into());
        }
        self.sync_worktrees(project_id).await?;
        Ok(Removal::Removed(format!(
            "Removed worktree {}.",
            worktree.path
        )))
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
        let note = self.memory_note(&project).await;
        match synced {
            Ok(count) => Ok(with_note(
                format!("Added project {name} with {}.", plural(count, "worktree")),
                note,
            )),
            // The project is saved; "refresh" tries git again.
            Err(error) => Err(EngineError::Invalid(format!(
                "Added project {name}, but git failed: {error}"
            ))),
        }
    }

    /// The machine, name and absolute target a clone or a creation needs,
    /// checked before anything runs.
    fn new_project_plan(
        &self,
        machine: &MachineId,
        parent: &str,
        name: &str,
    ) -> Result<(Machine, String, String, String), EngineError> {
        let machine = self.inner.store.machine(machine)?;
        let name = name.trim().to_owned();
        address::validate_project_name(&name)
            .map_err(|why| EngineError::Invalid(why.to_owned()))?;
        let parent = parent.trim();
        if !address::is_absolute_path(parent) {
            return Err(EngineError::Invalid(
                "The parent folder must be an absolute path.".to_owned(),
            ));
        }
        let target = address::join_path(parent, &name);
        Ok((machine, name, parent.to_owned(), target))
    }

    /// Runs one command on `machine` and answers its output, whether or not
    /// it succeeded; the caller decides what a failure means.
    async fn run_command(
        &self,
        machine: &Machine,
        spec: CommandSpec,
    ) -> Result<Output, EngineError> {
        let runner = SharedRunner(self.inner.runner.clone());
        let placed = leon_remote::run_on(machine, &spec, &self.ssh());
        runner
            .run(&placed)
            .await
            .map_err(|error| EngineError::Job(error.to_string()))
    }

    async fn clone_project(
        &self,
        machine: &MachineId,
        url: &str,
        parent: &str,
        name: &str,
    ) -> Result<String, EngineError> {
        // An empty name is what the URL offers, as git itself would name the
        // folder.
        let name = match name.trim() {
            "" => address::default_project_name_from_url(url),
            name => name.to_owned(),
        };
        let (machine, name, parent, target) = self.new_project_plan(machine, parent, &name)?;
        self.set_status(StatusKind::Busy, format!("Cloning {name}..."));
        let runner = SharedRunner(self.inner.runner.clone());
        Git::new(&runner, &machine, &self.ssh())
            .clone(url.trim(), &parent, &target)
            .await?;
        let (count, note) = self.register_project(&machine, &name, &target).await?;
        Ok(with_note(
            format!(
                "Cloned {name} into {target} with {}.",
                plural(count, "worktree")
            ),
            note,
        ))
    }

    async fn create_project(
        &self,
        machine: &MachineId,
        parent: &str,
        name: &str,
    ) -> Result<String, EngineError> {
        let (machine, name, parent, target) = self.new_project_plan(machine, parent, name)?;
        self.set_status(StatusKind::Busy, format!("Creating {name}..."));
        let made_parent = self
            .run_command(&machine, CommandSpec::new("mkdir").args(["-p", &parent]))
            .await?;
        if !made_parent.success() {
            return Err(EngineError::Invalid(format!(
                "Could not make {parent}: {}",
                made_parent.stderr.trim()
            )));
        }
        // An existing folder is only taken when it is empty: creating a
        // project must never write into files that are already there.
        let listing = self
            .run_command(&machine, CommandSpec::new("ls").args(["-A", &target]))
            .await?;
        if listing.success() && !listing.stdout.trim().is_empty() {
            return Err(EngineError::Invalid(format!(
                "{target} already exists and is not empty."
            )));
        }
        let made_target = self
            .run_command(&machine, CommandSpec::new("mkdir").args(["-p", &target]))
            .await?;
        if !made_target.success() {
            return Err(EngineError::Invalid(format!(
                "Could not make {target}: {}",
                made_target.stderr.trim()
            )));
        }
        let runner = SharedRunner(self.inner.runner.clone());
        Git::new(&runner, &machine, &self.ssh())
            .init(&target)
            .await?;
        let (count, note) = self.register_project(&machine, &name, &target).await?;
        Ok(with_note(
            format!(
                "Created project {name} at {target} with {}.",
                plural(count, "worktree")
            ),
            note,
        ))
    }

    /// Saves a freshly cloned or created repository as a project and fills in
    /// its worktrees, so the sidebar shows it whole at once.
    async fn register_project(
        &self,
        machine: &Machine,
        name: &str,
        path: &str,
    ) -> Result<(usize, Option<String>), EngineError> {
        let project = self.inner.store.add_project(&machine.id, name, path)?;
        let count = self.sync_worktrees(&project.id).await?;
        self.detect_icon_quietly(&project.id).await;
        // Somebody cloned or created it: the one moment to ask about its
        // agent memory.
        let note = self.memory_note(&project).await;
        Ok((count, note))
    }

    fn remove_project(&self, id: &ProjectId) -> Result<String, EngineError> {
        let project = self.inner.store.project(id)?;
        self.inner.store.remove_project(id)?;
        Ok(format!("Removed project {}.", project.name))
    }

    /// Writes the managed block of the shared memory into a project's
    /// instruction files, or takes it out. Only on this computer: the files of
    /// a project on another machine are not Leon's to reach.
    async fn project_memory(&self, id: &ProjectId, on: bool) -> Result<String, EngineError> {
        let project = self.inner.store.project(id)?;
        if !project.machine_id.is_local() {
            return Err(EngineError::Invalid(format!(
                "The agent memory only works on this computer: {} is on another machine.",
                project.name
            )));
        }
        let root = std::path::PathBuf::from(&project.root);
        let lines = self
            .blocking(move || crate::memory::set_block(&root, on, false))
            .await?
            .map_err(EngineError::File)?;
        if on {
            // The file the block points agents at is there from now on.
            self.write_memory_file(Some(project.root.clone())).await?;
        }
        let said = memory_status(&project.name, on, &lines).map_err(EngineError::File)?;
        // Only what was done is remembered: a failure above leaves no "yes".
        let choice = if on {
            leon_core::MemoryChoice::On
        } else {
            leon_core::MemoryChoice::Off
        };
        self.record_memory_choice(&project.root, choice);
        Ok(said)
    }

    /// Writes the memory file of the project rooted at `root` again, or,
    /// without a root, every memory file there is.
    async fn write_memory_file(&self, root: Option<String>) -> Result<(), EngineError> {
        let data_dir = self
            .inner
            .memory_dir
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(data_dir) = data_dir else {
            return Ok(());
        };
        let store = self.inner.store.clone();
        let budget = self.memory_budget();
        self.blocking(move || match root {
            Some(root) => crate::memory::write_file(&store, &data_dir, &root, budget).map(|_| ()),
            None => crate::memory::write_global_files(&store, &data_dir, budget),
        })
        .await?
        .map_err(EngineError::File)
    }

    fn rename_project(&self, id: &ProjectId, name: &str) -> Result<String, EngineError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::Invalid("A project needs a name.".to_owned()));
        }
        let old = self.inner.store.project(id)?.name;
        self.inner.store.rename_project(id, name)?;
        Ok(format!("Renamed {old} to {name}."))
    }

    fn reorder_projects(
        &self,
        machine: &MachineId,
        ordered: &[ProjectId],
    ) -> Result<String, EngineError> {
        self.inner.store.reorder_projects(machine, ordered)?;
        Ok("Moved the project.".to_owned())
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

    /// Takes sessions out of the history; ones it no longer has are skipped.
    /// Public and not async so the window can finish this on the way out of
    /// the application, where a submitted operation might never run.
    pub fn forget_sessions(&self, sessions: &[leon_core::SessionId]) -> Result<(), EngineError> {
        for id in sessions {
            match self.inner.store.remove_session(id) {
                Ok(()) | Err(StoreError::NotFound(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
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
        let mut note = None;
        if is_new {
            self.detect_icon_quietly(&project.id).await;
            // Somebody opened a folder Leon did not have: the one moment to
            // ask about its agent memory.
            note = self.memory_note(&project).await;
        }
        Ok((
            project.id.clone(),
            with_note(
                format!(
                    "Opened project {} with {}.",
                    project.name,
                    plural(count, "worktree")
                ),
                note,
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
                    // A project that has just been found has a sidebar too,
                    // and its sidebar asks what is merged as well: waiting for
                    // somebody to open it would leave the row unmarked for as
                    // long as nobody did.
                    match store.project(&id) {
                        Ok(project) => {
                            if let Err(error) = self.probe_merged(&project, true).await {
                                tracing::debug!(%error, "could not check the merged pull requests");
                            }
                        }
                        Err(error) => tracing::warn!(%root, %error, "could not read the project"),
                    }
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
        // A computer paired with a code shares what its own Leon knows: its
        // projects and history sessions, pulled here before anything else
        // finds out what there is to work on. A host that does not answer is
        // no failure of its own: its section of the sidebar stays as it was,
        // and the projects it brought count among those found.
        if matches!(machine.kind, MachineKind::Relay { .. }) {
            let shared = match self.sync_host(&machine.id).await {
                Ok(shared) => shared,
                Err(error) => {
                    tracing::debug!(
                        machine = %machine.name,
                        %error,
                        "could not sync what the paired host shares"
                    );
                    HostSync::default()
                }
            };
            done.found += shared.projects;
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
                self.refresh_checkouts_of(&project.id).await;
                continue;
            }
            match self.sync_worktrees(&project.id).await {
                Ok(_) => {
                    done.synced += 1;
                    self.refresh_checkouts_of(&project.id).await;
                }
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

    // ----- files -----------------------------------------------------------
    //
    // The same five operations on any machine: the local one answers with
    // `std::fs` (and git through the runner, so no shell is needed), any other
    // runs a script of `leon_remote::files` where the files are. Reading and
    // saving say what went wrong on the status line, because somebody is
    // waiting for them; the rest runs quietly like the checks on a timer.

    /// Reads a file. Text comes with the revision a save must find again;
    /// a binary file and one above [`files::MAX_FILE_BYTES`] are answers too,
    /// not failures.
    #[allow(dead_code)] // The editor is the first caller.
    pub fn read_file(
        &self,
        machine: MachineId,
        path: String,
    ) -> JoinHandle<Result<FileContent, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let read = engine.read_file_now(&machine, &path).await;
            engine.reported(read)
        })
    }

    /// [`Engine::read_file`] for a look nobody waits for: a failure (the file
    /// was deleted, the machine is away) goes to the log, not the status line.
    pub fn check_file(
        &self,
        machine: MachineId,
        path: String,
    ) -> JoinHandle<Result<FileContent, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let read = engine.read_file_now(&machine, &path).await;
            engine.quiet("look at a file", read)
        })
    }

    async fn read_file_now(
        &self,
        machine_id: &MachineId,
        path: &str,
    ) -> Result<FileContent, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let path = std::path::PathBuf::from(path);
            return Ok(self.blocking(move || files::read(&path)).await??);
        }
        let output = self
            .run_file_command(&machine, remote_files::read_command(path))
            .await?;
        match remote_files::parse_read(&output.stdout) {
            Some(remote_files::ReadOutcome::Found(content)) => Ok(content),
            Some(remote_files::ReadOutcome::Missing) => Err(file_error(format!(
                "{path} does not exist on {}.",
                machine.name
            ))),
            Some(remote_files::ReadOutcome::NotAFile) => {
                Err(file_error(format!("{path} is not a file.")))
            }
            None => Err(file_error(unanswered("read", path, &output))),
        }
    }

    /// Saves `contents` to `path`. `expected` is the revision the file was
    /// read with, and a file that has another is left alone and answered
    /// [`WriteOutcome::Conflict`]; `None` creates a file that must not exist.
    /// A symbolic link is followed to its target. The handle yields the
    /// revision the file has after the save.
    #[allow(dead_code)] // The editor is the first caller.
    pub fn write_file(
        &self,
        machine: MachineId,
        path: String,
        contents: String,
        expected: Option<FileRevision>,
    ) -> JoinHandle<Result<WriteOutcome, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let saved = engine
                .write_file_now(&machine, &path, contents, expected.as_ref())
                .await;
            match &saved {
                Ok(WriteOutcome::Saved(_)) => {
                    engine.set_status(StatusKind::Info, format!("Saved {}.", file_name(&path)));
                }
                Ok(WriteOutcome::Conflict) => engine.set_status(
                    StatusKind::Error,
                    format!(
                        "{} changed since you read it; nothing was saved.",
                        file_name(&path)
                    ),
                ),
                Err(_) => {}
            }
            engine.reported(saved)
        })
    }

    async fn write_file_now(
        &self,
        machine_id: &MachineId,
        path: &str,
        contents: String,
        expected: Option<&FileRevision>,
    ) -> Result<WriteOutcome, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let path = std::path::PathBuf::from(path);
            let expected = expected.cloned();
            return Ok(self
                .blocking(move || files::write(&path, expected.as_ref(), contents.as_bytes()))
                .await??);
        }
        if contents.len() > remote_files::MAX_FILE_BYTES {
            return Err(file_error(FileError::TooBig(path.to_owned()).to_string()));
        }
        let command = remote_files::write_command(path, expected, contents.as_bytes());
        let output = self.run_file_command(&machine, command).await?;
        remote_files::parse_write(&output.stdout)
            .ok_or_else(|| file_error(unanswered("save", path, &output)))
    }

    /// Lists the entries of one folder, folders first: what a tree asks for
    /// as a folder is opened.
    pub fn list_dir(
        &self,
        machine: MachineId,
        path: String,
    ) -> JoinHandle<Result<Vec<FileEntry>, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let listed = engine.list_dir_now(&machine, &path).await;
            engine.quiet("list a folder", listed)
        })
    }

    async fn list_dir_now(
        &self,
        machine_id: &MachineId,
        path: &str,
    ) -> Result<Vec<FileEntry>, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let path = std::path::PathBuf::from(path);
            return Ok(self.blocking(move || files::list_dir(&path)).await??);
        }
        let output = self
            .run_file_command(&machine, remote_files::list_command(path))
            .await?;
        match remote_files::parse_dir(&output.stdout) {
            Some(remote_files::DirListing::Entries(entries)) => Ok(entries),
            Some(remote_files::DirListing::Missing) => Err(file_error(format!(
                "{path} does not exist on {}.",
                machine.name
            ))),
            Some(remote_files::DirListing::NotADir) => {
                Err(file_error(format!("{path} is not a folder.")))
            }
            None => Err(file_error(unanswered("list", path, &output))),
        }
    }

    /// Lists every file under a project's `root` as relative paths with `/`
    /// separators: what git tracks and has not ignored, or a bounded walk of
    /// the folder when it is not a repository.
    #[allow(dead_code)] // The editor is the first caller.
    pub fn project_files(
        &self,
        machine: MachineId,
        root: String,
    ) -> JoinHandle<Result<Vec<String>, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let listed = engine.project_files_now(&machine, &root).await;
            engine.quiet("list the files of a project", listed)
        })
    }

    async fn project_files_now(
        &self,
        machine_id: &MachineId,
        root: &str,
    ) -> Result<Vec<String>, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let runner = SharedRunner(self.inner.runner.clone());
            if let Ok(output) = runner.run(&files::project_files_command(root)).await {
                if output.success() {
                    return Ok(remote_files::parse_paths(&output.stdout, '\0'));
                }
            }
            // Not a repository, or no git: walk the folder.
            let root = std::path::PathBuf::from(root);
            return Ok(self.blocking(move || files::walk(&root)).await??);
        }
        let output = self
            .run_file_command(&machine, remote_files::project_files_command(root))
            .await?;
        remote_files::parse_project_files(&output.stdout)
            .ok_or_else(|| file_error(unanswered("list the files of", root, &output)))
    }

    /// Looks for `query` in the files under a project's `root`, and says what
    /// it found: at most [`remote_search::MAX_HITS`] lines, with a flag for
    /// more. On this computer it reads the project's file list (so what git
    /// ignores is not searched) in-process; `cancel` ends it early. On any
    /// other machine it is one command there (ripgrep, `git grep` or `grep`)
    /// that is given up after 30 s. Dropping or aborting the handle stops it.
    /// A failure, a regular expression that is not one included, is the
    /// error: nobody is told on the status line, the search view says it.
    pub fn search_project(
        &self,
        machine: MachineId,
        root: String,
        query: remote_search::ProjectQuery,
        cancel: Arc<AtomicBool>,
    ) -> JoinHandle<Result<remote_search::SearchOutcome, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let found = engine
                .search_project_now(&machine, &root, query, cancel)
                .await;
            engine.quiet("search the files of a project", found)
        })
    }

    async fn search_project_now(
        &self,
        machine_id: &MachineId,
        root: &str,
        query: remote_search::ProjectQuery,
        cancel: Arc<AtomicBool>,
    ) -> Result<remote_search::SearchOutcome, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let matcher = crate::search::Matcher::new(&query).map_err(file_error)?;
            let files = self.project_files_now(machine_id, root).await?;
            let root = std::path::PathBuf::from(root);
            return self
                .blocking(move || crate::search::search_files(&root, &files, &matcher, &cancel))
                .await;
        }
        let command = remote_search::search_command(root, &query);
        let output = tokio::time::timeout(SEARCH_TIMEOUT, self.run_file_command(&machine, command))
            .await
            .map_err(|_| {
                file_error(format!(
                    "The search on {} took longer than {} s and was stopped.",
                    machine.name,
                    SEARCH_TIMEOUT.as_secs()
                ))
            })??;
        match remote_search::parse_search(&output.stdout) {
            Some(remote_search::SearchReply::Found(found)) => {
                // A tool that found nothing and complained (the script always
                // exits 0, its pipe hides the tool's status): a bad pattern.
                match last_line(&output.stderr) {
                    Some(why) if found.hits.is_empty() => Err(file_error(why.to_owned())),
                    _ => Ok(found),
                }
            }
            Some(remote_search::SearchReply::Missing) => Err(file_error(format!(
                "{root} does not exist on {}.",
                machine.name
            ))),
            None => Err(file_error(unanswered("search", root, &output))),
        }
    }

    /// Asks git what changed under `root`, a work tree's root or a folder
    /// inside one: a mark for each path (relative to `root`), rolled up to its
    /// folders. A folder that is not in a
    /// repository has no marks.
    pub fn git_marks(
        &self,
        machine: MachineId,
        root: String,
    ) -> JoinHandle<Result<GitMarks, EngineError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let marks = engine.git_marks_now(&machine, &root).await;
            engine.quiet("read the git marks", marks)
        })
    }

    async fn git_marks_now(
        &self,
        machine_id: &MachineId,
        root: &str,
    ) -> Result<GitMarks, EngineError> {
        let machine = self.inner.store.machine(machine_id)?;
        if machine.kind == MachineKind::Local {
            let runner = SharedRunner(self.inner.runner.clone());
            let status = match runner.run(&files::git_marks_command(root)).await {
                Ok(output) if output.success() => output.stdout,
                // Not a repository, or no git: nothing to mark.
                _ => return Ok(GitMarks::default()),
            };
            // Git names paths from the work tree's root; `root` may be a
            // folder inside it.
            let prefix = match runner.run(&files::git_prefix_command(root)).await {
                Ok(output) if output.success() => output.stdout,
                _ => String::new(),
            };
            return Ok(GitMarks::from_status_in(&status, &prefix));
        }
        let output = self
            .run_file_command(&machine, remote_files::git_marks_command(root))
            .await?;
        remote_files::parse_marks(&output.stdout)
            .ok_or_else(|| file_error(unanswered("read the git marks of", root, &output)))
    }

    /// Runs one file command where the machine is, through the shared runner.
    async fn run_file_command(
        &self,
        machine: &Machine,
        command: CommandSpec,
    ) -> Result<Output, EngineError> {
        let runner = SharedRunner(self.inner.runner.clone());
        let placed = run_on(machine, &command, &self.ssh());
        runner
            .run(&placed)
            .await
            .map_err(|error| file_error(error.to_string()))
    }

    /// Says a failure on the status line, and passes the result on.
    fn reported<T>(&self, result: Result<T, EngineError>) -> Result<T, EngineError> {
        if let Err(error) = &result {
            self.set_status(StatusKind::Error, error.to_string());
        }
        result
    }

    /// Logs a failure and passes the result on, for what nobody waits for.
    fn quiet<T>(&self, what: &str, result: Result<T, EngineError>) -> Result<T, EngineError> {
        if let Err(error) = &result {
            tracing::debug!(%error, "could not {what}");
        }
        result
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

fn file_error(text: String) -> EngineError {
    EngineError::File(text)
}

impl From<FileError> for EngineError {
    fn from(error: FileError) -> Self {
        EngineError::File(error.to_string())
    }
}

/// The last name of a path, for the status line.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The last line of a command's standard error that says something.
fn last_line(text: &str) -> Option<&str> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

/// What a file command that gave no answer says: the last line it wrote to
/// standard error, or its exit status.
fn unanswered(action: &str, path: &str, output: &Output) -> String {
    let why = output
        .stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty());
    match why {
        Some(why) => format!("Could not {action} {path}: {why}"),
        None => format!(
            "Could not {action} {path}: the machine answered with status {:?}.",
            output.status
        ),
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

/// What one sync of a paired host's own Leon brought into the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct HostSync {
    /// Projects mirrored that were not there yet.
    projects: usize,
    /// History sessions stored with their transcripts.
    sessions: u32,
}

impl HostSync {
    fn is_empty(&self) -> bool {
        self.projects == 0 && self.sessions == 0
    }

    /// The words for the status line: every number mentioned, in a list.
    fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.projects > 0 {
            parts.push(format!("{} new", plural(self.projects, "project")));
        }
        if self.sessions > 0 {
            parts.push(format!(
                "{} of history",
                plural(self.sessions as usize, "session")
            ));
        }
        parts.join(", ")
    }
}

/// One transcript entry, in the store's shape. An entry whose role tag or
/// timestamp cannot be understood is left out of the transcript.
fn transcript_message(role: &str, text: String, at_ms: i64) -> Option<NewMessage> {
    Some(NewMessage {
        role: leon_core::Role::parse(role)?,
        text,
        at: DateTime::from_timestamp_millis(at_ms).unwrap_or(DateTime::<chrono::Utc>::UNIX_EPOCH),
    })
}

/// The import cursor's source key of one shared session: the same table the
/// local importer keeps its fingerprints in, this time with the host's own
/// count for the session.
fn share_source_key(agent: &str, external_id: &str) -> String {
    format!("share:{agent}:{external_id}")
}

/// What a host shared of one session last: its own latest message time and
/// how many messages it says the session holds. Both together change as soon
/// as the session does, and only then.
fn share_fingerprint(shared: &leon_wire::SharedSession) -> String {
    format!("{}:{}", shared.updated_ms, shared.messages)
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

/// A path without trailing separators, for comparing what git reports with
/// what the store kept (the store drops them). A bare root keeps its own.
fn trim(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

/// A status line with what there is to add about the agent memory.
fn with_note(text: String, note: Option<String>) -> String {
    match note {
        Some(note) => format!("{text} {note}"),
        None => text,
    }
}

/// The status line of turning a project's agent memory on or off: what was
/// done, with the names of the files it was done to. An error when it was to
/// be turned on and no file could take the block.
fn memory_status(project: &str, on: bool, lines: &[String]) -> Result<String, String> {
    let changed: Vec<&str> = lines
        .iter()
        .filter(|line| !line.starts_with("left "))
        .filter_map(|line| line.rsplit(['/', '\\']).next())
        .collect();
    if !changed.is_empty() {
        let files = changed.join(", ");
        return Ok(if on {
            format!("Agent memory is on for {project}: the block is in {files}.")
        } else {
            format!("Agent memory is off for {project}: the block is out of {files}.")
        });
    }
    if !on {
        return Ok(format!("Agent memory was already off for {project}."));
    }
    if lines
        .iter()
        .any(|line| line.ends_with("already has the block"))
    {
        return Ok(format!("Agent memory was already on for {project}."));
    }
    Err(format!(
        "Agent memory was not turned on for {project}: {}.",
        lines.join("; ")
    ))
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
    async fn github_is_asked_when_somebody_looks_and_not_when_the_timer_watches() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok("## main\n"))
            .reply(Output::ok("## feature/login\n"))
            .reply(Output::ok("git@github.com:zavudev/leon.git\n"))
            .reply(Output::ok("[]"))
            .reply(Output::ok("[]")));
        let project = local_project(&rig.store);
        rig.engine
            .check_worktrees(project.id.clone())
            .await
            .unwrap();
        assert!(
            rig.runner.calls().iter().all(|call| call.program != "gh"),
            "the timer that watches worktrees does not leave the machine: {:?}",
            rig.runner.calls()
        );
        rig.engine
            .check_pull_requests(project.id.clone())
            .await
            .unwrap();
        assert!(
            rig.runner.calls().iter().any(|call| call.program == "gh"),
            "the slower cadence does ask, and keeps the worktrees it found"
        );
    }

    /// What the store says about the checkout of the worktree of `branch`.
    fn status_of(store: &Store, project: &ProjectId, branch: &str) -> leon_core::WorktreeStatus {
        store
            .worktree_statuses()
            .unwrap()
            .remove(&worktree_of(store, project, branch))
            .unwrap_or_default()
    }

    const ORIGIN: &str = "git@github.com:zavudev/leon.git\n";

    #[test]
    fn github_is_asked_again_only_after_a_minute() {
        let start = Instant::now();
        assert!(github_due(None, start), "never asked is due");
        assert!(!github_due(Some(start), start));
        assert!(!github_due(Some(start), start + Duration::from_secs(59)));
        assert!(github_due(Some(start), start + Duration::from_secs(60)));
        // A clock that went back is not a reason to ask.
        assert!(!github_due(Some(start + Duration::from_secs(5)), start));
    }

    #[tokio::test]
    async fn the_timer_reads_the_state_of_every_checkout_and_still_leaves_github_alone() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(
                "## main...origin/main [ahead 1]\n M src/lib.rs\n?? notes.md\n",
            ))
            .reply(Output::ok("## feature/login\n")));
        let project = local_project(&rig.store);
        rig.engine
            .check_worktrees(project.id.clone())
            .await
            .unwrap();
        let main = status_of(&rig.store, &project.id, "main");
        assert_eq!(main.changed, Some(2));
        assert_eq!(
            main.divergence,
            Some(leon_core::Divergence {
                ahead: 1,
                behind: 0
            })
        );
        let linked = status_of(&rig.store, &project.id, "feature/login");
        assert_eq!(linked.changed, Some(0), "clean is known, not unknown");
        assert_eq!(linked.divergence, None, "no upstream is not level");
        let calls = rig.runner.calls();
        assert_eq!(calls[1].cwd.as_deref(), Some(PROJECT_ROOT));
        assert_eq!(
            calls[2].cwd.as_deref(),
            Some("/srv/api-worktrees/feature-login")
        );
        assert!(calls.iter().all(|call| call.program == "git"));
    }

    #[tokio::test]
    async fn a_checkout_git_cannot_read_keeps_what_it_had_and_the_others_still_update() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::failed(128, "fatal: not a git repository"))
            .reply(Output::ok("## feature/login\n M a\n")));
        let project = project_with_worktrees(&rig.store);
        let main = worktree_of(&rig.store, &project.id, "main");
        rig.store
            .update_worktree_status(&main, |status| status.changed = Some(9))
            .unwrap();
        rig.engine.refresh_checkouts(&project).await.unwrap();
        assert_eq!(status_of(&rig.store, &project.id, "main").changed, Some(9));
        assert_eq!(
            status_of(&rig.store, &project.id, "feature/login").changed,
            Some(1)
        );
    }

    #[tokio::test]
    async fn the_open_pull_request_of_a_branch_is_stored_on_its_worktree() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(ORIGIN))
            .reply(Output::ok(
                r#"[{"number":31,"url":"https://github.com/zavudev/leon/pull/31",
                "headRefName":"feature/login","isDraft":false,
                "reviewDecision":"APPROVED","statusCheckRollup":[
                  {"status":"COMPLETED","conclusion":"SUCCESS"}]}]"#,
            )));
        let project = project_with_worktrees(&rig.store);
        rig.store
            .update_worktree_status(&worktree_of(&rig.store, &project.id, "main"), |status| {
                status.pull_request = Some(leon_core::PullRequest {
                    number: 1,
                    url: String::new(),
                    draft: false,
                    review: leon_core::Review::None,
                    checks: leon_core::Checks::None,
                })
            })
            .unwrap();
        rig.engine
            .probe_open_pull_requests(&project, None)
            .await
            .unwrap();
        let linked = status_of(&rig.store, &project.id, "feature/login");
        let pull_request = linked
            .pull_request
            .expect("the branch has an open pull request");
        assert_eq!(pull_request.number, 31);
        assert_eq!(pull_request.review, leon_core::Review::Approved);
        assert_eq!(pull_request.checks, leon_core::Checks::Passing);
        assert_eq!(
            status_of(&rig.store, &project.id, "main").pull_request,
            None,
            "GitHub answered and the branch has none any more"
        );
        let gh = &rig.runner.calls()[1];
        assert_eq!(gh.program, "gh");
        assert_eq!(gh.cwd.as_deref(), Some(PROJECT_ROOT));
    }

    #[tokio::test]
    async fn when_github_cannot_be_asked_every_worktree_keeps_what_it_had() {
        let known = leon_core::PullRequest {
            number: 7,
            url: "https://github.com/zavudev/leon/pull/7".into(),
            draft: true,
            review: leon_core::Review::None,
            checks: leon_core::Checks::None,
        };
        // No gh, gh signed out, and a repository that is not on GitHub.
        let silences = [
            ScriptedRunner::new()
                .reply(Output::ok(ORIGIN))
                .fail(RunError::Spawn {
                    program: "gh".into(),
                    source: std::io::Error::other("no gh"),
                }),
            ScriptedRunner::new()
                .reply(Output::ok(ORIGIN))
                .reply(Output::failed(4, "run gh auth login")),
            ScriptedRunner::new().reply(Output::ok("git@gitlab.com:zavudev/leon.git\n")),
        ];
        for runner in silences {
            let rig = rig(runner);
            let project = project_with_worktrees(&rig.store);
            let linked = worktree_of(&rig.store, &project.id, "feature/login");
            rig.store
                .update_worktree_status(&linked, |status| status.pull_request = Some(known.clone()))
                .unwrap();
            rig.engine
                .probe_open_pull_requests(&project, None)
                .await
                .unwrap();
            assert_eq!(
                status_of(&rig.store, &project.id, "feature/login").pull_request,
                Some(known.clone())
            );
        }
    }

    #[tokio::test]
    async fn github_is_asked_once_a_minute_per_project_whoever_asks() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(ORIGIN))
            .reply(Output::ok("[]"))
            .reply(Output::ok(ORIGIN))
            .reply(Output::ok("[]")));
        let project = project_with_worktrees(&rig.store);
        rig.engine
            .check_pull_requests(project.id.clone())
            .await
            .unwrap();
        let asked = rig.runner.calls().len();
        assert_eq!(
            rig.runner
                .calls()
                .iter()
                .filter(|call| call.program == "gh")
                .count(),
            2,
            "the merged ones and the open ones"
        );
        rig.engine
            .check_pull_requests(project.id.clone())
            .await
            .unwrap();
        rig.engine.probe_github(&project).await.unwrap();
        assert_eq!(rig.runner.calls().len(), asked, "a minute has not passed");
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

    /// A project with its main worktree and one more, as git reports them.
    fn project_with_worktrees(store: &Store) -> Project {
        let project = local_project(store);
        store
            .replace_worktrees(
                &project.id,
                new_worktrees(&leon_remote::parse_worktree_list(LISTING)),
            )
            .unwrap();
        project
    }

    /// The worktree of `branch`.
    fn worktree_of(store: &Store, project: &ProjectId, branch: &str) -> leon_core::WorktreeId {
        store
            .worktrees(project)
            .unwrap()
            .into_iter()
            .find(|worktree| worktree.branch.as_deref() == Some(branch))
            .expect("a worktree with that branch")
            .id
    }

    /// What each worktree says about being merged, by branch name.
    fn merged_by_branch(store: &Store, project: &ProjectId) -> Vec<(String, Option<bool>)> {
        store
            .worktrees(project)
            .unwrap()
            .into_iter()
            .map(|worktree| {
                (
                    worktree.branch.clone().unwrap_or_default(),
                    worktree.merged_pull_request,
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn a_branch_inside_the_base_is_not_called_merged_without_github_saying_so() {
        // The branch is inside the base, which is what git calls merged, and
        // it never had a commit of its own: there is no history to have been
        // merged. Asking git was the false positive; GitHub is the only
        // witness, so git is only asked where the remote comes from.
        let rig = rig(ScriptedRunner::new().reply(Output::ok("git@github.com:zavudev/leon.git\n")));
        let project = project_with_worktrees(&rig.store);
        rig.engine.probe_merged(&project, true).await.unwrap();
        assert_eq!(
            merged_by_branch(&rig.store, &project.id)[1].1,
            None,
            "nothing is known, which is not the same as not merged"
        );
        assert!(
            rig.runner.calls().iter().all(|call| !matches!(
                call.args.first().map(String::as_str),
                Some("for-each-ref" | "rev-list" | "status")
            )),
            "no branch, no history, no worktree: git is not asked about merges, only GitHub is"
        );
    }

    #[tokio::test]
    async fn a_pull_request_merged_with_a_squash_is_found_though_its_branch_is_not_inside() {
        // What GitHub does by default: the branch's commits never reach the
        // base, so git cannot see the merge at all, and only GitHub can.
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("git@github.com:zavudev/leon.git\n"))
            .reply(Output::ok(r#"[{"headRefName":"feature/login"}]"#)));
        let project = project_with_worktrees(&rig.store);
        rig.engine.probe_merged(&project, true).await.unwrap();
        assert_eq!(merged_by_branch(&rig.store, &project.id)[1].1, Some(true));
    }

    #[tokio::test]
    async fn a_pull_request_that_is_open_is_not_merged() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("git@github.com:zavudev/leon.git\n"))
            .reply(Output::ok("[]")));
        let project = project_with_worktrees(&rig.store);
        rig.engine.probe_merged(&project, true).await.unwrap();
        assert_eq!(merged_by_branch(&rig.store, &project.id)[1].1, Some(false));
    }

    #[tokio::test]
    async fn a_repository_that_is_not_on_github_is_not_asked_about() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok("git@gitlab.com:zavudev/leon.git\n")));
        let project = project_with_worktrees(&rig.store);
        rig.engine.probe_merged(&project, true).await.unwrap();
        assert_eq!(
            merged_by_branch(&rig.store, &project.id)[1].1,
            None,
            "a repository GitHub knows nothing about is not known, not merged"
        );
        assert!(
            rig.runner.calls().iter().all(|call| call.program == "git"),
            "gh is never run"
        );
    }

    #[tokio::test]
    async fn a_probe_that_fails_leaves_what_was_known_and_never_errors() {
        let rig = rig(ScriptedRunner::new().fail(RunError::Spawn {
            program: "git".into(),
            source: std::io::Error::other("no git"),
        }));
        let project = project_with_worktrees(&rig.store);
        rig.engine.probe_merged(&project, true).await.unwrap();
        assert_eq!(
            merged_by_branch(&rig.store, &project.id)[1].1,
            None,
            "nothing is known, which is not the same as not merged"
        );
    }

    #[tokio::test]
    async fn a_pull_request_probe_that_fails_keeps_what_was_known() {
        // The remote cannot even be read, so gh is never run at all.
        let rig = rig(ScriptedRunner::new().reply(Output::failed(1, "not logged in")));
        let project = project_with_worktrees(&rig.store);
        rig.store
            .set_merged(
                &worktree_of(&rig.store, &project.id, "feature/login"),
                Some(true),
            )
            .unwrap();
        rig.engine.probe_merged(&project, true).await.unwrap();
        assert_eq!(
            merged_by_branch(&rig.store, &project.id)[1].1,
            Some(true),
            "what GitHub said before is better than nothing"
        );
    }

    #[tokio::test]
    async fn the_state_survives_a_restart_and_is_not_written_again_unchanged() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("git@github.com:zavudev/leon.git\n"))
            .reply(Output::ok(r#"[{"headRefName":"feature/login"}]"#)));
        let project = project_with_worktrees(&rig.store);
        rig.engine.probe_merged(&project, true).await.unwrap();
        let worktree = rig
            .store
            .worktrees(&project.id)
            .unwrap()
            .into_iter()
            .find(|worktree| !worktree.is_main)
            .unwrap();
        assert_eq!(worktree.merged_pull_request, Some(true));
        // A second pass with the same answer writes nothing.
        assert!(!rig
            .store
            .set_merged(&worktree.id, worktree.merged_pull_request)
            .unwrap());
        // And what was written is what a fresh reading of the row gives.
        let reread = rig.store.worktrees(&project.id).unwrap();
        assert_eq!(
            reread
                .into_iter()
                .find(|other| other.id == worktree.id)
                .unwrap()
                .merged_pull_request,
            worktree.merged_pull_request
        );
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
    async fn the_location_setting_decides_where_the_worktree_goes() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(LISTING)));
        let project = local_project(&rig.store);
        prefs_of(&rig.engine, |prefs| {
            prefs.worktree_location = "{root}/.worktrees/{branch}".into();
        });
        rig.engine
            .run(Op::AddWorktree {
                project: project.id,
                branch: "feature/login".into(),
                base: None,
            })
            .await;
        assert_eq!(
            rig.runner.calls()[0].args,
            [
                "worktree",
                "add",
                "-b",
                "feature/login",
                "/srv/api/.worktrees/feature-login",
                "HEAD"
            ]
        );
    }

    #[tokio::test]
    async fn a_location_that_cannot_work_is_refused_before_git_runs() {
        let rig = rig(ScriptedRunner::new());
        let project = local_project(&rig.store);
        prefs_of(&rig.engine, |prefs| {
            prefs.worktree_location = "{root}-worktrees".into();
        });
        rig.engine
            .run(Op::AddWorktree {
                project: project.id,
                branch: "fix".into(),
                base: None,
            })
            .await;
        assert!(rig.runner.calls().is_empty());
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("{branch}"), "{}", line.text);
    }

    fn project_in(rig: &Rig, root: &str) -> Project {
        rig.store
            .add_project(&MachineId::local(), "api", root)
            .unwrap()
    }

    #[tokio::test]
    async fn a_project_file_is_read_from_the_projects_folder() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        let project = project_in(&rig, &root);
        assert_eq!(rig.engine.project_state(&project.id), None, "not read yet");

        // No file: nothing to say, on the status line or anywhere.
        let state = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, ProjectState::Absent);
        assert!(rig.engine.status().is_none(), "a missing file is not news");

        std::fs::write(
            dir.path().join("leon.toml"),
            "[worktree]\nsetup = \"make\"\n\n[[script]]\nname = \"Test\"\ncommand = \"make test\"\n",
        )
        .unwrap();
        let state = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        let ProjectState::Loaded(file) = &state else {
            panic!("expected a file, got {state:?}");
        };
        assert_eq!(
            file.setup.as_ref().map(|s| s.command.as_str()),
            Some("make")
        );
        assert_eq!(file.scripts[0].name, "Test");
        assert_eq!(rig.engine.project_state(&project.id), Some(state));
        assert!(
            rig.runner.calls().is_empty(),
            "this computer needs no command"
        );
    }

    #[tokio::test]
    async fn a_wrong_project_file_is_told_with_its_line_and_offers_nothing() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let project = project_in(&rig, &dir.path().to_string_lossy());
        std::fs::write(
            dir.path().join("leon.toml"),
            "[[script]]\nname = \"Test\"\ncommand = \"one\\ntwo\"\n",
        )
        .unwrap();
        let state = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        let ProjectState::Invalid(error) = &state else {
            panic!("expected an error, got {state:?}");
        };
        assert_eq!(error.line, 3);
        assert!(state.file().is_none());
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("leon.toml line 3"), "{}", line.text);
    }

    #[tokio::test]
    async fn a_project_file_on_another_machine_is_read_through_the_runner() {
        // `[worktree]\nsetup = "make"\n`
        let reply = "banner\nLEON-FILE 1\nTEXT 26 1\nW3dvcmt0cmVlXQpzZXR1cCA9ICJtYWtlIgo=\n";
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(reply))
            .reply(Output::ok("LEON-FILE 1\nMISSING\n")));
        let machine = ssh_machine(&rig.store);
        let project = rig
            .store
            .add_project(&machine.id, "api", "/home/dev/api")
            .unwrap();
        let state = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state
                .file()
                .and_then(|file| file.setup.as_ref())
                .map(|s| s.command.as_str()),
            Some("make")
        );
        let call = &rig.runner.calls()[0];
        assert_eq!(call.program, "ssh");
        assert!(
            call.args
                .last()
                .unwrap()
                .ends_with(" sh /home/dev/api/leon.toml"),
            "{:?}",
            call.args
        );
        // The file is gone on the next look: nothing is left of it, quietly.
        let state = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, ProjectState::Absent);
        assert!(rig.engine.status().is_none());
    }

    #[tokio::test]
    async fn a_machine_that_cannot_be_reached_keeps_what_was_known_of_the_file() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(
                "LEON-FILE 1\nTEXT 26 1\nW3dvcmt0cmVlXQpzZXR1cCA9ICJtYWtlIgo=\n",
            ))
            .reply(Output::failed(
                255,
                "ssh: connect to host box.example: refused\n",
            )));
        let machine = ssh_machine(&rig.store);
        let project = rig
            .store
            .add_project(&machine.id, "api", "/home/dev/api")
            .unwrap();
        let known = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        let after = rig
            .engine
            .read_project_file(project.id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after, known, "the setup is still there");
        assert!(
            rig.engine.status().is_none(),
            "nobody asked: the log has the reason"
        );
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
        let before = rig.runner.calls().len();
        let error = rig
            .engine
            .remove_worktree(project.id.clone(), main.id, false)
            .await
            .expect("the job runs")
            .expect_err("the main worktree cannot be removed");
        assert_eq!(rig.runner.calls().len(), before, "nothing else ran");
        assert!(error.to_string().contains("main worktree"));
    }

    #[tokio::test]
    async fn removing_a_worktree_runs_git_and_syncs() {
        let after = "worktree /srv/api\nHEAD abc\nbranch refs/heads/main\n";
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(""))
            .reply(Output::ok(""))
            .reply(Output::ok(after))
            .reply(Output::ok("")));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let extra = rig.store.worktrees(&project.id).unwrap()[1].clone();
        let outcome = rig
            .engine
            .remove_worktree(project.id.clone(), extra.id, false)
            .await
            .expect("the job runs")
            .expect("the worktree is removed");
        assert!(matches!(outcome, Removal::Removed(_)));
        assert!(
            rig.runner.calls().iter().any(|call| call.args
                == ["worktree", "remove", "/srv/api-worktrees/feature-login"]),
            "the worktree was removed: {:?}",
            rig.runner.calls()
        );
        assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn removing_a_worktree_the_store_no_longer_has_lists_again_and_says_it_is_gone() {
        let after = "worktree /srv/api\nHEAD abc\nbranch refs/heads/main\n";
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(""))
            .reply(Output::ok(after)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let extra = rig.store.worktrees(&project.id).unwrap()[1].clone();
        // Another window took it and wrote the store: the row this one shows
        // is old.
        rig.store
            .replace_worktrees(
                &project.id,
                vec![NewWorktree {
                    path: PROJECT_ROOT.into(),
                    branch: Some("main".into()),
                    head: Some("abc".into()),
                    is_main: true,
                }],
            )
            .unwrap();
        let outcome = rig
            .engine
            .remove_worktree(project.id.clone(), extra.id, false)
            .await
            .expect("the job runs")
            .expect("an old row is not an error");
        assert!(
            matches!(&outcome, Removal::Removed(text) if text.contains("already gone")),
            "{outcome:?}"
        );
        assert_eq!(rig.store.worktrees(&project.id).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn removing_a_worktree_git_no_longer_lists_takes_the_row_away_instead_of_erroring() {
        let after = "worktree /srv/api\nHEAD abc\nbranch refs/heads/main\n";
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(""))
            .reply(Output::failed(
                128,
                "fatal: '/srv/api-worktrees/feature-login' is not a working tree",
            ))
            .reply(Output::failed(128, "fatal: cannot change to the folder"))
            .reply(Output::ok(after)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let extra = rig.store.worktrees(&project.id).unwrap()[1].clone();
        let outcome = rig
            .engine
            .remove_worktree(project.id.clone(), extra.id, false)
            .await
            .expect("the job runs")
            .expect("a worktree git no longer lists is not an error");
        assert!(
            matches!(&outcome, Removal::Removed(text) if text.contains("already gone")),
            "{outcome:?}"
        );
        assert_eq!(
            rig.store.worktrees(&project.id).unwrap().len(),
            1,
            "the row is gone with it"
        );
    }

    #[tokio::test]
    async fn a_worktree_git_refuses_for_its_files_is_asked_about_and_removed_with_force() {
        let after = "worktree /srv/api\nHEAD abc\nbranch refs/heads/main\n";
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(LISTING))
            .reply(Output::ok(""))
            .reply(Output::failed(
                128,
                "fatal: '/srv/api-worktrees/feature-login' contains modified or untracked files, use --force to delete it",
            ))
            .reply(Output::ok("?? opencode.json\n"))
            .reply(Output::ok(""))
            .reply(Output::ok(after)));
        let project = local_project(&rig.store);
        rig.engine.run(Op::SyncWorktrees(project.id.clone())).await;
        let extra = rig.store.worktrees(&project.id).unwrap()[1].clone();
        // Without force, git's refusal is turned into the question.
        let outcome = rig
            .engine
            .remove_worktree(project.id.clone(), extra.id.clone(), false)
            .await
            .expect("the job runs")
            .expect("the refusal is an answer");
        assert_eq!(outcome, Removal::NeedsForce, "nothing was removed");
        assert_eq!(
            rig.store.worktrees(&project.id).unwrap().len(),
            2,
            "it is left whole while the person is asked"
        );
        // With force, the untracked file goes with it.
        let outcome = rig
            .engine
            .remove_worktree(project.id.clone(), extra.id, true)
            .await
            .expect("the job runs")
            .expect("the worktree is removed");
        assert!(matches!(outcome, Removal::Removed(_)));
        assert!(
            rig.runner.calls().iter().any(|call| call.args
                == [
                    "worktree",
                    "remove",
                    "--force",
                    "/srv/api-worktrees/feature-login"
                ]),
            "the force removal ran: {:?}",
            rig.runner.calls()
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
    async fn cloning_runs_git_then_saves_and_syncs_the_project() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("Cloning into 'repo'..."))
            .reply(Output::ok(LISTING)));
        rig.engine
            .run(Op::CloneProject {
                machine: MachineId::local(),
                url: "https://host/owner/repo.git".into(),
                parent: "/srv/code".into(),
                name: "repo".into(),
            })
            .await;
        let projects = rig.store.projects(None).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "repo");
        assert_eq!(projects[0].root, "/srv/code/repo");
        let calls = rig.runner.calls();
        assert_eq!(
            calls[0].args,
            [
                "clone",
                "--",
                "https://host/owner/repo.git",
                "/srv/code/repo"
            ]
        );
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Info);
        assert!(
            line.text.starts_with("Cloned repo into /srv/code/repo"),
            "{}",
            line.text
        );
    }

    #[tokio::test]
    async fn a_clone_of_a_name_with_a_slash_is_refused_before_git_runs() {
        let rig = rig(ScriptedRunner::new());
        rig.engine
            .run(Op::CloneProject {
                machine: MachineId::local(),
                url: "https://host/owner/repo.git".into(),
                parent: "/srv/code".into(),
                name: "a/b".into(),
            })
            .await;
        assert!(rig.store.projects(None).unwrap().is_empty());
        assert!(rig.runner.calls().is_empty());
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
    }

    #[tokio::test]
    async fn creating_a_project_makes_the_folder_initializes_git_and_commits() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("")) // mkdir -p parent
            .reply(Output::failed(2, "No such file or directory")) // ls -A target: new folder
            .reply(Output::ok("")) // mkdir -p target
            .reply(Output::ok("Initialized empty Git repository"))
            .reply(Output::ok(""))
            .reply(Output::ok(LISTING)));
        rig.engine
            .run(Op::CreateProject {
                machine: MachineId::local(),
                parent: "/srv/code".into(),
                name: "api".into(),
            })
            .await;
        let projects = rig.store.projects(None).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].root, "/srv/code/api");
        let calls = rig.runner.calls();
        assert_eq!(calls[1].args, ["-A", "/srv/code/api"]);
        assert_eq!(calls[3].args, ["init"]);
        assert_eq!(
            calls[4].args,
            ["commit", "--allow-empty", "-m", "Initial commit"]
        );
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Info);
        assert!(
            line.text
                .starts_with("Created project api at /srv/code/api"),
            "{}",
            line.text
        );
    }

    #[tokio::test]
    async fn creating_over_a_non_empty_folder_is_refused() {
        let rig = rig(
            ScriptedRunner::new()
                .reply(Output::ok("")) // mkdir -p parent
                .reply(Output::ok("something\n")), // ls -A target: not empty
        );
        rig.engine
            .run(Op::CreateProject {
                machine: MachineId::local(),
                parent: "/srv/code".into(),
                name: "api".into(),
            })
            .await;
        assert!(rig.store.projects(None).unwrap().is_empty());
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(
            line.text.contains("already exists and is not empty"),
            "{}",
            line.text
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
            .reply(Output::ok("main\n"))
            .reply(Output::ok(NO_ICON)));
        let machine = ssh_machine(&rig.store);
        let project = rig
            .store
            .add_project(&machine.id, "infra", "/opt/infra")
            .unwrap();
        rig.engine.run(Op::Refresh).await;
        let calls = rig.runner.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.args.iter().any(|arg| arg.contains("LEON-ICON")))
                .count(),
            1,
            "the one icon command, over the same ssh: {}",
            calls.len()
        );
        assert!(
            calls.iter().all(|call| call.program == "ssh"),
            "everything went to the machine: {calls:?}"
        );
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
        // Only the question that lists the worktrees: the refresh asks the
        // project's own folder about them being merged as well, which is a
        // different question in a different folder.
        let asked: Vec<_> = rig
            .runner
            .calls()
            .into_iter()
            .filter(|call| call.args.iter().any(|arg| arg == "worktree"))
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
            .filter(|call| call.args.iter().any(|arg| arg == "worktree"))
            .filter_map(|call| call.cwd)
            .collect();
        assert!(
            asked.iter().all(|cwd| cwd == "/srv/api"),
            "only the project's own folder was asked about: {asked:?}"
        );
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
    async fn agent_memory_is_turned_on_and_off_for_a_project_of_this_computer() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("api");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# Claude\n").unwrap();
        rig.engine.set_memory_dir(dir.path().join("data"));
        let root_text = root.to_string_lossy().into_owned();
        let project = rig
            .store
            .add_project(&MachineId::local(), "api", &root_text)
            .unwrap();
        let op = |on: bool| Op::ProjectMemory {
            project: project.id.clone(),
            on,
        };

        rig.engine.run(op(true)).await;
        assert_eq!(
            status(&rig.engine).text,
            "Agent memory is on for api: the block is in CLAUDE.md."
        );
        let text = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(leon_memory::block::current(&text));
        // Only the file that was there, and the memory file it points at.
        assert!(!root.join("AGENTS.md").exists());
        assert!(crate::memory::file_of(&dir.path().join("data"), &root_text).is_file());

        rig.engine.run(op(true)).await;
        assert_eq!(
            status(&rig.engine).text,
            "Agent memory was already on for api."
        );

        rig.engine.run(op(false)).await;
        assert_eq!(
            status(&rig.engine).text,
            "Agent memory is off for api: the block is out of CLAUDE.md."
        );
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "# Claude\n"
        );
        rig.engine.run(op(false)).await;
        assert_eq!(
            status(&rig.engine).text,
            "Agent memory was already off for api."
        );
    }

    /// A folder that is there, a runner that says it is a repository, and an
    /// engine that asks about agent memory.
    fn offer_rig(replies: usize) -> (Rig, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("api");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.to_string_lossy().into_owned();
        let listing = format!(
            "worktree {root}\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n"
        );
        let mut runner = ScriptedRunner::new();
        for _ in 0..replies {
            runner = runner.reply(Output::ok(listing.clone()));
        }
        let rig = rig(runner);
        rig.engine.set_memory_offer(true);
        (rig, dir, root)
    }

    fn add_by_hand(root: &str) -> Op {
        Op::AddProject {
            machine: MachineId::local(),
            path: root.to_owned(),
            name: "api".into(),
        }
    }

    #[tokio::test]
    async fn a_project_added_by_hand_is_asked_about_and_the_answer_is_carried_out() {
        let (rig, _dir, root) = offer_rig(1);
        std::fs::write(std::path::Path::new(&root).join("CLAUDE.md"), "# Claude\n").unwrap();
        let mut events = rig.engine.subscribe();
        rig.engine.run(add_by_hand(&root)).await;
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        assert!(seen.contains(&EngineEvent::MemoryOffer), "{seen:?}");
        // The adding itself says nothing of the memory: the question does.
        assert!(!status(&rig.engine).text.contains("memory"));
        let offer = rig.engine.take_memory_offer().expect("a question waits");
        assert_eq!(
            (offer.name.as_str(), offer.root.as_str()),
            ("api", root.as_str())
        );
        assert_eq!(
            (offer.writes.as_slice(), offer.creates),
            (&["CLAUDE.md"][..], None)
        );
        assert!(rig.engine.take_memory_offer().is_none(), "asked once");
        // Asking wrote nothing and recorded nothing.
        assert_eq!(
            std::fs::read_to_string(std::path::Path::new(&root).join("CLAUDE.md")).unwrap(),
            "# Claude\n"
        );
        assert_eq!(rig.store.memory_choice(&root).unwrap(), None);

        // "Turn it on" is the palette command's own operation.
        rig.engine
            .run(Op::ProjectMemory {
                project: offer.project.clone(),
                on: true,
            })
            .await;
        assert_eq!(
            status(&rig.engine).text,
            "Agent memory is on for api: the block is in CLAUDE.md."
        );
        assert_eq!(
            rig.store
                .memory_choice(&root)
                .unwrap()
                .map(|(choice, _)| choice),
            Some(leon_core::MemoryChoice::On)
        );
        // Turned off by hand: remembered too, so it is not asked about.
        rig.engine
            .run(Op::ProjectMemory {
                project: offer.project,
                on: false,
            })
            .await;
        assert_eq!(
            rig.store
                .memory_choice(&root)
                .unwrap()
                .map(|(choice, _)| choice),
            Some(leon_core::MemoryChoice::Off)
        );
    }

    #[tokio::test]
    async fn a_project_without_instruction_files_is_offered_agents_md() {
        let (rig, _dir, root) = offer_rig(1);
        rig.engine.run(add_by_hand(&root)).await;
        let offer = rig.engine.take_memory_offer().unwrap();
        assert_eq!((offer.writes.len(), offer.creates), (0, Some("AGENTS.md")));
        assert!(!std::path::Path::new(&root).join("AGENTS.md").exists());
    }

    #[tokio::test]
    async fn never_is_remembered_across_removing_and_adding_the_project_again() {
        let (rig, _dir, root) = offer_rig(2);
        rig.engine.run(add_by_hand(&root)).await;
        let offer = rig.engine.take_memory_offer().unwrap();
        rig.engine.run(Op::MemoryNever(offer.project.clone())).await;
        assert!(status(&rig.engine)
            .text
            .starts_with("Leon will not ask about agent memory for api again."));
        rig.engine.run(Op::RemoveProject(offer.project)).await;
        rig.engine.run(add_by_hand(&root)).await;
        assert_eq!(rig.store.projects(None).unwrap().len(), 1);
        assert!(rig.engine.take_memory_offer().is_none());
        assert!(!std::path::Path::new(&root).join("AGENTS.md").exists());
    }

    #[tokio::test]
    async fn not_now_is_not_asked_again_in_the_same_run_and_is_not_remembered() {
        let (rig, _dir, root) = offer_rig(2);
        rig.engine.run(add_by_hand(&root)).await;
        // "Not now": the window takes the question and answers nothing.
        let offer = rig.engine.take_memory_offer().unwrap();
        rig.engine.run(Op::RemoveProject(offer.project)).await;
        rig.engine.run(add_by_hand(&root)).await;
        assert!(
            rig.engine.take_memory_offer().is_none(),
            "not twice in one run"
        );
        assert_eq!(
            rig.store.memory_choice(&root).unwrap(),
            None,
            "the next run asks"
        );
    }

    #[tokio::test]
    async fn a_project_that_has_the_block_is_not_asked_about_only_said_to_be_on() {
        let (rig, _dir, root) = offer_rig(1);
        let file = std::path::Path::new(&root).join("AGENTS.md");
        std::fs::write(&file, leon_memory::block::insert("# Agents\n")).unwrap();
        let before = std::fs::read(&file).unwrap();
        rig.engine.run(add_by_hand(&root)).await;
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Info);
        assert!(
            line.text.ends_with("Agent memory is already on for api."),
            "{}",
            line.text
        );
        assert!(rig.engine.take_memory_offer().is_none());
        assert_eq!(std::fs::read(&file).unwrap(), before, "nothing was written");
        assert_eq!(
            rig.store
                .memory_choice(&root)
                .unwrap()
                .map(|(choice, _)| choice),
            Some(leon_core::MemoryChoice::On)
        );
    }

    #[tokio::test]
    async fn nothing_is_asked_with_the_setting_off_or_for_another_machine() {
        let (rig, _dir, root) = offer_rig(1);
        rig.engine.set_memory_offer(false);
        rig.engine.run(add_by_hand(&root)).await;
        assert!(rig.engine.take_memory_offer().is_none());
        // The command still works.
        let project = rig.store.projects(None).unwrap().remove(0);
        rig.engine
            .run(Op::ProjectMemory {
                project: project.id,
                on: true,
            })
            .await;
        assert!(std::path::Path::new(&root).join("AGENTS.md").is_file());

        let rig = rig_with(ScriptedRunner::new().reply(Output::ok(LISTING)));
        rig.engine.set_memory_offer(true);
        let remote = ssh_machine(&rig.store);
        rig.engine
            .run(Op::AddProject {
                machine: remote.id,
                path: PROJECT_ROOT.into(),
                name: "api".into(),
            })
            .await;
        assert_eq!(rig.store.projects(None).unwrap().len(), 1);
        assert!(rig.engine.take_memory_offer().is_none());
    }

    #[tokio::test]
    async fn projects_leon_finds_by_itself_are_never_asked_about() {
        let rig = discovery_rig(Answering::new(git_at(ALL_API_FOLDERS, LISTING)));
        rig.engine.set_memory_offer(true);
        let mut events = rig.engine.subscribe();
        session_in(&rig.store, &MachineId::local(), "/srv/api", "a");
        rig.engine.run(Op::Refresh).await;
        assert!(
            !rig.store.projects(None).unwrap().is_empty(),
            "it was found"
        );
        assert!(rig.engine.take_memory_offer().is_none());
        while let Ok(event) = events.try_recv() {
            assert_ne!(event, EngineEvent::MemoryOffer);
        }
    }

    #[tokio::test]
    async fn turning_it_on_where_it_cannot_be_written_records_no_yes() {
        let (rig, dir, root) = offer_rig(1);
        rig.engine.run(add_by_hand(&root)).await;
        let offer = rig.engine.take_memory_offer().unwrap();
        // The folder went away between the question and the answer.
        std::fs::remove_dir_all(dir.path().join("api")).unwrap();
        rig.engine
            .run(Op::ProjectMemory {
                project: offer.project,
                on: true,
            })
            .await;
        let line = status(&rig.engine);
        assert_eq!(line.kind, StatusKind::Error);
        assert!(line.text.contains("is not a folder"), "{}", line.text);
        assert_eq!(rig.store.memory_choice(&root).unwrap(), None);
    }

    #[tokio::test]
    async fn agent_memory_is_refused_for_a_project_on_another_machine() {
        let rig = rig(ScriptedRunner::new());
        let remote = ssh_machine(&rig.store);
        let project = rig
            .store
            .add_project(&remote.id, "api", "/srv/api")
            .unwrap();
        rig.engine
            .run(Op::ProjectMemory {
                project: project.id,
                on: true,
            })
            .await;
        let status = status(&rig.engine);
        assert_eq!(status.kind, StatusKind::Error);
        assert!(status.text.contains("only works on this computer"));
        // Nothing was asked of the machine.
        assert!(rig.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn a_memory_file_is_written_only_when_the_data_folder_is_known() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let op = || Op::WriteMemoryFile {
            root: "/srv/api".into(),
        };
        rig.engine.run(op()).await;
        assert!(rig.engine.status().is_none());

        rig.engine.set_memory_dir(dir.path().to_path_buf());
        rig.store
            .add_memory(
                &leon_core::NewMemory::note(
                    leon_core::MemoryScope::project("/srv/api"),
                    "Use pnpm.",
                ),
                1,
            )
            .unwrap();
        rig.engine.run(op()).await;
        assert!(rig.engine.status().is_none(), "it says nothing");
        let file = crate::memory::file_of(dir.path(), "/srv/api");
        assert!(std::fs::read_to_string(file).unwrap().contains("Use pnpm."));
    }

    #[tokio::test]
    async fn the_engine_and_the_command_line_write_the_same_file_for_one_setting() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_path_buf();
        let scope = leon_core::MemoryScope::project("/srv/api");
        for n in 0..200 {
            let text = format!("fact {n} {}", "word ".repeat(40));
            rig.store
                .add_memory(&leon_core::NewMemory::note(scope.clone(), text), n)
                .unwrap();
        }
        let file = crate::memory::file_of(&data, "/srv/api");
        let settings = data.join(crate::settings::FILE_NAME);
        rig.engine.set_memory_dir(data.clone());
        // The file is there from the first terminal started in the project;
        // a change of the setting then writes it again.
        rig.engine
            .run(Op::WriteMemoryFile {
                root: "/srv/api".into(),
            })
            .await;
        for budget in [3_000usize, 12_000, 50_000] {
            // The application: the setting as the window told it.
            rig.engine.set_memory_budget(budget);
            rig.engine.run(Op::RewriteMemoryFiles).await;
            let by_the_app = std::fs::read(&file).unwrap();
            assert!(
                by_the_app.len() <= budget,
                "{} for {budget}",
                by_the_app.len()
            );
            // It follows the setting: more room, more of the memory.
            assert!(by_the_app.len() > budget.min(30_000) - 1_000);
            // The command line: the same setting, read from the file.
            std::fs::write(&settings, format!(r#"{{"memory_budget": {budget}}}"#)).unwrap();
            std::fs::remove_file(&file).unwrap();
            let service =
                crate::memory::Service::new(rig.store.clone(), data.clone(), "/srv/api".into());
            service.refresh(&scope);
            assert_eq!(std::fs::read(&file).unwrap(), by_the_app, "budget {budget}");
        }
        // A change of the setting rewrites the global file and every
        // project's file that is there.
        assert!(crate::memory::global_file(&data).is_file());
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

    #[tokio::test]
    async fn forgetting_sessions_is_done_when_the_call_returns_and_skips_the_missing() {
        let rig = rig(ScriptedRunner::new());
        session_in(
            &rig.store,
            &MachineId::local(),
            "/srv/api",
            "closed a moment ago",
        );
        let id = rig
            .store
            .recent_sessions(&leon_core::SessionFilter::default(), 5)
            .unwrap()
            .remove(0)
            .id;
        let missing = leon_core::SessionId::from_string("no-such-session");
        // No await: this is what the window calls as the application ends.
        rig.engine
            .forget_sessions(&[missing.clone(), id.clone()])
            .unwrap();
        assert!(rig.store.session(&id).is_err());
        // Asking again, or through the operation, is not an error.
        rig.engine
            .forget_sessions(std::slice::from_ref(&id))
            .unwrap();
        rig.engine.run(Op::ForgetSessions(vec![id, missing])).await;
        assert!(rig.engine.status().is_none(), "forgetting says nothing");
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
            .reply(Output::ok(""))
            .reply(Output::ok("## main\n"))
            .reply(Output::ok("## feature/login\n"))
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

    // ----- files -------------------------------------------------------------

    use crate::files::EntryKind;
    use leon_remote::files::GitMark;
    fn local_path(dir: &tempfile::TempDir, name: &str) -> String {
        dir.path().join(name).to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn a_local_file_is_read_and_saved_with_its_revision_and_runs_no_command() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let path = local_path(&dir, "a b.txt");
        std::fs::write(&path, "\u{feff}one\r\n").unwrap();
        let engine = &rig.engine;

        let FileContent::Text { text, revision } = engine
            .read_file(MachineId::local(), path.clone())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("text");
        };
        assert_eq!(text, "\u{feff}one\r\n");

        let saved = engine
            .write_file(
                MachineId::local(),
                path.clone(),
                "two\n".into(),
                Some(revision.clone()),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(saved, WriteOutcome::Saved(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(status(engine).text, "Saved a b.txt.");

        // The revision that was read is stale now.
        let again = engine
            .write_file(
                MachineId::local(),
                path.clone(),
                "mine\n".into(),
                Some(revision),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again, WriteOutcome::Conflict);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(status(engine).kind, StatusKind::Error);
        assert!(status(engine).text.contains("changed since"));
        assert!(rig.runner.calls().is_empty());
    }

    #[tokio::test]
    async fn a_local_read_that_fails_says_so_on_the_status_line() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        let mut events = rig.engine.subscribe();
        let error = rig
            .engine
            .read_file(MachineId::local(), local_path(&dir, "nope.txt"))
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
        assert_eq!(events.try_recv().unwrap(), EngineEvent::Status);

        let big = local_path(&dir, "big.bin");
        std::fs::write(&big, vec![b'x'; remote_files::MAX_FILE_BYTES + 1]).unwrap();
        let content = rig
            .engine
            .read_file(MachineId::local(), big)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            content,
            FileContent::TooBig {
                size: remote_files::MAX_FILE_BYTES as u64 + 1
            }
        );
    }

    #[tokio::test]
    async fn a_local_folder_is_listed_one_level_at_a_time() {
        let rig = rig(ScriptedRunner::new());
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "x").unwrap();
        std::fs::write(dir.path().join("README.md"), "12").unwrap();
        let entries = rig
            .engine
            .list_dir(MachineId::local(), local_path(&dir, ""))
            .await
            .unwrap()
            .unwrap();
        let shown: Vec<_> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.kind, entry.size))
            .collect();
        assert_eq!(
            shown,
            [
                ("src", EntryKind::Dir, None),
                ("README.md", EntryKind::File, Some(2))
            ]
        );
    }

    #[tokio::test]
    async fn the_files_of_a_local_project_come_from_git_and_from_a_walk_without_it() {
        let rig = rig(ScriptedRunner::new().reply(Output::ok("docs/A b.md\0src/lib.rs\0")));
        let paths = rig
            .engine
            .project_files(MachineId::local(), "/srv/api".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paths, ["docs/A b.md", "src/lib.rs"]);
        let call = &rig.runner.calls()[0];
        assert_eq!(call.program, "git");
        assert_eq!(call.cwd.as_deref(), Some("/srv/api"));
        assert_eq!(call.args, ["ls-files", "-co", "--exclude-standard", "-z"]);

        // Git says it is not a repository: the folder is walked.
        let rig = rig_with(
            ScriptedRunner::new().reply(Output::failed(128, "fatal: not a git repository")),
        );
        let dir = tempfile::tempdir().unwrap();
        for path in ["a.md", "sub/b.md", "target/skip.md"] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        let paths = rig
            .engine
            .project_files(MachineId::local(), local_path(&dir, ""))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paths, ["a.md", "sub/b.md"]);
    }

    #[tokio::test]
    async fn local_git_marks_are_rolled_up_and_a_plain_folder_has_none() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(" M src/lib.rs\0?? notes/\0!! target/\0"))
            .reply(Output::ok("\n"))
            .reply(Output::failed(128, "fatal: not a git repository")));
        let marks = rig
            .engine
            .git_marks(MachineId::local(), "/srv/api".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(marks.get("src/lib.rs"), Some(GitMark::Modified));
        assert_eq!(marks.get("src"), Some(GitMark::Modified));
        assert_eq!(marks.get("notes/a.md"), Some(GitMark::Untracked));
        assert_eq!(marks.get("target/x"), Some(GitMark::Ignored));
        let call = &rig.runner.calls()[0];
        assert_eq!(call.program, "git");
        assert_eq!(call.args, ["status", "--porcelain=v1", "-z", "--ignored"]);

        // A folder inside the repository: paths are made relative to it.
        let inside = rig_with(
            ScriptedRunner::new()
                .reply(Output::ok(" M app/src/lib.rs\0 M other.rs\0"))
                .reply(Output::ok("app/\n")),
        );
        let inner = inside
            .engine
            .git_marks(MachineId::local(), "/srv/api/app".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(inner.get("src/lib.rs"), Some(GitMark::Modified));
        assert_eq!(inner.get("other.rs"), None);
        assert_eq!(
            inside.runner.calls()[1].args,
            ["rev-parse", "--show-prefix"]
        );

        let none = rig
            .engine
            .git_marks(MachineId::local(), "/tmp/plain".into())
            .await
            .unwrap()
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn reading_a_remote_file_runs_a_script_over_ssh_and_skips_a_banner() {
        // "# Hi\n" in base64.
        let reply = "Welcome!\nLEON-FILE 1\nTEXT 9 5\nIyBIaQo=\n";
        let rig = rig(ScriptedRunner::new().reply(Output::ok(reply)));
        let machine = ssh_machine(&rig.store);
        let content = rig
            .engine
            .read_file(machine.id.clone(), "/home/dev/my docs/it's.md".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            content,
            FileContent::Text {
                text: "# Hi\n".into(),
                revision: FileRevision::new("9 5"),
            }
        );
        let call = &rig.runner.calls()[0];
        assert_eq!(call.program, "ssh");
        let line = call.args.last().unwrap();
        assert!(line.contains(r"'/home/dev/my docs/it'\''s.md'"), "{line}");
    }

    #[tokio::test]
    async fn a_remote_file_that_is_missing_or_unreachable_is_an_error_with_a_reason() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("LEON-FILE 1\nMISSING\n"))
            .reply(Output::failed(
                255,
                "ssh: connect to host box.example: refused\n",
            )));
        let machine = ssh_machine(&rig.store);
        let error = rig
            .engine
            .read_file(machine.id.clone(), "/home/dev/x".into())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "/home/dev/x does not exist on box.");
        let error = rig
            .engine
            .read_file(machine.id.clone(), "/home/dev/x".into())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("refused"), "{error}");
        assert_eq!(status(&rig.engine).kind, StatusKind::Error);
    }

    #[tokio::test]
    async fn saving_a_remote_file_sends_the_bytes_as_stdin_with_the_expected_revision() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok("LEON-FILE 1\nOK 10 6\n"))
            .reply(Output::ok("LEON-FILE 1\nCONFLICT\n")));
        let machine = ssh_machine(&rig.store);
        let saved = rig
            .engine
            .write_file(
                machine.id.clone(),
                "/home/dev/README.md".into(),
                "\u{feff}# Hi\r\n".into(),
                Some(FileRevision::new("9 5")),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved, WriteOutcome::Saved(FileRevision::new("10 6")));
        let call = &rig.runner.calls()[0];
        assert_eq!(call.program, "ssh");
        assert_eq!(call.stdin.as_deref(), Some("\u{feff}# Hi\r\n".as_bytes()));
        assert!(call.args.last().unwrap().contains("'9 5'"));
        assert_eq!(status(&rig.engine).text, "Saved README.md.");

        let conflict = rig
            .engine
            .write_file(
                machine.id.clone(),
                "/home/dev/README.md".into(),
                "x".into(),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conflict, WriteOutcome::Conflict);
        assert!(status(&rig.engine).text.contains("changed since"));

        // Too much text is refused before anything is sent.
        let error = rig
            .engine
            .write_file(
                machine.id,
                "/home/dev/big".into(),
                "x".repeat(remote_files::MAX_FILE_BYTES + 1),
                None,
            )
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("too big"), "{error}");
        assert_eq!(rig.runner.calls().len(), 2);
    }

    fn flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[tokio::test]
    async fn a_local_search_reads_the_files_git_lists_and_runs_no_search_command() {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [
            ("src/a.rs", "fn main() {}\nlet Needle = 1;\n"),
            ("ignored.log", "needle\n"),
            ("b.md", "no\n"),
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        // git lists two of the three: the third is ignored, so not searched.
        let rig = rig(ScriptedRunner::new().reply(Output::ok("src/a.rs\0b.md\0")));
        let found = rig
            .engine
            .search_project(
                MachineId::local(),
                local_path(&dir, ""),
                remote_search::ProjectQuery::literal("needle"),
                flag(),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(!found.truncated);
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].path, "src/a.rs");
        assert_eq!(found.hits[0].line, 2);
        assert_eq!(rig.runner.calls().len(), 1, "only the file list was asked");
        assert_eq!(rig.runner.calls()[0].program, "git");

        // A regular expression that is none is an error, not an empty answer,
        // and git is not even asked.
        let error = rig
            .engine
            .search_project(
                MachineId::local(),
                local_path(&dir, ""),
                remote_search::ProjectQuery {
                    text: "(".into(),
                    regex: true,
                    case_sensitive: false,
                },
                flag(),
            )
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("unclosed group"), "{error}");
        assert_eq!(rig.runner.calls().len(), 1);
        assert!(rig.engine.status().is_none(), "a search failure is quiet");
    }

    #[tokio::test]
    async fn a_remote_search_is_one_command_there_and_reads_each_tools_format() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(
                "motd\nLEON-FILE 1\nRG\n./src/a.rs\u{0}2:5:let foo = 1; foo\n",
            ))
            .reply(Output::ok(
                "LEON-FILE 1\nGIT\nsrc/a.rs\u{0}2\u{0}5\u{0}let foo = 1; foo\n",
            ))
            .reply(Output::ok(
                "LEON-FILE 1\nGREP\n./src/a.rs\u{0}2:let foo = 1; foo\n",
            )));
        let machine = ssh_machine(&rig.store);
        let query = remote_search::ProjectQuery {
            text: "$(rm -rf /) 'x'".into(),
            regex: false,
            case_sensitive: true,
        };
        let mut columns = Vec::new();
        for _ in 0..3 {
            let found = rig
                .engine
                .search_project(
                    machine.id.clone(),
                    "/opt/my app".into(),
                    query.clone(),
                    flag(),
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found.hits.len(), 1);
            assert_eq!(found.hits[0].path, "src/a.rs");
            assert_eq!(found.hits[0].line, 2);
            columns.push(found.hits[0].col);
        }
        assert_eq!(columns, [Some(5), Some(5), None]);
        let calls = rig.runner.calls();
        assert_eq!(calls.len(), 3, "one command each");
        assert_eq!(calls[0].program, "ssh");
        // The query and the root are quoted arguments of the remote shell
        // line, and not in the script that is run.
        let line = calls[0].args.last().unwrap();
        assert!(line.contains("'/opt/my app'"), "{line}");
        assert!(line.contains(r"'$(rm -rf /) '\''x'\'''"), "{line}");
        assert!(
            line.ends_with(" F s") || line.ends_with(" 'F' 's'"),
            "{line}"
        );
        assert!(rig.engine.status().is_none());
    }

    #[tokio::test]
    async fn a_remote_search_that_fails_says_why_and_one_without_hits_is_an_empty_answer() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output {
                status: Some(0),
                stdout: "LEON-FILE 1\nRG\n".into(),
                stderr: "rg: regex parse error:\n    (?:()\n    ^\nerror: unclosed group\n".into(),
            })
            .reply(Output::ok("LEON-FILE 1\nRG\n"))
            .reply(Output::ok("LEON-FILE 1\nMISSING\n"))
            .reply(Output::failed(
                255,
                "ssh: connect to host box.example: refused\n",
            )));
        let machine = ssh_machine(&rig.store);
        let search = |text: &str| {
            rig.engine.search_project(
                machine.id.clone(),
                "/w".into(),
                remote_search::ProjectQuery::literal(text),
                flag(),
            )
        };
        let error = search("(").await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "error: unclosed group");
        let found = search("zzz").await.unwrap().unwrap();
        assert!(found.hits.is_empty() && !found.truncated);
        let error = search("x").await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "/w does not exist on box.");
        let error = search("x").await.unwrap().unwrap_err();
        assert!(error.to_string().contains("refused"), "{error}");
    }

    #[tokio::test]
    async fn a_remote_folder_its_files_and_its_git_marks_are_asked_where_they_are() {
        let rig = rig(ScriptedRunner::new()
            .reply(Output::ok(
                "motd\nLEON-FILE 1\nDIR\nd- src\nf- a b.rs\nSIZES\n  7 /w/a b.rs\n",
            ))
            .reply(Output::ok("hi\nLEON-FILE 1\nGIT\nsrc/a b.rs\0README.md\0"))
            .reply(Output::ok("LEON-FILE 1\nGIT\n\n M src/a b.rs\0")));
        let machine = ssh_machine(&rig.store);

        let entries = rig
            .engine
            .list_dir(machine.id.clone(), "/w".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "src");
        assert_eq!(entries[1].size, Some(7));

        let paths = rig
            .engine
            .project_files(machine.id.clone(), "/w".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(paths, ["src/a b.rs", "README.md"]);

        let marks = rig
            .engine
            .git_marks(machine.id.clone(), "/w".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(marks.get("src"), Some(GitMark::Modified));

        let calls = rig.runner.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|call| call.program == "ssh"));
        // Quiet operations leave the status line alone.
        assert!(rig.engine.status().is_none());
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

    /// A keeper that only has a pid.
    struct PidOnly(u32);

    impl crate::durable::Durable for PidOnly {
        fn held(&self) -> Result<Vec<crate::durable::Held>, String> {
            Ok(Vec::new())
        }
        fn open(
            &self,
            _: &leon_term::SpawnSpec,
            _: leon_term::GridSize,
            _: leon_term::TerminalTheme,
            _: leon_term::Wake,
            _: &str,
        ) -> Result<(leon_term::Terminal, crate::durable::KeeperSlot), leon_term::SpawnError>
        {
            Err(leon_term::SpawnError("not here".into()))
        }
        fn attach(
            &self,
            _: &crate::durable::Held,
            _: leon_term::TerminalTheme,
            _: leon_term::Wake,
        ) -> Result<leon_term::Terminal, leon_term::SpawnError> {
            Err(leon_term::SpawnError("not here".into()))
        }
        fn end_all(&self) -> bool {
            true
        }
        fn settle(&self) -> bool {
            true
        }
        fn keeper_pid(&self) -> Option<u32> {
            Some(self.0)
        }
        fn token(&self) -> String {
            String::new()
        }
    }

    #[tokio::test]
    async fn an_agent_below_the_keeper_is_this_leons_own_in_the_scan() {
        // The keeper is a `leon` of its own (500) next to the window (100); a
        // stranger's Leon (600) holds another session.
        let other = "0a1b2c3d-2222-4222-8333-444455556666";
        let output = format!(
            "now=1000000\nT 1 0 /sbin/launchd\nT 100 1 leon\nT 500 1 leon\nT 501 500 zsh\nT 502 501 claude\n\
             T 600 1 leon\nT 601 600 claude\n\
             A 502 501 pts/1 00:30 claude --resume {HELD}\nA 601 600 pts/2 00:30 claude --resume {other}\n"
        );
        let rig = scanning_rig(vec![Output::ok(output.clone()), Output::ok(output)]);
        history(&rig);
        // Without the keeper being known, its agent looks like another Leon's.
        rig.engine.run(Op::Scan(MachineId::local())).await;
        let report = rig.engine.elsewhere(&MachineId::local()).unwrap();
        let ours: Vec<(u32, bool)> = report.found.iter().map(|f| (f.pid, f.leon_child)).collect();
        assert_eq!(ours, [(502, false), (601, false)]);
        // Told about it, the scan counts what runs below it as this Leon's own.
        rig.engine.set_keeper(Some(Arc::new(PidOnly(500))));
        rig.engine.run(Op::Scan(MachineId::local())).await;
        let report = rig.engine.elsewhere(&MachineId::local()).unwrap();
        let ours: Vec<(u32, bool)> = report.found.iter().map(|f| (f.pid, f.leon_child)).collect();
        assert_eq!(ours, [(502, true), (601, false)]);
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
            .reply(Output::ok(""))
            .reply(Output::ok("## main\n"))
            .reply(Output::ok("## feature/login\n"))
            .reply(Output::ok(scan_with_logo())));
        let project = local_project(&rig.store);
        prefs_of(&rig.engine, |prefs| prefs.detect_logos = false);
        rig.engine.run(Op::Refresh).await;
        assert!(
            rig.store.project_icons().unwrap().is_empty(),
            "refresh looked for nothing"
        );
        assert!(
            rig.runner
                .calls()
                .iter()
                .all(|call| !call.args.iter().any(|arg| arg.contains("LEON-ICON"))),
            "refresh looked for no logo: {:?}",
            rig.runner.calls()
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
        let outcome = rig
            .engine
            .remove_worktree(project.id.clone(), extra.id, false)
            .await
            .expect("the job runs")
            .expect("the worktree is removed");
        assert!(matches!(outcome, Removal::Removed(_)));
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

    // ----- what a paired host shares of its own Leon -----------------------------------

    use leon_host::{Host, HostConfig, RelayState};
    use leon_link::relay_client::{dial, Target as DialTarget};
    use leon_link::test_support::TestRelay;
    use leon_link::{pair_as_client, Identity, PairingCode};
    use leon_wire::{SharedEntry, SharedProject, SharedSession, SharedTranscript};

    /// What a test host shares: one project and one Claude session ran at
    /// it, whose transcript it holds. The knob makes the session grow, the
    /// way a session does while its agent works: one dummy message more,
    /// its timestamp later, the count higher.
    struct SharedBox {
        extra: Arc<std::sync::atomic::AtomicU32>,
    }

    impl leon_host::ShareSource for SharedBox {
        fn projects(&self) -> Vec<SharedProject> {
            vec![SharedProject {
                name: "api".into(),
                root: "/srv/api".into(),
            }]
        }

        fn sessions(&self) -> Vec<SharedSession> {
            let extra = self.extra.load(std::sync::atomic::Ordering::SeqCst) as i64;
            vec![SharedSession {
                agent: "claude".into(),
                external_id: "one".into(),
                cwd: "/srv/api".into(),
                title: "fix the login bug".into(),
                model: Some("gpt".into()),
                started_ms: 1_700_000_000_000,
                updated_ms: 1_700_000_060_000 + extra * 1_000,
                messages: 2 + extra as u32,
            }]
        }

        fn transcript(&self, agent: &str, external_id: &str) -> Option<SharedTranscript> {
            let _ = agent;
            (external_id == "one").then(|| {
                let extra = self.extra.load(std::sync::atomic::Ordering::SeqCst);
                let mut messages = vec![
                    SharedEntry {
                        role: "user".into(),
                        text: "please fix the login".into(),
                        at_ms: 1_700_000_000_000,
                    },
                    SharedEntry {
                        role: "assistant".into(),
                        text: "done".into(),
                        at_ms: 1_700_000_060_000,
                    },
                ];
                for n in 0..extra {
                    messages.push(SharedEntry {
                        role: "assistant".into(),
                        text: format!("turn {n}"),
                        at_ms: 1_700_000_060_000 + i64::from(n) * 1_000,
                    });
                }
                SharedTranscript {
                    agent: "claude".into(),
                    external_id: "one".into(),
                    messages,
                }
            })
        }
    }

    /// A relay, a host on it that shares, a client paired with it, and a rig
    /// whose engine talks to that relay: the mirror's whole world. The host
    /// lives until the caller drops it; the knob is what makes its shared
    /// session grow.
    async fn mirrored_world() -> (
        Rig,
        TestRelay,
        Host,
        leon_core::Machine,
        Arc<std::sync::atomic::AtomicU32>,
    ) {
        let relay = TestRelay::start().await;
        let url = relay.url();
        let host_identity = Arc::new(Identity::generate());
        let mut config = HostConfig::new(url.clone(), "box");
        let knob = Arc::new(std::sync::atomic::AtomicU32::new(0));
        config.share = Some(leon_host::Shared(Arc::new(SharedBox {
            extra: knob.clone(),
        })));
        config.backoff = leon_host::Backoff {
            initial: Duration::from_millis(20),
            max: Duration::from_millis(200),
        };
        let host = Host::start(config, host_identity.clone(), None).unwrap();
        let mut state = host.watch_relay();
        while *state.borrow_and_update() != RelayState::Online {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let me = Arc::new(Identity::generate());
        let info = host.new_pairing_code().await;
        let pipe = dial(&url, &DialTarget::Room(info.room), None)
            .await
            .unwrap();
        let paired = pair_as_client(
            pipe,
            &me,
            &PairingCode::parse(&info.code).unwrap(),
            "Ana's laptop",
        )
        .await
        .unwrap();

        let rig = rig(ScriptedRunner::new());
        rig.engine.set_relay_hub(leon_remote::RelayHub::new(
            me,
            "Ana's laptop",
            Handle::current(),
        ));
        let machine = rig
            .engine
            .save_relay_machine(
                "",
                &paired.host_id.to_string(),
                &leon_remote::relay::key_hex(&paired.host_key),
                &url,
                "box",
            )
            .unwrap();
        (rig, relay, host, machine, knob)
    }

    #[tokio::test]
    async fn a_relay_machine_mirrors_what_its_host_shares() {
        let (rig, _relay, _host, machine, _) = mirrored_world().await;
        rig.engine.run(Op::SyncHost(machine.id.clone())).await;

        // The project: named and rooted as the host says.
        let projects = rig.store.projects(Some(&machine.id)).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(
            (projects[0].name.as_str(), projects[0].root.as_str()),
            ("api", "/srv/api")
        );

        // The session: attributed to this machine, tied to the mirrored
        // project, and stored complete.
        let filter = SessionFilter {
            machine_id: Some(machine.id.clone()),
            ..Default::default()
        };
        let sessions = rig.store.recent_sessions(&filter, 10).unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.agent, leon_core::AgentId::CLAUDE);
        assert_eq!(session.title, "fix the login bug");
        assert_eq!(session.cwd, "/srv/api");
        assert_eq!(session.project_id.as_ref(), Some(&projects[0].id));
        let messages = rig.store.session_messages(&session.id).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| (m.role, m.text.clone()))
                .collect::<Vec<_>>(),
            [
                (leon_core::Role::User, "please fix the login".to_owned()),
                (leon_core::Role::Assistant, "done".to_owned())
            ]
        );

        // A sync that changes nothing is silent.
        assert_eq!(
            rig.engine.sync_host(&machine.id).await.unwrap(),
            HostSync::default()
        );
    }

    /// A project this computer does not want stays out of the mirror: the
    /// host offers it again on every sync, and the store remembers it was
    /// dismissed.
    #[tokio::test]
    async fn a_dismissed_project_is_not_mirrored_again() {
        let (rig, _relay, _host, machine, _) = mirrored_world().await;
        rig.engine.run(Op::SyncHost(machine.id.clone())).await;
        let project = &rig.store.projects(Some(&machine.id)).unwrap()[0];
        rig.store.remove_project(&project.id).unwrap();

        assert_eq!(
            rig.engine.sync_host(&machine.id).await.unwrap(),
            HostSync::default()
        );
        assert!(rig.store.projects(Some(&machine.id)).unwrap().is_empty());
    }

    /// A session the host's history grew since the last sync is asked for
    /// again, updated in place, and keeps its id — so links to it (a live
    /// terminal, a pin) survive.
    #[tokio::test]
    async fn a_session_the_host_grew_is_updated_in_place() {
        let (rig, _relay, _host, machine, knob) = mirrored_world().await;
        rig.engine.run(Op::SyncHost(machine.id.clone())).await;
        let stored = rig
            .store
            .session_by_external(&machine.id, leon_core::AgentId::CLAUDE, "one")
            .unwrap()
            .unwrap();
        assert_eq!(stored.message_count, 2);

        // The host's history moved on: one message more, its timestamp later.
        use std::sync::atomic::Ordering::SeqCst;
        knob.store(1, SeqCst);
        let synced = rig.engine.sync_host(&machine.id).await.unwrap();
        assert_eq!(synced.sessions, 1, "the grown session is stored again");

        let grew = rig.store.session(&stored.id).unwrap();
        assert_eq!(grew.message_count, 3);
        assert_eq!(
            grew.updated_at,
            DateTime::from_timestamp_millis(1_700_000_061_000).unwrap()
        );
        let listed = rig.store.session_messages(&grew.id).unwrap();
        let texts: Vec<&str> = listed.iter().map(|message| message.text.as_str()).collect();
        assert_eq!(texts.last().copied(), Some("turn 0"));
    }
}
