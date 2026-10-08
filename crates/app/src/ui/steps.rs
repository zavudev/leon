//! The questions the palette asks, as data.
//!
//! A command that needs more than a keystroke (a new worktree, a new machine)
//! is a *flow*: a list of questions asked one after the other in the palette,
//! ending in an [`Action`]. [`advance`] is the whole flow as one pure
//! function: given the command, the answers so far and a snapshot of the
//! world, it says what to ask next, what to do, or why it cannot be done. The
//! palette only renders the question and feeds the answer back.

use leon_core::cleanup::{self, Blocker};
use leon_core::{
    Account, AgentId, AgentSpec, CustomAgent, Machine, MachineId, MachineKind, Project, ProjectId,
    SessionId, Worktree, WorktreeId, WorktreeStatus,
};
use std::collections::HashMap;

use super::den_store::DenChoice;
use super::live::LiveId;
use super::model::Snapshot;
use super::tree::{worktree_label, Folder, Placement};
use crate::address;
use crate::engine::Op;
use crate::format;
use crate::keys::{self, Command};
use crate::schema::{Def, Kind, Value};
use crate::settings::AppearanceChoice;
use crate::theme::{self, ThemeId};

/// A project with the machine it lives on and its worktrees.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// The project.
    pub project: Project,
    /// The name of its machine.
    pub machine_name: String,
    /// Its worktrees.
    pub worktrees: Vec<Worktree>,
    /// Whether its agent memory was turned on, as far as Leon recorded.
    pub memory_on: bool,
}

/// A folder where sessions ran that no project contains: a project nobody has
/// opened yet, as far as anybody knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    /// The machine it is on.
    pub machine: MachineId,
    /// The folder.
    pub cwd: String,
    /// How many sessions ran in it.
    pub sessions: usize,
}

/// Where the keyboard is: what "a new session" or "a new worktree" means
/// without being told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Where {
    /// The machine.
    pub machine: MachineId,
    /// Its name.
    pub machine_name: String,
    /// The project, when the keyboard is on one or on one of its worktrees.
    pub project: Option<ProjectId>,
    /// The folder a session would start in.
    pub cwd: String,
}

/// A live session as the flows need to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveInfo {
    /// Its identity.
    pub id: LiveId,
    /// What it is called.
    pub label: String,
    /// Whether closing it would end a program that is running in it.
    pub busy: bool,
    /// Whether its terminal is held by the keeper of durable sessions, so a
    /// plain quit leaves its program running.
    pub keeps: bool,
}

/// The lion selected in the Den, as the flows need to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LionInfo {
    /// Its name, as the Den writes it.
    pub name: String,
    /// Its session's terminal; `None` for a session that runs elsewhere.
    pub live: Option<LiveId>,
    /// Why nothing can be done to it from here, for a session that runs
    /// elsewhere.
    pub elsewhere: Option<String>,
    /// Why it cannot be sent a message as it is.
    pub no_message: Option<String>,
    /// The messages that wait for it, oldest first.
    pub queued: Vec<String>,
    /// The folder its session runs in, when it has a terminal here.
    pub cwd: Option<String>,
}

/// Whether a lion of the pride can be sent a message, as the flows need to
/// know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Post {
    /// Its agent waits at its prompt: a message is typed at once.
    Now,
    /// It is busy: a message waits for it.
    Later,
    /// It cannot be messaged, and why.
    Never(String),
}

/// A lion of the Den, as the flows that are about several need to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrideLion {
    /// The lion's id.
    pub id: u64,
    /// Its name, as the Den writes it.
    pub name: String,
    /// What it does, in the words of its truth card.
    pub state: String,
    /// Where it runs: its folder, or its project.
    pub place: String,
    /// Whether it needs the user.
    pub needs: bool,
    /// Whether it can be sent a message; `None` for a lion without a
    /// terminal here.
    pub post: Option<Post>,
}

/// The Den, as the flows need to know it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrideInfo {
    /// Whether the Den is what the main pane shows.
    pub open: bool,
    /// Its lions, the little ones left out, in the order of its roster.
    pub lions: Vec<PrideLion>,
    /// The sessions that were sent home: what wakes each, and its name.
    pub home: Vec<(String, String)>,
}

/// The file the keyboard is on, as the flows need to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Its leaf.
    pub id: LiveId,
    /// Its name.
    pub name: String,
    /// Whether it has changes that were not saved.
    pub dirty: bool,
    /// Whether a save found it changed by somebody else.
    pub conflict: bool,
}

/// The history session a "resume in…" is about, and where it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTarget {
    /// The session.
    pub session: SessionId,
    /// The machine it ran on.
    pub machine: MachineId,
    /// The folder it ran in.
    pub cwd: String,
    /// The project that folder belongs to, when one does.
    pub project: Option<ProjectId>,
}

/// A history session that runs in another terminal, and what is known of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElsewhereTarget {
    /// The session.
    pub session: SessionId,
    /// Its title.
    pub title: String,
    /// The pid of the process that holds it.
    pub pid: u32,
    /// Whether that is only the best fit.
    pub likely: bool,
    /// Whether another Leon runs it (else a plain terminal does).
    pub other_leon: bool,
    /// Whether Leon can ask that process to end: on this computer, on a
    /// system with signals.
    pub can_take_over: bool,
}

/// A new session that waits for the person to say which account of its agent it
/// starts with: the agent, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountAsk {
    /// The agent about to start.
    pub agent: AgentId,
    /// The name of the machine it starts on.
    pub machine_name: String,
    /// The folder it starts in.
    pub cwd: String,
}

/// What a rename would rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTarget {
    /// An SSH machine, with its name now.
    Machine(MachineId, String),
    /// A project, with its name now.
    Project(ProjectId, String),
    /// A live terminal, with its label now.
    Live(LiveId, String),
    /// A session of the history, with its title now, and the live terminal
    /// that runs it when there is one.
    Session(SessionId, String, Option<LiveId>),
}

impl RenameTarget {
    /// The name it has now.
    pub fn current(&self) -> &str {
        match self {
            Self::Machine(_, name)
            | Self::Project(_, name)
            | Self::Live(_, name)
            | Self::Session(_, name, _) => name,
        }
    }
}

/// When quitting asks first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuitConfirm {
    /// Only while a program runs in a terminal.
    #[default]
    Running,
    /// Every time.
    Always,
    /// Never.
    Never,
}

impl QuitConfirm {
    /// Reads the setting's value.
    pub fn parse(text: &str) -> Self {
        match text {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::Running,
        }
    }

    /// Whether quitting asks, given how many sessions have a program running.
    pub fn asks(self, busy: usize) -> bool {
        match self {
            Self::Running => busy > 0,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// What the settings change in the questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    /// Which agents are offered for a new session.
    pub agents_enabled: Vec<AgentId>,
    /// The agent a new session starts without asking.
    pub default_agent: Option<AgentId>,
    /// Whether closing a pane with a program running asks first.
    pub confirm_close: bool,
    /// When quitting asks.
    pub quit: QuitConfirm,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            agents_enabled: leon_core::agent::all().iter().map(|spec| spec.id).collect(),
            default_agent: None,
            confirm_close: true,
            quit: QuitConfirm::Running,
        }
    }
}

impl Prefs {
    /// The agents offered for a new session, in the catalogue's order.
    pub fn offered(&self) -> Vec<AgentId> {
        leon_core::agent::all()
            .iter()
            .map(|spec| spec.id)
            .filter(|id| self.agents_enabled.contains(id))
            .collect()
    }
}

/// Everything [`advance`] needs to know.
#[derive(Debug, Clone)]
pub struct World {
    /// Every machine, the local one first.
    pub machines: Vec<Machine>,
    /// The machine on screen.
    pub selected: Option<MachineId>,
    /// Every project of every machine.
    pub projects: Vec<ProjectInfo>,
    /// The folders of the unsorted sessions, newest first.
    pub folders: Vec<FolderInfo>,
    /// Where the keyboard is.
    pub here: Option<Where>,
    /// The live sessions.
    pub live: Vec<LiveInfo>,
    /// The live session the keyboard is on or that is on screen.
    pub here_live: Option<LiveId>,
    /// The lion selected in the Den, while the Den has the keyboard.
    pub lion: Option<LionInfo>,
    /// The Den and its lions.
    pub pride: PrideInfo,
    /// The file the keyboard is on.
    pub file: Option<FileInfo>,
    /// The names of the files with changes that were not saved.
    pub unsaved: Vec<String>,
    /// The machine the cursor is on, when it is on a machine's row.
    pub machine_row: Option<MachineId>,
    /// The history session the cursor is on, with its title.
    pub here_session: Option<(SessionId, String)>,
    /// What time it is and the person's distance from UTC, for the questions
    /// that take a time.
    pub clock: Option<(chrono::DateTime<chrono::Utc>, chrono::FixedOffset)>,
    /// What "rename" would rename.
    pub rename: Option<RenameTarget>,
    /// The history session "resume in…" is about.
    pub resume: Option<ResumeTarget>,
    /// The session the keyboard is on, when it runs in another terminal.
    pub elsewhere: Option<ElsewhereTarget>,
    /// The appearance in use.
    pub theme: AppearanceChoice,
    /// The theme in use (kept, not previewed).
    pub theme_id: ThemeId,
    /// The interface size in use, in percent.
    pub scale: u16,
    /// The dens there are to choose: the built-in ones, then the user's.
    pub dens: Vec<DenChoice>,
    /// The id of the den in use.
    pub den_id: String,
    /// What the settings change in the questions.
    pub prefs: Prefs,
    /// Git repositories found on the machines (by the Connect screen), by
    /// machine: offered as folders when a project is added.
    pub repositories: Vec<(MachineId, String)>,
    /// The agents each machine has, where that is known: from this
    /// computer's own search, or the probe of a machine. A machine that is
    /// not in the list has not been looked at.
    pub installed: Vec<(MachineId, Vec<AgentId>)>,
    /// The version of the update that is downloaded and waiting for a restart.
    pub update_ready: Option<String>,
    /// What was last read about the checkout of each worktree.
    pub statuses: HashMap<WorktreeId, WorktreeStatus>,
    /// How many live sessions run in the folder of each worktree.
    pub running: HashMap<WorktreeId, usize>,
    /// The scripts of the project in view, when there is one.
    pub scripts: Option<ScriptsView>,
    /// The user's accounts of the agents.
    pub accounts: Vec<Account>,
    /// The default-account lines of the settings (`agent=name`).
    pub default_accounts: Vec<String>,
    /// A new session that waits for its account: when set, the flow of a new
    /// session asks only which account.
    pub account_ask: Option<AccountAsk>,
}

/// The scripts of the project the keyboard is in, and where one would run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptsView {
    /// The project.
    pub project: ProjectId,
    /// Its name.
    pub project_name: String,
    /// The machine it is on.
    pub machine: MachineId,
    /// The worktree (or the project folder) a script would run in.
    pub cwd: String,
    /// What the project's file says.
    pub file: ScriptsFile,
}

/// What is known of the project's `leon.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptsFile {
    /// It was not read (the machine is away).
    Unknown,
    /// There is no file.
    Absent,
    /// The file is wrong; the text names the line.
    Invalid(String),
    /// The scripts it lists.
    Scripts(Vec<ScriptEntry>),
}

/// One script as the question lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntry {
    /// Its name.
    pub name: String,
    /// Its command.
    pub command: String,
    /// The icon it asked for.
    pub icon: Option<String>,
    /// Its shortcut as the platform writes it, when it works.
    pub key: Option<String>,
    /// Why its shortcut does not work, when it was asked for and does not.
    pub key_note: Option<String>,
}

impl World {
    /// The machines, projects and unsorted folders of a snapshot.
    pub fn build(
        snapshot: &Snapshot,
        placement: &Placement,
        selected: Option<MachineId>,
        here: Option<Where>,
        theme: AppearanceChoice,
        theme_id: ThemeId,
        scale: u16,
    ) -> Self {
        let mut projects = Vec::new();
        let mut folders = Vec::new();
        for machine in &snapshot.machines {
            for entry in snapshot
                .projects
                .iter()
                .filter(|entry| entry.project.machine_id == machine.id)
            {
                projects.push(ProjectInfo {
                    project: entry.project.clone(),
                    machine_name: machine.name.clone(),
                    worktrees: entry.worktrees.clone(),
                    memory_on: entry.project.machine_id.is_local()
                        && snapshot
                            .memory
                            .get(&leon_core::path::key(&entry.project.root))
                            == Some(&leon_core::MemoryChoice::On),
                });
            }
            for Folder { cwd, sessions } in placement
                .unsorted
                .get(&machine.id)
                .map_or(&[][..], Vec::as_slice)
            {
                folders.push(FolderInfo {
                    machine: machine.id.clone(),
                    cwd: cwd.clone(),
                    sessions: sessions.len(),
                });
            }
        }
        Self {
            machines: snapshot.machines.clone(),
            selected,
            projects,
            folders,
            here,
            live: Vec::new(),
            here_live: None,
            lion: None,
            pride: PrideInfo::default(),
            file: None,
            unsaved: Vec::new(),
            machine_row: None,
            here_session: None,
            clock: None,
            rename: None,
            resume: None,
            elsewhere: None,
            theme,
            theme_id,
            scale,
            dens: Vec::new(),
            den_id: String::new(),
            prefs: Prefs::default(),
            repositories: Vec::new(),
            installed: Vec::new(),
            update_ready: None,
            statuses: HashMap::new(),
            running: HashMap::new(),
            scripts: None,
            accounts: Vec::new(),
            default_accounts: Vec::new(),
            account_ask: None,
        }
    }

    /// The same world knowing the user's accounts and the default-account
    /// lines of the settings.
    pub fn with_accounts(mut self, accounts: Vec<Account>, defaults: Vec<String>) -> Self {
        self.accounts = accounts;
        self.default_accounts = defaults;
        self
    }

    /// The same world knowing a new session waits for its account.
    pub fn with_account_ask(mut self, ask: Option<AccountAsk>) -> Self {
        self.account_ask = ask;
        self
    }

    /// The same world knowing what was read about each checkout and how many
    /// sessions run in each worktree's folder.
    pub fn with_checkouts(
        mut self,
        statuses: HashMap<WorktreeId, WorktreeStatus>,
        running: HashMap<WorktreeId, usize>,
    ) -> Self {
        self.statuses = statuses;
        self.running = running;
        self
    }

    /// The same world knowing the scripts of the project in view.
    pub fn with_scripts(mut self, scripts: Option<ScriptsView>) -> Self {
        self.scripts = scripts;
        self
    }

    /// The same world knowing what time it is, and the person's distance from
    /// UTC.
    pub fn with_clock(
        mut self,
        now: chrono::DateTime<chrono::Utc>,
        offset: chrono::FixedOffset,
    ) -> Self {
        self.clock = Some((now, offset));
        self
    }

    /// The same world knowing the dens there are to choose and the one in
    /// use.
    pub fn with_dens(mut self, dens: Vec<DenChoice>, current: String) -> Self {
        self.dens = dens;
        self.den_id = current;
        self
    }

    /// The den in use.
    pub fn den(&self) -> Option<&DenChoice> {
        self.dens.iter().find(|den| den.id == self.den_id)
    }

    /// The same world knowing which update is waiting for a restart.
    pub fn with_update_ready(mut self, version: Option<String>) -> Self {
        self.update_ready = version;
        self
    }

    /// The same world knowing the repositories found on the machines.
    pub fn with_repositories(mut self, repositories: Vec<(MachineId, String)>) -> Self {
        self.repositories = repositories;
        self
    }

    /// The same world knowing which agents the machines have.
    pub fn with_installed(mut self, installed: Vec<(MachineId, Vec<AgentId>)>) -> Self {
        self.installed = installed;
        self
    }

    /// The agents `machine` has, when that is known.
    pub fn installed_on(&self, machine: &MachineId) -> Option<&[AgentId]> {
        self.installed
            .iter()
            .find(|(id, _)| id == machine)
            .map(|(_, agents)| agents.as_slice())
    }

    /// The same world knowing what the settings change.
    pub fn with_prefs(mut self, prefs: Prefs) -> Self {
        self.prefs = prefs;
        self
    }

    /// The same world knowing the live sessions and which one is on screen.
    pub fn with_live(mut self, live: Vec<LiveInfo>, here_live: Option<LiveId>) -> Self {
        self.live = live;
        self.here_live = here_live;
        self
    }

    /// The same world knowing the lion selected in the Den.
    pub fn with_lion(mut self, lion: Option<LionInfo>) -> Self {
        self.lion = lion;
        self
    }

    /// The same world knowing the Den and its lions.
    pub fn with_pride(mut self, pride: PrideInfo) -> Self {
        self.pride = pride;
        self
    }

    /// The same world knowing the file the keyboard is on and which files
    /// have unsaved changes.
    pub fn with_files(mut self, file: Option<FileInfo>, unsaved: Vec<String>) -> Self {
        self.file = file;
        self.unsaved = unsaved;
        self
    }

    /// The same world knowing what the cursor is on.
    pub fn with_targets(
        mut self,
        machine_row: Option<MachineId>,
        here_session: Option<(SessionId, String)>,
        rename: Option<RenameTarget>,
    ) -> Self {
        self.machine_row = machine_row;
        self.here_session = here_session;
        self.rename = rename;
        self
    }

    /// The same world knowing which history session "resume in…" is about.
    pub fn with_resume(mut self, resume: Option<ResumeTarget>) -> Self {
        self.resume = resume;
        self
    }

    /// The same world knowing the session the keyboard is on runs elsewhere.
    pub fn with_elsewhere(mut self, elsewhere: Option<ElsewhereTarget>) -> Self {
        self.elsewhere = elsewhere;
        self
    }

    /// Where a new session in the worktree with this id would go.
    pub fn worktree_target(&self, worktree: &str) -> Option<Where> {
        self.projects.iter().find_map(|info| {
            info.worktrees
                .iter()
                .find(|candidate| candidate.id.as_str() == worktree)
                .map(|candidate| Where {
                    machine: info.project.machine_id.clone(),
                    machine_name: info.machine_name.clone(),
                    project: Some(info.project.id.clone()),
                    cwd: candidate.path.clone(),
                })
        })
    }
}

/// One answer on offer in a choice step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What is shown.
    pub label: String,
    /// Dim text beside it.
    pub detail: String,
    /// What the answer is, as the flow reads it back.
    pub value: String,
    /// Marks the value in use.
    pub current: bool,
    /// The agent the choice stands for, drawn as its logo.
    pub agent: Option<AgentId>,
    /// The theme the choice stands for, drawn as a row of swatches.
    pub swatch: Option<ThemeId>,
    /// Whether the choice is drawn dimmed: it can be picked but will say why
    /// it cannot be done (an agent that is not installed).
    pub dim: bool,
    /// The name of the icon that leads the row (one of `icons::SCRIPT_ICONS`).
    pub icon: Option<String>,
    /// The chord written at the end of the row, as the platform writes it.
    pub keys: Option<String>,
}

impl Choice {
    fn new(label: impl Into<String>, detail: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            value: value.into(),
            current: false,
            agent: None,
            swatch: None,
            dim: false,
            icon: None,
            keys: None,
        }
    }

    fn dim(mut self, dim: bool) -> Self {
        self.dim = dim;
        self
    }

    fn swatch(mut self, theme: ThemeId) -> Self {
        self.swatch = Some(theme);
        self
    }

    fn agent(mut self, agent: AgentId) -> Self {
        self.agent = Some(agent);
        self
    }

    fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
}

/// How typed text is checked before it is an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validate {
    /// Anything but nothing.
    Required,
    /// A message for an agent: anything but nothing, and it may have
    /// several lines, which are kept (the palette takes a line with
    /// Shift+Enter).
    Message,
    /// Anything, nothing included.
    Optional,
    /// A branch name git accepts.
    Branch,
    /// A prompt that can be typed into a terminal (see
    /// [`crate::launch::clean_prompt`]); the answer is the cleaned text.
    Prompt,
    /// A whole number from `min` to `max`.
    Number {
        /// The least.
        min: i64,
        /// The most.
        max: i64,
    },
}

/// Checks typed text, returning the answer or what is wrong with it.
pub fn validate(how: Validate, text: &str) -> Result<String, String> {
    let text = text.trim();
    match how {
        Validate::Optional => Ok(text.to_owned()),
        Validate::Required if text.is_empty() => Err("Type a name.".to_owned()),
        Validate::Required => Ok(text.to_owned()),
        Validate::Message if text.is_empty() => Err("Type a message.".to_owned()),
        Validate::Message => Ok(text.to_owned()),
        Validate::Branch => address::validate_branch(text)
            .map(|()| text.to_owned())
            .map_err(str::to_owned),
        Validate::Prompt => crate::launch::clean_prompt(text),
        Validate::Number { min, max } => text
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse::<i64>()
            .ok()
            .filter(|number| (min..=max).contains(number))
            .map(|number| number.to_string())
            .ok_or_else(|| format!("Use a number from {min} to {max}.")),
    }
}

/// Whether typed text that is not one of the choices is an answer too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Custom {
    /// No: only the choices.
    No,
    /// Any text.
    Any,
    /// A path that is absolute on the machine.
    AbsolutePath,
}

/// What a question wants as its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    /// Free text.
    Text {
        /// What the empty field says.
        placeholder: String,
        /// How the text is checked.
        validate: Validate,
    },
    /// One of a list. With `custom`, typed text that is not in the list is an
    /// answer too.
    Choices {
        /// The answers on offer.
        choices: Vec<Choice>,
        /// Whether typed text may be the answer.
        custom: Custom,
    },
}

/// One question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What is being asked, in a word or two: shown beside the field.
    pub prompt: &'static str,
    /// What kind of answer it wants.
    pub kind: StepKind,
}

/// An agent session somebody asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIntent {
    /// The agent to start.
    pub agent: AgentId,
    /// The machine to start it on.
    pub machine: MachineId,
    /// That machine's name.
    pub machine_name: String,
    /// The folder to start it in.
    pub cwd: String,
    /// The id of the account to start it as; `None` is the agent's own setup.
    pub account: Option<String>,
}

/// What a finished flow does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Hand an operation to the engine.
    Engine(Op),
    /// Start an agent session.
    StartSession(SessionIntent),
    /// Give a live terminal a name.
    RenameLive(LiveId, String),
    /// Give a session a name of Leon's own, and its live terminal too (which
    /// also renames the session inside an agent that can be asked to).
    RenameSession(SessionId, Option<LiveId>, String),
    /// Close a live session for good: its terminal ends and its row leaves
    /// the sidebar, with the history session that belongs to it.
    CloseLive(LiveId),
    /// Put a live session to sleep: its terminal ends, and the session stays
    /// in the sidebar to be resumed.
    SleepLive(LiveId),
    /// Hide a session on the Snoozed shelf until this time.
    Snooze(SessionId, chrono::DateTime<chrono::Utc>),
    /// Send a lion home from the Den: the whole session is put to sleep.
    SendHome(LiveId),
    /// Type a message into a live session as a prompt, now or when its
    /// agent next waits.
    MessageLive(LiveId, String),
    /// Type one message into several live sessions, each now or when its
    /// agent next waits.
    MessagePride(Vec<LiveId>, String),
    /// Take back a message that waits for a live session (its place in the
    /// line, the oldest at 0), or all of them.
    CancelQueued(LiveId, Option<usize>),
    /// Select a lion in the Den.
    SelectLion(u64),
    /// Wake a session that was sent home: what the Den knows it by.
    WakeHome(String),
    /// Start an agent session from the Den and stay there: it joins as an
    /// egg.
    HatchSession(SessionIntent),
    /// Open the file at this path, as typed: relative to the folder the
    /// keyboard is in, or absolute.
    OpenFile(String),
    /// Save a file.
    SaveFile(LiveId),
    /// Save a file, then close it.
    SaveAndCloseFile(LiveId),
    /// Close a file, its changes thrown away.
    CloseFile(LiveId),
    /// Save every file with changes, then quit.
    SaveAllAndQuit,
    /// Quit without saving the files with changes; their drafts go too.
    DiscardAndQuit,
    /// Save a file over what somebody else wrote there.
    OverwriteFile(LiveId),
    /// Read a file again, the changes in the editor thrown away.
    ReloadFile(LiveId),
    /// Remove a worktree, its confirmation already answered. `force` passes
    /// `--force` to git, which deletes its uncommitted and untracked files:
    /// the window asks for it when git refused the worktree for them.
    RemoveWorktree {
        /// The project the worktree belongs to.
        project: ProjectId,
        /// The worktree to remove.
        worktree: WorktreeId,
        /// Whether its local changes may be deleted with it.
        force: bool,
    },
    /// Remove these worktrees, one after the other, as `git worktree remove`
    /// does: a worktree git refuses is kept and named, never forced.
    RemoveWorktrees(Vec<(ProjectId, WorktreeId)>),
    /// Open the changes of a worktree in a tab.
    ShowChanges {
        /// The project the worktree belongs to.
        project: ProjectId,
        /// The worktree.
        worktree: WorktreeId,
    },
    /// Resume a history session where it ran, once confirmed.
    ResumeSession(SessionId),
    /// Open the transcript of the session the keyboard is on.
    OpenTranscript,
    /// Resume a history session in another folder of its machine.
    ResumeIn(SessionId, String),
    /// Resume, in a terminal of Leon, a session that runs in another terminal.
    ResumeAnyway(SessionId),
    /// Ask the process that holds a session in another terminal to end, then
    /// resume the session in a terminal of Leon.
    TakeOver(SessionId),
    /// Use this den: a built-in one's id or the name of a file of the user's.
    SetDen(String),
    /// Save the Den's room as a new den with this name.
    SaveDenAs(String),
    /// Give the user's den in use this name.
    RenameDen(String),
    /// Delete the user's den in use.
    DeleteDen,
    /// Change the appearance: light, dark or the desktop's.
    SetAppearance(AppearanceChoice),
    /// Change the theme.
    SetThemeId(ThemeId),
    /// Change the interface size.
    SetScale(u16),
    /// Choose a value for a setting, by key.
    SetSetting(&'static str, Value),
    /// Press the button of a setting, by key.
    Button(&'static str),
    /// Quit the application.
    Quit,
    /// Quit and end every session, the held ones too: the old behaviour.
    QuitAndEnd,
    /// Save every file with changes, then quit and end every session.
    SaveAllEndAndQuit,
    /// Quit and end every session without saving the files with changes.
    DiscardEndAndQuit,
    /// Restart into the update that is ready.
    RestartToUpdate,
    /// Write a theme file with this name that extends the theme in use.
    NewTheme(String),
    /// Add an agent of the user's: name, command, arguments of a new session
    /// and of a resume.
    AddAgent {
        /// The name shown.
        name: String,
        /// The program.
        command: String,
        /// Arguments of a new session, as typed.
        args: String,
        /// Arguments that resume a session, as typed.
        resume_args: String,
        /// Arguments that give the agent a first prompt, as typed, with
        /// `{prompt}` for the prompt.
        prompt_args: String,
    },
    /// Remove an agent of the user's.
    RemoveAgent(AgentId),
    /// Add an account of an agent: its name and the variables as typed.
    AddAccount {
        /// The agent.
        agent: AgentId,
        /// The name shown.
        name: String,
        /// `NAME=value` words.
        variables: String,
    },
    /// Give an account another name.
    RenameAccount {
        /// The account's id.
        id: String,
        /// The new name.
        name: String,
    },
    /// Remove an account.
    RemoveAccount(String),
    /// Start the new session that waited for its account, as this one (an id;
    /// `None` is the agent's own setup).
    StartWithAccount(Option<String>),
    /// Clone `url` as `name`, asking this computer for the parent folder with
    /// the system's folder dialog.
    PickCloneParent {
        /// The repository to clone.
        url: String,
        /// The project (and folder) name.
        name: String,
    },
    /// Create the project `name`, asking this computer for the parent folder
    /// with the system's folder dialog.
    PickProjectParent {
        /// The project (and folder) name.
        name: String,
    },
    /// Run a script of the project's `leon.toml` in a new terminal tab.
    RunScript {
        /// The project the script belongs to.
        project: ProjectId,
        /// The machine it runs on.
        machine: MachineId,
        /// The folder it runs in.
        cwd: String,
        /// The script's name.
        name: String,
    },
    /// Give one prompt to several agents, each in a worktree of its own.
    PromptAgents {
        /// The project the worktrees are made in.
        project: ProjectId,
        /// The prompt, cleaned.
        prompt: String,
        /// The agents, two or more, in the order they were ticked.
        agents: Vec<AgentId>,
        /// What the branches start from; `None` is `HEAD`.
        base: Option<String>,
    },
    /// Do nothing: the person backed out.
    Nothing,
}

/// What comes after the answers so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Ask this next.
    Ask(Step),
    /// Do this.
    Run(Action),
    /// It cannot be done; say why.
    Refuse(String),
}

/// Whether a command is asked for in steps. (Restart to update is not: it is
/// run as a command, which decides whether there is anything to ask, and then
/// starts [`Command::RestartToUpdate`]'s questions itself. Neither is Run a
/// script: the project's file is read first, and then its questions start.)
pub fn is_flow(command: Command) -> bool {
    matches!(
        command,
        Command::NewSession
            | Command::AddAgent
            | Command::RemoveAgent
            | Command::AddAccount
            | Command::RenameAccount
            | Command::RemoveAccount
            | Command::CloseSession
            | Command::SleepSession
            | Command::SnoozeSession
            | Command::OpenFile
            | Command::SaveFile
            | Command::CloseFile
            | Command::Rename
            | Command::RemoveMachine
            | Command::RemoveFromHistory
            | Command::ResumeSession
            | Command::ResumeIn
            | Command::ResumeAnyway
            | Command::TakeOver
            | Command::NewWorktree
            | Command::PromptAgents
            | Command::AddProject
            | Command::CloneProject
            | Command::NewProject
            | Command::RemoveProject
            | Command::EnableMemory
            | Command::DisableMemory
            | Command::RemoveWorktree
            | Command::SetAppearance
            | Command::ChooseTheme
            | Command::SetInterfaceSize
            | Command::Quit
            | Command::QuitAndEnd
            | Command::NewThemeFromCurrent
            | Command::ChooseDen
            | Command::SaveDenAs
            | Command::RenameDen
            | Command::DeleteDen
            | Command::MessageLion
            | Command::SendLionHome
            | Command::MessagePride
            | Command::QueuedMessages
            | Command::HatchLion
            | Command::WakeLion
            | Command::GoToLion
    )
}

fn text(prompt: &'static str, placeholder: impl Into<String>, validate: Validate) -> Outcome {
    Outcome::Ask(Step {
        prompt,
        kind: StepKind::Text {
            placeholder: placeholder.into(),
            validate,
        },
    })
}

fn choices(prompt: &'static str, choices: Vec<Choice>, custom: Custom) -> Outcome {
    Outcome::Ask(Step {
        prompt,
        kind: StepKind::Choices { choices, custom },
    })
}

/// The next move of `command`'s flow, given the `answers` so far.
pub fn advance(command: Command, answers: &[String], world: &World) -> Outcome {
    match command {
        Command::NewSession => new_session(answers, world),
        Command::AddAgent => add_agent(answers),
        Command::RemoveAgent => remove_agent(answers),
        Command::AddAccount => add_account(answers, world),
        Command::RenameAccount => rename_account(answers, world),
        Command::RemoveAccount => remove_account(answers, world),
        Command::CloseSession => close_session(answers, world),
        Command::SleepSession => sleep_session(answers, world),
        Command::SnoozeSession => snooze_session(answers, world),
        Command::MessageLion => message_lion(answers, world),
        Command::SendLionHome => send_lion_home(answers, world),
        Command::MessagePride => message_pride(answers, world),
        Command::QueuedMessages => queued_messages(answers, world),
        Command::HatchLion => hatch_lion(answers, world),
        Command::WakeLion => wake_lion(answers, world),
        Command::GoToLion => go_to_lion(answers, world),
        Command::OpenFile => open_file(answers),
        Command::SaveFile => save_file(answers, world),
        Command::CloseFile => close_file(answers, world),
        Command::Rename => rename(answers, world),
        Command::RemoveMachine => remove_machine(answers, world),
        Command::RemoveFromHistory => remove_from_history(answers, world),
        Command::ResumeSession => resume_session(answers, world),
        Command::ResumeIn => resume_in(answers, world),
        Command::ResumeAnyway => resume_anyway(answers, world),
        Command::TakeOver => take_over(answers, world),
        Command::NewWorktree => new_worktree(answers, world),
        Command::PromptAgents => prompt_agents(answers, world),
        Command::RunScript => run_script(answers, world),
        Command::AddProject => add_project(answers, world),
        Command::CloneProject => clone_project(answers, world),
        Command::NewProject => new_project(answers, world),
        Command::RemoveProject => remove_project(answers, world),
        Command::EnableMemory => project_memory(answers, world, true),
        Command::DisableMemory => project_memory(answers, world, false),
        Command::RemoveWorktree => remove_worktree(answers, world),
        Command::RemoveMergedWorktrees => remove_merged_worktrees(answers, world),
        Command::OpenChanges => show_changes(answers, world),
        Command::SetAppearance => set_appearance(answers, world),
        Command::ChooseTheme => choose_theme(answers, world),
        Command::SetInterfaceSize => set_size(answers, world),
        Command::Quit => quit(answers, world),
        Command::QuitAndEnd => quit_and_end(answers, world),
        Command::RestartToUpdate => restart_update(answers, world),
        Command::NewThemeFromCurrent => match answers {
            [] => text("Theme name", "My theme", Validate::Required),
            [name, ..] => Outcome::Run(Action::NewTheme(name.clone())),
        },
        Command::ChooseDen => choose_den(answers, world),
        Command::SaveDenAs => match answers {
            [] => text("Den name", "My den", Validate::Required),
            [name, ..] => Outcome::Run(Action::SaveDenAs(name.clone())),
        },
        Command::RenameDen => match (world.den(), answers) {
            (Some(den), _) if !den.user => Outcome::Refuse(format!(
                "{} is built in: save it under a name first.",
                den.name
            )),
            (None, _) => Outcome::Refuse("There is no den in use.".to_owned()),
            (Some(den), []) => text("New name", den.name.clone(), Validate::Required),
            (Some(_), [name, ..]) => Outcome::Run(Action::RenameDen(name.clone())),
        },
        Command::DeleteDen => match (world.den(), answers) {
            (Some(den), _) if !den.user => {
                Outcome::Refuse(format!("{} is built in: it cannot be deleted.", den.name))
            }
            (None, _) => Outcome::Refuse("There is no den in use.".to_owned()),
            (Some(den), []) => choices(
                "Delete this den?",
                vec![
                    Choice::new(
                        format!("Delete {}", den.name),
                        "Its file is removed",
                        "delete",
                    ),
                    Choice::new("Keep it", "", "keep"),
                ],
                Custom::No,
            ),
            (Some(_), [answer, ..]) if answer == "delete" => Outcome::Run(Action::DeleteDen),
            (Some(_), _) => Outcome::Run(Action::Nothing),
        },
        _ => Outcome::Refuse(format!("{} has no steps.", keys::label(command))),
    }
}

/// The flow that adds an agent of the user's: any command line tool.
fn add_agent(answers: &[String]) -> Outcome {
    match answers {
        [] => text("Agent name", "My agent", Validate::Required),
        [name] => {
            // A name that cannot be used is said before the rest is asked.
            match CustomAgent::new(name, "x", "", "", &[]).and_then(|agent| {
                let names: Vec<&str> = leon_core::agent::all()
                    .iter()
                    .map(|spec| spec.name.as_str())
                    .collect();
                agent.to_spec(&names).map(|_| ())
            }) {
                Ok(()) => text("Command", "the program that starts it", Validate::Required),
                Err(error) => Outcome::Refuse(error.to_string()),
            }
        }
        [_, _] => text(
            "Arguments",
            "arguments of a new session (optional)",
            Validate::Optional,
        ),
        [_, _, _] => text(
            "Resume arguments",
            "e.g. --resume {id}, or --continue; empty: cannot be resumed",
            Validate::Optional,
        ),
        [name, command, args, resume_args] => {
            // What was typed so far is checked before the last question.
            match CustomAgent::new(name, command, args, resume_args, &[]) {
                Ok(_) => text(
                    "Prompt arguments",
                    "e.g. --prompt {prompt}; empty: not given a prompt when it starts",
                    Validate::Optional,
                ),
                Err(error) => Outcome::Refuse(error.to_string()),
            }
        }
        [name, command, args, resume_args, prompt_args, ..] => {
            let names: Vec<&str> = leon_core::agent::all()
                .iter()
                .map(|spec| spec.name.as_str())
                .collect();
            match CustomAgent::new(name, command, args, resume_args, &[])
                .and_then(|agent| agent.with_prompt_args(prompt_args))
                .and_then(|agent| agent.to_spec(&names))
            {
                Ok(_) => Outcome::Run(Action::AddAgent {
                    name: name.clone(),
                    command: command.clone(),
                    args: args.clone(),
                    resume_args: resume_args.clone(),
                    prompt_args: prompt_args.clone(),
                }),
                Err(error) => Outcome::Refuse(error.to_string()),
            }
        }
    }
}

/// The flow that removes an agent of the user's.
fn remove_agent(answers: &[String]) -> Outcome {
    let custom: Vec<&'static AgentSpec> = leon_core::agent::all()
        .into_iter()
        .filter(|spec| spec.custom)
        .collect();
    if custom.is_empty() {
        return Outcome::Refuse("You have not added any agent of your own.".to_owned());
    }
    match answers {
        [] => choices(
            "Agent",
            custom
                .iter()
                .map(|spec| {
                    Choice::new(spec.name.clone(), spec.command.clone(), spec.id.as_str())
                        .agent(spec.id)
                })
                .collect(),
            Custom::No,
        ),
        [id, ..] => match custom.iter().find(|spec| spec.id.as_str() == id) {
            Some(spec) => Outcome::Run(Action::RemoveAgent(spec.id)),
            None => Outcome::Refuse(format!("Unknown agent {id:?}.")),
        },
    }
}

fn new_session(answers: &[String], world: &World) -> Outcome {
    // A session that waits for its account asks only that.
    if let Some(ask) = &world.account_ask {
        return match answers {
            [] => account_choices(ask.agent, &ask.machine_name, &ask.cwd, world),
            [chosen, ..] => match chosen_account(ask.agent, chosen, world) {
                Some(account) => Outcome::Run(Action::StartWithAccount(account)),
                None => Outcome::Refuse(format!("Unknown account {chosen:?}.")),
            },
        };
    }
    // The worktree the keyboard is on; without one, it is the first question.
    let (target, rest): (Where, &[String]) = match &world.here {
        Some(here) => (here.clone(), answers),
        None => match answers {
            [] => return worktree_choices(world),
            [chosen, rest @ ..] => match world.worktree_target(chosen) {
                Some(target) => (target, rest),
                None => return Outcome::Refuse(format!("Unknown worktree {chosen:?}.")),
            },
        },
    };
    let offered = world.prefs.offered();
    if offered.is_empty() {
        return Outcome::Refuse("Every agent is turned off in the settings.".to_owned());
    }
    // The agent the settings name starts without a question.
    if let Some(agent) = world
        .prefs
        .default_agent
        .filter(|agent| offered.contains(agent))
    {
        if rest.is_empty() || rest.len() == 1 && account_is_asked(agent, world) {
            return start_as(agent, rest.first(), target, world);
        }
    }
    let installed = world.installed_on(&target.machine);
    let has = |agent: &AgentId| installed.is_none_or(|list| list.contains(agent));
    match rest {
        [] => {
            // What the machine has first, then the rest dimmed with where to
            // get it; the palette filters the list as the person types.
            let (have, lack): (Vec<AgentId>, Vec<AgentId>) =
                offered.iter().partition(|agent| has(agent));
            let place = format!("on {} in {}", target.machine_name, target.cwd);
            let mut list: Vec<Choice> = have
                .iter()
                .map(|agent| {
                    Choice::new(format::agent_name(*agent), place.clone(), agent.as_str())
                        .agent(*agent)
                })
                .collect();
            list.extend(lack.iter().map(|agent| {
                let docs = agent
                    .spec()
                    .and_then(|spec| spec.docs.clone())
                    .map_or(String::new(), |url| format!(" · {url}"));
                Choice::new(
                    format::agent_name(*agent),
                    format!("not installed on {}{docs}", target.machine_name),
                    agent.as_str(),
                )
                .agent(*agent)
                .dim(true)
            }));
            choices("Agent", list, Custom::No)
        }
        [agent, account @ ..] => {
            match AgentId::parse(agent).filter(|agent| offered.contains(agent)) {
                Some(agent) if !has(&agent) => {
                    let docs = agent
                        .spec()
                        .and_then(|spec| spec.docs.clone())
                        .map_or(String::new(), |url| format!(" Install it from {url}."));
                    Outcome::Refuse(format!(
                        "{} is not installed on {}.{docs}",
                        format::agent_name(agent),
                        target.machine_name
                    ))
                }
                Some(agent) => start_as(agent, account.first(), target, world),
                None => Outcome::Refuse(format!("Unknown agent {agent:?}.")),
            }
        }
    }
}

/// Whether a new session of `agent` has to be asked which account.
fn account_is_asked(agent: AgentId, world: &World) -> bool {
    matches!(
        leon_core::account::resolve(agent, &world.accounts, &world.default_accounts),
        leon_core::account::Pick::Ask(_)
    )
}

/// The session of `agent` at `target`, as the account the settings name, or as
/// the one answered (`asked`), or the question when neither is there.
fn start_as(agent: AgentId, asked: Option<&String>, target: Where, world: &World) -> Outcome {
    use leon_core::account::{resolve, Pick};
    let intent = |account: Option<String>| {
        Outcome::Run(Action::StartSession(SessionIntent {
            agent,
            machine: target.machine.clone(),
            machine_name: target.machine_name.clone(),
            cwd: target.cwd.clone(),
            account,
        }))
    };
    match resolve(agent, &world.accounts, &world.default_accounts) {
        Pick::Plain => intent(None),
        Pick::Account(account) => intent(Some(account.id.clone())),
        Pick::Ask(_) => match asked {
            None => account_choices(agent, &target.machine_name, &target.cwd, world),
            Some(chosen) => match chosen_account(agent, chosen, world) {
                Some(account) => intent(account),
                None => Outcome::Refuse(format!("Unknown account {chosen:?}.")),
            },
        },
    }
}

/// The question "which account": the agent's own setup first, then the
/// accounts by name.
fn account_choices(agent: AgentId, machine_name: &str, cwd: &str, world: &World) -> Outcome {
    let place = format!("on {machine_name} in {cwd}");
    let mut list = vec![Choice::new(
        format!("{} (default)", format::agent_name(agent)),
        format!("the agent's own setup · {place}"),
        leon_core::account::DEFAULT_NAME,
    )
    .agent(agent)];
    list.extend(
        leon_core::account::of_agent(&world.accounts, agent)
            .into_iter()
            .map(|account| {
                let detail = match account.config_dir() {
                    Some(folder) => format!("{folder} · {place}"),
                    None => place.clone(),
                };
                Choice::new(
                    format!("{} ({})", format::agent_name(agent), account.name),
                    detail,
                    account.id.as_str(),
                )
                .agent(agent)
            }),
    );
    choices("Account", list, Custom::No)
}

/// The account an answer to [`account_choices`] stands for: `Some(None)` is the
/// agent's own setup, `None` is an answer that is not on offer.
fn chosen_account(agent: AgentId, answer: &str, world: &World) -> Option<Option<String>> {
    if answer == leon_core::account::DEFAULT_NAME {
        return Some(None);
    }
    leon_core::account::of_agent(&world.accounts, agent)
        .into_iter()
        .find(|account| account.id == answer)
        .map(|account| Some(account.id.clone()))
}

/// The flow that adds an account of an agent: which agent, a name, and the
/// variables that make the agent another account.
fn add_account(answers: &[String], world: &World) -> Outcome {
    let offered = world.prefs.offered();
    match answers {
        [] => choices(
            "Agent",
            offered
                .iter()
                .map(|agent| {
                    let detail = match agent.spec().and_then(|spec| spec.config_env.as_deref()) {
                        Some(variable) => format!("{variable} moves its configuration folder"),
                        None => {
                            "sets any variable; its history and limits are not read per account"
                                .to_owned()
                        }
                    };
                    Choice::new(format::agent_name(*agent), detail, agent.as_str()).agent(*agent)
                })
                .collect(),
            Custom::No,
        ),
        [agent] => match AgentId::parse(agent).filter(|agent| offered.contains(agent)) {
            Some(_) => text("Account name", "Work", Validate::Required),
            None => Outcome::Refuse(format!("Unknown agent {agent:?}.")),
        },
        [agent, name] => {
            let Some(agent) = AgentId::parse(agent).filter(|agent| offered.contains(agent)) else {
                return Outcome::Refuse(format!("Unknown agent {agent:?}."));
            };
            // A name that cannot be used is said before the rest is asked.
            if let Err(error) = Account::new(agent, name, Vec::new(), &world.accounts) {
                return Outcome::Refuse(error.to_string());
            }
            let placeholder = match agent.spec().and_then(|spec| spec.config_env.as_deref()) {
                Some(variable) => format!(
                    "{variable}=~/.{}-work; empty keeps the agent's own setup",
                    agent.as_str()
                ),
                None => "NAME=value words (their history and limits are not read per account)"
                    .to_owned(),
            };
            text("Variables", placeholder, Validate::Optional)
        }
        [agent, name, variables, ..] => {
            let Some(agent) = AgentId::parse(agent).filter(|agent| offered.contains(agent)) else {
                return Outcome::Refuse(format!("Unknown agent {agent:?}."));
            };
            let checked = leon_core::account::parse_variables(variables)
                .and_then(|env| Account::new(agent, name, env, &world.accounts).map(|_| ()));
            match checked {
                Ok(()) => Outcome::Run(Action::AddAccount {
                    agent,
                    name: name.clone(),
                    variables: variables.clone(),
                }),
                Err(error) => Outcome::Refuse(error.to_string()),
            }
        }
    }
}

/// What the status line says when an account was added: what it is for, and
/// what Leon can and cannot do per account for its agent.
pub fn added_note(account: &Account) -> String {
    let agent = format::agent_name(account.agent);
    let read = match account.agent.spec().and_then(|spec| spec.config_env.as_deref()) {
        Some(variable) if account.value_of(variable).is_some() => {
            "its sessions are read from its own folder and its limits get a line of their own (on this computer).".to_owned()
        }
        Some(variable) => format!(
            "set {variable} on it to give it a folder of its own: without it, its sessions and limits are the {agent}'s own."
        ),
        None => format!(
            "{agent}'s history and limits are not read per account: only the launch uses the account."
        ),
    };
    format!("The account {} of {agent} was added: {read}", account.name)
}

/// The accounts as choices of a flow that acts on one of them.
fn account_list(world: &World) -> Vec<Choice> {
    world
        .accounts
        .iter()
        .map(|account| {
            let detail = if account.env.is_empty() {
                "no variables".to_owned()
            } else {
                leon_core::account::variables_line(&account.env)
            };
            Choice::new(
                format!("{} ({})", format::agent_name(account.agent), account.name),
                detail,
                account.id.as_str(),
            )
            .agent(account.agent)
        })
        .collect()
}

/// The flow that gives an account another name.
fn rename_account(answers: &[String], world: &World) -> Outcome {
    if world.accounts.is_empty() {
        return Outcome::Refuse("You have not added any account.".to_owned());
    }
    match answers {
        [] => choices("Account", account_list(world), Custom::No),
        [id] => match leon_core::account::by_id(&world.accounts, id) {
            Some(account) => text("New name", account.name.clone(), Validate::Required),
            None => Outcome::Refuse(format!("Unknown account {id:?}.")),
        },
        [id, name, ..] => match leon_core::account::by_id(&world.accounts, id) {
            Some(account) => {
                let mut renamed = account.clone();
                renamed.name = name.trim().to_owned();
                match renamed.check(&world.accounts) {
                    Ok(()) => Outcome::Run(Action::RenameAccount {
                        id: id.clone(),
                        name: name.clone(),
                    }),
                    Err(error) => Outcome::Refuse(error.to_string()),
                }
            }
            None => Outcome::Refuse(format!("Unknown account {id:?}.")),
        },
    }
}

/// The flow that removes an account, after asking.
fn remove_account(answers: &[String], world: &World) -> Outcome {
    if world.accounts.is_empty() {
        return Outcome::Refuse("You have not added any account.".to_owned());
    }
    match answers {
        [] => choices("Account", account_list(world), Custom::No),
        [id] => match leon_core::account::by_id(&world.accounts, id) {
            Some(account) => choices(
                "Confirm",
                vec![
                    Choice::new(
                        format!("Remove {}", account.name),
                        "the sessions that ran with it stay in the history, but cannot be resumed until it is added again with the same name",
                        "yes",
                    ),
                    Choice::new("Cancel", "keep it", "no"),
                ],
                Custom::No,
            ),
            None => Outcome::Refuse(format!("Unknown account {id:?}.")),
        },
        [id, confirmed, ..] if confirmed == "yes" => {
            match leon_core::account::by_id(&world.accounts, id) {
                Some(_) => Outcome::Run(Action::RemoveAccount(id.clone())),
                None => Outcome::Refuse(format!("Unknown account {id:?}.")),
            }
        }
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Every worktree of every project, for a session that has nowhere to start
/// yet.
/// The flow that runs a script of the project in view.
fn run_script(answers: &[String], world: &World) -> Outcome {
    let Some(view) = &world.scripts else {
        return Outcome::Refuse("Select a project or a worktree first.".to_owned());
    };
    let entries = match &view.file {
        ScriptsFile::Unknown => {
            return Outcome::Refuse(format!(
                "Could not read {} of {}.",
                crate::project::FILE_NAME,
                view.project_name
            ))
        }
        ScriptsFile::Absent => {
            return Outcome::Refuse(format!(
            "{} has no {}: add [[script]] entries with a name and a command to list scripts here.",
            view.project_name,
            crate::project::FILE_NAME
        ))
        }
        ScriptsFile::Invalid(why) => return Outcome::Refuse(why.clone()),
        ScriptsFile::Scripts(entries) if entries.is_empty() => {
            return Outcome::Refuse(format!("{} lists no scripts.", crate::project::FILE_NAME))
        }
        ScriptsFile::Scripts(entries) => entries,
    };
    match answers {
        [] => choices(
            "Script",
            entries
                .iter()
                .map(|entry| {
                    let detail = match &entry.key_note {
                        Some(note) => format!("{} ({note})", entry.command),
                        None => entry.command.clone(),
                    };
                    let mut choice = Choice::new(entry.name.clone(), detail, entry.name.clone());
                    choice.icon = entry.icon.clone();
                    choice.keys = entry.key.clone();
                    choice
                })
                .collect(),
            Custom::No,
        ),
        [name, ..] => match entries.iter().find(|entry| &entry.name == name) {
            Some(entry) => Outcome::Run(Action::RunScript {
                project: view.project.clone(),
                machine: view.machine.clone(),
                cwd: view.cwd.clone(),
                name: entry.name.clone(),
            }),
            None => Outcome::Refuse(format!("Unknown script {name:?}.")),
        },
    }
}

fn worktree_choices(world: &World) -> Outcome {
    let list: Vec<Choice> = world
        .projects
        .iter()
        .flat_map(|info| {
            info.worktrees.iter().map(move |worktree| {
                Choice::new(
                    format!("{} / {}", info.project.name, worktree_label(worktree)),
                    format!("{} {}", info.machine_name, worktree.path),
                    worktree.id.as_str(),
                )
            })
        })
        .collect();
    if list.is_empty() {
        return Outcome::Refuse("Add a project first.".to_owned());
    }
    choices("Worktree", list, Custom::No)
}

fn rename(answers: &[String], world: &World) -> Outcome {
    let Some(target) = &world.rename else {
        return Outcome::Refuse(
            "Select a project, a machine, a session or a live terminal to rename.".to_owned(),
        );
    };
    match answers {
        [] => text("New name", target.current().to_owned(), Validate::Required),
        [name] if matches!(target, RenameTarget::Session(..)) => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Rename to \"{}\"", name.trim()),
                    format!("from \"{}\"", target.current()),
                    "yes",
                ),
                Choice::new("Cancel", "keep the name", "no"),
            ],
            Custom::No,
        ),
        [name, confirmed, ..] if matches!(target, RenameTarget::Session(..)) => match target {
            RenameTarget::Session(id, _, live) if confirmed == "yes" => {
                Outcome::Run(Action::RenameSession(id.clone(), *live, name.clone()))
            }
            _ => Outcome::Run(Action::Nothing),
        },
        [name, ..] => match target {
            RenameTarget::Session(..) => Outcome::Run(Action::Nothing),
            RenameTarget::Machine(machine, _) => Outcome::Run(Action::Engine(Op::RenameMachine {
                machine: machine.clone(),
                name: name.clone(),
            })),
            RenameTarget::Project(project, _) => Outcome::Run(Action::Engine(Op::RenameProject {
                project: project.clone(),
                name: name.clone(),
            })),
            RenameTarget::Live(id, _) => Outcome::Run(Action::RenameLive(*id, name.clone())),
        },
    }
}

fn remove_machine(answers: &[String], world: &World) -> Outcome {
    let Some(id) = &world.machine_row else {
        return Outcome::Refuse("Select a machine to remove.".to_owned());
    };
    let Some(machine) = world.machines.iter().find(|machine| &machine.id == id) else {
        return Outcome::Refuse("That machine is not known.".to_owned());
    };
    if machine.kind == MachineKind::Local {
        return Outcome::Refuse("The local machine cannot be removed.".to_owned());
    }
    match answers {
        [] => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Remove {}", machine.name),
                    "with its projects and sessions; nothing on the machine is touched",
                    "yes",
                ),
                Choice::new("Cancel", "keep it", "no"),
            ],
            Custom::No,
        ),
        [confirmed, ..] if confirmed == "yes" => {
            Outcome::Run(Action::Engine(Op::RemoveMachine(id.clone())))
        }
        _ => Outcome::Run(Action::Nothing),
    }
}

fn remove_from_history(answers: &[String], world: &World) -> Outcome {
    let Some((id, title)) = &world.here_session else {
        return Outcome::Refuse("Select a history session to remove.".to_owned());
    };
    match answers {
        [] => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Remove \"{title}\""),
                    "from Leon's history; the agent's own file stays",
                    "yes",
                ),
                Choice::new("Cancel", "keep it", "no"),
            ],
            Custom::No,
        ),
        [confirmed, ..] if confirmed == "yes" => {
            Outcome::Run(Action::Engine(Op::RemoveSession(id.clone())))
        }
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Resuming a history session starts its agent, so a click on its row asks
/// first; `Enter` takes the first answer, which resumes it.
fn resume_session(answers: &[String], world: &World) -> Outcome {
    let Some((id, title)) = &world.here_session else {
        return Outcome::Refuse("Select a history session to resume.".to_owned());
    };
    if let Some(held) = &world.elsewhere {
        let probably = if held.likely { "probably " } else { "" };
        return match answers {
            [] => choices(
                if held.other_leon {
                    "This session is running in another Leon. Resume anyway?"
                } else {
                    "This session is running in another terminal. Resume anyway?"
                },
                vec![
                    Choice::new(
                        "Open transcript",
                        format!(
                            "read it without starting anything; {probably}held by pid {}",
                            held.pid
                        ),
                        "transcript",
                    ),
                    Choice::new(
                        format!("Resume \"{title}\" anyway"),
                        "two processes on one session can corrupt its history",
                        "yes",
                    ),
                ]
                .into_iter()
                .chain(held.can_take_over.then(|| {
                    Choice::new(
                        format!("Take over \"{title}\""),
                        format!(
                            "asks pid {} to end (SIGTERM, never forced), then resumes it here",
                            held.pid
                        ),
                        "takeover",
                    )
                }))
                .chain([Choice::new("Cancel", "leave it to the other one", "no")])
                .collect(),
                Custom::No,
            ),
            [chosen, ..] if chosen == "transcript" => Outcome::Run(Action::OpenTranscript),
            [chosen, ..] if chosen == "takeover" && held.can_take_over => {
                Outcome::Run(Action::TakeOver(id.clone()))
            }
            [chosen, ..] if chosen == "yes" => Outcome::Run(Action::ResumeAnyway(id.clone())),
            _ => Outcome::Run(Action::Nothing),
        };
    }
    match answers {
        [] => choices(
            "Resume this session?",
            vec![
                Choice::new(
                    format!("Resume \"{title}\""),
                    "starts its agent in a terminal, where it ran",
                    "yes",
                ),
                Choice::new("Cancel", "leave it in the history", "no"),
            ],
            Custom::No,
        ),
        [confirmed, ..] if confirmed == "yes" => Outcome::Run(Action::ResumeSession(id.clone())),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// The worktrees a history session could be resumed in instead of its own
/// folder: the other worktrees of its project, or, for a folder that no
/// project contains, every worktree of its machine.
fn resume_in(answers: &[String], world: &World) -> Outcome {
    let Some(target) = &world.resume else {
        return Outcome::Refuse("Select a history session to resume.".to_owned());
    };
    let own = target.cwd.trim_end_matches(['/', '\\']);
    let worktrees = || {
        world
            .projects
            .iter()
            .filter(|info| info.project.machine_id == target.machine)
            .filter(|info| {
                target
                    .project
                    .as_ref()
                    .is_none_or(|project| &info.project.id == project)
            })
            .flat_map(|info| info.worktrees.iter().map(move |worktree| (info, worktree)))
            .filter(|(_, worktree)| worktree.path.trim_end_matches(['/', '\\']) != own)
    };
    match answers {
        [] => {
            let list: Vec<Choice> = worktrees()
                .map(|(info, worktree)| {
                    Choice::new(
                        format!("{} / {}", info.project.name, worktree_label(worktree)),
                        worktree.path.clone(),
                        worktree.id.as_str(),
                    )
                })
                .collect();
            if list.is_empty() {
                return Outcome::Refuse(
                    "There is no other worktree of this project to resume it in.".to_owned(),
                );
            }
            choices("Resume in", list, Custom::No)
        }
        [chosen, ..] => match worktrees().find(|(_, worktree)| worktree.id.as_str() == chosen) {
            Some((_, worktree)) => Outcome::Run(Action::ResumeIn(
                target.session.clone(),
                worktree.path.clone(),
            )),
            None => Outcome::Refuse(format!("Unknown worktree {chosen:?}.")),
        },
    }
}

/// Resuming a session that another terminal runs. A likely match is only a
/// guess and resumes at once; a certain one asks first, and the first answer,
/// which `Enter` takes, is to leave it alone.
fn resume_anyway(answers: &[String], world: &World) -> Outcome {
    let Some(target) = &world.elsewhere else {
        return Outcome::Refuse("Select a session that is running in another terminal.".to_owned());
    };
    if target.likely {
        return Outcome::Run(Action::ResumeAnyway(target.session.clone()));
    }
    match answers {
        [] => choices(
            "Confirm",
            vec![
                Choice::new("Cancel", "leave it to the other terminal", "no"),
                Choice::new(
                    format!("Resume \"{}\" here anyway", target.title),
                    format!(
                        "pid {} also writes this session: two processes on one session can corrupt its history",
                        target.pid
                    ),
                    "yes",
                ),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "yes" => {
            Outcome::Run(Action::ResumeAnyway(target.session.clone()))
        }
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Taking over a session that another terminal or Leon runs: the process is
/// asked to end, and the session resumes here once it has. Always asks, the
/// first answer, which `Enter` takes, is to leave it alone.
fn take_over(answers: &[String], world: &World) -> Outcome {
    let Some(target) = &world.elsewhere else {
        return Outcome::Refuse("Select a session that is running in another terminal.".to_owned());
    };
    if !target.can_take_over {
        return Outcome::Refuse(
            "Leon can only end a process on this computer: close it where it runs.".to_owned(),
        );
    }
    match answers {
        [] => choices(
            if target.other_leon {
                "Take over: the other Leon's agent will be ended"
            } else {
                "Take over: the other terminal's agent will be ended"
            },
            vec![
                Choice::new("Cancel", "leave it to the other one", "no"),
                Choice::new(
                    format!("End pid {} and resume \"{}\" here", target.pid, target.title),
                    "SIGTERM, never forced: the agent saves its session and quits; if it does not, nothing is resumed",
                    "yes",
                ),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "yes" => Outcome::Run(Action::TakeOver(target.session.clone())),
        _ => Outcome::Run(Action::Nothing),
    }
}

fn close_session(answers: &[String], world: &World) -> Outcome {
    let Some(info) = live_here(world) else {
        return Outcome::Refuse("There is no live session to close.".to_owned());
    };
    let id = info.id;
    if !info.busy || !world.prefs.confirm_close {
        return Outcome::Run(Action::CloseLive(id));
    }
    match answers {
        [] => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Close {}", info.label),
                    "stops the program that is running in it and removes it from the sidebar",
                    "yes",
                ),
                Choice::new("Cancel", "keep it running", "no"),
            ],
            Custom::No,
        ),
        [confirmed, ..] if confirmed == "yes" => Outcome::Run(Action::CloseLive(id)),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Sleeping asks like closing does while a program runs, though what it ends
/// stays in the sidebar.
fn sleep_session(answers: &[String], world: &World) -> Outcome {
    let Some(info) = live_here(world) else {
        return Outcome::Refuse("There is no live session to put to sleep.".to_owned());
    };
    let id = info.id;
    if !info.busy || !world.prefs.confirm_close {
        return Outcome::Run(Action::SleepLive(id));
    }
    match answers {
        [] => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Sleep {}", info.label),
                    "stops the program that is running in it; the session stays in the sidebar",
                    "yes",
                ),
                Choice::new("Cancel", "keep it running", "no"),
            ],
            Custom::No,
        ),
        [confirmed, ..] if confirmed == "yes" => Outcome::Run(Action::SleepLive(id)),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Snoozing asks until when: a preset, or a length of time or a date typed
/// (see `shelf::snooze_until`, which reads both).
fn snooze_session(answers: &[String], world: &World) -> Outcome {
    let Some((session, _)) = &world.here_session else {
        return Outcome::Refuse("Select a session to snooze.".to_owned());
    };
    let Some((now, offset)) = world.clock else {
        return Outcome::Refuse("The clock is not known.".to_owned());
    };
    match answers {
        [] => choices(
            "Snooze until",
            super::shelf::PRESETS
                .iter()
                .map(|(label, detail, value)| Choice::new(*label, *detail, *value))
                .collect(),
            Custom::Any,
        ),
        [when, ..] => match super::shelf::snooze_until(when, now, offset) {
            Ok(until) => Outcome::Run(Action::Snooze(session.clone(), until)),
            Err(why) => Outcome::Refuse(why),
        },
    }
}

/// The live lion a flow of the Den is about, or why there is none.
fn lion_here(world: &World) -> Result<(&LionInfo, LiveId), String> {
    let Some(lion) = &world.lion else {
        return Err("Select a lion in the Den first.".to_owned());
    };
    match (lion.live, &lion.elsewhere) {
        (Some(live), None) => Ok((lion, live)),
        (_, Some(why)) => Err(why.clone()),
        (None, None) => Err(format!("{} has no terminal here.", lion.name)),
    }
}

/// A message to the lion selected in the Den: its text is all that is asked.
fn message_lion(answers: &[String], world: &World) -> Outcome {
    let (lion, live) = match lion_here(world) {
        Ok(found) => found,
        Err(why) => return Outcome::Refuse(why),
    };
    if let Some(why) = &lion.no_message {
        return Outcome::Refuse(why.clone());
    }
    match answers {
        [] => text(
            "Message",
            format!("Message to {}", lion.name),
            Validate::Message,
        ),
        [message, ..] => Outcome::Run(Action::MessageLive(live, message.clone())),
    }
}

/// The messages that wait for the selected lion: one of them, or all, can
/// be taken back before it is typed.
fn queued_messages(answers: &[String], world: &World) -> Outcome {
    let (lion, live) = match lion_here(world) {
        Ok(found) => found,
        Err(why) => return Outcome::Refuse(why),
    };
    if lion.queued.is_empty() {
        return Outcome::Refuse(format!("No message is queued for {}.", lion.name));
    }
    match answers {
        [] => {
            let mut rows: Vec<Choice> = lion
                .queued
                .iter()
                .enumerate()
                .map(|(place, message)| {
                    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
                    Choice::new(
                        format!("{}. {flat}", place + 1),
                        "take this one back: it will not be typed",
                        place.to_string(),
                    )
                })
                .collect();
            if lion.queued.len() > 1 {
                rows.push(Choice::new(
                    format!("Take all {} back", lion.queued.len()),
                    format!("none of them is typed into {}", lion.name),
                    "all",
                ));
            }
            rows.push(Choice::new(
                "Keep them",
                format!("they are typed when {} next waits at its prompt", lion.name),
                "keep",
            ));
            choices("Queued", rows, Custom::No)
        }
        [chosen, ..] if chosen == "all" => Outcome::Run(Action::CancelQueued(live, None)),
        [chosen, ..] => match chosen.parse::<usize>() {
            Ok(place) if place < lion.queued.len() => {
                Outcome::Run(Action::CancelQueued(live, Some(place)))
            }
            _ => Outcome::Run(Action::Nothing),
        },
    }
}

/// The lions a message to the pride can go to: those with a terminal here
/// that can be messaged.
fn reachable(world: &World) -> Vec<&PrideLion> {
    world
        .pride
        .lions
        .iter()
        .filter(|lion| matches!(lion.post, Some(Post::Now | Post::Later)))
        .collect()
}

/// The names of some lions, as a sentence lists them.
fn names(lions: &[&PrideLion]) -> String {
    lions
        .iter()
        .map(|lion| lion.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One message to several lions. First who: all that wait at their prompt,
/// all that can be messaged, or lions picked one by one. Then the message,
/// then a confirmation that names who gets it now, for whom it waits, and
/// who is left out and why.
fn message_pride(answers: &[String], world: &World) -> Outcome {
    if !world.pride.open {
        return Outcome::Refuse("Open the Den first: a message goes to its lions.".to_owned());
    }
    let all = reachable(world);
    if all.is_empty() {
        return Outcome::Refuse(
            "No lion can be messaged: each needs a terminal here and a transcript Leon follows."
                .to_owned(),
        );
    }
    let waiting: Vec<&PrideLion> = all
        .iter()
        .copied()
        .filter(|lion| lion.post == Some(Post::Now))
        .collect();
    let Some((who, rest)) = answers.split_first() else {
        let mut rows = Vec::new();
        if !waiting.is_empty() {
            rows.push(Choice::new(
                format!("Those that wait at their prompt ({})", waiting.len()),
                names(&waiting),
                "waiting",
            ));
        }
        rows.push(Choice::new(
            format!("All that can be messaged ({})", all.len()),
            names(&all),
            "all",
        ));
        rows.push(Choice::new(
            "Choose lions\u{2026}",
            "one at a time",
            "choose",
        ));
        return choices("To", rows, Custom::No);
    };
    // Who was chosen, and what is left of the answers after that.
    let (chosen, rest): (Vec<&PrideLion>, &[String]) = match who.as_str() {
        "waiting" => (waiting, rest),
        "all" => (all.clone(), rest),
        "choose" => {
            let mut picked: Vec<&PrideLion> = Vec::new();
            let mut left = rest;
            loop {
                let offer = |picked: &[&PrideLion]| {
                    let mut rows: Vec<Choice> = all
                        .iter()
                        .filter(|lion| !picked.iter().any(|one| one.id == lion.id))
                        .map(|lion| {
                            let when = match lion.post {
                                Some(Post::Now) => "typed at once",
                                _ => "queued until it waits",
                            };
                            Choice::new(
                                lion.name.clone(),
                                format!("{}, {}: {when}", lion.state, lion.place),
                                lion.id.to_string(),
                            )
                        })
                        .collect();
                    if !picked.is_empty() {
                        rows.insert(
                            0,
                            Choice::new(
                                format!("Done: {} chosen", picked.len()),
                                names(picked),
                                "done",
                            ),
                        );
                    }
                    choices("To", rows, Custom::No)
                };
                match left.split_first() {
                    None => return offer(&picked),
                    Some((answer, after)) if answer == "done" && !picked.is_empty() => {
                        left = after;
                        break;
                    }
                    Some((answer, after)) => {
                        let found = answer
                            .parse::<u64>()
                            .ok()
                            .and_then(|id| all.iter().copied().find(|lion| lion.id == id));
                        match found {
                            Some(lion) if !picked.iter().any(|one| one.id == lion.id) => {
                                picked.push(lion);
                            }
                            Some(_) => {}
                            None => return Outcome::Run(Action::Nothing),
                        }
                        left = after;
                        // Everybody is chosen: there is nobody left to ask of.
                        if picked.len() == all.len() {
                            break;
                        }
                    }
                }
            }
            (picked, left)
        }
        _ => return Outcome::Run(Action::Nothing),
    };
    if chosen.is_empty() {
        return Outcome::Refuse("Nobody waits at their prompt now.".to_owned());
    }
    let Some((message, rest)) = rest.split_first() else {
        return text(
            "Message",
            format!("Message to {}", names(&chosen)),
            Validate::Message,
        );
    };
    let now: Vec<&PrideLion> = chosen
        .iter()
        .copied()
        .filter(|lion| lion.post == Some(Post::Now))
        .collect();
    let later: Vec<&PrideLion> = chosen
        .iter()
        .copied()
        .filter(|lion| lion.post == Some(Post::Later))
        .collect();
    match rest {
        [] => {
            let mut said = Vec::new();
            if !now.is_empty() {
                said.push(format!("typed now into {}", names(&now)));
            }
            if !later.is_empty() {
                said.push(format!(
                    "queued for {} until each waits at its prompt",
                    names(&later)
                ));
            }
            // Who is left out of "all", and why: said before, not after.
            let skipped: Vec<String> = world
                .pride
                .lions
                .iter()
                .filter_map(|lion| match &lion.post {
                    Some(Post::Never(why)) => Some(format!("{} ({why})", lion.name)),
                    None => Some(format!("{} (runs elsewhere)", lion.name)),
                    _ => None,
                })
                .collect();
            if who != "choose" && !skipped.is_empty() {
                said.push(format!("left out: {}", skipped.join("; ")));
            }
            let count = match chosen.len() {
                1 => "1 lion".to_owned(),
                n => format!("{n} lions"),
            };
            choices(
                "Confirm",
                vec![
                    Choice::new(format!("Send to {count}"), said.join("; "), "yes"),
                    Choice::new("Cancel", "nothing is typed anywhere", "no"),
                ],
                Custom::No,
            )
        }
        [confirmed, ..] if confirmed == "yes" => Outcome::Run(Action::MessagePride(
            chosen.iter().map(|lion| LiveId(lion.id)).collect(),
            message.clone(),
        )),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// A new agent session from the Den: the questions of a new session, always
/// from the first (where), whatever row the sidebar's cursor was left on.
fn hatch_lion(answers: &[String], world: &World) -> Outcome {
    // A lion is hatched in a worktree: said with what to do about it, since
    // the Den has no project of its own to fall back on.
    if world.projects.iter().all(|info| info.worktrees.is_empty()) {
        return Outcome::Refuse(
            "A lion is hatched in a project: add one first (+ in the sidebar).".to_owned(),
        );
    }
    let mut anywhere = world.clone();
    anywhere.here = None;
    // Where the selected lion works is offered first, never taken: a second
    // agent in a worktree shares its files, which is for the user to choose.
    if answers.is_empty() {
        if let Some((worktree, name)) = lion_worktree(world) {
            if let Outcome::Ask(Step {
                prompt,
                kind:
                    StepKind::Choices {
                        mut choices,
                        custom,
                    },
            }) = worktree_choices(&anywhere)
            {
                if let Some(at) = choices.iter().position(|choice| choice.value == worktree) {
                    let mut beside = choices.remove(at);
                    beside.detail = format!("where {name} works: the two would share its files");
                    choices.insert(0, beside.current(true));
                }
                return Outcome::Ask(Step {
                    prompt,
                    kind: StepKind::Choices { choices, custom },
                });
            }
        }
    }
    match new_session(answers, &anywhere) {
        Outcome::Run(Action::StartSession(intent)) => Outcome::Run(Action::HatchSession(intent)),
        other => other,
    }
}

/// The worktree the selected lion's session runs in (the deepest one that
/// holds its folder), and the lion's name.
fn lion_worktree(world: &World) -> Option<(String, String)> {
    let lion = world.lion.as_ref()?;
    let cwd = lion.cwd.as_deref()?.trim_end_matches(['/', '\\']);
    world
        .projects
        .iter()
        .flat_map(|info| info.worktrees.iter())
        .filter(|worktree| {
            let root = worktree.path.trim_end_matches(['/', '\\']);
            cwd == root
                || cwd
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with(['/', '\\']))
        })
        .max_by_key(|worktree| worktree.path.len())
        .map(|worktree| (worktree.id.as_str().to_owned(), lion.name.clone()))
}

/// Waking a session that was sent home: which one is all that is asked.
fn wake_lion(answers: &[String], world: &World) -> Outcome {
    if world.pride.home.is_empty() {
        return Outcome::Refuse("Nobody is at home: no session was sent home.".to_owned());
    }
    match answers {
        [] => choices(
            "Wake",
            world
                .pride
                .home
                .iter()
                .map(|(key, name)| {
                    Choice::new(
                        name.clone(),
                        "starts its agent again where the session left off",
                        key.clone(),
                    )
                })
                .collect(),
            Custom::No,
        ),
        [key, ..] if world.pride.home.iter().any(|(known, _)| known == key) => {
            Outcome::Run(Action::WakeHome(key.clone()))
        }
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Going to a lion: which one is all that is asked. Those that need the
/// user are listed first, as in the roster.
fn go_to_lion(answers: &[String], world: &World) -> Outcome {
    if world.pride.lions.is_empty() {
        return Outcome::Refuse("The Den is empty: no agent session is live.".to_owned());
    }
    match answers {
        [] => choices(
            "Lion",
            world
                .pride
                .lions
                .iter()
                .map(|lion| {
                    // Its state and its place are in what is typed against:
                    // "waiting" and the project's name find it as its own
                    // name does.
                    Choice::new(
                        format!("{} ({}, {})", lion.name, lion.state, lion.place),
                        if lion.needs { "needs you" } else { "" },
                        lion.id.to_string(),
                    )
                })
                .collect(),
            Custom::No,
        ),
        [id, ..] => match id.parse::<u64>() {
            Ok(id) if world.pride.lions.iter().any(|lion| lion.id == id) => {
                Outcome::Run(Action::SelectLion(id))
            }
            _ => Outcome::Run(Action::Nothing),
        },
    }
}

/// Sending a lion home always asks: it is done from a picture of the
/// session, not from its terminal, and what it ends is said first.
fn send_lion_home(answers: &[String], world: &World) -> Outcome {
    let (lion, live) = match lion_here(world) {
        Ok(found) => found,
        Err(why) => return Outcome::Refuse(why),
    };
    match answers {
        [] => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Send {} home", lion.name),
                    "stops its agent now; the session stays in the sidebar, asleep, and you can wake it where it left off",
                    "yes",
                ),
                Choice::new("Cancel", "keep it working", "no"),
            ],
            Custom::No,
        ),
        [confirmed, ..] if confirmed == "yes" => Outcome::Run(Action::SendHome(live)),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// The flow that opens a file: its path is all that is asked.
fn open_file(answers: &[String]) -> Outcome {
    match answers {
        [] => text(
            "Open file",
            "a path, relative to this folder or absolute",
            Validate::Required,
        ),
        [path, ..] => Outcome::Run(Action::OpenFile(path.clone())),
    }
}

/// Saving is not asked about, unless the file changed on disk since it was
/// read: then what to do with the two versions is.
fn save_file(answers: &[String], world: &World) -> Outcome {
    let Some(file) = &world.file else {
        return Outcome::Refuse("There is no file to save.".to_owned());
    };
    if !file.conflict {
        return Outcome::Run(Action::SaveFile(file.id));
    }
    match answers {
        [] => choices(
            "Changed on disk",
            vec![
                Choice::new(
                    "Cancel",
                    format!("{} stays as it is, here and on disk", file.name),
                    "cancel",
                ),
                Choice::new(
                    format!("Overwrite {}", file.name),
                    "saves what is in the editor over the other version",
                    "overwrite",
                ),
                Choice::new(
                    format!("Reload {}", file.name),
                    "reads the other version; the changes in the editor are lost",
                    "reload",
                ),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "overwrite" => Outcome::Run(Action::OverwriteFile(file.id)),
        [chosen, ..] if chosen == "reload" => Outcome::Run(Action::ReloadFile(file.id)),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Closing a file asks only when it has changes that were not saved.
fn close_file(answers: &[String], world: &World) -> Outcome {
    let Some(file) = &world.file else {
        return Outcome::Refuse("There is no file to close.".to_owned());
    };
    if !file.dirty {
        return Outcome::Run(Action::CloseFile(file.id));
    }
    match answers {
        [] => choices(
            "Unsaved changes",
            vec![
                Choice::new(
                    format!("Save {}", file.name),
                    "writes the changes, then closes it",
                    "save",
                ),
                Choice::new(
                    format!("Discard the changes to {}", file.name),
                    "closes it without saving",
                    "discard",
                ),
                Choice::new("Cancel", "keep it open", "cancel"),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "save" => Outcome::Run(Action::SaveAndCloseFile(file.id)),
        [chosen, ..] if chosen == "discard" => Outcome::Run(Action::CloseFile(file.id)),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// The live session the keyboard or the menu is on.
fn live_here(world: &World) -> Option<&LiveInfo> {
    let id = world.here_live?;
    world.live.iter().find(|info| info.id == id)
}

fn new_worktree(answers: &[String], world: &World) -> Outcome {
    if world.projects.is_empty() {
        return Outcome::Refuse("Add a project first.".to_owned());
    }
    match answers {
        [] => project_choices(world),
        [_] => text("Branch name", "feature/login", Validate::Branch),
        [project, _] => base_choices(project, world),
        [project, branch, base, ..] => Outcome::Run(Action::Engine(Op::AddWorktree {
            project: ProjectId::from_string(project.as_str()),
            branch: branch.clone(),
            base: (base != "HEAD").then(|| base.clone()),
        })),
    }
}

/// The projects to make a worktree in, the one the keyboard is on first.
fn project_choices(world: &World) -> Outcome {
    let here = world.here.as_ref().and_then(|here| here.project.as_ref());
    let mut list: Vec<Choice> = world
        .projects
        .iter()
        .map(|info| {
            Choice::new(
                info.project.name.clone(),
                format!("{} {}", info.machine_name, info.project.root),
                info.project.id.as_str(),
            )
            .current(Some(&info.project.id) == here)
        })
        .collect();
    // The project the keyboard is on comes first.
    list.sort_by_key(|choice| !choice.current);
    choices("Project", list, Custom::No)
}

/// What a new branch of `project` can start from: `HEAD`, or a branch in use,
/// or any name typed.
fn base_choices(project: &str, world: &World) -> Outcome {
    let mut list = vec![Choice::new("HEAD", "the current checkout", "HEAD")];
    if let Some(info) = world
        .projects
        .iter()
        .find(|info| info.project.id.as_str() == project)
    {
        let mut seen = std::collections::HashSet::new();
        for branch in info.worktrees.iter().filter_map(|w| w.branch.as_deref()) {
            if seen.insert(branch) {
                list.push(Choice::new(branch, "a branch in use", branch));
            }
        }
    }
    choices("Base", list, Custom::Any)
}

/// The answer that ends the ticking of agents in [`prompt_agents`]. It is not
/// the shape of an agent id (`parse` refuses a leading dash), so no agent can
/// be mistaken for it.
const START: &str = "--start";

/// The agents that can be given a prompt where `info`'s project is: on, with a
/// verified prompt form, and installed there (or not known to be missing).
fn prompt_candidates(world: &World, machine: &MachineId) -> Vec<AgentId> {
    let installed = world.installed_on(machine);
    world
        .prefs
        .offered()
        .into_iter()
        .filter(|agent| agent.spec().is_some_and(|spec| spec.prompt.is_some()))
        .filter(|agent| installed.is_none_or(|list| list.contains(agent)))
        .collect()
}

/// "One prompt, several agents": the project, the prompt, then the agents
/// ticked one answer at a time (choosing a ticked agent again unticks it)
/// until `Start`, which is offered once two are ticked, and the base.
fn prompt_agents(answers: &[String], world: &World) -> Outcome {
    if world.projects.is_empty() {
        return Outcome::Refuse("Add a project first.".to_owned());
    }
    let (project, prompt, rest) = match answers {
        [] => return project_choices(world),
        [_] => return text("Prompt", "what every agent should do", Validate::Prompt),
        [project, prompt, rest @ ..] => (project, prompt, rest),
    };
    let Some(info) = world
        .projects
        .iter()
        .find(|info| info.project.id.as_str() == project)
    else {
        return Outcome::Refuse(format!("Unknown project {project:?}."));
    };
    let candidates = prompt_candidates(world, &info.project.machine_id);
    if candidates.len() < crate::fanout::LEAST_AGENTS {
        return Outcome::Refuse(format!(
            "Two agents that take a prompt are needed, and {} has {} installed and turned on.",
            info.machine_name,
            candidates.len()
        ));
    }
    let (picks, after) = match rest.iter().position(|answer| answer == START) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let mut ticked: Vec<AgentId> = Vec::new();
    for pick in picks {
        match AgentId::parse(pick).filter(|agent| candidates.contains(agent)) {
            Some(agent) => match ticked.iter().position(|known| *known == agent) {
                Some(at) => {
                    ticked.remove(at);
                }
                None => ticked.push(agent),
            },
            None => return Outcome::Refuse(format!("{pick:?} cannot be given a prompt here.")),
        }
    }
    match after {
        None => {
            let place = format!("on {}", info.machine_name);
            let mut list = Vec::new();
            if ticked.len() >= crate::fanout::LEAST_AGENTS {
                let names: Vec<&str> = ticked
                    .iter()
                    .map(|agent| format::agent_name(*agent))
                    .collect();
                list.push(Choice::new(
                    format!("Start {} agents", ticked.len()),
                    names.join(", "),
                    START,
                ));
            }
            list.extend(candidates.iter().map(|agent| {
                let on = ticked.contains(agent);
                Choice::new(
                    format::agent_name(*agent),
                    if on {
                        "ticked: choose it again to untick".to_owned()
                    } else {
                        place.clone()
                    },
                    agent.as_str(),
                )
                .agent(*agent)
                .current(on)
            }));
            choices("Agents (two or more)", list, Custom::No)
        }
        Some([]) => base_choices(project, world),
        Some([base, ..]) if ticked.len() >= crate::fanout::LEAST_AGENTS => {
            Outcome::Run(Action::PromptAgents {
                project: ProjectId::from_string(project.as_str()),
                prompt: prompt.clone(),
                agents: ticked,
                base: (base != "HEAD").then(|| base.clone()),
            })
        }
        Some(_) => Outcome::Refuse("Tick two agents or more.".to_owned()),
    }
}

/// The machines a project can be cloned into or created on, this computer
/// first.
fn machine_choices(world: &World) -> Outcome {
    if world.machines.is_empty() {
        return Outcome::Refuse("There is no machine to work on.".to_owned());
    }
    choices(
        "Machine",
        world
            .machines
            .iter()
            .map(|machine| {
                Choice::new(
                    machine.name.clone(),
                    if machine.kind == MachineKind::Local {
                        "this computer"
                    } else {
                        ""
                    },
                    machine.id.as_str(),
                )
                .current(world.selected.as_ref() == Some(&machine.id))
            })
            .collect(),
        Custom::No,
    )
}

/// The parent folder question: the system's folder dialog on this computer
/// (a `pick` answer, handled by the window), a typed absolute path on any
/// other machine.
fn parent_step(machine: &str) -> Outcome {
    if machine == MachineId::local().as_str() {
        choices(
            "Parent folder",
            vec![Choice::new(
                "Choose with the folder dialog\u{2026}",
                "on this computer",
                "pick",
            )],
            Custom::AbsolutePath,
        )
    } else {
        text(
            "Parent folder",
            "an absolute path on that machine, such as /srv/code",
            Validate::Required,
        )
    }
}

/// The name a clone takes: what was typed, else what the URL offers.
fn clone_name(url: &str, typed: &str) -> String {
    match typed.trim() {
        "" => address::default_project_name_from_url(url),
        name => name.to_owned(),
    }
}

/// Clone a git URL into a folder and add it as a project: the machine, the
/// URL, a name (offered from the URL, as git itself would name it), and the
/// parent folder.
fn clone_project(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] => machine_choices(world),
        [machine] => {
            let known = world.machines.iter().any(|row| row.id.as_str() == machine);
            if !known {
                return Outcome::Refuse(format!("Unknown machine {machine:?}."));
            }
            text(
                "Git URL",
                "https://github.com/owner/repo.git",
                Validate::Required,
            )
        }
        [_, url] => text(
            "Project name",
            address::default_project_name_from_url(url),
            Validate::Optional,
        ),
        [machine, url, name] => match address::validate_project_name(&clone_name(url, name)) {
            Ok(()) => parent_step(machine),
            Err(why) => Outcome::Refuse(why.to_owned()),
        },
        [machine, url, name, parent]
            if parent == "pick" && machine == MachineId::local().as_str() =>
        {
            Outcome::Run(Action::PickCloneParent {
                url: url.clone(),
                name: clone_name(url, name),
            })
        }
        [machine, url, name, parent, ..] => Outcome::Run(Action::Engine(Op::CloneProject {
            machine: MachineId::from_string(machine.as_str()),
            url: url.clone(),
            parent: parent.clone(),
            name: clone_name(url, name),
        })),
    }
}

/// Create a brand-new git repository and add it as a project: the machine, a
/// name, and the parent folder.
fn new_project(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] => machine_choices(world),
        [machine] => {
            let known = world.machines.iter().any(|row| row.id.as_str() == machine);
            if !known {
                return Outcome::Refuse(format!("Unknown machine {machine:?}."));
            }
            text("Project name", "my-project", Validate::Required)
        }
        [machine, name] => match address::validate_project_name(name) {
            Ok(()) => parent_step(machine),
            Err(why) => Outcome::Refuse(why.to_owned()),
        },
        [machine, name, parent] if parent == "pick" && machine == MachineId::local().as_str() => {
            Outcome::Run(Action::PickProjectParent { name: name.clone() })
        }
        [machine, name, parent, ..] => Outcome::Run(Action::Engine(Op::CreateProject {
            machine: MachineId::from_string(machine.as_str()),
            parent: parent.clone(),
            name: name.clone(),
        })),
    }
}

fn add_project(answers: &[String], world: &World) -> Outcome {
    // Typing a path is for machines that have no folder dialog: on this
    // computer a project is opened with the system's own dialog.
    let remote: Vec<&Machine> = world
        .machines
        .iter()
        .filter(|machine| machine.kind != MachineKind::Local)
        .collect();
    if remote.is_empty() {
        return Outcome::Refuse(
            "Add a machine first. On this computer, use Open project… to pick the folder."
                .to_owned(),
        );
    }
    if answers
        .first()
        .is_some_and(|machine| machine == MachineId::local().as_str())
    {
        return Outcome::Refuse(
            "On this computer, use Open project… to pick the folder.".to_owned(),
        );
    }
    match answers {
        [] => choices(
            "Machine",
            remote
                .iter()
                .map(|machine| {
                    Choice::new(machine.name.clone(), "", machine.id.as_str())
                        .current(world.selected.as_ref() == Some(&machine.id))
                })
                .collect(),
            Custom::No,
        ),
        [machine] => {
            let mut offered: Vec<Choice> = world
                .folders
                .iter()
                .filter(|folder| folder.machine.as_str() == machine)
                .map(|folder| {
                    Choice::new(
                        folder.cwd.clone(),
                        format!("{} ran here", plural_sessions(folder.sessions)),
                        folder.cwd.clone(),
                    )
                })
                .collect();
            // The repositories found on the machine, unless a session folder
            // already is one of them.
            for (_, root) in world
                .repositories
                .iter()
                .filter(|(owner, _)| owner.as_str() == machine)
            {
                if !offered.iter().any(|choice| &choice.value == root) {
                    offered.push(Choice::new(
                        root.clone(),
                        "git repository found there",
                        root.clone(),
                    ));
                }
            }
            choices("Folder", offered, Custom::AbsolutePath)
        }
        [_, path] => text(
            "Project name",
            address::default_project_name(path),
            Validate::Optional,
        ),
        [machine, path, name, ..] => Outcome::Run(Action::Engine(Op::AddProject {
            machine: MachineId::from_string(machine.as_str()),
            path: path.clone(),
            name: name.clone(),
        })),
    }
}

fn plural_sessions(count: usize) -> String {
    match count {
        1 => "1 session".to_owned(),
        count => format!("{count} sessions"),
    }
}

fn remove_project(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] if world.projects.is_empty() => {
            Outcome::Refuse("There is no project to remove.".to_owned())
        }
        [] => choices(
            "Project",
            world
                .projects
                .iter()
                .map(|info| {
                    Choice::new(
                        info.project.name.clone(),
                        format!("{} {}", info.machine_name, info.project.root),
                        info.project.id.as_str(),
                    )
                })
                .collect(),
            Custom::No,
        ),
        [chosen] => {
            let name = world
                .projects
                .iter()
                .find(|info| info.project.id.as_str() == chosen)
                .map_or_else(
                    || "this project".to_owned(),
                    |info| info.project.name.clone(),
                );
            choices(
                "Confirm",
                vec![
                    Choice::new(
                        format!("Remove {name}"),
                        "leaves its folder and sessions alone",
                        "yes",
                    ),
                    Choice::new("Cancel", "keep it", "no"),
                ],
                Custom::No,
            )
        }
        [chosen, confirmed, ..] => {
            if confirmed != "yes" {
                return Outcome::Run(Action::Nothing);
            }
            Outcome::Run(Action::Engine(Op::RemoveProject(ProjectId::from_string(
                chosen.as_str(),
            ))))
        }
    }
}

/// Turning the agents' shared memory on or off asks for which project: one of
/// this computer's (the engine writes into its folder), the one the keyboard
/// is on first.
fn project_memory(answers: &[String], world: &World, on: bool) -> Outcome {
    match answers {
        [] => {
            let here = world.here.as_ref().and_then(|here| here.project.as_ref());
            // The list says which projects have it on, and puts the ones
            // the command would change first, the one the keyboard is on
            // before all.
            let mut list: Vec<(bool, Choice)> = world
                .projects
                .iter()
                .filter(|info| info.project.machine_id.is_local())
                .map(|info| {
                    let state = if info.memory_on { " · memory on" } else { "" };
                    (
                        info.memory_on == on,
                        Choice::new(
                            info.project.name.clone(),
                            format!("{}{state}", info.project.root),
                            info.project.id.as_str(),
                        )
                        .current(Some(&info.project.id) == here),
                    )
                })
                .collect();
            if list.is_empty() {
                return Outcome::Refuse(
                    "There is no project on this computer: the agent memory works here only."
                        .to_owned(),
                );
            }
            list.sort_by_key(|(done, choice)| (!choice.current, *done));
            let list = list.into_iter().map(|(_, choice)| choice).collect();
            choices("Project", list, Custom::No)
        }
        [project, ..] => Outcome::Run(Action::Engine(Op::ProjectMemory {
            project: ProjectId::from_string(project.as_str()),
            on,
        })),
    }
}

/// The answer the window adds when git refused a worktree for the files it
/// holds: the question it leads to is the one about forcing them away.
pub const REFUSED: &str = "refused";

/// Which worktree to show the changes of, when none is in view: any of them,
/// the main one of a project included.
fn show_changes(answers: &[String], world: &World) -> Outcome {
    let label = |worktree: &Worktree| {
        worktree
            .branch
            .clone()
            .unwrap_or_else(|| worktree.path.clone())
    };
    match answers {
        [] => {
            let list: Vec<Choice> = world
                .projects
                .iter()
                .flat_map(|info| {
                    info.worktrees.iter().map(move |worktree| {
                        Choice::new(
                            label(worktree),
                            format!("{} {}", info.project.name, worktree.path),
                            format!("{}|{}", info.project.id, worktree.id),
                        )
                    })
                })
                .collect();
            if list.is_empty() {
                Outcome::Refuse("There is no worktree to show the changes of.".to_owned())
            } else {
                choices("Worktree", list, Custom::No)
            }
        }
        [chosen, ..] => match chosen.split_once('|') {
            Some((project, worktree)) => Outcome::Run(Action::ShowChanges {
                project: ProjectId::from_string(project),
                worktree: WorktreeId::from_string(worktree),
            }),
            None => Outcome::Refuse("That worktree is not known.".to_owned()),
        },
    }
}

/// What the status line says to a second removal of merged worktrees asked for
/// while the first is still going.
pub const REMOVAL_RUNNING: &str =
    "Merged worktrees are still being removed: ask again when that has finished.";

/// The status line after a removal of merged worktrees: how many went, and
/// which were kept and why.
pub fn removal_words(removed: usize, kept: &[String]) -> String {
    let noun = |count: usize| if count == 1 { "worktree" } else { "worktrees" };
    let mut text = format!("Removed {removed} {}.", noun(removed));
    if !kept.is_empty() {
        text.push_str(&format!(
            " Kept {} {}: {}.",
            kept.len(),
            noun(kept.len()),
            kept.join("; ")
        ));
    }
    text
}

/// The branches to offer the cleanup for, when the setting asks for it and a
/// worktree's pull request was just seen merged. Nothing is removed by this:
/// the answer is a banner that opens the flow.
pub fn merged_offer(enabled: bool, before: &[Worktree], after: &[Worktree]) -> Option<Vec<String>> {
    if !enabled {
        return None;
    }
    let merged = cleanup::newly_merged(before, after);
    let labels: Vec<String> = after
        .iter()
        .filter(|worktree| merged.contains(&worktree.id))
        .map(|worktree| {
            worktree
                .branch
                .clone()
                .unwrap_or_else(|| worktree.path.clone())
        })
        .collect();
    (!labels.is_empty()).then_some(labels)
}

/// What the merged-pull-request banner says, for the branches it names.
pub fn merged_offer_words(labels: &[String]) -> String {
    match labels {
        [one] => format!("Pull request merged: {one}."),
        many => format!("{} pull requests merged: {}.", many.len(), many.join(", ")),
    }
}

/// A merged worktree the cleanup offers, and what keeps it from removal.
struct Offer {
    /// `project|worktree`, the answer the choice gives.
    key: String,
    /// The project it belongs to.
    project: ProjectId,
    /// The worktree.
    worktree: WorktreeId,
    /// Its branch, or its path when it has none.
    label: String,
    /// The project's name and the worktree's path.
    detail: String,
    /// Why it cannot be removed; empty when it can.
    blockers: Vec<Blocker>,
}

fn offers(world: &World) -> Vec<Offer> {
    world
        .projects
        .iter()
        .flat_map(|info| {
            info.worktrees
                .iter()
                .filter(|worktree| cleanup::offered(worktree))
                .map(move |worktree| Offer {
                    key: format!("{}|{}", info.project.id, worktree.id),
                    project: info.project.id.clone(),
                    worktree: worktree.id.clone(),
                    label: worktree
                        .branch
                        .clone()
                        .unwrap_or_else(|| worktree.path.clone()),
                    detail: format!("{} {}", info.project.name, worktree.path),
                    blockers: cleanup::blockers(
                        world.statuses.get(&worktree.id),
                        world.running.get(&worktree.id).copied().unwrap_or(0),
                    ),
                })
        })
        .collect()
}

/// Which offers are ticked after the answers so far. Each answer toggles the
/// offer it names, so choosing a ticked one unticks it; an answer naming a
/// blocked offer, or none, changes nothing.
fn ticked(answers: &[String], offers: &[Offer]) -> Vec<bool> {
    let mut on = vec![false; offers.len()];
    for answer in answers {
        if let Some(at) = offers
            .iter()
            .position(|offer| offer.key == *answer && offer.blockers.is_empty())
        {
            on[at] = !on[at];
        }
    }
    on
}

/// The "Remove merged worktrees" flow: a list where each choice ticks or
/// unticks one worktree, then "Remove" or "Cancel". The worktrees with
/// changes, unpushed commits or a running session are listed with the reason
/// and cannot be ticked.
fn remove_merged_worktrees(answers: &[String], world: &World) -> Outcome {
    let offers = offers(world);
    if offers.is_empty() {
        return Outcome::Refuse("No worktree's pull request is merged.".to_owned());
    }
    match answers.split_last() {
        Some((last, _)) if last == "no" => Outcome::Run(Action::Nothing),
        Some((last, before)) if last == "yes" => {
            let on = ticked(before, &offers);
            let removing: Vec<(ProjectId, WorktreeId)> = offers
                .iter()
                .zip(&on)
                .filter(|(_, ticked)| **ticked)
                .map(|(offer, _)| (offer.project.clone(), offer.worktree.clone()))
                .collect();
            if removing.is_empty() {
                Outcome::Refuse("Tick the worktrees to remove first.".to_owned())
            } else {
                Outcome::Run(Action::RemoveWorktrees(removing))
            }
        }
        _ => {
            let on = ticked(answers, &offers);
            let count = on.iter().filter(|ticked| **ticked).count();
            let mut list: Vec<Choice> = offers
                .iter()
                .zip(&on)
                .map(|(offer, ticked)| {
                    if offer.blockers.is_empty() {
                        Choice::new(&offer.label, &offer.detail, &offer.key).current(*ticked)
                    } else {
                        let why: Vec<String> = offer.blockers.iter().map(Blocker::words).collect();
                        Choice::new(
                            &offer.label,
                            format!("kept: {}", why.join(", ")),
                            &offer.key,
                        )
                        .dim(true)
                    }
                })
                .collect();
            let noun = if count == 1 { "worktree" } else { "worktrees" };
            list.push(Choice::new(
                format!("Remove {count} {noun}"),
                "git worktree remove, and the sessions in them close",
                "yes",
            ));
            list.push(Choice::new("Cancel", "keep them all", "no"));
            choices("Merged worktrees", list, Custom::No)
        }
    }
}

fn remove_worktree(answers: &[String], world: &World) -> Outcome {
    let linked = || {
        world.projects.iter().flat_map(|info| {
            info.worktrees
                .iter()
                .filter(|worktree| !worktree.is_main)
                .map(move |worktree| (info, worktree))
        })
    };
    let label = |worktree: &Worktree| {
        worktree
            .branch
            .clone()
            .unwrap_or_else(|| worktree.path.clone())
    };
    let name = |chosen: &str| {
        linked()
            .find(|(info, worktree)| format!("{}|{}", info.project.id, worktree.id) == chosen)
            .map_or_else(
                || "this worktree".to_owned(),
                |(_, worktree)| label(worktree),
            )
    };
    let run = |chosen: &str, force: bool| match chosen.split_once('|') {
        Some((project, worktree)) => Outcome::Run(Action::RemoveWorktree {
            project: ProjectId::from_string(project),
            worktree: WorktreeId::from_string(worktree),
            force,
        }),
        None => Outcome::Refuse("That worktree is not known.".to_owned()),
    };
    match answers {
        [] => {
            let list: Vec<Choice> = linked()
                .map(|(info, worktree)| {
                    Choice::new(
                        label(worktree),
                        format!("{} {}", info.project.name, worktree.path),
                        format!("{}|{}", info.project.id, worktree.id),
                    )
                })
                .collect();
            if list.is_empty() {
                Outcome::Refuse("There is no linked worktree to remove.".to_owned())
            } else {
                choices("Worktree", list, Custom::No)
            }
        }
        [chosen] => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Remove {}", name(chosen)),
                    "git worktree remove",
                    "yes",
                ),
                Choice::new("Cancel", "keep it", "no"),
            ],
            Custom::No,
        ),
        // git refused it for the modified or untracked files it holds: the
        // only way is `--force`, which deletes them, so ask once more.
        [chosen, refused] if refused == REFUSED => choices(
            "Confirm",
            vec![
                Choice::new(
                    format!("Force remove {}", name(chosen)),
                    "deletes its uncommitted and untracked files",
                    "yes",
                ),
                Choice::new("Cancel", "keep it", "no"),
            ],
            Custom::No,
        ),
        [chosen, refused, confirmed, ..] if refused == REFUSED => {
            if confirmed == "yes" {
                run(chosen, true)
            } else {
                Outcome::Run(Action::Nothing)
            }
        }
        [chosen, confirmed, ..] => {
            if confirmed == "yes" {
                run(chosen, false)
            } else {
                Outcome::Run(Action::Nothing)
            }
        }
    }
}

/// Which den: the built-in ones, then the user's, the one in use marked.
fn choose_den(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] => choices(
            "Den",
            world
                .dens
                .iter()
                .map(|den| {
                    Choice::new(den.name.clone(), den.detail.clone(), den.id.clone())
                        .current(den.id == world.den_id)
                })
                .collect(),
            Custom::No,
        ),
        [chosen, ..] => match world.dens.iter().find(|den| den.id == *chosen) {
            Some(den) => Outcome::Run(Action::SetDen(den.id.clone())),
            None => Outcome::Refuse(format!("Unknown den {chosen:?}.")),
        },
    }
}

/// Which theme: every theme with its swatches, the one in use marked. The
/// palette previews the one under the selection.
fn choose_theme(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] => choices(
            "Theme",
            theme::registry::all()
                .into_iter()
                .map(|id| {
                    let choice = Choice::new(id.name(), id.detail(), id.slug())
                        .current(id == world.theme_id);
                    // An invalid theme has no look to show.
                    if id.is_usable() {
                        choice.swatch(id)
                    } else {
                        choice
                    }
                })
                .collect(),
            Custom::No,
        ),
        [chosen, ..] => match ThemeId::parse(chosen) {
            Some(id) => match id.problem() {
                Some(reason) => Outcome::Refuse(format!("{} is invalid: {reason}", id.name())),
                None => Outcome::Run(Action::SetThemeId(id)),
            },
            None => Outcome::Refuse(format!("Unknown theme {chosen:?}.")),
        },
    }
}

/// How many running sessions a plain quit closes and how many it leaves
/// running (the ones the keeper holds).
fn quit_counts(world: &World) -> (usize, usize) {
    let counts = super::keeping::summary(
        super::keeping::Leaving::Keep,
        world
            .live
            .iter()
            .filter(|session| session.busy)
            .map(|session| session.keeps),
    );
    (counts.closed, counts.kept)
}

/// The sessions a quit leaves running, in words.
fn keep_running(kept: usize) -> String {
    match kept {
        1 => "1 running session keeps running.".to_owned(),
        n => format!("{n} running sessions keep running."),
    }
}

/// Whether to quit while programs run in terminals: it is asked only then
/// (with nothing running the shell quits without a question), and the first
/// answer, which `Enter` takes, is to quit. A program held by the keeper of
/// durable sessions keeps running through a quit, so it is not a reason to
/// ask, and the question says that it keeps running.
fn quit(answers: &[String], world: &World) -> Outcome {
    let (busy, kept) = quit_counts(world);
    let unsaved = world.unsaved.len();
    let ask =
        world.prefs.quit.asks(busy) || (unsaved > 0 && world.prefs.quit != QuitConfirm::Never);
    match answers {
        [] if !ask => Outcome::Run(Action::Quit),
        // Files with unsaved changes come first: saving them is the way out
        // that loses nothing, so it is the first choice and Enter takes it.
        [] if unsaved > 0 => unsaved_choices(unsaved, &world.unsaved[0], busy),
        [] => choices(
            "Quit Leon?",
            vec![
                Choice::new(
                    match (busy, kept) {
                        (0, 0) => format!("Quit {}", crate::product::PRODUCT_NAME),
                        (0, kept) => {
                            format!(
                                "Quit {}: {}",
                                crate::product::PRODUCT_NAME,
                                keep_running(kept)
                            )
                        }
                        (1, kept) => format!(
                            "Quit {}: 1 running session will be closed.{}",
                            crate::product::PRODUCT_NAME,
                            if kept > 0 {
                                format!(" {}", keep_running(kept))
                            } else {
                                String::new()
                            }
                        ),
                        (busy, kept) => format!(
                            "Quit {}: {busy} running sessions will be closed.{}",
                            crate::product::PRODUCT_NAME,
                            if kept > 0 {
                                format!(" {}", keep_running(kept))
                            } else {
                                String::new()
                            }
                        ),
                    },
                    if busy == 0 && kept > 0 {
                        "lets go of the terminals; they keep running"
                    } else {
                        "hangs the terminals up"
                    },
                    "yes",
                ),
                Choice::new("Cancel", "keep working", "no"),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "yes" => Outcome::Run(Action::Quit),
        [chosen, ..] if chosen == "save" && unsaved > 0 => Outcome::Run(Action::SaveAllAndQuit),
        [chosen, ..] if chosen == "discard" => Outcome::Run(Action::DiscardAndQuit),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// The question about files with changes that were not saved, before a quit.
fn unsaved_choices(unsaved: usize, first: &str, closing: usize) -> Outcome {
    choices(
        "Unsaved changes",
        vec![
            Choice::new(
                if unsaved == 1 {
                    format!("Save {first} and quit")
                } else {
                    format!("Save all {unsaved} files and quit")
                },
                "writes the changes, then quits",
                "save",
            ),
            Choice::new(
                "Quit without saving",
                if closing > 0 {
                    format!(
                        "the changes are lost and {closing} running session{} closed",
                        if closing == 1 { " is" } else { "s are" }
                    )
                } else {
                    "the changes are lost".to_owned()
                },
                "discard",
            ),
            Choice::new("Cancel", "keep working", "no"),
        ],
        Custom::No,
    )
}

/// Quit and end every session: what quitting did before durable sessions, for
/// the terminals the keeper holds as well. Asked by the same setting, on all
/// the running sessions, since all of them end.
fn quit_and_end(answers: &[String], world: &World) -> Outcome {
    let busy = world.live.iter().filter(|session| session.busy).count();
    let unsaved = world.unsaved.len();
    let ask =
        world.prefs.quit.asks(busy) || (unsaved > 0 && world.prefs.quit != QuitConfirm::Never);
    match answers {
        [] if !ask => Outcome::Run(Action::QuitAndEnd),
        [] if unsaved > 0 => unsaved_choices(unsaved, &world.unsaved[0], busy),
        [] => choices(
            "End every session and quit?",
            vec![
                Choice::new(
                    match busy {
                        0 => format!(
                            "End every session and quit {}",
                            crate::product::PRODUCT_NAME
                        ),
                        1 => "End 1 running session and quit.".to_owned(),
                        busy => format!("End {busy} running sessions and quit."),
                    },
                    "hangs the terminals up, the ones that outlive the window too",
                    "yes",
                ),
                Choice::new("Cancel", "keep working", "no"),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "yes" => Outcome::Run(Action::QuitAndEnd),
        [chosen, ..] if chosen == "save" && unsaved > 0 => Outcome::Run(Action::SaveAllEndAndQuit),
        [chosen, ..] if chosen == "discard" => Outcome::Run(Action::DiscardEndAndQuit),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Whether to restart into the update: always asked, because a restart ends
/// every terminal, and it says how many that is.
fn restart_update(answers: &[String], world: &World) -> Outcome {
    let Some(version) = &world.update_ready else {
        return Outcome::Refuse("No update is ready to install yet.".to_owned());
    };
    // What the keeper of durable sessions holds keeps running through the
    // restart: only the rest is closed.
    let busy = world
        .live
        .iter()
        .filter(|session| session.busy && !session.keeps)
        .count();
    let open = world.live.iter().filter(|session| !session.keeps).count();
    match answers {
        [] => choices(
            "Restart to update?",
            vec![
                Choice::new(
                    match (busy, open) {
                        (0, 0) => format!(
                            "Restart {} and update to {version}",
                            crate::product::PRODUCT_NAME
                        ),
                        (1, _) => format!(
                            "Restart and update to {version}: 1 running session will be closed."
                        ),
                        (busy, _) if busy > 1 => format!(
                            "Restart and update to {version}: {busy} running sessions will be closed."
                        ),
                        (_, 1) => format!(
                            "Restart and update to {version}: 1 open terminal will be closed."
                        ),
                        (_, open) => format!(
                            "Restart and update to {version}: {open} open terminals will be closed."
                        ),
                    },
                    "hangs the terminals up and starts the new version",
                    "yes",
                ),
                Choice::new("Cancel", "keep working", "no"),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "yes" => Outcome::Run(Action::RestartToUpdate),
        _ => Outcome::Run(Action::Nothing),
    }
}

fn set_appearance(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] => choices(
            "Appearance",
            AppearanceChoice::ALL
                .into_iter()
                .map(|choice| {
                    Choice::new(choice.label(), "", choice.label().to_lowercase())
                        .current(choice == world.theme)
                })
                .collect(),
            Custom::No,
        ),
        [chosen, ..] => match AppearanceChoice::parse(chosen) {
            Some(choice) => Outcome::Run(Action::SetAppearance(choice)),
            None => Outcome::Refuse(format!("Unknown theme {chosen:?}.")),
        },
    }
}

fn set_size(answers: &[String], world: &World) -> Outcome {
    match answers {
        [] => choices(
            "Size",
            theme::SCALE_STEPS
                .iter()
                .map(|step| {
                    Choice::new(format!("{step}%"), "", step.to_string())
                        .current(*step == world.scale)
                })
                .collect(),
            Custom::No,
        ),
        [chosen, ..] => match chosen.parse::<u16>() {
            Ok(step) => Outcome::Run(Action::SetScale(step)),
            Err(_) => Outcome::Refuse(format!("Unknown size {chosen:?}.")),
        },
    }
}

/// The questions that edit one setting, of the right kind: a toggle and a
/// choice ask for one of their answers, a number or a text for what is typed,
/// a list for adding, removing or clearing an entry, an action runs. `current`
/// is the value in use.
pub fn advance_setting(def: &'static Def, answers: &[String], current: &Value) -> Outcome {
    let set = |value: Value| Outcome::Run(Action::SetSetting(def.key, value));
    match def.kind {
        Kind::Action => match (def.key, answers) {
            ("reset_all", []) => choices(
                def.label,
                vec![
                    Choice::new(
                        "Reset every setting",
                        "keeps themes, projects and history",
                        "yes",
                    ),
                    Choice::new("Cancel", "keep the settings", "no"),
                ],
                Custom::No,
            ),
            ("reset_all", [chosen, ..]) if chosen == "yes" => Outcome::Run(Action::Button(def.key)),
            ("reset_all", _) => Outcome::Run(Action::Nothing),
            _ => Outcome::Run(Action::Button(def.key)),
        },
        Kind::Toggle => match answers {
            [] => {
                let on = current.as_bool().unwrap_or_default();
                choices(
                    def.label,
                    vec![
                        Choice::new("On", "", "on").current(on),
                        Choice::new("Off", "", "off").current(!on),
                    ],
                    Custom::No,
                )
            }
            [chosen, ..] => match def.parse_input(chosen) {
                Ok(value) => set(value),
                Err(why) => Outcome::Refuse(why),
            },
        },
        Kind::Choice(_) => match answers {
            [] => choices(
                def.label,
                def.options()
                    .into_iter()
                    .map(|(value, label)| {
                        let current = current.as_text() == Some(value.as_str());
                        let choice = Choice::new(label, "", value.clone()).current(current);
                        match (def.key, ThemeId::parse(&value)) {
                            ("theme_id", Some(id)) if id.is_usable() => choice.swatch(id),
                            _ => choice,
                        }
                    })
                    .collect(),
                Custom::No,
            ),
            [chosen, ..] => match def.parse_input(chosen) {
                Ok(value) => set(value),
                Err(why) => Outcome::Refuse(why),
            },
        },
        Kind::Number { min, max, .. } => match answers {
            [] => text(
                def.label,
                format!("{} ({min} to {max})", current.display()),
                Validate::Number { min, max },
            ),
            [typed, ..] => match def.parse_input(typed) {
                Ok(value) => set(value),
                Err(why) => Outcome::Refuse(why),
            },
        },
        Kind::Text { placeholder } | Kind::Path { placeholder } => match answers {
            [] => text(
                def.label,
                if current.display().is_empty() {
                    placeholder.to_owned()
                } else {
                    current.display()
                },
                Validate::Optional,
            ),
            [typed, ..] => set(Value::Text(typed.clone())),
        },
        Kind::List { placeholder } => {
            let items = current.as_list().unwrap_or_default().to_vec();
            match answers {
                [] => {
                    let mut list = vec![Choice::new("Add an entry", placeholder, "add")];
                    if !items.is_empty() {
                        list.push(Choice::new("Remove an entry", items.join(", "), "remove"));
                        list.push(Choice::new("Remove them all", "", "clear"));
                    }
                    choices(def.label, list, Custom::No)
                }
                [what] if what == "add" => text(def.label, placeholder, Validate::Required),
                [what, entry, ..] if what == "add" => {
                    let mut next = items;
                    next.push(entry.clone());
                    set(Value::List(next))
                }
                [what] if what == "remove" => choices(
                    def.label,
                    items
                        .iter()
                        .enumerate()
                        .map(|(at, item)| Choice::new(item.clone(), "", at.to_string()))
                        .collect(),
                    Custom::No,
                ),
                [what, at, ..] if what == "remove" => {
                    let mut next = items;
                    match at.parse::<usize>() {
                        Ok(at) if at < next.len() => {
                            next.remove(at);
                            set(Value::List(next))
                        }
                        _ => Outcome::Refuse("That entry is not there.".to_owned()),
                    }
                }
                [what, ..] if what == "clear" => set(Value::List(Vec::new())),
                _ => Outcome::Run(Action::Nothing),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use leon_core::{MachineKind, WorktreeId};

    fn machine(id: &str, name: &str) -> Machine {
        Machine {
            id: MachineId::from_string(id),
            name: name.to_owned(),
            kind: if id == "local" {
                MachineKind::Local
            } else {
                MachineKind::Ssh {
                    host: format!("{id}.example"),
                    user: None,
                    port: None,
                    identity_file: None,
                }
            },
        }
    }

    fn worktree(id: &str, path: &str, branch: Option<&str>, is_main: bool) -> Worktree {
        Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string("api"),
            path: path.to_owned(),
            branch: branch.map(str::to_owned),
            head: None,
            is_main,
            merged_pull_request: None,
        }
    }

    fn world() -> World {
        let local = machine("local", "Local");
        let project = |id: &str, name: &str, root: &str| Project {
            id: ProjectId::from_string(id),
            machine_id: MachineId::local(),
            name: name.to_owned(),
            root: root.to_owned(),
        };
        World {
            statuses: HashMap::new(),
            running: HashMap::new(),
            machines: vec![local, machine("m2", "build box")],
            folders: vec![
                FolderInfo {
                    machine: MachineId::local(),
                    cwd: "/home/me/notes".into(),
                    sessions: 3,
                },
                FolderInfo {
                    machine: MachineId::from_string("m2"),
                    cwd: "/opt/scratch".into(),
                    sessions: 1,
                },
            ],
            selected: Some(MachineId::local()),
            projects: vec![
                ProjectInfo {
                    project: project("web", "web", "/srv/web"),
                    machine_name: "Local".into(),
                    worktrees: vec![worktree("w0", "/srv/web", Some("main"), true)],
                    memory_on: false,
                },
                ProjectInfo {
                    project: project("api", "api", "/srv/api"),
                    machine_name: "Local".into(),
                    worktrees: vec![
                        worktree("a0", "/srv/api", Some("main"), true),
                        worktree("a1", "/srv/api-worktrees/x", Some("x"), false),
                        worktree("a2", "/srv/api-worktrees/detached", None, false),
                    ],
                    memory_on: false,
                },
            ],
            here: Some(Where {
                machine: MachineId::local(),
                machine_name: "Local".into(),
                project: Some(ProjectId::from_string("api")),
                cwd: "/srv/api".into(),
            }),
            live: Vec::new(),
            here_live: None,
            lion: None,
            pride: PrideInfo::default(),
            file: None,
            unsaved: Vec::new(),
            machine_row: None,
            here_session: None,
            clock: None,
            rename: None,
            resume: None,
            elsewhere: None,
            theme: AppearanceChoice::Dark,
            theme_id: ThemeId::Leon,
            scale: 100,
            dens: Vec::new(),
            den_id: String::new(),
            prefs: Prefs::default(),
            repositories: Vec::new(),
            installed: Vec::new(),
            update_ready: None,
            scripts: None,
            accounts: Vec::new(),
            default_accounts: Vec::new(),
            account_ask: None,
        }
    }

    fn resuming(project: Option<&str>, cwd: &str) -> World {
        let mut world = world();
        world.resume = Some(ResumeTarget {
            session: SessionId::from_string("s1"),
            machine: MachineId::local(),
            cwd: cwd.to_owned(),
            project: project.map(ProjectId::from_string),
        });
        world
    }

    #[test]
    fn resume_in_offers_the_other_worktrees_of_the_sessions_project() {
        let world = resuming(Some("api"), "/srv/api-worktrees/x");
        assert_eq!(
            labels(&advance(Command::ResumeIn, &[], &world)),
            ["api / main", "api / detached"],
            "not the folder it ran in, and nothing of another project"
        );
        assert_eq!(
            advance(Command::ResumeIn, &strings(&["a0"]), &world),
            Outcome::Run(Action::ResumeIn(
                SessionId::from_string("s1"),
                "/srv/api".into()
            ))
        );
    }

    #[test]
    fn resume_in_for_a_folder_of_no_project_offers_every_worktree_of_the_machine() {
        let world = resuming(None, "/home/me/gone");
        let offered = labels(&advance(Command::ResumeIn, &[], &world));
        assert_eq!(offered.len(), 4, "{offered:?}");
        assert!(offered.contains(&"web / main".to_owned()));
    }

    #[test]
    fn resume_in_without_a_session_or_another_worktree_says_so() {
        assert!(matches!(
            advance(Command::ResumeIn, &[], &world()),
            Outcome::Refuse(why) if why.contains("history session")
        ));
        let world = resuming(Some("web"), "/srv/web");
        assert!(matches!(
            advance(Command::ResumeIn, &[], &world),
            Outcome::Refuse(why) if why.contains("no other worktree")
        ));
        let world = resuming(Some("api"), "/x");
        assert!(matches!(
            advance(Command::ResumeIn, &strings(&["nope"]), &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn resuming_a_history_session_asks_first_and_the_first_answer_resumes() {
        let mut world = world();
        world.here_session = Some((SessionId::from_string("s1"), "fix it".to_owned()));
        let ask = advance(Command::ResumeSession, &[], &world);
        assert_eq!(labels(&ask), ["Resume \"fix it\"", "Cancel"]);
        assert_eq!(
            advance(Command::ResumeSession, &strings(&["yes"]), &world),
            Outcome::Run(Action::ResumeSession(SessionId::from_string("s1")))
        );
        assert_eq!(
            advance(Command::ResumeSession, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
        assert!(matches!(
            advance(Command::ResumeSession, &[], &self::world()),
            Outcome::Refuse(_)
        ));
    }

    fn labels(outcome: &Outcome) -> Vec<String> {
        match outcome {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => choices.iter().map(|c| c.label.clone()).collect(),
            other => panic!("expected choices, got {other:?}"),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_new_session_asks_for_the_agent_then_starts_it() {
        let world = world();
        let first = advance(Command::NewSession, &[], &world);
        assert_eq!(labels(&first)[..3], ["Claude Code", "Codex", "opencode"]);
        assert_eq!(
            labels(&first).len(),
            leon_core::agent::builtin().len(),
            "the whole catalogue is offered"
        );
        let done = advance(Command::NewSession, &strings(&["codex"]), &world);
        assert_eq!(
            done,
            Outcome::Run(Action::StartSession(SessionIntent {
                agent: AgentId::CODEX,
                machine: MachineId::local(),
                machine_name: "Local".into(),
                cwd: "/srv/api".into(),
                account: None,
            }))
        );
    }

    #[test]
    fn the_agent_choices_carry_their_logos() {
        let first = advance(Command::NewSession, &[], &world());
        let Outcome::Ask(Step {
            kind: StepKind::Choices { choices, .. },
            ..
        }) = first
        else {
            panic!("expected choices");
        };
        let agents: Vec<_> = choices.iter().map(|choice| choice.agent).collect();
        assert_eq!(
            agents[..3],
            [
                Some(AgentId::CLAUDE),
                Some(AgentId::CODEX),
                Some(AgentId::OPENCODE)
            ]
        );
        assert!(
            agents.iter().all(Option::is_some),
            "every row has a logo or a letter-mark"
        );
    }

    fn choices_of(outcome: Outcome) -> Vec<Choice> {
        match outcome {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => choices,
            other => panic!("expected choices, got {other:?}"),
        }
    }

    #[test]
    fn what_the_machine_has_comes_first_and_the_rest_is_dimmed_with_where_to_get_it() {
        let mut world = world();
        world.installed = vec![(MachineId::local(), vec![AgentId::OPENCODE, AgentId::GROK])];
        let list = choices_of(advance(Command::NewSession, &[], &world));
        let labels: Vec<&str> = list.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels[..2], ["opencode", "Grok"]);
        assert!(!list[0].dim && !list[1].dim);
        assert_eq!(list.len(), leon_core::agent::builtin().len());
        let claude = list.iter().find(|c| c.value == "claude").unwrap();
        assert!(claude.dim);
        assert!(
            claude.detail.starts_with("not installed on Local"),
            "{}",
            claude.detail
        );
        assert!(
            claude.detail.contains("https://"),
            "the docs link: {}",
            claude.detail
        );
        // The rest keeps the catalogue's order.
        let rest: Vec<&str> = labels[2..].to_vec();
        assert_eq!(rest[0], "Claude Code");
        assert_eq!(rest[1], "Codex");
    }

    #[test]
    fn an_agent_that_is_not_installed_is_refused_with_its_docs_link() {
        let mut world = world();
        world.installed = vec![(MachineId::local(), vec![AgentId::CLAUDE])];
        match advance(Command::NewSession, &strings(&["grok"]), &world) {
            Outcome::Refuse(text) => {
                assert!(
                    text.starts_with("Grok is not installed on Local."),
                    "{text}"
                );
                assert!(text.contains("https://x.ai/cli"), "{text}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        // An installed one starts, and a machine nobody looked at offers all.
        assert!(matches!(
            advance(Command::NewSession, &strings(&["claude"]), &world),
            Outcome::Run(Action::StartSession(_))
        ));
        let unknown = super::World {
            installed: vec![],
            update_ready: None,
            ..world.clone()
        };
        assert!(matches!(
            advance(Command::NewSession, &strings(&["grok"]), &unknown),
            Outcome::Run(Action::StartSession(_))
        ));
    }

    fn scripts_view(file: ScriptsFile) -> World {
        let mut world = world();
        world.scripts = Some(ScriptsView {
            project: ProjectId::from_string("api"),
            project_name: "api".into(),
            machine: MachineId::local(),
            cwd: "/srv/api-worktrees/x".into(),
            file,
        });
        world
    }

    fn entry(name: &str, command: &str) -> ScriptEntry {
        ScriptEntry {
            name: name.into(),
            command: command.into(),
            icon: None,
            key: None,
            key_note: None,
        }
    }

    #[test]
    fn running_a_script_lists_the_scripts_of_the_project_in_view() {
        let mut test = entry("Test", "cargo test");
        test.icon = Some("terminal".into());
        test.key = Some("Ctrl+Shift+U".into());
        let mut shell = entry("Shell", "bash");
        shell.key_note = Some("Ctrl+Shift+T is Leon's".into());
        let world = scripts_view(ScriptsFile::Scripts(vec![
            test,
            shell,
            entry("Lint", "make lint"),
        ]));
        let Outcome::Ask(step) = advance(Command::RunScript, &[], &world) else {
            panic!("it asks which script");
        };
        let StepKind::Choices { choices, custom } = step.kind else {
            panic!("it is a choice");
        };
        assert_eq!(custom, Custom::No);
        let names: Vec<_> = choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(names, ["Test", "Shell", "Lint"]);
        assert_eq!(choices[0].detail, "cargo test");
        assert_eq!(choices[0].icon.as_deref(), Some("terminal"));
        assert_eq!(choices[0].keys.as_deref(), Some("Ctrl+Shift+U"));
        assert!(
            choices[1].detail.contains("Ctrl+Shift+T is Leon's"),
            "{}",
            choices[1].detail
        );
        assert_eq!(choices[1].keys, None);
    }

    #[test]
    fn the_chosen_script_runs_in_the_worktree_in_view() {
        let world = scripts_view(ScriptsFile::Scripts(vec![entry("Test", "cargo test")]));
        assert_eq!(
            advance(Command::RunScript, &strings(&["Test"]), &world),
            Outcome::Run(Action::RunScript {
                project: ProjectId::from_string("api"),
                machine: MachineId::local(),
                cwd: "/srv/api-worktrees/x".into(),
                name: "Test".into(),
            })
        );
        assert!(matches!(
            advance(Command::RunScript, &strings(&["Nope"]), &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn running_a_script_says_why_there_is_nothing_to_run() {
        let refused = |world: &World| match advance(Command::RunScript, &[], world) {
            Outcome::Refuse(why) => why,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(refused(&world()).contains("Select a project"));
        assert!(refused(&scripts_view(ScriptsFile::Absent)).contains("leon.toml"));
        assert!(refused(&scripts_view(ScriptsFile::Unknown)).contains("Could not read"));
        assert!(refused(&scripts_view(ScriptsFile::Scripts(Vec::new()))).contains("no scripts"));
        assert_eq!(
            refused(&scripts_view(ScriptsFile::Invalid(
                "leon.toml line 3: wrong".into()
            ))),
            "leon.toml line 3: wrong"
        );
    }

    #[test]
    fn adding_a_custom_agent_asks_five_questions_and_checks_the_answers() {
        let world = world();
        let ask = |answers: &[&str]| advance(Command::AddAgent, &strings(answers), &world);
        let prompt = |outcome: Outcome| match outcome {
            Outcome::Ask(step) => step.prompt,
            other => panic!("expected a question, got {other:?}"),
        };
        assert_eq!(prompt(ask(&[])), "Agent name");
        assert_eq!(prompt(ask(&["My Tool"])), "Command");
        assert_eq!(prompt(ask(&["My Tool", "mytool"])), "Arguments");
        assert_eq!(
            prompt(ask(&["My Tool", "mytool", "--fast"])),
            "Resume arguments"
        );
        assert_eq!(
            prompt(ask(&["My Tool", "mytool", "--fast", "--resume {id}"])),
            "Prompt arguments"
        );
        assert_eq!(
            ask(&[
                "My Tool",
                "mytool",
                "--fast",
                "--resume {id}",
                "--ask {prompt}"
            ]),
            Outcome::Run(Action::AddAgent {
                name: "My Tool".into(),
                command: "mytool".into(),
                args: "--fast".into(),
                resume_args: "--resume {id}".into(),
                prompt_args: "--ask {prompt}".into(),
            })
        );
        match ask(&["My Tool", "mytool", "", "", "--ask"]) {
            Outcome::Refuse(text) => assert!(text.contains("{prompt}"), "{text}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // A name that is a built-in's is refused before the rest is asked.
        assert!(matches!(ask(&["claude code"]), Outcome::Refuse(_)));
        // Bad resume arguments are refused with the reason.
        match ask(&["My Tool", "mytool", "", "--resume {id} {x}"]) {
            Outcome::Refuse(text) => assert!(text.contains("{id}"), "{text}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(matches!(
            ask(&["My Tool", "a\nb", "", ""]),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn a_new_session_without_a_selected_worktree_asks_for_one_first() {
        let mut world = world();
        world.here = None;
        let first = advance(Command::NewSession, &[], &world);
        assert_eq!(
            labels(&first),
            ["web / main", "api / main", "api / x", "api / detached"]
        );
        let agent = advance(Command::NewSession, &strings(&["a1"]), &world);
        assert_eq!(labels(&agent)[..3], ["Claude Code", "Codex", "opencode"]);
        let done = advance(Command::NewSession, &strings(&["a1", "claude"]), &world);
        assert_eq!(
            done,
            Outcome::Run(Action::StartSession(SessionIntent {
                agent: AgentId::CLAUDE,
                machine: MachineId::local(),
                machine_name: "Local".into(),
                cwd: "/srv/api-worktrees/x".into(),
                account: None,
            }))
        );
    }

    #[test]
    fn a_new_session_with_no_project_at_all_says_so() {
        let mut world = world();
        world.here = None;
        world.projects.clear();
        assert!(matches!(
            advance(Command::NewSession, &[], &world),
            Outcome::Refuse(why) if why.contains("project")
        ));
        let mut world = self::world();
        world.here = None;
        assert!(matches!(
            advance(Command::NewSession, &strings(&["nope"]), &world),
            Outcome::Refuse(_)
        ));
    }

    fn with_live(busy: bool) -> World {
        world().with_live(
            vec![LiveInfo {
                id: LiveId(7),
                label: "Claude Code".into(),
                busy,
                keeps: false,
            }],
            Some(LiveId(7)),
        )
    }

    #[test]
    fn sleeping_a_running_session_asks_first_and_keeps_it_in_the_sidebar() {
        let world = with_live(true);
        let ask = advance(Command::SleepSession, &[], &world);
        assert_eq!(labels(&ask), ["Sleep Claude Code", "Cancel"]);
        assert_eq!(
            advance(Command::SleepSession, &strings(&["yes"]), &world),
            Outcome::Run(Action::SleepLive(LiveId(7)))
        );
        assert_eq!(
            advance(Command::SleepSession, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
        assert_eq!(
            advance(Command::SleepSession, &[], &with_live(false)),
            Outcome::Run(Action::SleepLive(LiveId(7)))
        );
        assert!(matches!(
            advance(Command::SleepSession, &[], &self::world()),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn closing_a_running_session_asks_first() {
        let world = with_live(true);
        let ask = advance(Command::CloseSession, &[], &world);
        assert_eq!(labels(&ask), ["Close Claude Code", "Cancel"]);
        assert_eq!(
            advance(Command::CloseSession, &strings(&["yes"]), &world),
            Outcome::Run(Action::CloseLive(LiveId(7)))
        );
        assert_eq!(
            advance(Command::CloseSession, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
    }

    #[test]
    fn closing_an_exited_session_does_not_ask() {
        assert_eq!(
            advance(Command::CloseSession, &[], &with_live(false)),
            Outcome::Run(Action::CloseLive(LiveId(7)))
        );
    }

    #[test]
    fn closing_needs_a_live_session() {
        assert!(matches!(
            advance(Command::CloseSession, &[], &world()),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn a_new_worktree_asks_project_then_branch_then_base() {
        let world = world();
        let project = advance(Command::NewWorktree, &[], &world);
        assert_eq!(
            labels(&project),
            ["api", "web"],
            "the project in focus first"
        );
        let branch = advance(Command::NewWorktree, &strings(&["api"]), &world);
        assert!(matches!(
            branch,
            Outcome::Ask(Step {
                kind: StepKind::Text {
                    validate: Validate::Branch,
                    ..
                },
                ..
            })
        ));
        let base = advance(
            Command::NewWorktree,
            &strings(&["api", "feature/x"]),
            &world,
        );
        assert_eq!(labels(&base), ["HEAD", "main", "x"]);
        assert!(matches!(
            base,
            Outcome::Ask(Step {
                kind: StepKind::Choices {
                    custom: Custom::Any,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn a_new_worktree_runs_with_the_base_or_from_head() {
        let world = world();
        let with_base = advance(
            Command::NewWorktree,
            &strings(&["api", "feature/x", "main"]),
            &world,
        );
        assert_eq!(
            with_base,
            Outcome::Run(Action::Engine(Op::AddWorktree {
                project: ProjectId::from_string("api"),
                branch: "feature/x".into(),
                base: Some("main".into())
            }))
        );
        let from_head = advance(
            Command::NewWorktree,
            &strings(&["api", "feature/x", "HEAD"]),
            &world,
        );
        assert!(matches!(
            from_head,
            Outcome::Run(Action::Engine(Op::AddWorktree { base: None, .. }))
        ));
    }

    #[test]
    fn a_new_worktree_needs_a_project() {
        let mut world = world();
        world.projects.clear();
        assert!(matches!(
            advance(Command::NewWorktree, &[], &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn repositories_found_on_a_machine_are_offered_as_folders_once() {
        let mut world = world();
        world.repositories = vec![
            (MachineId::from_string("m2"), "/home/d/api".into()),
            (MachineId::from_string("m2"), "/home/d/web".into()),
            (MachineId::from_string("other"), "/elsewhere".into()),
        ];
        let step = advance(Command::AddProject, &strings(&["m2"]), &world);
        let Outcome::Ask(Step {
            kind: StepKind::Choices { choices, .. },
            ..
        }) = step
        else {
            panic!("a choice was expected");
        };
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"/home/d/api") && values.contains(&"/home/d/web"));
        assert!(!values.contains(&"/elsewhere"));
        assert_eq!(values.iter().filter(|v| **v == "/home/d/api").count(), 1);
    }

    #[test]
    fn adding_a_project_asks_machine_folder_then_name_with_the_folder_as_default() {
        let world = world();
        let machine = advance(Command::AddProject, &[], &world);
        assert_eq!(
            labels(&machine),
            ["build box"],
            "this computer has its own dialog"
        );
        let path = advance(Command::AddProject, &strings(&["m2"]), &world);
        assert!(matches!(
            path,
            Outcome::Ask(Step {
                kind: StepKind::Choices {
                    custom: Custom::AbsolutePath,
                    ..
                },
                ..
            })
        ));
        match advance(Command::AddProject, &strings(&["m2", "/srv/leon"]), &world) {
            Outcome::Ask(Step {
                kind: StepKind::Text { placeholder, .. },
                ..
            }) => assert_eq!(placeholder, "leon"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            advance(
                Command::AddProject,
                &strings(&["m2", "/srv/leon", ""]),
                &world
            ),
            Outcome::Run(Action::Engine(Op::AddProject { .. }))
        ));
    }

    #[test]
    fn the_folder_step_suggests_the_unsorted_folders_of_the_chosen_machine_only() {
        let world = world();
        let remote = advance(Command::AddProject, &strings(&["m2"]), &world);
        assert_eq!(labels(&remote), ["/opt/scratch"]);
        match remote {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => {
                assert_eq!(choices[0].value, "/opt/scratch");
                assert_eq!(choices[0].detail, "1 session ran here");
            }
            other => panic!("unexpected {other:?}"),
        }
        // This computer's folders are never typed.
        assert!(matches!(
            advance(Command::AddProject, &strings(&["local"]), &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn cloning_asks_machine_url_name_and_folder_with_a_smart_name_from_the_url() {
        let world = world();
        let machine = advance(Command::CloneProject, &[], &world);
        assert_eq!(labels(&machine), ["Local", "build box"], "every machine");
        // The name is offered from the URL, as git would have it.
        match advance(
            Command::CloneProject,
            &strings(&["m2", "git@github.com:zavudev/leon.git"]),
            &world,
        ) {
            Outcome::Ask(Step {
                kind: StepKind::Text { placeholder, .. },
                ..
            }) => assert_eq!(placeholder, "leon"),
            other => panic!("unexpected {other:?}"),
        }
        // A remote machine types the folder; a local one picks it.
        assert!(matches!(
            advance(
                Command::CloneProject,
                &strings(&["m2", "git@host:a/b.git", "b"]),
                &world
            ),
            Outcome::Ask(Step {
                kind: StepKind::Text { .. },
                ..
            })
        ));
        assert_eq!(
            advance(
                Command::CloneProject,
                &strings(&["m2", "git@host:a/b.git", "b", "/srv/code"]),
                &world
            ),
            Outcome::Run(Action::Engine(Op::CloneProject {
                machine: MachineId::from_string("m2"),
                url: "git@host:a/b.git".into(),
                parent: "/srv/code".into(),
                name: "b".into(),
            }))
        );
        // On this computer the folder comes from the dialog.
        assert_eq!(
            advance(
                Command::CloneProject,
                &strings(&["local", "git@host:a/b.git", "b", "pick"]),
                &world
            ),
            Outcome::Run(Action::PickCloneParent {
                url: "git@host:a/b.git".into(),
                name: "b".into(),
            })
        );
        // An empty name takes the URL's, like git would name the folder.
        assert_eq!(
            advance(
                Command::CloneProject,
                &strings(&["local", "git@host:a/leon.git", "", "/srv"]),
                &world
            ),
            Outcome::Run(Action::Engine(Op::CloneProject {
                machine: MachineId::local(),
                url: "git@host:a/leon.git".into(),
                parent: "/srv".into(),
                name: "leon".into(),
            }))
        );
        // A name that is not a folder name is refused before anything runs.
        assert!(matches!(
            advance(
                Command::CloneProject,
                &strings(&["local", "git@host:a/b.git", "b/c"]),
                &world
            ),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn creating_a_project_asks_machine_name_and_folder() {
        let world = world();
        assert_eq!(labels(&advance(Command::NewProject, &[], &world)).len(), 2);
        assert!(matches!(
            advance(Command::NewProject, &strings(&["m2", "b c"]), &world),
            Outcome::Ask(Step {
                kind: StepKind::Text { .. },
                ..
            })
        ));
        // On this computer the folder comes from the dialog.
        assert_eq!(
            advance(
                Command::NewProject,
                &strings(&["local", "api", "pick"]),
                &world
            ),
            Outcome::Run(Action::PickProjectParent { name: "api".into() })
        );
        assert_eq!(
            advance(
                Command::NewProject,
                &strings(&["m2", "api", "/srv/code"]),
                &world
            ),
            Outcome::Run(Action::Engine(Op::CreateProject {
                machine: MachineId::from_string("m2"),
                parent: "/srv/code".into(),
                name: "api".into(),
            }))
        );
        assert!(matches!(
            advance(Command::NewProject, &strings(&["local", "a/b"]), &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn a_machine_without_unsorted_folders_still_asks_for_a_typed_path() {
        let mut world = world();
        world.folders.clear();
        match advance(Command::AddProject, &strings(&["m2"]), &world) {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, custom },
                ..
            }) => {
                assert!(choices.is_empty());
                assert_eq!(custom, Custom::AbsolutePath);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn removing_a_project_asks_which_then_to_confirm_and_cancelling_does_nothing() {
        let world = world();
        assert_eq!(
            labels(&advance(Command::RemoveProject, &[], &world)),
            ["web", "api"]
        );
        assert_eq!(
            labels(&advance(Command::RemoveProject, &strings(&["api"]), &world)),
            ["Remove api", "Cancel"]
        );
        assert_eq!(
            advance(Command::RemoveProject, &strings(&["api", "yes"]), &world),
            Outcome::Run(Action::Engine(Op::RemoveProject(ProjectId::from_string(
                "api"
            ))))
        );
        assert_eq!(
            advance(Command::RemoveProject, &strings(&["api", "no"]), &world),
            Outcome::Run(Action::Nothing)
        );
        let mut empty = world;
        empty.projects.clear();
        assert!(matches!(
            advance(Command::RemoveProject, &[], &empty),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn agent_memory_asks_for_a_project_of_this_computer_the_current_one_first() {
        let mut world = world();
        // A project on another machine is not offered.
        let mut far = world.projects[0].clone();
        far.project.id = ProjectId::from_string("far");
        far.project.name = "far".into();
        far.project.machine_id = MachineId::from_string("m2");
        world.projects.push(far);
        world.here = Some(Where {
            machine: MachineId::local(),
            machine_name: "Local".into(),
            project: Some(ProjectId::from_string("api")),
            cwd: "/srv/api".into(),
        });
        for (command, on) in [
            (Command::EnableMemory, true),
            (Command::DisableMemory, false),
        ] {
            assert!(is_flow(command));
            assert_eq!(labels(&advance(command, &[], &world)), ["api", "web"]);
            // What is on already is said, and goes after what is not when
            // turning on, before it when turning off.
            let mut marked = world.clone();
            marked.here = None;
            marked.projects[0].memory_on = true;
            let Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) = advance(command, &[], &marked)
            else {
                panic!("expected the projects");
            };
            let listed: Vec<(&str, &str)> = choices
                .iter()
                .map(|choice| (choice.label.as_str(), choice.detail.as_str()))
                .collect();
            let web = ("web", "/srv/web · memory on");
            let api = ("api", "/srv/api");
            assert_eq!(listed, if on { [api, web] } else { [web, api] });
            assert_eq!(
                advance(command, &strings(&["api"]), &world),
                Outcome::Run(Action::Engine(Op::ProjectMemory {
                    project: ProjectId::from_string("api"),
                    on,
                }))
            );
        }
        world
            .projects
            .retain(|info| !info.project.machine_id.is_local());
        assert!(matches!(
            advance(Command::EnableMemory, &[], &world),
            Outcome::Refuse(why) if why.contains("on this computer")
        ));
    }

    #[test]
    fn removing_a_worktree_lists_only_linked_ones_and_asks_to_confirm() {
        let world = world();
        let list = advance(Command::RemoveWorktree, &[], &world);
        assert_eq!(labels(&list), ["x", "/srv/api-worktrees/detached"]);
        let confirm = advance(Command::RemoveWorktree, &strings(&["api|a1"]), &world);
        assert_eq!(labels(&confirm), ["Remove x", "Cancel"]);
    }

    #[test]
    fn showing_changes_lists_every_worktree_main_ones_included_and_runs_on_the_answer() {
        let world = world();
        let list = advance(Command::OpenChanges, &[], &world);
        assert_eq!(
            labels(&list),
            ["main", "main", "x", "/srv/api-worktrees/detached"]
        );
        assert_eq!(
            advance(Command::OpenChanges, &strings(&["api|a1"]), &world),
            Outcome::Run(Action::ShowChanges {
                project: ProjectId::from_string("api"),
                worktree: WorktreeId::from_string("a1"),
            })
        );
        assert!(matches!(
            advance(Command::OpenChanges, &strings(&["nonsense"]), &world),
            Outcome::Refuse(_)
        ));
        let mut empty = world;
        empty.projects.clear();
        assert!(matches!(
            advance(Command::OpenChanges, &[], &empty),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn confirming_removes_and_cancelling_does_nothing() {
        let world = world();
        assert_eq!(
            advance(
                Command::RemoveWorktree,
                &strings(&["api|a1", "yes"]),
                &world
            ),
            Outcome::Run(Action::RemoveWorktree {
                project: ProjectId::from_string("api"),
                worktree: WorktreeId::from_string("a1"),
                force: false,
            })
        );
        assert_eq!(
            advance(Command::RemoveWorktree, &strings(&["api|a1", "no"]), &world),
            Outcome::Run(Action::Nothing)
        );
    }

    #[test]
    fn a_worktree_git_refused_is_asked_about_again_and_only_force_removes_it() {
        let world = world();
        // The refusal comes back as the question of forcing it, which says
        // what is deleted with it.
        let confirm = advance(
            Command::RemoveWorktree,
            &strings(&["api|a1", REFUSED]),
            &world,
        );
        assert_eq!(labels(&confirm), ["Force remove x", "Cancel"]);
        assert_eq!(
            advance(
                Command::RemoveWorktree,
                &strings(&["api|a1", REFUSED, "yes"]),
                &world
            ),
            Outcome::Run(Action::RemoveWorktree {
                project: ProjectId::from_string("api"),
                worktree: WorktreeId::from_string("a1"),
                force: true,
            })
        );
        assert_eq!(
            advance(
                Command::RemoveWorktree,
                &strings(&["api|a1", REFUSED, "no"]),
                &world
            ),
            Outcome::Run(Action::Nothing),
            "declining the force question keeps the worktree"
        );
    }

    #[test]
    fn with_no_linked_worktree_there_is_nothing_to_remove() {
        let mut world = world();
        world.projects.truncate(1);
        assert!(matches!(
            advance(Command::RemoveWorktree, &[], &world),
            Outcome::Refuse(_)
        ));
    }

    /// The test world with the worktrees `merged` named (by id) merged on
    /// GitHub, and every checkout read as clean with nothing running in it.
    fn cleanup_world(merged: &[&str]) -> World {
        let mut world = world();
        for info in &mut world.projects {
            for worktree in &mut info.worktrees {
                worktree.merged_pull_request = Some(merged.contains(&worktree.id.as_str()));
                world.statuses.insert(
                    worktree.id.clone(),
                    WorktreeStatus {
                        changed: Some(0),
                        divergence: Some(leon_core::Divergence::default()),
                        pull_request: None,
                    },
                );
            }
        }
        world
    }

    #[test]
    fn the_cleanup_offers_only_linked_worktrees_whose_merge_github_reports() {
        let world = cleanup_world(&["a1", "a0"]);
        let list = advance(Command::RemoveMergedWorktrees, &[], &world);
        assert_eq!(labels(&list), ["x", "Remove 0 worktrees", "Cancel"]);
        let mut open = cleanup_world(&[]);
        assert!(matches!(
            advance(Command::RemoveMergedWorktrees, &[], &open),
            Outcome::Refuse(_)
        ));
        open.projects.truncate(1);
    }

    #[test]
    fn a_worktree_with_changes_or_unread_checkout_is_listed_with_why_and_not_ticked() {
        let mut world = cleanup_world(&["a1"]);
        world.statuses.insert(
            WorktreeId::from_string("a1"),
            WorktreeStatus {
                changed: Some(2),
                ..WorktreeStatus::default()
            },
        );
        let list = choices_of(advance(Command::RemoveMergedWorktrees, &[], &world));
        assert!(list[0].dim);
        assert_eq!(list[0].detail, "kept: 2 uncommitted changes");
        assert!(!list[0].current);
        // Nothing was read at all: that is a reason too, never a clean bill.
        world.statuses.clear();
        let list = choices_of(advance(Command::RemoveMergedWorktrees, &[], &world));
        assert_eq!(list[0].detail, "kept: its checkout has not been read yet");
    }

    #[test]
    fn a_session_running_in_a_worktree_keeps_it_out_of_the_removal() {
        let mut world = cleanup_world(&["a1"]);
        world.running.insert(WorktreeId::from_string("a1"), 1);
        let list = choices_of(advance(Command::RemoveMergedWorktrees, &[], &world));
        assert!(list[0].dim);
        assert_eq!(list[0].detail, "kept: a session is running in it");
    }

    #[test]
    fn choosing_a_worktree_ticks_it_and_choosing_it_again_unticks_it() {
        let world = cleanup_world(&["a1"]);
        let ticked = choices_of(advance(
            Command::RemoveMergedWorktrees,
            &strings(&["api|a1"]),
            &world,
        ));
        assert!(ticked[0].current);
        assert_eq!(ticked[1].label, "Remove 1 worktree");
        let unticked = choices_of(advance(
            Command::RemoveMergedWorktrees,
            &strings(&["api|a1", "api|a1"]),
            &world,
        ));
        assert!(!unticked[0].current);
        assert_eq!(unticked[1].label, "Remove 0 worktrees");
    }

    #[test]
    fn a_blocked_worktree_cannot_be_ticked_even_when_chosen() {
        let mut world = cleanup_world(&["a1"]);
        world.running.insert(WorktreeId::from_string("a1"), 1);
        assert_eq!(
            advance(
                Command::RemoveMergedWorktrees,
                &strings(&["api|a1", "yes"]),
                &world
            ),
            Outcome::Refuse("Tick the worktrees to remove first.".to_owned())
        );
    }

    #[test]
    fn removing_runs_exactly_the_ticked_worktrees_in_their_order() {
        let world = cleanup_world(&["a1", "a2"]);
        assert_eq!(
            advance(
                Command::RemoveMergedWorktrees,
                &strings(&["api|a2", "api|a1", "yes"]),
                &world
            ),
            Outcome::Run(Action::RemoveWorktrees(vec![
                (ProjectId::from_string("api"), WorktreeId::from_string("a1")),
                (ProjectId::from_string("api"), WorktreeId::from_string("a2")),
            ]))
        );
        assert_eq!(
            advance(
                Command::RemoveMergedWorktrees,
                &strings(&["api|a2", "yes"]),
                &world
            ),
            Outcome::Run(Action::RemoveWorktrees(vec![(
                ProjectId::from_string("api"),
                WorktreeId::from_string("a2")
            )]))
        );
    }

    #[test]
    fn cancelling_and_removing_nothing_change_nothing() {
        let world = cleanup_world(&["a1"]);
        assert_eq!(
            advance(
                Command::RemoveMergedWorktrees,
                &strings(&["api|a1", "no"]),
                &world
            ),
            Outcome::Run(Action::Nothing)
        );
        assert!(matches!(
            advance(Command::RemoveMergedWorktrees, &strings(&["yes"]), &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn the_merged_banner_is_offered_only_when_the_setting_is_on_and_a_merge_is_new() {
        let worktree = |id: &str, merged: Option<bool>| Worktree {
            id: WorktreeId::from_string(id),
            project_id: ProjectId::from_string("api"),
            path: format!("/srv/{id}"),
            branch: Some(id.to_owned()),
            head: None,
            is_main: false,
            merged_pull_request: merged,
        };
        let before = [worktree("x", Some(false)), worktree("y", Some(false))];
        let after = [worktree("x", Some(true)), worktree("y", Some(false))];
        assert_eq!(merged_offer(false, &before, &after), None, "off by default");
        assert_eq!(
            merged_offer(true, &before, &after),
            Some(vec!["x".to_owned()])
        );
        assert_eq!(
            merged_offer(true, &after, &after),
            None,
            "nothing new merged"
        );
        assert_eq!(
            merged_offer(true, &[], &after),
            None,
            "the first read is silent"
        );
        assert_eq!(
            merged_offer_words(&["x".to_owned()]),
            "Pull request merged: x."
        );
        assert_eq!(
            merged_offer_words(&["x".to_owned(), "y".to_owned()]),
            "2 pull requests merged: x, y."
        );
    }

    #[test]
    fn the_status_line_counts_what_went_and_names_what_was_kept() {
        assert_eq!(removal_words(2, &[]), "Removed 2 worktrees.");
        assert_eq!(removal_words(1, &[]), "Removed 1 worktree.");
        assert_eq!(
            removal_words(
                1,
                &["x (git refused it: it has uncommitted files)".to_owned()]
            ),
            "Removed 1 worktree. Kept 1 worktree: x (git refused it: it has uncommitted files)."
        );
    }

    #[test]
    fn the_theme_and_size_steps_mark_the_value_in_use() {
        let world = world();
        match advance(Command::SetAppearance, &[], &world) {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => {
                let current: Vec<&str> = choices
                    .iter()
                    .filter(|c| c.current)
                    .map(|c| c.label.as_str())
                    .collect();
                assert_eq!(current, ["Dark"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            advance(Command::SetAppearance, &strings(&["light"]), &world),
            Outcome::Run(Action::SetAppearance(AppearanceChoice::Light))
        );
        assert_eq!(
            labels(&advance(Command::SetInterfaceSize, &[], &world)),
            ["80%", "90%", "100%", "110%", "125%"]
        );
        assert_eq!(
            advance(Command::SetInterfaceSize, &strings(&["110"]), &world),
            Outcome::Run(Action::SetScale(110))
        );
    }

    #[test]
    fn the_theme_step_lists_every_theme_with_swatches_and_marks_the_one_in_use() {
        let mut world = world();
        world.theme_id = ThemeId::Zavu;
        match advance(Command::ChooseTheme, &[], &world) {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => {
                let names: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
                assert_eq!(names, ["Leon", "Zavu"]);
                assert!(choices.iter().all(|c| c.swatch.is_some()));
                let current: Vec<&str> = choices
                    .iter()
                    .filter(|c| c.current)
                    .map(|c| c.label.as_str())
                    .collect();
                assert_eq!(current, ["Zavu"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            advance(Command::ChooseTheme, &strings(&["leon"]), &world),
            Outcome::Run(Action::SetThemeId(ThemeId::Leon))
        );
        assert!(matches!(
            advance(Command::ChooseTheme, &strings(&["leon-lime"]), &world),
            Outcome::Refuse(_)
        ));
        assert!(matches!(
            advance(Command::ChooseTheme, &strings(&["nope"]), &world),
            Outcome::Refuse(_)
        ));
    }

    fn def(key: &str) -> &'static Def {
        schema::find(key).unwrap()
    }

    #[test]
    fn a_toggle_asks_for_on_or_off_and_sets_the_answer() {
        let d = def("terminal_copy_on_select");
        let asked = advance_setting(d, &[], &Value::Bool(false));
        assert_eq!(labels(&asked), ["On", "Off"]);
        assert_eq!(
            advance_setting(d, &strings(&["on"]), &Value::Bool(false)),
            Outcome::Run(Action::SetSetting(d.key, Value::Bool(true)))
        );
    }

    #[test]
    fn a_choice_lists_its_answers_with_the_current_one_marked() {
        let d = def("terminal_cursor");
        let Outcome::Ask(step) = advance_setting(d, &[], &Value::Text("beam".into())) else {
            panic!("asks");
        };
        let StepKind::Choices { choices, .. } = step.kind else {
            panic!("choices");
        };
        let current: Vec<&str> = choices
            .iter()
            .filter(|c| c.current)
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(current, ["Beam"]);
        assert_eq!(
            advance_setting(d, &strings(&["underline"]), &Value::Text("beam".into())),
            Outcome::Run(Action::SetSetting(d.key, Value::Text("underline".into())))
        );
    }

    #[test]
    fn a_number_is_typed_and_checked_against_its_range() {
        let d = def("terminal_font_size");
        let Outcome::Ask(step) = advance_setting(d, &[], &Value::Int(13)) else {
            panic!("asks");
        };
        assert!(matches!(
            step.kind,
            StepKind::Text {
                validate: Validate::Number { min: 8, max: 32 },
                ..
            }
        ));
        assert!(validate(Validate::Number { min: 8, max: 32 }, "40").is_err());
        assert_eq!(
            validate(Validate::Number { min: 8, max: 32 }, "14px"),
            Ok("14".into())
        );
        assert_eq!(
            advance_setting(d, &strings(&["14"]), &Value::Int(13)),
            Outcome::Run(Action::SetSetting(d.key, Value::Int(14)))
        );
    }

    #[test]
    fn a_list_adds_removes_and_clears_entries() {
        let d = def("terminal_env");
        let none = Value::List(Vec::new());
        assert_eq!(labels(&advance_setting(d, &[], &none)), ["Add an entry"]);
        let one = Value::List(vec!["A=1".into()]);
        assert_eq!(
            labels(&advance_setting(d, &[], &one)),
            ["Add an entry", "Remove an entry", "Remove them all"]
        );
        assert_eq!(
            advance_setting(d, &strings(&["add", "B=2"]), &one),
            Outcome::Run(Action::SetSetting(
                d.key,
                Value::List(vec!["A=1".into(), "B=2".into()])
            ))
        );
        assert_eq!(
            advance_setting(d, &strings(&["remove", "0"]), &one),
            Outcome::Run(Action::SetSetting(d.key, Value::List(Vec::new())))
        );
        assert_eq!(
            advance_setting(d, &strings(&["clear"]), &one),
            Outcome::Run(Action::SetSetting(d.key, Value::List(Vec::new())))
        );
    }

    #[test]
    fn a_button_runs_and_reset_all_asks_first() {
        let d = def("reimport");
        assert_eq!(
            advance_setting(d, &[], &Value::Bool(false)),
            Outcome::Run(Action::Button("reimport"))
        );
        let reset = def("reset_all");
        assert_eq!(
            labels(&advance_setting(reset, &[], &Value::Bool(false))),
            ["Reset every setting", "Cancel"]
        );
        assert_eq!(
            advance_setting(reset, &strings(&["no"]), &Value::Bool(false)),
            Outcome::Run(Action::Nothing)
        );
        assert_eq!(
            advance_setting(reset, &strings(&["yes"]), &Value::Bool(false)),
            Outcome::Run(Action::Button("reset_all"))
        );
    }

    #[test]
    fn typed_answers_are_checked_by_their_validator() {
        assert_eq!(validate(Validate::Required, "  box "), Ok("box".into()));
        assert!(validate(Validate::Required, "  ").is_err());
        assert_eq!(validate(Validate::Optional, " "), Ok(String::new()));
        assert!(validate(Validate::Branch, "-x").is_err());
        assert!(validate(Validate::Branch, "feature/x").is_ok());
    }

    #[test]
    fn only_the_commands_with_steps_are_flows() {
        for command in [
            Command::NewSession,
            Command::NewWorktree,
            Command::AddProject,
            Command::RemoveProject,
            Command::RemoveWorktree,
            Command::SetAppearance,
            Command::ChooseTheme,
            Command::SetInterfaceSize,
        ] {
            assert!(is_flow(command));
        }
        for command in [
            Command::Refresh,
            Command::ToggleAppearance,
            Command::Shortcuts,
            Command::OpenProject,
            Command::Settings,
        ] {
            assert!(!is_flow(command));
        }
    }

    #[test]
    fn a_world_is_built_from_a_snapshot_with_the_local_machine_first() {
        let store = leon_core::Store::open_in_memory().unwrap();
        store
            .add_machine(
                "zed",
                MachineKind::Ssh {
                    host: "z".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();
        let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        store
            .upsert_session(
                &leon_core::NewSession {
                    agent: AgentId::CLAUDE,
                    external_id: "s".into(),
                    machine_id: MachineId::local(),
                    cwd: "/home/me/notes".into(),
                    title: "notes".into(),
                    model: None,
                    started_at: at,
                    updated_at: at,
                },
                &[],
            )
            .unwrap();
        let snapshot = Snapshot::load(&store).unwrap();
        let placement = Placement::compute(&snapshot);
        let world = World::build(
            &snapshot,
            &placement,
            None,
            None,
            AppearanceChoice::Dark,
            ThemeId::Leon,
            100,
        );
        assert!(world.machines[0].id.is_local());
        assert_eq!(world.projects.len(), 1);
        assert_eq!(world.projects[0].machine_name, "This machine");
        assert_eq!(
            world.folders,
            [FolderInfo {
                machine: MachineId::local(),
                cwd: "/home/me/notes".into(),
                sessions: 1
            }]
        );
    }

    #[test]
    fn adding_a_project_by_path_is_for_remote_machines_only() {
        let mut world = world();
        world
            .machines
            .retain(|machine| machine.kind == MachineKind::Local);
        assert!(matches!(
            advance(Command::AddProject, &[], &world),
            Outcome::Refuse(why) if why.contains("Open project")
        ));
    }

    #[test]
    fn renaming_asks_for_the_new_name_and_names_what_it_renames() {
        let mut world = world();
        world.rename = Some(RenameTarget::Live(LiveId(4), "Shell".into()));
        match advance(Command::Rename, &[], &world) {
            Outcome::Ask(Step {
                kind: StepKind::Text { placeholder, .. },
                ..
            }) => {
                assert_eq!(placeholder, "Shell")
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            advance(Command::Rename, &strings(&["build"]), &world),
            Outcome::Run(Action::RenameLive(LiveId(4), "build".into()))
        );
        world.rename = Some(RenameTarget::Machine(
            MachineId::from_string("m2"),
            "box".into(),
        ));
        assert_eq!(
            advance(Command::Rename, &strings(&["staging"]), &world),
            Outcome::Run(Action::Engine(Op::RenameMachine {
                machine: MachineId::from_string("m2"),
                name: "staging".into()
            }))
        );
        world.rename = None;
        assert!(matches!(
            advance(Command::Rename, &[], &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn renaming_a_session_shows_old_and_new_and_asks_before_it_does() {
        let mut world = world();
        let id = SessionId::from_string("s1");
        world.rename = Some(RenameTarget::Session(
            id.clone(),
            "old name".into(),
            Some(LiveId(2)),
        ));
        assert!(matches!(
            advance(Command::Rename, &[], &world),
            Outcome::Ask(Step {
                kind: StepKind::Text { .. },
                ..
            })
        ));
        let confirm = advance(Command::Rename, &strings(&["new name"]), &world);
        assert_eq!(
            labels(&confirm),
            ["Rename to \"new name\"", "Cancel"],
            "{confirm:?}"
        );
        assert_eq!(
            advance(Command::Rename, &strings(&["new name", "yes"]), &world),
            Outcome::Run(Action::RenameSession(
                id,
                Some(LiveId(2)),
                "new name".into()
            ))
        );
        assert_eq!(
            advance(Command::Rename, &strings(&["new name", "no"]), &world),
            Outcome::Run(Action::Nothing)
        );
    }

    #[test]
    fn removing_a_machine_asks_to_confirm_and_never_offers_the_local_one() {
        let mut world = world();
        world.machine_row = Some(MachineId::from_string("m2"));
        assert_eq!(
            labels(&advance(Command::RemoveMachine, &[], &world)),
            ["Remove build box", "Cancel"]
        );
        assert_eq!(
            advance(Command::RemoveMachine, &strings(&["yes"]), &world),
            Outcome::Run(Action::Engine(Op::RemoveMachine(MachineId::from_string(
                "m2"
            ))))
        );
        assert_eq!(
            advance(Command::RemoveMachine, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
        world.machine_row = Some(MachineId::local());
        assert!(matches!(
            advance(Command::RemoveMachine, &[], &world),
            Outcome::Refuse(_)
        ));
        world.machine_row = None;
        assert!(matches!(
            advance(Command::RemoveMachine, &[], &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn removing_from_the_history_asks_to_confirm_and_names_the_session() {
        let mut world = world();
        world.here_session = Some((SessionId::from_string("s1"), "fix the bug".into()));
        assert_eq!(
            labels(&advance(Command::RemoveFromHistory, &[], &world)),
            ["Remove \"fix the bug\"", "Cancel"]
        );
        assert_eq!(
            advance(Command::RemoveFromHistory, &strings(&["yes"]), &world),
            Outcome::Run(Action::Engine(Op::RemoveSession(SessionId::from_string(
                "s1"
            ))))
        );
        world.here_session = None;
        assert!(matches!(
            advance(Command::RemoveFromHistory, &[], &world),
            Outcome::Refuse(_)
        ));
    }

    fn elsewhere_world(likely: bool) -> World {
        World {
            elsewhere: Some(ElsewhereTarget {
                session: SessionId::from_string("s1"),
                title: "Fix the build".into(),
                pid: 70645,
                likely,
                other_leon: false,
                can_take_over: true,
            }),
            here_session: Some((SessionId::from_string("s1"), "Fix the build".into())),
            ..world()
        }
    }

    #[test]
    fn resuming_a_session_held_elsewhere_offers_the_transcript_first() {
        for other_leon in [false, true] {
            let mut world = elsewhere_world(false);
            world.elsewhere.as_mut().unwrap().other_leon = other_leon;
            world.elsewhere.as_mut().unwrap().can_take_over = false;
            let Outcome::Ask(step) = advance(Command::ResumeSession, &[], &world) else {
                panic!("a question")
            };
            assert!(step.prompt.contains(if other_leon {
                "another Leon"
            } else {
                "another terminal"
            }));
            let StepKind::Choices { choices, .. } = step.kind else {
                panic!("choices")
            };
            assert_eq!(choices[0].label, "Open transcript");
            assert!(choices[1].label.contains("anyway"));
            assert_eq!(choices[2].label, "Cancel");
            assert_eq!(
                advance(Command::ResumeSession, &strings(&["transcript"]), &world),
                Outcome::Run(Action::OpenTranscript)
            );
            assert_eq!(
                advance(Command::ResumeSession, &strings(&["yes"]), &world),
                Outcome::Run(Action::ResumeAnyway(SessionId::from_string("s1")))
            );
            assert_eq!(
                advance(Command::ResumeSession, &strings(&["no"]), &world),
                Outcome::Run(Action::Nothing)
            );
        }
    }

    #[test]
    fn resuming_anyway_needs_a_session_that_runs_elsewhere() {
        assert!(matches!(
            advance(Command::ResumeAnyway, &[], &world()),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn a_certain_match_asks_first_and_the_default_answer_leaves_it_alone() {
        let world = elsewhere_world(false);
        let Outcome::Ask(step) = advance(Command::ResumeAnyway, &[], &world) else {
            panic!("a question")
        };
        let StepKind::Choices { choices, .. } = step.kind else {
            panic!("choices")
        };
        assert_eq!(choices[0].label, "Cancel");
        assert!(choices[1].label.contains("here anyway"));
        assert!(choices[1].detail.contains("70645"));
        assert!(choices[1].detail.contains("corrupt"));
        assert_eq!(
            advance(Command::ResumeAnyway, &strings(&["yes"]), &world),
            Outcome::Run(Action::ResumeAnyway(SessionId::from_string("s1")))
        );
        assert_eq!(
            advance(Command::ResumeAnyway, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
    }

    #[test]
    fn a_likely_match_resumes_without_a_question() {
        assert_eq!(
            advance(Command::ResumeAnyway, &[], &elsewhere_world(true)),
            Outcome::Run(Action::ResumeAnyway(SessionId::from_string("s1")))
        );
    }

    fn take_over_world(can_take_over: bool) -> World {
        let mut world = elsewhere_world(false);
        let held = world.elsewhere.as_mut().unwrap();
        held.other_leon = true;
        held.can_take_over = can_take_over;
        world
    }

    #[test]
    fn the_resume_question_offers_to_take_over_only_where_leon_can_signal() {
        let values = |can| {
            let Outcome::Ask(step) = advance(Command::ResumeSession, &[], &take_over_world(can))
            else {
                panic!("a question")
            };
            let StepKind::Choices { choices, .. } = step.kind else {
                panic!("choices")
            };
            choices.into_iter().map(|c| c.value).collect::<Vec<_>>()
        };
        assert_eq!(values(true), ["transcript", "yes", "takeover", "no"]);
        assert_eq!(values(false), ["transcript", "yes", "no"]);
    }

    #[test]
    fn taking_over_asks_first_and_the_first_answer_leaves_it_alone() {
        let world = take_over_world(true);
        let Outcome::Ask(step) = advance(Command::TakeOver, &[], &world) else {
            panic!("a question")
        };
        assert!(step.prompt.contains("other Leon"), "{}", step.prompt);
        let StepKind::Choices { choices, .. } = step.kind else {
            panic!("choices")
        };
        assert_eq!(choices[0].value, "no", "Enter leaves it alone");
        assert_eq!(
            advance(Command::TakeOver, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
        let take = Outcome::Run(Action::TakeOver(SessionId::from_string("s1")));
        assert_eq!(advance(Command::TakeOver, &strings(&["yes"]), &world), take);
        assert_eq!(
            advance(Command::ResumeSession, &strings(&["takeover"]), &world),
            take
        );
    }

    #[test]
    fn taking_over_is_refused_when_leon_cannot_end_the_process() {
        assert!(matches!(
            advance(Command::TakeOver, &[], &take_over_world(false)),
            Outcome::Refuse(_)
        ));
        assert!(matches!(
            advance(Command::TakeOver, &[], &world()),
            Outcome::Refuse(_)
        ));
        // A forged answer does not slip past the check.
        assert_eq!(
            advance(
                Command::ResumeSession,
                &strings(&["takeover"]),
                &take_over_world(false)
            ),
            Outcome::Run(Action::Nothing)
        );
    }

    fn with(prefs: Prefs) -> World {
        let mut world = world();
        world.here = Some(Where {
            machine: MachineId::local(),
            machine_name: "Local".into(),
            project: None,
            cwd: "/srv/api".into(),
        });
        world.prefs = prefs;
        world
    }

    #[test]
    fn a_disabled_agent_is_not_offered_for_new_sessions() {
        let world = with(Prefs {
            agents_enabled: vec![AgentId::CLAUDE, AgentId::OPENCODE],
            ..Prefs::default()
        });
        assert_eq!(
            labels(&advance(Command::NewSession, &[], &world)),
            ["Claude Code", "opencode"]
        );
        assert!(matches!(
            advance(Command::NewSession, &strings(&["codex"]), &world),
            Outcome::Refuse(_)
        ));
        let none = with(Prefs {
            agents_enabled: vec![],
            ..Prefs::default()
        });
        assert!(matches!(
            advance(Command::NewSession, &[], &none),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn the_default_agent_starts_without_asking_unless_it_is_disabled() {
        let world = with(Prefs {
            default_agent: Some(AgentId::CODEX),
            ..Prefs::default()
        });
        assert!(matches!(
            advance(Command::NewSession, &[], &world),
            Outcome::Run(Action::StartSession(SessionIntent {
                agent: AgentId::CODEX,
                ..
            }))
        ));
        let off = with(Prefs {
            default_agent: Some(AgentId::CODEX),
            agents_enabled: vec![AgentId::CLAUDE, AgentId::OPENCODE],
            ..Prefs::default()
        });
        assert!(matches!(
            advance(Command::NewSession, &[], &off),
            Outcome::Ask(_)
        ));
    }

    #[test]
    fn quitting_asks_by_the_setting() {
        let mut busy = with(Prefs::default());
        busy.live = vec![LiveInfo {
            id: LiveId(1),
            label: "Claude Code".into(),
            busy: true,
            keeps: false,
        }];
        assert!(matches!(
            advance(Command::Quit, &[], &busy),
            Outcome::Ask(_)
        ));
        busy.prefs.quit = QuitConfirm::Never;
        assert_eq!(
            advance(Command::Quit, &[], &busy),
            Outcome::Run(Action::Quit)
        );
        let mut idle = with(Prefs {
            quit: QuitConfirm::Always,
            ..Prefs::default()
        });
        assert!(matches!(
            advance(Command::Quit, &[], &idle),
            Outcome::Ask(_)
        ));
        idle.prefs.quit = QuitConfirm::Running;
        assert_eq!(
            advance(Command::Quit, &[], &idle),
            Outcome::Run(Action::Quit)
        );
    }

    fn held_session(id: u64, busy: bool) -> LiveInfo {
        LiveInfo {
            keeps: true,
            ..session(id, busy)
        }
    }

    #[test]
    fn sessions_the_keeper_holds_are_no_reason_to_ask_before_quitting() {
        let mut all_held = with(Prefs::default());
        all_held.live = vec![held_session(1, true), held_session(2, true)];
        assert_eq!(
            advance(Command::Quit, &[], &all_held),
            Outcome::Run(Action::Quit),
            "nothing is lost: they keep running"
        );
        // One that lives in the window is.
        all_held.live.push(session(3, true));
        assert!(matches!(
            advance(Command::Quit, &[], &all_held),
            Outcome::Ask(_)
        ));
    }

    #[test]
    fn the_quit_question_says_which_sessions_keep_running() {
        let mut world = with(Prefs {
            quit: QuitConfirm::Always,
            ..Prefs::default()
        });
        world.live = vec![held_session(1, true), held_session(2, true)];
        let text = labels(&advance(Command::Quit, &[], &world));
        assert!(
            text[0].contains("2 running sessions keep running"),
            "{text:?}"
        );
        world.live.push(session(3, true));
        let text = labels(&advance(Command::Quit, &[], &world));
        assert!(
            text[0].contains("1 running session will be closed"),
            "{text:?}"
        );
        assert!(
            text[0].contains("2 running sessions keep running"),
            "{text:?}"
        );
        world.live = vec![held_session(1, true)];
        let text = labels(&advance(Command::Quit, &[], &world));
        assert!(
            text[0].contains("1 running session keeps running"),
            "{text:?}"
        );
    }

    #[test]
    fn quit_and_end_closes_everything_and_says_so() {
        let mut world = with(Prefs::default());
        world.live = vec![held_session(1, true), session(2, true)];
        let text = labels(&advance(Command::QuitAndEnd, &[], &world));
        assert!(text[0].contains("End 2 running sessions"), "{text:?}");
        assert_eq!(
            advance(Command::QuitAndEnd, &strings(&["yes"]), &world),
            Outcome::Run(Action::QuitAndEnd)
        );
        assert_eq!(
            advance(Command::QuitAndEnd, &strings(&["no"]), &world),
            Outcome::Run(Action::Nothing)
        );
        // With nothing running and no question asked it just goes.
        let idle = with(Prefs::default());
        assert_eq!(
            advance(Command::QuitAndEnd, &[], &idle),
            Outcome::Run(Action::QuitAndEnd)
        );
        // Held sessions ask here, unlike a plain quit: they are ended.
        let mut only_held = with(Prefs::default());
        only_held.live = vec![held_session(1, true)];
        assert!(matches!(
            advance(Command::QuitAndEnd, &[], &only_held),
            Outcome::Ask(_)
        ));
    }

    #[test]
    fn the_restart_for_an_update_does_not_count_what_the_keeper_holds() {
        let mut world = with(Prefs::default());
        world.update_ready = Some("0.7.0".into());
        world.live = vec![held_session(1, true)];
        let text = labels(&advance(Command::RestartToUpdate, &[], &world));
        assert!(!text[0].contains("will be closed"), "{text:?}");
    }

    fn session(id: u64, busy: bool) -> LiveInfo {
        LiveInfo {
            id: LiveId(id),
            label: "Claude Code".into(),
            busy,
            keeps: false,
        }
    }

    fn first_choice(world: &World) -> String {
        match advance(Command::RestartToUpdate, &[], world) {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => choices[0].label.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn restarting_to_update_needs_an_update_that_is_ready() {
        let world = with(Prefs::default());
        assert!(matches!(
            advance(Command::RestartToUpdate, &[], &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn restarting_to_update_always_asks_and_names_what_it_closes() {
        let mut world = with(Prefs::default()).with_update_ready(Some("0.2.1".into()));
        assert_eq!(first_choice(&world), "Restart Leon and update to 0.2.1");
        world.live = vec![session(1, true)];
        assert_eq!(
            first_choice(&world),
            "Restart and update to 0.2.1: 1 running session will be closed."
        );
        world.live = vec![session(1, true), session(2, true), session(3, false)];
        assert_eq!(
            first_choice(&world),
            "Restart and update to 0.2.1: 2 running sessions will be closed."
        );
        world.live = vec![session(1, false)];
        assert_eq!(
            first_choice(&world),
            "Restart and update to 0.2.1: 1 open terminal will be closed."
        );
        world.live = vec![session(1, false), session(2, false)];
        assert_eq!(
            first_choice(&world),
            "Restart and update to 0.2.1: 2 open terminals will be closed."
        );
        // The quit setting does not skip this question: a restart is not a quit.
        world.prefs.quit = QuitConfirm::Never;
        assert!(matches!(
            advance(Command::RestartToUpdate, &[], &world),
            Outcome::Ask(_)
        ));
    }

    #[test]
    fn the_answer_to_the_restart_question_is_read() {
        let world = with(Prefs::default()).with_update_ready(Some("0.2.1".into()));
        assert_eq!(
            advance(Command::RestartToUpdate, &["yes".into()], &world),
            Outcome::Run(Action::RestartToUpdate)
        );
        assert_eq!(
            advance(Command::RestartToUpdate, &["no".into()], &world),
            Outcome::Run(Action::Nothing)
        );
    }

    #[test]
    fn closing_a_running_pane_asks_only_when_the_setting_says_so() {
        let mut world = with(Prefs::default());
        world.live = vec![LiveInfo {
            id: LiveId(1),
            label: "Claude Code".into(),
            busy: true,
            keeps: false,
        }];
        world.here_live = Some(LiveId(1));
        assert!(matches!(
            advance(Command::CloseSession, &[], &world),
            Outcome::Ask(_)
        ));
        world.prefs.confirm_close = false;
        assert_eq!(
            advance(Command::CloseSession, &[], &world),
            Outcome::Run(Action::CloseLive(LiveId(1)))
        );
    }

    // ----- the Den's lions -------------------------------------------------------------

    fn lion(live: Option<LiveId>) -> LionInfo {
        LionInfo {
            name: "MOSS".into(),
            live,
            elsewhere: None,
            no_message: None,
            queued: Vec::new(),
            cwd: None,
        }
    }

    #[test]
    fn a_message_to_a_lion_asks_for_its_text_and_is_refused_with_the_reason() {
        // Nobody is selected.
        assert_eq!(
            advance(Command::MessageLion, &[], &world()),
            Outcome::Refuse("Select a lion in the Den first.".into())
        );
        let known = world().with_lion(Some(lion(Some(LiveId(3)))));
        let Outcome::Ask(step) = advance(Command::MessageLion, &[], &known) else {
            panic!("a question");
        };
        assert_eq!(step.prompt, "Message");
        assert_eq!(
            step.kind,
            StepKind::Text {
                placeholder: "Message to MOSS".into(),
                validate: Validate::Message
            }
        );
        assert_eq!(
            advance(Command::MessageLion, &["run the tests".into()], &known),
            Outcome::Run(Action::MessageLive(LiveId(3), "run the tests".into()))
        );
        // One that cannot be messaged says why, before anything is asked.
        let mut paused = lion(Some(LiveId(3)));
        paused.no_message = Some("MOSS is paused.".into());
        assert_eq!(
            advance(Command::MessageLion, &[], &world().with_lion(Some(paused))),
            Outcome::Refuse("MOSS is paused.".into())
        );
        // One that runs elsewhere has no terminal here.
        let mut away = lion(None);
        away.elsewhere = Some("MOSS runs elsewhere.".into());
        for command in [Command::MessageLion, Command::SendLionHome] {
            assert_eq!(
                advance(command, &[], &world().with_lion(Some(away.clone()))),
                Outcome::Refuse("MOSS runs elsewhere.".into())
            );
        }
    }

    #[test]
    fn sending_a_lion_home_always_asks_and_says_what_it_does() {
        // Not busy, and the setting says not to ask about closing: it asks
        // all the same.
        let prefs = Prefs {
            confirm_close: false,
            ..Prefs::default()
        };
        let known = with(prefs).with_lion(Some(lion(Some(LiveId(3)))));
        let Outcome::Ask(step) = advance(Command::SendLionHome, &[], &known) else {
            panic!("a question");
        };
        let StepKind::Choices { choices, custom } = step.kind else {
            panic!("choices");
        };
        assert_eq!(custom, Custom::No);
        assert_eq!(choices[0].label, "Send MOSS home");
        assert_eq!(
            choices[0].detail,
            "stops its agent now; the session stays in the sidebar, asleep, and you can wake it where it left off"
        );
        assert_eq!(
            (choices[1].label.as_str(), choices[1].detail.as_str()),
            ("Cancel", "keep it working")
        );
        assert_eq!(
            advance(Command::SendLionHome, &["yes".into()], &known),
            Outcome::Run(Action::SendHome(LiveId(3)))
        );
        assert_eq!(
            advance(Command::SendLionHome, &["no".into()], &known),
            Outcome::Run(Action::Nothing)
        );
        assert_eq!(
            advance(Command::SendLionHome, &[], &world()),
            Outcome::Refuse("Select a lion in the Den first.".into())
        );
    }

    fn pride_lion(id: u64, name: &str, post: Option<Post>) -> PrideLion {
        PrideLion {
            id,
            name: name.into(),
            state: "Thinking".into(),
            place: "leon".into(),
            needs: false,
            post,
        }
    }

    fn pride() -> PrideInfo {
        PrideInfo {
            open: true,
            lions: vec![
                pride_lion(1, "MOSS", Some(Post::Now)),
                pride_lion(2, "FERN", Some(Post::Later)),
                pride_lion(3, "ASH", Some(Post::Never("it is paused".into()))),
                pride_lion(4, "WREN", None),
            ],
            home: vec![("s:abc".into(), "Fix the build".into())],
        }
    }

    fn rows_of(outcome: &Outcome) -> Vec<(String, String, String)> {
        let Outcome::Ask(Step {
            kind: StepKind::Choices { choices, .. },
            ..
        }) = outcome
        else {
            panic!("expected choices, got {outcome:?}");
        };
        choices
            .iter()
            .map(|choice| {
                (
                    choice.label.clone(),
                    choice.detail.clone(),
                    choice.value.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn the_queued_messages_of_a_lion_are_listed_and_one_or_all_are_taken_back() {
        let mut waiting = lion(Some(LiveId(3)));
        // Nothing waits: nothing to list.
        assert_eq!(
            advance(
                Command::QueuedMessages,
                &[],
                &world().with_lion(Some(waiting.clone()))
            ),
            Outcome::Refuse("No message is queued for MOSS.".into())
        );
        waiting.queued = vec!["run the\ntests".into(), "then commit".into()];
        let known = world().with_lion(Some(waiting));
        let rows = rows_of(&advance(Command::QueuedMessages, &[], &known));
        let labels: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
        assert_eq!(
            labels,
            [
                "1. run the tests",
                "2. then commit",
                "Take all 2 back",
                "Keep them"
            ]
        );
        assert_eq!(rows[0].1, "take this one back: it will not be typed");
        assert_eq!(
            advance(Command::QueuedMessages, &strings(&["1"]), &known),
            Outcome::Run(Action::CancelQueued(LiveId(3), Some(1)))
        );
        assert_eq!(
            advance(Command::QueuedMessages, &strings(&["all"]), &known),
            Outcome::Run(Action::CancelQueued(LiveId(3), None))
        );
        for kept in ["keep", "9"] {
            assert_eq!(
                advance(Command::QueuedMessages, &strings(&[kept]), &known),
                Outcome::Run(Action::Nothing)
            );
        }
        assert_eq!(
            advance(Command::QueuedMessages, &[], &world()),
            Outcome::Refuse("Select a lion in the Den first.".into())
        );
    }

    #[test]
    fn a_message_to_the_pride_says_who_gets_it_now_who_later_and_who_is_left_out() {
        // The Den is closed: there is no pride to message.
        assert_eq!(
            advance(Command::MessagePride, &[], &world()),
            Outcome::Refuse("Open the Den first: a message goes to its lions.".into())
        );
        let known = world().with_pride(pride());
        let who = rows_of(&advance(Command::MessagePride, &[], &known));
        assert_eq!(
            who.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
            [
                "Those that wait at their prompt (1)",
                "All that can be messaged (2)",
                "Choose lions\u{2026}"
            ]
        );
        assert_eq!(
            (who[0].1.as_str(), who[1].1.as_str()),
            ("MOSS", "MOSS, FERN")
        );
        // The message is asked next, and may have several lines.
        let Outcome::Ask(step) = advance(Command::MessagePride, &strings(&["all"]), &known) else {
            panic!("a question");
        };
        assert_eq!(
            step.kind,
            StepKind::Text {
                placeholder: "Message to MOSS, FERN".into(),
                validate: Validate::Message
            }
        );
        // Then what will happen, before anything is typed.
        let confirm = rows_of(&advance(
            Command::MessagePride,
            &strings(&["all", "pull and rebuild"]),
            &known,
        ));
        assert_eq!(confirm[0].0, "Send to 2 lions");
        assert_eq!(
            confirm[0].1,
            "typed now into MOSS; queued for FERN until each waits at its prompt; \
             left out: ASH (it is paused); WREN (runs elsewhere)"
        );
        assert_eq!(
            (confirm[1].0.as_str(), confirm[1].1.as_str()),
            ("Cancel", "nothing is typed anywhere")
        );
        assert_eq!(
            advance(
                Command::MessagePride,
                &strings(&["all", "pull and rebuild", "yes"]),
                &known
            ),
            Outcome::Run(Action::MessagePride(
                vec![LiveId(1), LiveId(2)],
                "pull and rebuild".into()
            ))
        );
        assert_eq!(
            advance(
                Command::MessagePride,
                &strings(&["all", "pull and rebuild", "no"]),
                &known
            ),
            Outcome::Run(Action::Nothing)
        );
        // Only those that wait.
        assert_eq!(
            advance(
                Command::MessagePride,
                &strings(&["waiting", "go on", "yes"]),
                &known
            ),
            Outcome::Run(Action::MessagePride(vec![LiveId(1)], "go on".into()))
        );
        let one = rows_of(&advance(
            Command::MessagePride,
            &strings(&["waiting", "go on"]),
            &known,
        ));
        assert_eq!(one[0].0, "Send to 1 lion");
    }

    #[test]
    fn the_lions_of_a_message_can_be_chosen_one_at_a_time_and_never_those_that_cannot_be_messaged()
    {
        let known = world().with_pride(pride());
        let first = rows_of(&advance(
            Command::MessagePride,
            &strings(&["choose"]),
            &known,
        ));
        // Only those that can be messaged are on offer, with what would happen.
        assert_eq!(
            first
                .iter()
                .map(|row| (row.0.as_str(), row.1.as_str()))
                .collect::<Vec<_>>(),
            [
                ("MOSS", "Thinking, leon: typed at once"),
                ("FERN", "Thinking, leon: queued until it waits")
            ]
        );
        // One chosen: the other is still on offer, under the way on.
        let second = rows_of(&advance(
            Command::MessagePride,
            &strings(&["choose", "2"]),
            &known,
        ));
        assert_eq!(
            second.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
            ["Done: 1 chosen", "MOSS"]
        );
        assert_eq!(second[0].1, "FERN");
        // Done: the message, then the confirmation, which leaves nobody out
        // unasked for.
        let Outcome::Ask(step) = advance(
            Command::MessagePride,
            &strings(&["choose", "2", "done"]),
            &known,
        ) else {
            panic!("a question");
        };
        assert!(matches!(
            step.kind,
            StepKind::Text { ref placeholder, .. } if placeholder == "Message to FERN"
        ));
        let confirm = rows_of(&advance(
            Command::MessagePride,
            &strings(&["choose", "2", "done", "stop"]),
            &known,
        ));
        assert_eq!(
            confirm[0].1,
            "queued for FERN until each waits at its prompt"
        );
        assert_eq!(
            advance(
                Command::MessagePride,
                &strings(&["choose", "2", "done", "stop", "yes"]),
                &known
            ),
            Outcome::Run(Action::MessagePride(vec![LiveId(2)], "stop".into()))
        );
        // Everybody chosen: the message is asked without "done".
        assert!(matches!(
            advance(
                Command::MessagePride,
                &strings(&["choose", "1", "2"]),
                &known
            ),
            Outcome::Ask(Step {
                kind: StepKind::Text { .. },
                ..
            })
        ));
        // One that cannot be messaged is no answer.
        assert_eq!(
            advance(Command::MessagePride, &strings(&["choose", "3"]), &known),
            Outcome::Run(Action::Nothing)
        );
        // Nobody can be messaged at all.
        let mut none = pride();
        none.lions.clear();
        assert!(matches!(
            advance(Command::MessagePride, &[], &world().with_pride(none)),
            Outcome::Refuse(why) if why.starts_with("No lion can be messaged")
        ));
    }

    #[test]
    fn a_lion_is_gone_to_by_its_name_its_state_or_its_place() {
        assert_eq!(
            advance(Command::GoToLion, &[], &world()),
            Outcome::Refuse("The Den is empty: no agent session is live.".into())
        );
        let mut den = pride();
        den.lions[1].needs = true;
        den.lions[1].state = "Waiting for you".into();
        let known = world().with_pride(den);
        let rows = rows_of(&advance(Command::GoToLion, &[], &known));
        assert_eq!(rows[0].0, "MOSS (Thinking, leon)");
        assert_eq!(
            (rows[1].0.as_str(), rows[1].1.as_str()),
            ("FERN (Waiting for you, leon)", "needs you")
        );
        // One that runs elsewhere can be gone to as well.
        assert_eq!(rows.len(), 4);
        assert_eq!(
            advance(Command::GoToLion, &strings(&["4"]), &known),
            Outcome::Run(Action::SelectLion(4))
        );
        assert_eq!(
            advance(Command::GoToLion, &strings(&["77"]), &known),
            Outcome::Run(Action::Nothing)
        );
    }

    #[test]
    fn a_session_at_home_is_woken_by_choosing_it_and_a_lion_is_hatched_as_a_session_is_started() {
        assert_eq!(
            advance(Command::WakeLion, &[], &world()),
            Outcome::Refuse("Nobody is at home: no session was sent home.".into())
        );
        let known = world().with_pride(pride());
        let rows = rows_of(&advance(Command::WakeLion, &[], &known));
        assert_eq!(
            rows,
            [(
                "Fix the build".to_owned(),
                "starts its agent again where the session left off".to_owned(),
                "s:abc".to_owned()
            )]
        );
        assert_eq!(
            advance(Command::WakeLion, &strings(&["s:abc"]), &known),
            Outcome::Run(Action::WakeHome("s:abc".into()))
        );
        assert_eq!(
            advance(Command::WakeLion, &strings(&["s:gone"]), &known),
            Outcome::Run(Action::Nothing)
        );
        // Hatching asks where first, whatever row the sidebar's cursor is
        // on, then which agent, and the session it starts stays in the Den.
        let world = world();
        assert!(world.here.is_some());
        let first = advance(Command::HatchLion, &[], &world);
        assert_eq!(labels(&first)[..2], ["web / main", "api / main"]);
        // Beside a selected lion, its worktree is offered first and said
        // to be shared: it is still a question, never taken.
        let mut works = lion(Some(LiveId(1)));
        works.cwd = Some("/srv/api/src".into());
        let beside = advance(
            Command::HatchLion,
            &[],
            &world.clone().with_lion(Some(works)),
        );
        assert_eq!(labels(&beside)[..2], ["api / main", "web / main"]);
        let Outcome::Ask(Step {
            kind: StepKind::Choices { choices, .. },
            ..
        }) = &beside
        else {
            panic!("a question");
        };
        assert_eq!(
            choices[0].detail,
            "where MOSS works: the two would share its files"
        );
        assert!(choices[0].current);
        let agent = advance(Command::HatchLion, &strings(&["a1"]), &world);
        assert_eq!(labels(&agent)[..2], ["Claude Code", "Codex"]);
        assert!(matches!(
            advance(Command::HatchLion, &strings(&["a1", "claude"]), &world),
            Outcome::Run(Action::HatchSession(SessionIntent { agent, .. })) if agent == AgentId::CLAUDE
        ));
    }

    #[test]
    fn a_message_keeps_its_lines_and_an_empty_one_is_no_answer() {
        assert_eq!(
            validate(Validate::Message, "  first line\n\nsecond line \n"),
            Ok("first line\n\nsecond line".to_owned())
        );
        assert_eq!(
            validate(Validate::Message, " \n "),
            Err("Type a message.".to_owned())
        );
    }

    // ----- files -----------------------------------------------------------------------

    fn with_file(dirty: bool, conflict: bool) -> World {
        world().with_files(
            Some(FileInfo {
                id: LiveId(4),
                name: "main.rs".into(),
                dirty,
                conflict,
            }),
            Vec::new(),
        )
    }

    #[test]
    fn opening_a_file_asks_for_its_path_and_opens_what_was_typed() {
        let Outcome::Ask(step) = advance(Command::OpenFile, &[], &world()) else {
            panic!("a question");
        };
        assert!(matches!(
            step.kind,
            StepKind::Text {
                validate: Validate::Required,
                ..
            }
        ));
        assert_eq!(
            advance(Command::OpenFile, &strings(&["src/main.rs"]), &world()),
            Outcome::Run(Action::OpenFile("src/main.rs".into()))
        );
    }

    #[test]
    fn closing_a_clean_file_asks_nothing_and_a_dirty_one_asks_three_ways() {
        assert_eq!(
            advance(Command::CloseFile, &[], &with_file(false, false)),
            Outcome::Run(Action::CloseFile(LiveId(4)))
        );
        let dirty = with_file(true, false);
        let Outcome::Ask(step) = advance(Command::CloseFile, &[], &dirty) else {
            panic!("a question");
        };
        let StepKind::Choices { choices, .. } = step.kind else {
            panic!("choices");
        };
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, ["save", "discard", "cancel"]);
        for (answer, action) in [
            ("save", Action::SaveAndCloseFile(LiveId(4))),
            ("discard", Action::CloseFile(LiveId(4))),
            ("cancel", Action::Nothing),
        ] {
            assert_eq!(
                advance(Command::CloseFile, &strings(&[answer]), &dirty),
                Outcome::Run(action)
            );
        }
        assert!(matches!(
            advance(Command::CloseFile, &[], &world()),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn saving_asks_only_when_the_file_changed_on_disk() {
        assert_eq!(
            advance(Command::SaveFile, &[], &with_file(true, false)),
            Outcome::Run(Action::SaveFile(LiveId(4)))
        );
        let conflict = with_file(true, true);
        let Outcome::Ask(step) = advance(Command::SaveFile, &[], &conflict) else {
            panic!("a question");
        };
        let StepKind::Choices { choices, .. } = step.kind else {
            panic!("choices");
        };
        assert_eq!(
            choices[0].value, "cancel",
            "the first answer changes nothing"
        );
        for (answer, action) in [
            ("overwrite", Action::OverwriteFile(LiveId(4))),
            ("reload", Action::ReloadFile(LiveId(4))),
            ("cancel", Action::Nothing),
        ] {
            assert_eq!(
                advance(Command::SaveFile, &strings(&[answer]), &conflict),
                Outcome::Run(action)
            );
        }
        assert!(matches!(
            advance(Command::SaveFile, &[], &world()),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn quitting_asks_for_unsaved_files_unless_the_setting_says_never() {
        let unsaved = world().with_files(None, vec!["a.rs".into(), "b.rs".into()]);
        let Outcome::Ask(step) = advance(Command::Quit, &[], &unsaved) else {
            panic!("a question");
        };
        let StepKind::Choices { choices, .. } = step.kind else {
            panic!("choices");
        };
        assert!(choices[0].label.contains("2 files"), "{}", choices[0].label);
        assert_eq!(choices.len(), 3, "save, quit without saving, cancel");
        for (answer, action) in [
            ("save", Action::SaveAllAndQuit),
            ("discard", Action::DiscardAndQuit),
            ("no", Action::Nothing),
        ] {
            assert_eq!(
                advance(Command::Quit, &strings(&[answer]), &unsaved),
                Outcome::Run(action)
            );
        }
        let mut never = unsaved.clone();
        never.prefs.quit = QuitConfirm::Never;
        assert_eq!(
            advance(Command::Quit, &[], &never),
            Outcome::Run(Action::Quit)
        );
        assert_eq!(
            advance(Command::Quit, &[], &world()),
            Outcome::Run(Action::Quit)
        );
    }

    // ----- one prompt, several agents --------------------------------------

    fn ticked(outcome: &Outcome) -> Vec<(String, bool)> {
        match outcome {
            Outcome::Ask(Step {
                kind: StepKind::Choices { choices, .. },
                ..
            }) => choices
                .iter()
                .map(|choice| (choice.value.clone(), choice.current))
                .collect(),
            other => panic!("expected choices, got {other:?}"),
        }
    }

    #[test]
    fn the_prompt_flow_asks_for_the_project_the_prompt_then_the_agents() {
        let world = world();
        assert_eq!(
            labels(&advance(Command::PromptAgents, &[], &world)),
            ["api", "web"],
            "the project the keyboard is on comes first"
        );
        let Outcome::Ask(step) = advance(Command::PromptAgents, &strings(&["api"]), &world) else {
            panic!("the prompt is asked");
        };
        assert_eq!(step.prompt, "Prompt");
        assert!(matches!(
            step.kind,
            StepKind::Text {
                validate: Validate::Prompt,
                ..
            }
        ));
        let agents = advance(Command::PromptAgents, &strings(&["api", "Fix it"]), &world);
        let offered = ticked(&agents);
        assert_eq!(
            offered
                .iter()
                .map(|(value, _)| value.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex", "opencode", "grok", "cursor", "gemini"],
            "only the agents with a verified prompt form, in the catalogue's order"
        );
        assert!(offered.iter().all(|(_, on)| !on));
    }

    #[test]
    fn choosing_an_agent_ticks_it_again_unticks_it_and_start_appears_with_two() {
        let world = world();
        let at = |picks: &[&str]| {
            let mut answers = strings(&["api", "Fix it"]);
            answers.extend(picks.iter().map(|pick| (*pick).to_owned()));
            ticked(&advance(Command::PromptAgents, &answers, &world))
        };
        let one = at(&["claude"]);
        assert_eq!(one[0], ("claude".to_owned(), true), "no start with one");
        let two = at(&["claude", "codex"]);
        assert_eq!(two[0].0, "--start", "start is first once two are ticked");
        let on: Vec<&str> = two
            .iter()
            .filter(|(_, on)| *on)
            .map(|(value, _)| value.as_str())
            .collect();
        assert_eq!(on, ["claude", "codex"]);
        let undone = at(&["claude", "codex", "claude"]);
        assert!(
            undone.iter().all(|(value, _)| value != "--start"),
            "{undone:?}"
        );
        assert_eq!(undone.iter().filter(|(_, on)| *on).count(), 1, "{undone:?}");
    }

    #[test]
    fn after_start_the_base_is_asked_and_then_the_work_is_done() {
        let world = world();
        let base = advance(
            Command::PromptAgents,
            &strings(&["api", "Fix it", "claude", "codex", "--start"]),
            &world,
        );
        assert_eq!(labels(&base), ["HEAD", "main", "x"]);
        assert_eq!(
            advance(
                Command::PromptAgents,
                &strings(&["api", "Fix it", "claude", "codex", "--start", "main"]),
                &world,
            ),
            Outcome::Run(Action::PromptAgents {
                project: ProjectId::from_string("api"),
                prompt: "Fix it".into(),
                agents: vec![AgentId::CLAUDE, AgentId::CODEX],
                base: Some("main".into()),
            })
        );
        match advance(
            Command::PromptAgents,
            &strings(&["api", "Fix it", "codex", "claude", "--start", "HEAD"]),
            &world,
        ) {
            Outcome::Run(Action::PromptAgents { agents, base, .. }) => {
                assert_eq!(
                    agents,
                    [AgentId::CODEX, AgentId::CLAUDE],
                    "in the order ticked"
                );
                assert_eq!(base, None, "HEAD is no base");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn starting_with_fewer_than_two_agents_or_an_unknown_one_is_refused() {
        let world = world();
        for answers in [
            &["api", "Fix it", "claude", "--start", "HEAD"][..],
            &["api", "Fix it", "--start", "HEAD"][..],
            &["api", "Fix it", "claude", "nope"][..],
            &["api", "Fix it", "aider"][..],
            &["nope", "Fix it"][..],
        ] {
            assert!(
                matches!(
                    advance(Command::PromptAgents, &strings(answers), &world),
                    Outcome::Refuse(_)
                ),
                "{answers:?}"
            );
        }
    }

    #[test]
    fn only_agents_that_are_turned_on_and_installed_on_the_projects_machine_are_offered() {
        let mut world = world();
        world.prefs.agents_enabled = vec![AgentId::CLAUDE, AgentId::CODEX, AgentId::GROK];
        world.installed = vec![(MachineId::local(), vec![AgentId::CLAUDE, AgentId::GROK])];
        let offered = ticked(&advance(
            Command::PromptAgents,
            &strings(&["api", "Fix it"]),
            &world,
        ));
        assert_eq!(
            offered
                .into_iter()
                .map(|(value, _)| value)
                .collect::<Vec<_>>(),
            ["claude", "grok"]
        );
        world.installed = vec![(MachineId::local(), vec![AgentId::CLAUDE])];
        match advance(Command::PromptAgents, &strings(&["api", "Fix it"]), &world) {
            Outcome::Refuse(why) => assert!(why.contains("Two agents"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_prompt_that_cannot_be_typed_is_refused_where_it_is_typed() {
        assert_eq!(
            validate(Validate::Prompt, "  Fix it \n"),
            Ok("Fix it".into())
        );
        for text in ["", "--help", "a\u{1b}b"] {
            assert!(validate(Validate::Prompt, text).is_err(), "{text:?}");
        }
    }

    // ----- accounts -----------------------------------------------------------

    fn account(agent: AgentId, name: &str, variables: &str) -> Account {
        Account::new(
            agent,
            name,
            leon_core::account::parse_variables(variables).unwrap(),
            &[],
        )
        .unwrap()
    }

    fn with_accounts(defaults: &[&str]) -> World {
        world().with_accounts(
            vec![
                account(AgentId::CLAUDE, "Work", "CLAUDE_CONFIG_DIR=~/.claude-work"),
                account(AgentId::CLAUDE, "Home", "CLAUDE_CONFIG_DIR=~/.claude-home"),
                account(AgentId::CODEX, "Lab", "CODEX_HOME=/data/codex-lab"),
            ],
            defaults.iter().map(|d| (*d).to_owned()).collect(),
        )
    }

    fn intent(agent: AgentId, account: Option<&str>) -> Outcome {
        Outcome::Run(Action::StartSession(SessionIntent {
            agent,
            machine: MachineId::local(),
            machine_name: "Local".into(),
            cwd: "/srv/api".into(),
            account: account.map(str::to_owned),
        }))
    }

    #[test]
    fn an_agent_with_accounts_asks_which_after_the_agent_and_the_default_comes_first() {
        let world = with_accounts(&[]);
        // An agent without accounts is not asked anything more.
        assert_eq!(
            advance(Command::NewSession, &strings(&["opencode"]), &world),
            intent(AgentId::OPENCODE, None)
        );
        // Claude Code has two: its own setup and the accounts are the choices.
        let asked = advance(Command::NewSession, &strings(&["claude"]), &world);
        let Outcome::Ask(Step {
            prompt,
            kind: StepKind::Choices { choices, .. },
        }) = asked
        else {
            panic!("expected the account question, got {asked:?}");
        };
        assert_eq!(prompt, "Account");
        assert_eq!(
            choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            [
                "Claude Code (default)",
                "Claude Code (Work)",
                "Claude Code (Home)"
            ]
        );
        assert_eq!(choices[0].value, "default");
        assert_eq!(choices[1].value, "claude-work");
        assert!(choices[1].detail.contains("~/.claude-work"));
        assert!(choices.iter().all(|c| c.agent == Some(AgentId::CLAUDE)));
        // Each answer starts the session as that account.
        assert_eq!(
            advance(
                Command::NewSession,
                &strings(&["claude", "claude-work"]),
                &world
            ),
            intent(AgentId::CLAUDE, Some("claude-work"))
        );
        assert_eq!(
            advance(
                Command::NewSession,
                &strings(&["claude", "default"]),
                &world
            ),
            intent(AgentId::CLAUDE, None)
        );
        // An account of another agent is not on offer.
        assert!(matches!(
            advance(
                Command::NewSession,
                &strings(&["claude", "codex-lab"]),
                &world
            ),
            Outcome::Refuse(_)
        ));
        // One named account is already two choices.
        let one = with_accounts(&[]);
        assert!(matches!(
            advance(Command::NewSession, &strings(&["codex"]), &one),
            Outcome::Ask(_)
        ));
    }

    #[test]
    fn the_default_account_setting_skips_the_question_and_a_stale_one_asks() {
        let world = with_accounts(&["claude=work", "codex=default"]);
        assert_eq!(
            advance(Command::NewSession, &strings(&["claude"]), &world),
            intent(AgentId::CLAUDE, Some("claude-work"))
        );
        assert_eq!(
            advance(Command::NewSession, &strings(&["codex"]), &world),
            intent(AgentId::CODEX, None)
        );
        let stale = with_accounts(&["claude=gone"]);
        assert!(matches!(
            advance(Command::NewSession, &strings(&["claude"]), &stale),
            Outcome::Ask(_)
        ));
    }

    #[test]
    fn the_default_agent_of_the_settings_is_still_asked_its_account() {
        let mut asking = with_accounts(&[]);
        asking.prefs.default_agent = Some(AgentId::CLAUDE);
        assert!(matches!(
            advance(Command::NewSession, &[], &asking),
            Outcome::Ask(Step {
                prompt: "Account",
                ..
            })
        ));
        assert_eq!(
            advance(Command::NewSession, &strings(&["claude-home"]), &asking),
            intent(AgentId::CLAUDE, Some("claude-home"))
        );
        // Without accounts the default agent starts at once, as it always did.
        let mut plain = world();
        plain.prefs.default_agent = Some(AgentId::CLAUDE);
        assert_eq!(
            advance(Command::NewSession, &[], &plain),
            intent(AgentId::CLAUDE, None)
        );
    }

    #[test]
    fn a_session_that_waits_for_its_account_asks_only_that_and_hands_the_answer_back() {
        let world = with_accounts(&[]).with_account_ask(Some(AccountAsk {
            agent: AgentId::CLAUDE,
            machine_name: "Local".into(),
            cwd: "/srv/web".into(),
        }));
        let asked = advance(Command::NewSession, &[], &world);
        let Outcome::Ask(Step {
            kind: StepKind::Choices { choices, .. },
            ..
        }) = asked
        else {
            panic!("expected the account question");
        };
        assert!(
            choices[0].detail.contains("/srv/web"),
            "the folder is named"
        );
        assert_eq!(
            advance(Command::NewSession, &strings(&["claude-work"]), &world),
            Outcome::Run(Action::StartWithAccount(Some("claude-work".into())))
        );
        assert_eq!(
            advance(Command::NewSession, &strings(&["default"]), &world),
            Outcome::Run(Action::StartWithAccount(None))
        );
        assert!(matches!(
            advance(Command::NewSession, &strings(&["nope"]), &world),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn adding_an_account_asks_the_agent_a_name_and_the_variables_and_checks_them() {
        let world = with_accounts(&[]);
        let agents = advance(Command::AddAccount, &[], &world);
        let Outcome::Ask(Step {
            kind: StepKind::Choices { choices, .. },
            ..
        }) = agents
        else {
            panic!("expected the agents");
        };
        let claude = choices.iter().find(|c| c.value == "claude").unwrap();
        assert!(claude.detail.contains("CLAUDE_CONFIG_DIR"));
        let opencode = choices.iter().find(|c| c.value == "opencode").unwrap();
        assert!(
            opencode.detail.contains("not read per account"),
            "an agent without a verified folder says so: {}",
            opencode.detail
        );
        assert!(matches!(
            advance(Command::AddAccount, &strings(&["claude"]), &world),
            Outcome::Ask(Step {
                prompt: "Account name",
                ..
            })
        ));
        // A taken or reserved name is said before the variables are asked.
        for name in ["work", "WORK", "default"] {
            assert!(
                matches!(
                    advance(Command::AddAccount, &strings(&["claude", name]), &world),
                    Outcome::Refuse(_)
                ),
                "{name}"
            );
        }
        let variables = advance(Command::AddAccount, &strings(&["claude", "Side"]), &world);
        let Outcome::Ask(Step {
            prompt,
            kind: StepKind::Text { placeholder, .. },
        }) = variables
        else {
            panic!("expected the variables");
        };
        assert_eq!(prompt, "Variables");
        assert!(placeholder.contains("CLAUDE_CONFIG_DIR"));
        assert_eq!(
            advance(
                Command::AddAccount,
                &strings(&["claude", "Side", "CLAUDE_CONFIG_DIR=~/.side"]),
                &world
            ),
            Outcome::Run(Action::AddAccount {
                agent: AgentId::CLAUDE,
                name: "Side".into(),
                variables: "CLAUDE_CONFIG_DIR=~/.side".into(),
            })
        );
        // No variables is allowed: the agent's own setup under another name.
        assert!(matches!(
            advance(
                Command::AddAccount,
                &strings(&["claude", "Side", ""]),
                &world
            ),
            Outcome::Run(Action::AddAccount { .. })
        ));
        for bad in ["oops", "A B=1", "1A=x", "A=1 A=2"] {
            assert!(
                matches!(
                    advance(
                        Command::AddAccount,
                        &strings(&["claude", "Side", bad]),
                        &world
                    ),
                    Outcome::Refuse(_)
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn snoozing_offers_the_presets_and_reads_what_is_typed() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 10, 7, 12, 0, 0).unwrap();
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        let mut here = world();
        here.here_session = Some((SessionId::from_string("s"), "build".to_owned()));
        here.clock = Some((now, utc));
        let Outcome::Ask(step) = advance(Command::SnoozeSession, &[], &here) else {
            panic!("a question");
        };
        let StepKind::Choices { choices, custom } = step.kind else {
            panic!("choices");
        };
        assert_eq!(
            custom,
            Custom::Any,
            "a length of time or a date may be typed"
        );
        let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "In an hour",
                "In three hours",
                "Tomorrow morning",
                "Next week"
            ]
        );
        assert_eq!(
            advance(Command::SnoozeSession, &["1h".to_owned()], &here),
            Outcome::Run(Action::Snooze(
                SessionId::from_string("s"),
                now + chrono::Duration::hours(1)
            ))
        );
        assert_eq!(
            advance(
                Command::SnoozeSession,
                &["2026-10-12 14:00".to_owned()],
                &here
            ),
            Outcome::Run(Action::Snooze(
                SessionId::from_string("s"),
                chrono::Utc
                    .with_ymd_and_hms(2026, 10, 12, 14, 0, 0)
                    .unwrap()
            ))
        );
        assert_eq!(
            advance(Command::SnoozeSession, &["soon".to_owned()], &here),
            Outcome::Refuse(super::super::shelf::SNOOZE_HELP.to_owned())
        );
        assert!(matches!(
            advance(Command::SnoozeSession, &[], &world()),
            Outcome::Refuse(_)
        ));
    }

    #[test]
    fn renaming_and_removing_an_account_choose_it_check_the_name_and_confirm() {
        let named = with_accounts(&[]);
        let listed = advance(Command::RenameAccount, &[], &named);
        assert_eq!(
            labels(&listed),
            ["Claude Code (Work)", "Claude Code (Home)", "Codex (Lab)"]
        );
        assert!(matches!(
            advance(Command::RenameAccount, &strings(&["claude-work"]), &named),
            Outcome::Ask(Step {
                prompt: "New name",
                ..
            })
        ));
        assert_eq!(
            advance(
                Command::RenameAccount,
                &strings(&["claude-work", "Job"]),
                &named
            ),
            Outcome::Run(Action::RenameAccount {
                id: "claude-work".into(),
                name: "Job".into()
            })
        );
        // Keeping its own name in another case is fine; another account's is not.
        assert!(matches!(
            advance(
                Command::RenameAccount,
                &strings(&["claude-work", "WORK"]),
                &named
            ),
            Outcome::Run(_)
        ));
        assert!(matches!(
            advance(
                Command::RenameAccount,
                &strings(&["claude-work", "home"]),
                &named
            ),
            Outcome::Refuse(_)
        ));
        assert!(matches!(
            advance(
                Command::RenameAccount,
                &strings(&["claude-work", "default"]),
                &named
            ),
            Outcome::Refuse(_)
        ));
        assert!(matches!(
            advance(Command::RemoveAccount, &strings(&["codex-lab"]), &named),
            Outcome::Ask(Step {
                prompt: "Confirm",
                ..
            })
        ));
        assert_eq!(
            advance(
                Command::RemoveAccount,
                &strings(&["codex-lab", "yes"]),
                &named
            ),
            Outcome::Run(Action::RemoveAccount("codex-lab".into()))
        );
        assert_eq!(
            advance(
                Command::RemoveAccount,
                &strings(&["codex-lab", "no"]),
                &named
            ),
            Outcome::Run(Action::Nothing)
        );
        for command in [Command::RenameAccount, Command::RemoveAccount] {
            assert!(
                matches!(advance(command, &[], &world()), Outcome::Refuse(_)),
                "nothing to act on"
            );
        }
    }

    #[test]
    fn adding_an_account_says_what_leon_can_do_per_account_for_its_agent() {
        let claude = account(AgentId::CLAUDE, "Work", "CLAUDE_CONFIG_DIR=~/.w");
        assert!(added_note(&claude).contains("limits get a line of their own"));
        let bare = account(AgentId::CODEX, "Bare", "");
        assert!(added_note(&bare).contains("CODEX_HOME"));
        let other = account(AgentId::OPENCODE, "Side", "XDG_DATA_HOME=/d");
        assert!(
            added_note(&other).contains("history and limits are not read per account"),
            "{}",
            added_note(&other)
        );
    }
}
