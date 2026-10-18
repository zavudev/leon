//! The connection to one paired computer, and the turns run on it.
//!
//! A [`HostSession`] wraps the durable client of `leon-link`: it reconnects
//! by itself and says where it stands through a [`ConnectionListener`]. On
//! top of it:
//!
//! * what the other Leon shares: its projects, its history, a transcript;
//! * [`HostSession::start_turn`]: one prompt to one agent, in a terminal on
//!   the host labelled so that it can be found again. What happens is
//!   delivered to a [`TurnListener`]; an agent driven as a session can be
//!   answered, told more and interrupted through the [`Turn`]
//!   (`crate::turn` says what is guaranteed);
//! * [`HostSession::running_turns`] and [`HostSession::attach_turn`]: after
//!   the application was closed, the turn is still on the host; attaching
//!   replays it from the start through the same reader.
//!
//! Listeners are called from a thread of their own, one per stream, in
//! order. A listener may take its time; nothing else waits for it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use leon_link::client::{Client, ConnState, Failure, PtyEvent, PtyHandle, PtyStream};
use leon_remote::parse_worktree_list;
use leon_wire::{ExecSpec, Grid, SharedProject, SharedSession, SharedTranscript};

use crate::agents::{login_spec, parse_label, turn_label};
use crate::bridge::{bridge_spec, ARGUMENT_BYTES};
use crate::error::MobileError;
use crate::events::{Answer, TurnEvent};
use crate::logo::ProjectLogo;
use crate::options::AgentOptions;
use crate::runtime::{on_runtime, runtime};
use crate::turn::{Effects, Engine};
use leon_turn::{adapter, Action, Adapter, TurnSpec};

/// The terminal of a turn. Nobody looks at it: it is wide so that a program
/// which asks for the width does not fold its lines.
const TURN_GRID: Grid = Grid {
    cols: 2000,
    rows: 50,
    cell_width: 0,
    cell_height: 0,
};

/// How long the small commands this module runs for itself may take.
const PROBE_TIMEOUT_MS: u32 = 20_000;
/// How long an agent may take to say what it offers: it starts for it.
const OPTIONS_TIMEOUT_MS: u32 = 45_000;
/// How often a turn looks at the clock: to write again what was not
/// acknowledged, and to notice that a replay has caught up.
const TURN_TICK: Duration = Duration::from_millis(250);
/// How many folders one command of [`HostSession::worktrees`] asks about.
const TREE_CHUNK: usize = 200;
/// The longest path [`HostSession::worktrees`] asks about; a longer one is
/// answered with nothing.
const TREE_PATH_MAX: usize = 4096;
/// What starts every answer line of [`HostSession::worktrees`].
const TREE_MARK: &str = "leon-tree:";

/// Why the session is not online.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OfflineReason {
    /// The relay cannot be reached: no network, or the service is down.
    RelayUnreachable,
    /// The other computer is off, asleep, or not sharing.
    HostOffline,
    /// The other computer no longer accepts this device. Pair again.
    Revoked,
    /// The other computer runs a version this one cannot talk to.
    VersionMismatch,
    /// Anything else; it is retried.
    Other,
}

/// Where the connection stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectionState {
    /// Connecting for the first time.
    Connecting,
    /// Connected and authenticated.
    Online,
    /// Not connected. It is retried with a growing delay, except after
    /// [`OfflineReason::Revoked`] and [`OfflineReason::VersionMismatch`],
    /// which wait for [`HostSession::reconnect_now`].
    Offline {
        /// The kind of failure.
        reason: OfflineReason,
        /// A sentence for a person.
        message: String,
        /// Failed attempts in a row.
        attempt: u32,
    },
    /// Closed by the application.
    Closed,
}

impl From<ConnState> for ConnectionState {
    fn from(state: ConnState) -> Self {
        match state {
            ConnState::Connecting => ConnectionState::Connecting,
            ConnState::Online => ConnectionState::Online,
            ConnState::Offline {
                failure,
                reason,
                attempt,
            } => ConnectionState::Offline {
                reason: match failure {
                    Failure::RelayUnreachable => OfflineReason::RelayUnreachable,
                    Failure::HostOffline => OfflineReason::HostOffline,
                    Failure::Revoked => OfflineReason::Revoked,
                    Failure::VersionMismatch => OfflineReason::VersionMismatch,
                    Failure::Other => OfflineReason::Other,
                },
                message: reason,
                attempt,
            },
            ConnState::Closed => ConnectionState::Closed,
        }
    }
}

/// Told every change of the connection, starting with the current state.
#[uniffi::export(callback_interface)]
pub trait ConnectionListener: Send + Sync {
    /// The connection is now in `state`.
    fn on_state(&self, state: ConnectionState);
}

/// Told what happens during one turn, in order.
#[uniffi::export(callback_interface)]
pub trait TurnListener: Send + Sync {
    /// The next event. [`TurnEvent::Exited`] or [`TurnEvent::Gone`] is the
    /// last one.
    fn on_event(&self, event: TurnEvent);
}

/// A project of the other computer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Project {
    /// Its name.
    pub name: String,
    /// Its folder, a path on that computer.
    pub root: String,
}

/// A git worktree of a repository on the other computer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Worktree {
    /// Its folder, a path on that computer.
    pub path: String,
    /// The branch checked out there; nothing when the head is detached.
    pub branch: Option<String>,
    /// Whether it is the repository's main worktree.
    pub main: bool,
}

/// A conversation in the other computer's history.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistorySession {
    /// The agent's catalogue tag (`claude`).
    pub agent: String,
    /// The id the agent resumes it by.
    pub external_id: String,
    /// The folder it ran in.
    pub cwd: String,
    /// Its title.
    pub title: String,
    /// The model, when known.
    pub model: Option<String>,
    /// When it started, milliseconds since the Unix epoch.
    pub started_ms: i64,
    /// When it last changed.
    pub updated_ms: i64,
    /// How many messages it has.
    pub messages: u32,
    /// Whether a turn can continue it from here.
    pub can_resume: bool,
}

/// What the other Leon knows: the same its sidebar shows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SharedState {
    /// Its projects.
    pub projects: Vec<Project>,
    /// Its history, newest first.
    pub sessions: Vec<HistorySession>,
    /// There were more sessions than one answer carries.
    pub truncated: bool,
    /// The sessions it has pinned, in the order of its Pinned section. They
    /// may be older than everything in `sessions`. None from a Leon that
    /// does not say.
    pub pinned: Vec<HistorySession>,
    /// Whether it has a Den to show ([`HostSession::den`]). A Leon that does
    /// not say has none, and must not be asked.
    pub den: bool,
}

/// One lion of the other Leon's Den: a live agent session there, or a
/// sub-agent of one, with what it is doing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DenCub {
    /// The lion's id over there, the same for as long as its session lives.
    pub id: u64,
    /// What its sidebar calls the session.
    pub name: String,
    /// Its agent's tag, such as `claude`; empty when not known.
    pub agent: String,
    /// What it is doing, as a tag: `editing`, `reading`, `searching`,
    /// `running`, `web`, `delegating`, `planning`, `using-tool`, `thinking`,
    /// `mystery`, `waiting-for-user`, `needs-permission`, `idle`, `asleep`,
    /// `fainted` or `gone`. One not listed here is some kind of working.
    pub state: String,
    /// The plain fact: the file it edits, the command as typed.
    pub detail: Option<String>,
    /// How many tools it has used so far.
    pub level: u32,
    /// The lion that sent this one out, for a sub-agent.
    pub parent: Option<u64>,
    /// Nothing but working or quiet is known of it.
    pub mystery: bool,
    /// The agent's own id of its session, once known: with `agent`, the
    /// session of the shared history it is.
    pub external_id: Option<String>,
    /// The folder it runs in.
    pub cwd: Option<String>,
}

/// One message of a transcript.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TranscriptEntry {
    /// `user`, `assistant`, `tool` or `system`.
    pub role: String,
    /// Its text.
    pub text: String,
    /// When, milliseconds since the Unix epoch.
    pub at_ms: i64,
}

/// What to ask, of whom, where.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TurnRequest {
    /// The agent's catalogue tag (see [`crate::agents()`]).
    pub agent: String,
    /// The folder on the other computer to work in; nothing for the folder
    /// its Leon runs in.
    pub cwd: Option<String>,
    /// What the person wrote.
    pub prompt: String,
    /// The agent's conversation to continue ([`TurnEvent::Started`] of an
    /// earlier turn, or a session of the history); nothing starts a new one.
    pub resume_id: Option<String>,
    /// The agent's permission mode, one of [`AgentOptions::modes`]; nothing
    /// for the one that computer is configured for.
    pub mode: Option<String>,
    /// The model, one of [`AgentOptions::models`]; nothing for that
    /// computer's default.
    pub model: Option<String>,
    /// How hard it should think, one of the model's efforts; nothing for
    /// its default.
    pub effort: Option<String>,
    /// More folders on the other computer it may work in.
    pub add_dirs: Vec<String>,
    /// The application's own name for the chat. It labels the turn on the
    /// host, so a turn found by [`HostSession::running_turns`] can be put
    /// back in its chat.
    pub chat_id: String,
}

/// A turn that is on the other computer, running or recently ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RunningTurn {
    /// What [`HostSession::attach_turn`] takes.
    pub pty: u64,
    /// The agent's catalogue tag.
    pub agent: String,
    /// The chat it was started from.
    pub chat_id: String,
    /// The folder it runs in.
    pub cwd: Option<String>,
    /// When it started, seconds since the Unix epoch.
    pub started_unix: u64,
    /// Its exit code once it has ended (the host lists it for half an hour).
    pub exit_code: Option<u32>,
}

/// How turns are started, where a test needs it to differ: without the
/// login shell, and with stand-in programs. Not part of the exported API.
#[derive(Debug, Clone, Default)]
pub struct SpawnOverrides {
    /// Run the agent directly instead of through the owner's login shell.
    pub direct: bool,
    /// The program to run for an agent's tag, instead of the catalogue's.
    pub programs: HashMap<String, String>,
}

/// The connection to one paired computer.
#[derive(uniffi::Object)]
pub struct HostSession {
    client: Client,
    spawn: Mutex<SpawnOverrides>,
}

impl HostSession {
    /// Wraps a started client and reports its state to `listener`.
    pub(crate) fn new(client: Client, listener: Box<dyn ConnectionListener>) -> Arc<Self> {
        let mut states = client.watch();
        let handle = runtime().handle().clone();
        runtime().spawn_blocking(move || loop {
            let state = states.borrow_and_update().clone();
            let closed = state == ConnState::Closed;
            listener.on_state(state.into());
            if closed || handle.block_on(states.changed()).is_err() {
                break;
            }
        });
        Arc::new(Self {
            client,
            spawn: Mutex::new(SpawnOverrides::default()),
        })
    }

    /// Replaces how turns are started. For tests.
    pub fn set_spawn_overrides(&self, overrides: SpawnOverrides) {
        if let Ok(mut spawn) = self.spawn.lock() {
            *spawn = overrides;
        }
    }

    fn overrides(&self) -> SpawnOverrides {
        self.spawn
            .lock()
            .map(|spawn| spawn.clone())
            .unwrap_or_default()
    }

    /// A command for this module's own questions, through the login shell
    /// unless a test said otherwise.
    fn probe(&self, argv: &[&str]) -> ExecSpec {
        let argv = argv.iter().map(|arg| (*arg).to_owned()).collect();
        login_spec(argv, None, !self.overrides().direct)
    }
}

#[uniffi::export]
impl HostSession {
    /// Where the connection stands now.
    pub fn state(&self) -> ConnectionState {
        self.client.state().into()
    }

    /// The other computer's name, once it has said it.
    pub fn host_name(&self) -> Option<String> {
        self.client.host_name()
    }

    /// The wire version agreed with the other computer the last time the
    /// connection opened: the current one, or an older one that computer
    /// still speaks. Nothing until the first connection.
    pub fn protocol(&self) -> Option<u16> {
        self.client.protocol()
    }

    /// Tries to connect now instead of waiting out the delay. The way back
    /// from a state that is not retried.
    pub fn reconnect_now(&self) {
        self.client.reconnect_now();
    }

    /// Ends the session. Turns keep running on the other computer.
    pub fn close(&self) {
        self.client.close();
    }

    /// What the other Leon shares. A computer that runs only the headless
    /// service shares nothing: the lists are empty.
    pub async fn share_state(&self) -> Result<SharedState, MobileError> {
        let client = self.client.clone();
        on_runtime(async move {
            let shared = client.share_state().await?;
            Ok(SharedState {
                projects: shared.projects.into_iter().map(project).collect(),
                sessions: shared.sessions.into_iter().map(history_session).collect(),
                truncated: shared.truncated,
                pinned: shared.pinned.into_iter().map(history_session).collect(),
                den: shared.den,
            })
        })
        .await
    }

    /// Who is in the other Leon's Den now. Only for a Leon whose
    /// [`SharedState::den`] is set: one that does not know the question
    /// drops the connection. Asking is what keeps its Den worked out while
    /// nobody looks at it there, so the first answer may hold nobody.
    pub async fn den(&self) -> Result<Vec<DenCub>, MobileError> {
        let client = self.client.clone();
        on_runtime(async move {
            let cubs = client.share_den().await?;
            Ok(cubs
                .into_iter()
                .map(|cub| DenCub {
                    id: cub.id,
                    name: cub.name,
                    agent: cub.agent,
                    state: cub.state,
                    detail: cub.detail,
                    level: cub.level,
                    parent: cub.parent,
                    mystery: cub.mystery,
                    external_id: cub.external_id,
                    cwd: cub.cwd,
                })
                .collect())
        })
        .await
    }

    /// The transcript of a session of the history, or nothing when the other
    /// computer does not have it.
    pub async fn transcript(
        &self,
        agent: String,
        external_id: String,
    ) -> Result<Option<Vec<TranscriptEntry>>, MobileError> {
        let client = self.client.clone();
        on_runtime(async move {
            let transcript = client.share_transcript(&agent, &external_id).await?;
            Ok(transcript.map(entries))
        })
        .await
    }

    /// The home folder of the user the other computer runs Leon as: where to
    /// work when it shares no project.
    pub async fn home_dir(&self) -> Result<String, MobileError> {
        let client = self.client.clone();
        let spec = self.probe(&["sh", "-c", r#"printf '\n%s\n' "$HOME""#]);
        on_runtime(async move {
            let output = client.exec(spec, Some(PROBE_TIMEOUT_MS)).await?;
            // The last line: a login shell may have printed before it.
            let text = String::from_utf8_lossy(&output.stdout);
            let home = text.lines().rfind(|line| !line.trim().is_empty());
            home.map(|line| line.trim().to_owned())
                .ok_or(MobileError::Protocol)
        })
        .await
    }

    /// The folder a command starts in on the other computer when it is
    /// given none.
    async fn start_folder(&self) -> Result<String, MobileError> {
        let client = self.client.clone();
        let spec = self.probe(&["sh", "-c", r#"printf '\n%s\n' "$PWD""#]);
        on_runtime(async move {
            let output = client.exec(spec, Some(PROBE_TIMEOUT_MS)).await?;
            let text = String::from_utf8_lossy(&output.stdout);
            let folder = text.lines().rfind(|line| line.trim().starts_with('/'));
            folder
                .map(|line| line.trim().to_owned())
                .ok_or(MobileError::Protocol)
        })
        .await
    }

    /// The folders inside `path` on the other computer, sorted, without the
    /// hidden ones: for choosing where to work.
    pub async fn subdirs(&self, path: String) -> Result<Vec<String>, MobileError> {
        const LIST: &str = r#"cd "$1" || exit 1
echo
for dir in */; do
    [ -d "$dir" ] && printf '%s\n' "${dir%/}"
done
exit 0"#;
        let client = self.client.clone();
        let spec = self.probe(&["sh", "-c", LIST, "leon", &path]);
        on_runtime(async move {
            let output = client.exec(spec, Some(PROBE_TIMEOUT_MS)).await?;
            if output.status != Some(0) {
                return Err(MobileError::Host {
                    message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                });
            }
            // What follows the last empty line: a login shell may have
            // printed before the listing.
            let text = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = text.lines().collect();
            let from = lines
                .iter()
                .rposition(|line| line.is_empty())
                .map_or(0, |at| at + 1);
            let mut dirs: Vec<String> = lines[from..]
                .iter()
                .map(|line| (*line).to_owned())
                .collect();
            dirs.sort();
            Ok(dirs)
        })
        .await
    }

    /// The git worktrees of the repository each of `roots` is in, in the
    /// same order, the main worktree first: what `git worktree list` says
    /// there, which is what the desktop lists under a project, so the phone
    /// can sort sessions into the same places. Nothing for a folder that is
    /// in no repository or no longer exists and when git is missing; the
    /// entry of a bare repository is left out, as it is no checkout.
    pub async fn worktrees(&self, roots: Vec<String>) -> Result<Vec<Vec<Worktree>>, MobileError> {
        // Every line of git's answer is numbered: a login shell may print
        // before them, and a folder git does not know has no line at all.
        const LIST: &str = r#"index=0
for dir in "$@"; do
    git -C "$dir" worktree list --porcelain 2>/dev/null | while IFS= read -r line; do
        printf 'leon-tree:%s:%s\n' "$index" "$line"
    done
    index=$((index + 1))
done
exit 0"#;
        let mut found = vec![Vec::new(); roots.len()];
        let asked: Vec<usize> = (0..roots.len())
            .filter(|&at| !roots[at].is_empty() && roots[at].len() <= TREE_PATH_MAX)
            .collect();
        let mut chunks = Vec::new();
        for chunk in asked.chunks(TREE_CHUNK) {
            let mut argv = vec!["sh", "-c", LIST, "leon"];
            argv.extend(chunk.iter().map(|&at| roots[at].as_str()));
            chunks.push((chunk.to_vec(), self.probe(&argv)));
        }
        let client = self.client.clone();
        on_runtime(async move {
            for (chunk, spec) in chunks {
                let output = client.exec(spec, Some(PROBE_TIMEOUT_MS)).await?;
                let text = String::from_utf8_lossy(&output.stdout);
                for (at, trees) in chunk.into_iter().zip(parse_worktrees(&text, TREE_CHUNK)) {
                    found[at] = trees;
                }
            }
            Ok(found)
        })
        .await
    }

    /// What stands for the project rooted at `root`: the logo the desktop
    /// would show for it, looked for on the other computer in one command, two
    /// at the most (see [`crate::logo`]).
    pub async fn project_logo(&self, root: String) -> Result<ProjectLogo, MobileError> {
        let client = self.client.clone();
        on_runtime(async move { crate::logo::find(client, &root).await }).await
    }

    /// Which of the agents this library can drive are installed on the other
    /// computer (their catalogue tags).
    pub async fn installed_agents(&self) -> Result<Vec<String>, MobileError> {
        const WHICH: &str = r#"for program in "$@"; do
    command -v "$program" >/dev/null 2>&1 && printf 'leon-has:%s\n' "$program"
done
exit 0"#;
        let overrides = self.overrides();
        let programs: Vec<(String, &'static str)> = leon_turn::ADAPTERS
            .iter()
            .map(|adapter| (program_of(adapter, &overrides), adapter.id))
            .collect();
        let mut argv = vec!["sh", "-c", WHICH, "leon"];
        argv.extend(programs.iter().map(|(program, _)| program.as_str()));
        let spec = self.probe(&argv);
        let client = self.client.clone();
        on_runtime(async move {
            let output = client.exec(spec, Some(PROBE_TIMEOUT_MS)).await?;
            let text = String::from_utf8_lossy(&output.stdout);
            let found: Vec<&str> = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("leon-has:"))
                .collect();
            Ok(programs
                .iter()
                .filter(|(program, _)| found.contains(&program.as_str()))
                .map(|(_, id)| (*id).to_owned())
                .collect())
        })
        .await
    }

    /// What `agent` offers on the other computer: its permission modes, its
    /// models and their efforts, and whether it can be answered and told
    /// more while it works. The agent itself is asked where it can be, so
    /// the lists are those of the version installed there.
    pub async fn agent_options(&self, agent: String) -> Result<AgentOptions, MobileError> {
        let adapter = adapter(&agent).ok_or(MobileError::UnknownAgent)?;
        let options = &adapter.options;
        let program = program_of(adapter, &self.overrides());
        let arguments = (options.arguments)();
        let mut argv = vec!["sh", "-c", options.script, "leon", &program];
        argv.extend(arguments.iter().map(String::as_str));
        let spec = self.probe(&argv);
        let parse = options.parse;
        let client = self.client.clone();
        on_runtime(async move {
            let output = client.exec(spec, Some(OPTIONS_TIMEOUT_MS)).await?;
            Ok(parse(&String::from_utf8_lossy(&output.stdout)).into())
        })
        .await
    }

    /// Sends one prompt to an agent. What happens arrives at `listener`;
    /// the returned [`Turn`] answers the agent's questions and stops it. The
    /// turn runs on the other computer whether or not this device stays
    /// connected.
    pub async fn start_turn(
        &self,
        request: TurnRequest,
        listener: Box<dyn TurnListener>,
    ) -> Result<Arc<Turn>, MobileError> {
        let adapter = adapter(&request.agent).ok_or(MobileError::UnknownAgent)?;
        let overrides = self.overrides();
        let program = program_of(adapter, &overrides);
        let some = |text: &'_ Option<String>| -> Option<String> {
            text.clone().filter(|text| !text.is_empty())
        };
        let (resume, mode, model, effort) = (
            some(&request.resume_id),
            some(&request.mode),
            some(&request.model),
            some(&request.effort),
        );
        // An agent whose protocol names the folder is told the one the
        // turn starts in, when the turn names none.
        let folder = match some(&request.cwd) {
            None if adapter.needs_folder => Some(self.start_folder().await?),
            named => named,
        };
        let turn = TurnSpec {
            program: &program,
            prompt: &request.prompt,
            resume: resume.as_deref(),
            mode: mode.as_deref(),
            model: model.as_deref(),
            effort: effort.as_deref(),
            cwd: folder.as_deref(),
            add_dirs: &request.add_dirs,
            ..TurnSpec::default()
        };
        let login = !overrides.direct;
        let (spec, engine) = {
            let mut session = (adapter.session)(clock_ms());
            let mut waiting = session.first(&turn);
            // As much of the opening as fits goes with the command: it is
            // then the agent's whatever happens to this connection.
            let mut budget = ARGUMENT_BYTES;
            let fits = waiting
                .iter()
                .take_while(|outgoing| {
                    let fit = outgoing.line.len() <= budget;
                    budget = budget.saturating_sub(outgoing.line.len());
                    fit
                })
                .count();
            let first: Vec<String> = waiting
                .drain(..fits)
                .map(|outgoing| outgoing.line)
                .collect();
            (
                bridge_spec(&first, (adapter.argv)(&turn), request.cwd.clone(), login),
                Engine::session(session, clock_ms(), waiting),
            )
        };
        let label = turn_label(adapter.id, &request.chat_id);
        let client = self.client.clone();
        on_runtime(async move {
            let stream = client.pty_open(spec, TURN_GRID, &label).await?;
            Ok(Turn::follow(stream, engine, listener))
        })
        .await
    }

    /// The turns this library started that are still on the other computer:
    /// running, or ended within the last half hour.
    pub async fn running_turns(&self) -> Result<Vec<RunningTurn>, MobileError> {
        let client = self.client.clone();
        on_runtime(async move {
            let ptys = client.pty_list().await?;
            Ok(ptys
                .into_iter()
                .filter_map(|info| {
                    let (agent, chat_id) = parse_label(&info.label)?;
                    adapter(agent)?;
                    Some(RunningTurn {
                        pty: info.pty,
                        agent: agent.to_owned(),
                        chat_id: chat_id.to_owned(),
                        cwd: info.cwd,
                        started_unix: info.started_unix,
                        exit_code: info.exit.map(|exit| exit.code),
                    })
                })
                .collect())
        })
        .await
    }

    /// Follows a turn found by [`HostSession::running_turns`] from its
    /// beginning: `listener` receives everything again, then what is new,
    /// and the returned [`Turn`] answers what the agent still asks (a
    /// question comes back open unless its answer had reached the agent). A
    /// turn that printed more than the host keeps starts with
    /// [`TurnEvent::Gap`]. One listener follows a turn at a time: one that
    /// was following it in this session stops receiving, without a last
    /// event.
    pub async fn attach_turn(
        &self,
        pty: u64,
        listener: Box<dyn TurnListener>,
    ) -> Result<Arc<Turn>, MobileError> {
        let client = self.client.clone();
        on_runtime(async move {
            let ptys = client.pty_list().await?;
            let info = ptys
                .iter()
                .find(|info| info.pty == pty)
                .ok_or(MobileError::TurnNotFound)?;
            let adapter = parse_label(&info.label)
                .and_then(|(agent, _)| adapter(agent))
                .ok_or(MobileError::TurnNotFound)?;
            let engine = Engine::attached(
                (adapter.session)(clock_ms()),
                clock_ms(),
                info.end_offset.saturating_sub(info.first_offset),
                Instant::now(),
            );
            let stream = client.pty_attach(pty, 0).await?;
            Ok(Turn::follow(stream, engine, listener))
        })
        .await
    }
}

fn program_of(adapter: &Adapter, overrides: &SpawnOverrides) -> String {
    overrides
        .programs
        .get(adapter.id)
        .cloned()
        .unwrap_or_else(|| adapter.program.to_owned())
}

/// Reads the answer of [`HostSession::worktrees`]'s command: for each of the
/// first `count` folders asked about, the worktrees git listed there. Lines
/// that are not answers (a login shell's own output) are skipped.
fn parse_worktrees(text: &str, count: usize) -> Vec<Vec<Worktree>> {
    let mut listings = vec![String::new(); count];
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((index, value)) = line
            .strip_prefix(TREE_MARK)
            .and_then(|rest| rest.split_once(':'))
        else {
            continue;
        };
        if let Some(listing) = index
            .parse::<usize>()
            .ok()
            .and_then(|at| listings.get_mut(at))
        {
            listing.push_str(value);
            listing.push('\n');
        }
    }
    listings
        .iter()
        .map(|listing| {
            parse_worktree_list(listing)
                .into_iter()
                .filter(|tree| !tree.is_bare)
                .map(|tree| Worktree {
                    path: tree.path,
                    branch: tree.branch,
                    main: tree.is_main,
                })
                .collect()
        })
        .collect()
}

fn project(project: SharedProject) -> Project {
    Project {
        name: project.name,
        root: project.root,
    }
}

fn history_session(session: SharedSession) -> HistorySession {
    HistorySession {
        can_resume: adapter(&session.agent).is_some(),
        agent: session.agent,
        external_id: session.external_id,
        cwd: session.cwd,
        title: session.title,
        model: session.model,
        started_ms: session.started_ms,
        updated_ms: session.updated_ms,
        messages: session.messages,
    }
}

fn entries(transcript: SharedTranscript) -> Vec<TranscriptEntry> {
    transcript
        .messages
        .into_iter()
        .map(|entry| TranscriptEntry {
            role: entry.role,
            text: entry.text,
            at_ms: entry.at_ms,
        })
        .collect()
}

/// Milliseconds since the Unix epoch: where a turn's frame numbers start.
fn clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// One prompt being answered on the other computer.
#[derive(uniffi::Object)]
pub struct Turn {
    pty: u64,
    handle: PtyHandle,
    engine: Arc<Mutex<Engine>>,
}

/// What a turn's thread waits for.
enum Input {
    Pty(PtyEvent),
    Tick,
}

impl Turn {
    /// Reads `stream` until the turn ends, on a thread of its own.
    fn follow(mut stream: PtyStream, engine: Engine, listener: Box<dyn TurnListener>) -> Arc<Self> {
        let engine = Arc::new(Mutex::new(engine));
        let turn = Arc::new(Self {
            pty: stream.pty,
            handle: stream.handle(),
            engine: engine.clone(),
        });
        // The terminal's events and the clock, in one queue. The task owns
        // the stream: when the thread below is done and stops listening,
        // the task ends and the terminal is let go of.
        let (inputs, queue) = std::sync::mpsc::channel();
        runtime().spawn(async move {
            let mut clock = tokio::time::interval(TURN_TICK);
            loop {
                let input = tokio::select! {
                    event = stream.events.recv() => match event {
                        Some(event) => Input::Pty(event),
                        // The stream ends without a last event when the
                        // session closes.
                        None => break,
                    },
                    _ = clock.tick() => Input::Tick,
                };
                if inputs.send(input).is_err() {
                    break;
                }
            }
        });
        let handle = turn.handle.clone();
        runtime().spawn_blocking(move || {
            while let Ok(input) = queue.recv() {
                let now = Instant::now();
                let (effects, last) = {
                    let Ok(mut engine) = engine.lock() else { break };
                    match input {
                        Input::Tick => (engine.tick(now), false),
                        Input::Pty(PtyEvent::Data(bytes)) => (engine.data(&bytes, now), false),
                        Input::Pty(PtyEvent::Gap) => (engine.gap(now), false),
                        Input::Pty(PtyEvent::Exited(exit)) => (engine.exited(exit.code), true),
                        Input::Pty(PtyEvent::Gone) => {
                            let mut effects = engine.exited(0);
                            effects.events.retain(|event| {
                                matches!(event, leon_turn::TurnEvent::QuestionResolved { .. })
                            });
                            effects.events.push(leon_turn::TurnEvent::Gone);
                            (effects, true)
                        }
                        Input::Pty(PtyEvent::Disconnected) => (engine.disconnected(), false),
                        Input::Pty(PtyEvent::Reconnected) => (engine.reconnected(now), false),
                        // The answer to a probe this side never sends.
                        Input::Pty(PtyEvent::Foreground(_)) => continue,
                    }
                };
                let Effects { events, write } = effects;
                for bytes in write {
                    handle.write(bytes);
                }
                for event in events.into_iter().filter_map(TurnEvent::crossing) {
                    listener.on_event(event);
                }
                if last {
                    break;
                }
            }
        });
        turn
    }

    fn act(&self, action: Action) -> Result<(), MobileError> {
        let effects = self
            .engine
            .lock()
            .map_err(|_| MobileError::Internal {
                message: "the turn's state is damaged".to_owned(),
            })?
            .act(action, Instant::now())?;
        for bytes in effects.write {
            self.handle.write(bytes);
        }
        Ok(())
    }
}

#[uniffi::export]
impl Turn {
    /// The turn's terminal on the host: what [`HostSession::attach_turn`]
    /// takes to follow it again.
    pub fn pty_id(&self) -> u64 {
        self.pty
    }

    /// Answers the question with this id ([`TurnEvent::Question`]). The
    /// answer is kept until the agent has it, across a dropped connection,
    /// and [`TurnEvent::QuestionResolved`] says when it does; until then it
    /// may be answered again and the later answer stands.
    pub fn answer(&self, question_id: String, answer: Answer) -> Result<(), MobileError> {
        self.act(Action::Answer(question_id, answer.into()))
    }

    /// Tells the agent more while it works. It reads it when it gets to it;
    /// the turn then ends after the answer to this, too.
    pub fn send(&self, text: String) -> Result<(), MobileError> {
        self.act(Action::Prompt(text))
    }

    /// Asks the agent to stop working. It keeps the conversation and ends
    /// the turn itself ([`TurnEvent::Finished`], `interrupted`). An agent
    /// that cannot be asked ([`MobileError::NotSupported`]) can only be
    /// stopped with [`Turn::cancel`].
    pub fn interrupt(&self) -> Result<(), MobileError> {
        self.act(Action::Interrupt)
    }

    /// Changes the agent's permission mode from now on
    /// ([`TurnEvent::ModeChanged`] confirms).
    pub fn set_mode(&self, mode: String) -> Result<(), MobileError> {
        self.act(Action::SetMode(mode))
    }

    /// Changes the model that answers from now on.
    pub fn set_model(&self, model: String) -> Result<(), MobileError> {
        self.act(Action::SetModel(model))
    }

    /// How many messages (answers, more to say) the agent does not have
    /// yet: written while offline, or on their way.
    pub fn unsent(&self) -> u32 {
        self.engine
            .lock()
            .map_or(0, |engine| engine.unsent() as u32)
    }

    /// Kills the agent: its terminal is hung up, whatever it was doing. The
    /// listener still receives [`TurnEvent::Exited`]. Without effect while
    /// disconnected.
    pub fn cancel(&self) {
        self.handle.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(path: &str, branch: Option<&str>, main: bool) -> Worktree {
        Worktree {
            path: path.to_owned(),
            branch: branch.map(str::to_owned),
            main,
        }
    }

    #[test]
    fn each_folder_gets_the_worktrees_git_listed_for_it() {
        let text = "leon-tree:0:worktree /home/ana/code/leon\n\
                    leon-tree:0:HEAD 2f1c\n\
                    leon-tree:0:branch refs/heads/main\n\
                    leon-tree:0:\n\
                    leon-tree:0:worktree /home/ana/trees/fix\n\
                    leon-tree:0:HEAD 9a0b\n\
                    leon-tree:0:branch refs/heads/feat/fix-login\n\
                    leon-tree:0:\n\
                    leon-tree:2:worktree /srv/api\n\
                    leon-tree:2:HEAD 77aa\n\
                    leon-tree:2:branch refs/heads/trunk\n";
        assert_eq!(
            parse_worktrees(text, 3),
            [
                vec![
                    tree("/home/ana/code/leon", Some("main"), true),
                    tree("/home/ana/trees/fix", Some("feat/fix-login"), false),
                ],
                // The second folder is in no repository: git said nothing.
                vec![],
                vec![tree("/srv/api", Some("trunk"), true)],
            ]
        );
    }

    #[test]
    fn a_detached_head_has_no_branch_and_a_bare_entry_is_left_out() {
        let text = "leon-tree:0:worktree /srv/git/leon.git\nleon-tree:0:bare\nleon-tree:0:\n\
                    leon-tree:0:worktree /srv/trees/review\nleon-tree:0:HEAD 9a0b\n\
                    leon-tree:0:detached\n";
        assert_eq!(
            parse_worktrees(text, 1),
            [vec![tree("/srv/trees/review", None, false)]]
        );
    }

    #[test]
    fn what_a_login_shell_prints_first_is_skipped() {
        let text = "Welcome back\r\nleon-tree: not ours\nworktree /not/ours\n\
                    leon-tree:0:worktree C:/code/leon\r\nleon-tree:0:branch refs/heads/main\r\n";
        assert_eq!(
            parse_worktrees(text, 1),
            [vec![tree("C:/code/leon", Some("main"), true)]]
        );
    }

    #[test]
    fn a_line_numbered_past_the_list_is_ignored() {
        let text = "leon-tree:7:worktree /srv/other\n";
        assert_eq!(parse_worktrees(text, 2), [vec![], vec![]]);
    }
}
