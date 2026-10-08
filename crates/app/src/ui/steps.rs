//! The questions the palette asks, as data.
//!
//! A command that needs more than a keystroke (a new worktree, a new machine)
//! is a *flow*: a list of questions asked one after the other in the palette,
//! ending in an [`Action`]. [`advance`] is the whole flow as one pure
//! function: given the command, the answers so far and a snapshot of the
//! world, it says what to ask next, what to do, or why it cannot be done. The
//! palette only renders the question and feeds the answer back.

use leon_core::{
    AgentId, AgentSpec, CustomAgent, Machine, MachineId, MachineKind, Project, ProjectId,
    SessionId, Worktree, WorktreeId,
};

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
    /// The machine the cursor is on, when it is on a machine's row.
    pub machine_row: Option<MachineId>,
    /// The history session the cursor is on, with its title.
    pub here_session: Option<(SessionId, String)>,
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
            machine_row: None,
            here_session: None,
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
        }
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
    /// Anything, nothing included.
    Optional,
    /// A branch name git accepts.
    Branch,
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
        Validate::Branch => address::validate_branch(text)
            .map(|()| text.to_owned())
            .map_err(str::to_owned),
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
    },
    /// Remove an agent of the user's.
    RemoveAgent(AgentId),
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
/// starts [`Command::RestartToUpdate`]'s questions itself.)
pub fn is_flow(command: Command) -> bool {
    matches!(
        command,
        Command::NewSession
            | Command::AddAgent
            | Command::RemoveAgent
            | Command::CloseSession
            | Command::SleepSession
            | Command::Rename
            | Command::RemoveMachine
            | Command::RemoveFromHistory
            | Command::ResumeSession
            | Command::ResumeIn
            | Command::ResumeAnyway
            | Command::TakeOver
            | Command::NewWorktree
            | Command::AddProject
            | Command::CloneProject
            | Command::NewProject
            | Command::RemoveProject
            | Command::RemoveWorktree
            | Command::SetAppearance
            | Command::ChooseTheme
            | Command::SetInterfaceSize
            | Command::Quit
            | Command::NewThemeFromCurrent
            | Command::ChooseDen
            | Command::SaveDenAs
            | Command::RenameDen
            | Command::DeleteDen
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
        Command::CloseSession => close_session(answers, world),
        Command::SleepSession => sleep_session(answers, world),
        Command::Rename => rename(answers, world),
        Command::RemoveMachine => remove_machine(answers, world),
        Command::RemoveFromHistory => remove_from_history(answers, world),
        Command::ResumeSession => resume_session(answers, world),
        Command::ResumeIn => resume_in(answers, world),
        Command::ResumeAnyway => resume_anyway(answers, world),
        Command::TakeOver => take_over(answers, world),
        Command::NewWorktree => new_worktree(answers, world),
        Command::AddProject => add_project(answers, world),
        Command::CloneProject => clone_project(answers, world),
        Command::NewProject => new_project(answers, world),
        Command::RemoveProject => remove_project(answers, world),
        Command::RemoveWorktree => remove_worktree(answers, world),
        Command::SetAppearance => set_appearance(answers, world),
        Command::ChooseTheme => choose_theme(answers, world),
        Command::SetInterfaceSize => set_size(answers, world),
        Command::Quit => quit(answers, world),
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
        [name, command, args, resume_args, ..] => {
            let names: Vec<&str> = leon_core::agent::all()
                .iter()
                .map(|spec| spec.name.as_str())
                .collect();
            match CustomAgent::new(name, command, args, resume_args, &[])
                .and_then(|agent| agent.to_spec(&names))
            {
                Ok(_) => Outcome::Run(Action::AddAgent {
                    name: name.clone(),
                    command: command.clone(),
                    args: args.clone(),
                    resume_args: resume_args.clone(),
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
        if rest.is_empty() {
            return Outcome::Run(Action::StartSession(SessionIntent {
                agent,
                machine: target.machine,
                machine_name: target.machine_name,
                cwd: target.cwd,
            }));
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
        [agent, ..] => match AgentId::parse(agent).filter(|agent| offered.contains(agent)) {
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
            Some(agent) => Outcome::Run(Action::StartSession(SessionIntent {
                agent,
                machine: target.machine,
                machine_name: target.machine_name,
                cwd: target.cwd,
            })),
            None => Outcome::Refuse(format!("Unknown agent {agent:?}.")),
        },
    }
}

/// Every worktree of every project, for a session that has nowhere to start
/// yet.
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
        [] => {
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
        [_] => text("Branch name", "feature/login", Validate::Branch),
        [project, _] => {
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
        [project, branch, base, ..] => Outcome::Run(Action::Engine(Op::AddWorktree {
            project: ProjectId::from_string(project.as_str()),
            branch: branch.clone(),
            base: (base != "HEAD").then(|| base.clone()),
        })),
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

/// The answer the window adds when git refused a worktree for the files it
/// holds: the question it leads to is the one about forcing them away.
pub const REFUSED: &str = "refused";

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

/// Whether to quit while programs run in terminals: it is asked only then
/// (with nothing running the shell quits without a question), and the first
/// answer, which `Enter` takes, is to quit.
fn quit(answers: &[String], world: &World) -> Outcome {
    let busy = world.live.iter().filter(|session| session.busy).count();
    let ask = world.prefs.quit.asks(busy);
    match answers {
        [] if !ask => Outcome::Run(Action::Quit),
        [] => choices(
            "Quit Leon?",
            vec![
                Choice::new(
                    match busy {
                        0 => format!("Quit {}", crate::product::PRODUCT_NAME),
                        1 => format!(
                            "Quit {}: 1 running session will be closed.",
                            crate::product::PRODUCT_NAME
                        ),
                        busy => format!(
                            "Quit {}: {busy} running sessions will be closed.",
                            crate::product::PRODUCT_NAME
                        ),
                    },
                    "hangs the terminals up",
                    "yes",
                ),
                Choice::new("Cancel", "keep working", "no"),
            ],
            Custom::No,
        ),
        [chosen, ..] if chosen == "yes" => Outcome::Run(Action::Quit),
        _ => Outcome::Run(Action::Nothing),
    }
}

/// Whether to restart into the update: always asked, because a restart ends
/// every terminal, and it says how many that is.
fn restart_update(answers: &[String], world: &World) -> Outcome {
    let Some(version) = &world.update_ready else {
        return Outcome::Refuse("No update is ready to install yet.".to_owned());
    };
    let busy = world.live.iter().filter(|session| session.busy).count();
    let open = world.live.len();
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
                },
                ProjectInfo {
                    project: project("api", "api", "/srv/api"),
                    machine_name: "Local".into(),
                    worktrees: vec![
                        worktree("a0", "/srv/api", Some("main"), true),
                        worktree("a1", "/srv/api-worktrees/x", Some("x"), false),
                        worktree("a2", "/srv/api-worktrees/detached", None, false),
                    ],
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
            machine_row: None,
            here_session: None,
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

    #[test]
    fn adding_a_custom_agent_asks_four_questions_and_checks_the_answers() {
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
            ask(&["My Tool", "mytool", "--fast", "--resume {id}"]),
            Outcome::Run(Action::AddAgent {
                name: "My Tool".into(),
                command: "mytool".into(),
                args: "--fast".into(),
                resume_args: "--resume {id}".into(),
            })
        );
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
    fn removing_a_worktree_lists_only_linked_ones_and_asks_to_confirm() {
        let world = world();
        let list = advance(Command::RemoveWorktree, &[], &world);
        assert_eq!(labels(&list), ["x", "/srv/api-worktrees/detached"]);
        let confirm = advance(Command::RemoveWorktree, &strings(&["api|a1"]), &world);
        assert_eq!(labels(&confirm), ["Remove x", "Cancel"]);
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

    fn session(id: u64, busy: bool) -> LiveInfo {
        LiveInfo {
            id: LiveId(id),
            label: "Claude Code".into(),
            busy,
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
}
