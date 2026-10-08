//! What a live session runs: a base terminal, and optionally an agent in it.
//!
//! Every terminal starts the machine's interactive login shell in the target
//! folder; an agent is then started *inside* that shell by typing its command
//! line as input. When the agent ends, the terminal is still there at the
//! shell prompt in that folder: only the shell ending (or the user closing
//! the pane) ends it. [`plan`] is the pure function from where, what and what
//! is known about the machine to a [`Plan`]:
//!
//! * `spawn`: what the terminal starts. Local: `$SHELL -l -i` in the folder
//!   (PowerShell, else `COMSPEC`, on Windows). Remote: `ssh -t` running
//!   `cd <folder> && exec "${SHELL:-/bin/sh}" -l -i`, the remote login shell
//!   landing in the folder (the `cd` is part of the remote command rather
//!   than typed, so nothing shows on the screen and it cannot race the
//!   prompt).
//! * `send`: the line to type once the shell is ready, quoted for that
//!   shell, when an agent was asked for. The shell's own `PATH` resolves the
//!   agent, so a tool installed by the user's profile files just works.
//!
//! The agent is still checked first: this computer is searched for it (as
//! the old launcher did) and an SSH machine's probe report says whether it
//! has it. One that is not installed gets a clear refusal instead of a
//! terminal that says `command not found`. A machine that was not probed yet
//! is tried anyway.
//!
//! An agent can be started as one of the user's accounts of it (see
//! `leon_core::Account`): the account's variables are added to the environment
//! of that terminal, and of no other. On this computer they are the process
//! environment of the shell; on an SSH machine or over a relay they are the
//! `env` of the command line the other side runs, each `NAME=value` quoted as one
//! word, so a value can hold anything and run nothing. A `~` at the start of a
//! value is the home folder of the machine the terminal is on ([`account_env`]).

use leon_core::{Account, AgentId, AgentSpec, Machine, MachineKind};
use leon_remote::{
    agent_launch, interactive_on, sh_quote, sh_quote_typed, CommandSpec, ProbeReport, SshOptions,
};
use leon_term::SpawnSpec;
use std::path::{Path, PathBuf};

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// An agent, resuming the session with this id of its own when given.
    Agent {
        /// Which agent.
        kind: AgentId,
        /// The agent's own id of the session to resume.
        resume: Option<String>,
        /// The id of the account to run it as; `None` is the agent's own setup.
        account: Option<String>,
    },
    /// An agent started with a first prompt on its launch line (see
    /// [`command_line_prompted`]): a new session, never a resumed one.
    Prompted {
        /// Which agent.
        kind: AgentId,
        /// The prompt, as [`clean_prompt`] returns it.
        prompt: String,
        /// The id of the account to run it as; `None` is the agent's own setup.
        account: Option<String>,
    },
    /// The user's login shell.
    Shell,
}

/// Why nothing was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The agent is not installed on the machine.
    NotInstalled {
        /// The agent.
        agent: AgentId,
        /// The machine's name.
        machine: String,
    },
    /// The folder does not exist on this computer.
    NoSuchFolder(String),
    /// The agent is not in the catalogue (it was a custom agent that has been
    /// removed).
    UnknownAgent(AgentId),
    /// A session was to be resumed but the agent is launch only.
    CannotResume {
        /// The agent.
        agent: AgentId,
    },
    /// The agent has no verified way to take a prompt on its launch line.
    NoPromptForm {
        /// The agent.
        agent: AgentId,
    },
    /// The shell is not a POSIX one: the prompt cannot be quoted for it.
    PromptNeedsPosixShell,
    /// The session belongs to an account that is not in the settings any more
    /// (or is another agent's): starting it as another account would sign it
    /// in as somebody else.
    UnknownAccount {
        /// The account's id.
        account: String,
    },
    /// An account's variable starts with `~` and the home folder of the
    /// machine is not known (it has not been looked at yet).
    NoHome {
        /// The account's name.
        account: String,
    },
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled { agent, machine } => write!(
                f,
                "{} is not installed on {machine}.",
                crate::format::agent_name(*agent)
            ),
            Self::NoSuchFolder(path) => write!(f, "The folder {path} does not exist."),
            Self::UnknownAgent(agent) => write!(f, "{agent} is not an agent Leon knows."),
            Self::CannotResume { agent } => write!(
                f,
                "{} cannot be resumed from Leon: it starts new sessions only.",
                crate::format::agent_name(*agent)
            ),
            Self::NoPromptForm { agent } => write!(
                f,
                "{} has no known way to be given a prompt when it starts.",
                crate::format::agent_name(*agent)
            ),
            Self::PromptNeedsPosixShell => f.write_str(
                "A prompt can only be given to an agent through a POSIX shell (bash, zsh, fish...), not PowerShell or cmd.exe.",
            ),
            Self::UnknownAccount { account } => write!(
                f,
                "The account {account} is not in your settings any more, so this session was not started: add it again with the same name, or start a new session."
            ),
            Self::NoHome { account } => write!(
                f,
                "The account {account} uses ~, and this machine's home folder is not known yet: wait for the machine to be checked, or write the folder in full."
            ),
        }
    }
}

impl std::error::Error for LaunchError {}

/// What is asked of this computer.
pub trait System {
    /// The absolute path of a program, searched for as a login shell would
    /// and in the places agents install themselves.
    fn find_program(&self, name: &str) -> Option<PathBuf>;
    /// The program and arguments of the user's login shell.
    fn login_shell(&self) -> (String, Vec<String>);
    /// Whether a folder exists.
    fn dir_exists(&self, path: &str) -> bool;
    /// The home folder of the user of this computer, for the `~` an account's
    /// variable may start with.
    fn home_dir(&self) -> Option<String> {
        None
    }
}

/// This computer.
pub struct RealSystem;

/// Where agents install themselves, relative to the home directory, besides
/// the `PATH`.
const HOME_BINS: [&str; 6] = [
    ".local/bin",
    ".claude/local",
    ".cargo/bin",
    ".bun/bin",
    ".npm-global/bin",
    ".opencode/bin",
];

/// Where they are on a Mac or a Linux box besides the `PATH`: a program
/// started from the Finder has a short one.
const SYSTEM_BINS: [&str; 3] = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

fn is_runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

impl System for RealSystem {
    fn find_program(&self, name: &str) -> Option<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        if let Some(home) = dirs::home_dir() {
            dirs.extend(HOME_BINS.iter().map(|bin| home.join(bin)));
        }
        dirs.extend(SYSTEM_BINS.iter().map(PathBuf::from));
        find_in(&dirs, name)
    }

    fn login_shell(&self) -> (String, Vec<String>) {
        if cfg!(windows) {
            for name in ["pwsh", "powershell"] {
                if let Some(path) = self.find_program(name) {
                    return (path.to_string_lossy().into_owned(), Vec::new());
                }
            }
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned());
            (comspec, Vec::new())
        } else {
            unix_login_shell(std::env::var("SHELL").ok())
        }
    }

    fn dir_exists(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }

    fn home_dir(&self) -> Option<String> {
        dirs::home_dir().map(|home| home.to_string_lossy().into_owned())
    }
}

/// The first runnable file called `name` (with the extensions a program has
/// on Windows) in `dirs`, in order.
fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|ext| format!("{name}.{ext}"))
            .collect()
    } else {
        vec![name.to_owned()]
    };
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |file| dir.join(file)))
        .find(|candidate| is_runnable(candidate))
}

/// The login shell on Unix given the value of `$SHELL`: that shell, or
/// `/bin/sh` when it is unset or empty, started as a login shell,
/// interactive.
fn unix_login_shell(shell: Option<String>) -> (String, Vec<String>) {
    let shell = shell
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_owned());
    (shell, vec!["-l".to_owned(), "-i".to_owned()])
}

/// The path of the agent in a probe report: the first of its binaries the
/// machine has. `None` when the machine was probed and has none of them.
pub fn probed<'a>(report: &'a ProbeReport, spec: &AgentSpec) -> Option<&'a str> {
    spec.detect.iter().find_map(|name| report.tool(name))
}

/// The terminal's spec for a command of this crate's command type.
pub fn spawn_spec(command: CommandSpec) -> SpawnSpec {
    SpawnSpec {
        program: command.program,
        args: command.args,
        env: command.env,
        cwd: command.cwd,
        route: command.route,
    }
}

/// The program the remote side runs: the login shell of the remote user,
/// named by that machine's own `$SHELL`, interactive.
const REMOTE_SHELL: &str = r#"exec "${SHELL:-/bin/sh}" -l -i"#;

/// How a shell wants its words quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// bash, zsh, fish, dash: single quotes.
    Posix,
    /// PowerShell: single quotes, doubled inside.
    PowerShell,
    /// `cmd.exe`: double quotes.
    Cmd,
}

/// The flavour of the shell a terminal on `machine` runs: the one of the
/// shell of the settings, else the login shell, on this computer; a POSIX
/// shell anywhere else.
pub fn shell_flavor(machine: &Machine, system: &dyn System, prefs: &LaunchPrefs) -> Flavor {
    match machine.kind {
        MachineKind::Local => {
            let (program, _) = prefs.shell.clone().unwrap_or_else(|| system.login_shell());
            flavor_of(&program)
        }
        _ => Flavor::Posix,
    }
}

/// What the first command of [`chain`] is, which decides how it is typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum First {
    /// A script of the project: typed as written into the user's own shell.
    Script,
    /// The setup of a new worktree: handed to `sh -c` whatever follows it.
    Setup,
}

/// The line that types `first`, and then `then` only when `first` succeeded,
/// with its Enter. `then` is a line as [`Plan::send`] has it (its Enter is
/// dropped here), or nothing when `first` is all there is to run.
///
/// The shell does the sequencing, so it holds where a terminal's process
/// cannot be asked what became of a command: on this computer, over SSH and
/// through the relay alike. When `first` fails the terminal stays at its
/// prompt with the output on the screen and `then` is never typed.
///
/// On a POSIX shell a [`First::Setup`] always runs in `sh -c`, with an agent
/// after it or not, so the text that was shown and trusted means the same in
/// every shell (`{ ...; }` is not fish, `!` is not history expansion) and what
/// it exports does not reach `then`. A [`First::Script`] with nothing after it
/// is typed as it is, in the user's own shell, so that shell's aliases and
/// history expansion apply; PowerShell and `cmd.exe` type either as written.
pub fn chain(first: &str, kind: First, then: Option<&str>, flavor: Flavor) -> String {
    let then = then.map(|line| line.trim_end_matches(['\r', '\n']));
    let line = match (flavor, then) {
        (Flavor::Posix, Some(then)) => format!("sh -c {} && {then}", sh_quote_typed(first)),
        (Flavor::Posix, None) if kind == First::Setup => {
            format!("sh -c {}", sh_quote_typed(first))
        }
        (Flavor::PowerShell, Some(then)) => format!("{first}; if ($?) {{ {then} }}"),
        (Flavor::Cmd, Some(then)) => format!("{first} && {then}"),
        (_, None) => first.to_owned(),
    };
    format!("{line}\r")
}

/// The flavour of the shell a program path names.
pub fn flavor_of(shell: &str) -> Flavor {
    // Either separator: the path may be a Windows one whatever this build is.
    let file = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    let name = file
        .strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_ascii_lowercase();
    match name.as_str() {
        "pwsh" | "powershell" => Flavor::PowerShell,
        "cmd" => Flavor::Cmd,
        _ => Flavor::Posix,
    }
}

fn quote(word: &str, flavor: Flavor) -> String {
    match flavor {
        Flavor::Posix => sh_quote(word),
        Flavor::PowerShell => {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.:/@%+,".contains(c));
            if plain {
                word.to_owned()
            } else {
                format!("'{}'", word.replace('\'', "''"))
            }
        }
        Flavor::Cmd => {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.:/@%+,\\".contains(c));
            if plain {
                word.to_owned()
            } else {
                format!("\"{}\"", word.replace('"', "\"\""))
            }
        }
    }
}

/// How the user asked an agent to be started, from the settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentPrefs {
    /// The program that starts it; the login shell finds the agent's own name
    /// when absent.
    pub executable: Option<String>,
    /// Extra arguments of a new session.
    pub args: Vec<String>,
    /// Extra arguments of a resumed one.
    pub resume_args: Vec<String>,
}

/// What the settings change about how terminals start.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchPrefs {
    /// A shell of the user's choice for this computer, with its arguments,
    /// instead of the login shell.
    pub shell: Option<(String, Vec<String>)>,
    /// Variables added to the environment of terminals on this computer.
    pub env: Vec<(String, String)>,
    /// What Leon itself tells a terminal on this computer (`LEON_BIN`,
    /// `LEON_MEMORY`): worked out by the caller for the terminal's folder, set
    /// before the user's variables, so one of theirs with the same name wins.
    pub leon_env: Vec<(String, String)>,
    /// How each agent starts, by agent; an agent without an entry starts as
    /// the catalogue says.
    pub agents: std::collections::HashMap<AgentId, AgentPrefs>,
    /// The user's accounts of the agents, to look an account's variables up.
    pub accounts: Vec<Account>,
}

static NO_PREFS: AgentPrefs = AgentPrefs {
    executable: None,
    args: Vec::new(),
    resume_args: Vec::new(),
};

impl LaunchPrefs {
    /// How `kind` is started.
    pub fn agent(&self, kind: AgentId) -> &AgentPrefs {
        self.agents.get(&kind).unwrap_or(&NO_PREFS)
    }
}

pub use leon_core::agent::split_words;

/// The variables the account `account` adds to the launch of `kind`: its own,
/// in order, with a leading `~` standing for `home`, the home folder of the
/// machine the terminal is on.
///
/// No account is no variable. An account that is not among `accounts`, or
/// belongs to another agent, is an error: the session is never started as a
/// different account than the one asked for.
pub fn account_env(
    account: Option<&str>,
    kind: AgentId,
    accounts: &[Account],
    home: Option<&str>,
) -> Result<Vec<(String, String)>, LaunchError> {
    let Some(id) = account else {
        return Ok(Vec::new());
    };
    let found = leon_core::account::by_id(accounts, id)
        .filter(|found| found.agent == kind)
        .ok_or_else(|| LaunchError::UnknownAccount {
            account: id.to_owned(),
        })?;
    found
        .env
        .iter()
        .map(|(name, value)| {
            leon_core::account::expand_home(value, home)
                .map(|value| (name.clone(), value))
                .ok_or_else(|| LaunchError::NoHome {
                    account: found.name.clone(),
                })
        })
        .collect()
}

/// The command line that starts an agent in a shell, without the Enter.
#[cfg(test)]
pub fn command_line(kind: AgentId, resume: Option<&str>, flavor: Flavor) -> String {
    command_line_with(kind, resume, flavor, &AgentPrefs::default()).unwrap()
}

/// [`command_line`] with the executable and the extra arguments the settings
/// give: the arguments follow the command (`claude --resume <id> <extra>`).
/// `None` when the agent is not in the catalogue, or a resume was asked of one
/// that is launch only.
pub fn command_line_with(
    kind: AgentId,
    resume: Option<&str>,
    flavor: Flavor,
    prefs: &AgentPrefs,
) -> Option<String> {
    command_line_for(kind.spec()?, resume, flavor, prefs)
}

/// [`command_line_with`] for a spec at hand.
pub fn command_line_for(
    spec: &AgentSpec,
    resume: Option<&str>,
    flavor: Flavor,
    prefs: &AgentPrefs,
) -> Option<String> {
    let (program, args) = agent_launch(spec, resume)?;
    let program = prefs.executable.clone().unwrap_or(program);
    let extra = if resume.is_some() {
        &prefs.resume_args
    } else {
        &prefs.args
    };
    Some(
        std::iter::once(program)
            .chain(args)
            .chain(extra.iter().cloned())
            .map(|word| quote(&word, flavor))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The most characters a prompt may have: it is typed into a terminal, and a
/// line editor is not made for more.
pub const MAX_PROMPT: usize = 8000;

/// A prompt as it will be typed: line endings are line feeds, tabs are spaces
/// (a tab typed into a shell completes), and the ends are trimmed. What cannot
/// be typed safely is refused with the reason: nothing, a control character
/// (an escape or a bell is a key to the line editor), more than
/// [`MAX_PROMPT`] characters, a first character that is a dash (the agent
/// would read the prompt as an option) and a single plain word (the agent
/// would take it for one of its own commands, like `codex apply`).
pub fn clean_prompt(text: &str) -> Result<String, String> {
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', " ");
    let text = text.trim();
    if text.is_empty() {
        return Err("Type a prompt.".to_owned());
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(
            "A prompt cannot hold control characters: a terminal would take them as keys."
                .to_owned(),
        );
    }
    if text.chars().count() > MAX_PROMPT {
        return Err(format!(
            "That prompt is too long: the most is {MAX_PROMPT} characters."
        ));
    }
    if text.starts_with('-') {
        return Err(
            "A prompt cannot start with a dash: the agent would read it as an option.".to_owned(),
        );
    }
    // `codex apply` and `claude update` are commands of the agents, and an
    // agent takes its first word for one before it takes it for a prompt.
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "A prompt of one word could be taken for a command of the agent: write a sentence."
                .to_owned(),
        );
    }
    Ok(text.to_owned())
}

/// The command line that starts `spec` with `prompt` (as [`clean_prompt`]
/// returns it) given on the line, without the Enter: the command, its
/// arguments, the extra arguments of the settings, and then the agent's own
/// form for a prompt (`claude 'fix it'`, `opencode --prompt 'fix it'`). The
/// prompt is quoted for typing into a POSIX shell ([`sh_quote_typed`]: quotes,
/// `$`, backticks, `!`, backslashes and line breaks included); that is every
/// shell of a remote machine, and the one of this computer unless it is
/// PowerShell or `cmd.exe`, which are refused.
pub fn command_line_prompted(
    spec: &AgentSpec,
    prompt: &str,
    flavor: Flavor,
    prefs: &AgentPrefs,
) -> Result<String, LaunchError> {
    if flavor != Flavor::Posix {
        return Err(LaunchError::PromptNeedsPosixShell);
    }
    let form = spec
        .prompt
        .as_ref()
        .ok_or(LaunchError::NoPromptForm { agent: spec.id })?;
    let (program, args) = agent_launch(spec, None).ok_or(LaunchError::UnknownAgent(spec.id))?;
    let program = prefs.executable.clone().unwrap_or(program);
    let words = std::iter::once(program)
        .chain(args)
        .chain(prefs.args.iter().cloned())
        .map(|word| quote(&word, flavor))
        .chain(form.iter().map(|word| {
            if word == leon_core::agent::PROMPT_WORD {
                sh_quote_typed(prompt)
            } else {
                quote(word, flavor)
            }
        }));
    Ok(words.collect::<Vec<_>>().join(" "))
}

/// What a terminal starts and what is typed into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The process the terminal runs: the interactive login shell.
    pub spawn: SpawnSpec,
    /// The line typed into the shell once it is ready (with its Enter), when
    /// an agent was asked for.
    pub send: Option<String>,
    /// Whether the foreground process group of the terminal tells the shell
    /// from a program in it: true on this computer, where the terminal's own
    /// process is the shell. Over SSH the terminal's process is `ssh`, always.
    pub can_detect_foreground: bool,
}

/// The terminal for `launch` in `cwd` on `machine`.
///
/// `report` is what the last probe of the machine found, if it has been
/// probed.
pub fn plan(
    machine: &Machine,
    report: Option<&ProbeReport>,
    cwd: &str,
    launch: &Launch,
    ssh: &SshOptions,
    system: &dyn System,
) -> Result<Plan, LaunchError> {
    plan_with(
        machine,
        report,
        cwd,
        launch,
        ssh,
        system,
        &LaunchPrefs::default(),
    )
}

/// [`plan`] with what the settings change: the shell, the environment, and
/// how each agent is started.
pub fn plan_with(
    machine: &Machine,
    report: Option<&ProbeReport>,
    cwd: &str,
    launch: &Launch,
    ssh: &SshOptions,
    system: &dyn System,
    prefs: &LaunchPrefs,
) -> Result<Plan, LaunchError> {
    let local = matches!(machine.kind, MachineKind::Local);
    // The agent must exist on the machine; its path is the shell's to find.
    let mut account_vars = Vec::new();
    let asked = match launch {
        Launch::Agent {
            kind,
            resume,
            account,
        } => Some((*kind, resume.as_deref(), account.as_deref())),
        Launch::Prompted { kind, account, .. } => Some((*kind, None, account.as_deref())),
        Launch::Shell => None,
    };
    if let Some((kind, resume, account)) = asked {
        let spec = kind.spec().ok_or(LaunchError::UnknownAgent(kind))?;
        if resume.is_some() && !spec.can_resume() {
            return Err(LaunchError::CannotResume { agent: kind });
        }
        let own = prefs.agent(kind).executable.as_deref();
        let present = |program: &str| {
            // A chosen program is a file, or a name the shell finds.
            if Path::new(program).components().count() > 1 {
                is_runnable(Path::new(program))
            } else {
                system.find_program(program).is_some()
            }
        };
        let installed = if local {
            match own {
                Some(program) => present(program),
                None if spec.detect.is_empty() => present(&spec.command),
                None => spec.detect.iter().any(|name| present(name)),
            }
        } else {
            // A command that is a path is not looked for by name: the machine
            // is not asked, and the shell reports it if it is not there.
            report.is_none_or(|report| {
                spec.detect.is_empty() || own.is_some() || probed(report, spec).is_some()
            })
        };
        if !installed {
            return Err(LaunchError::NotInstalled {
                agent: kind,
                machine: machine.name.clone(),
            });
        }
        let home = if local {
            system.home_dir()
        } else {
            report.and_then(|report| report.home.clone())
        };
        account_vars = account_env(account, kind, &prefs.accounts, home.as_deref())?;
    }
    if local && !system.dir_exists(cwd) {
        return Err(LaunchError::NoSuchFolder(cwd.to_owned()));
    }
    let (spawn, flavor) = if local {
        let (program, args) = prefs.shell.clone().unwrap_or_else(|| system.login_shell());
        let flavor = flavor_of(&program);
        let mut command = CommandSpec::new(program).args(args).cwd(cwd);
        // Leon's own first: the terminal applies them in order, so a
        // variable of the user's with the same name is the one that stays.
        command.env = prefs.leon_env.iter().chain(&prefs.env).cloned().collect();
        command.env.extend(account_vars);
        (spawn_spec(command), flavor)
    } else {
        let mut command = CommandSpec::new("sh").args(["-c", REMOTE_SHELL]).cwd(cwd);
        command.env = account_vars;
        (
            spawn_spec(interactive_on(machine, &command, ssh)),
            Flavor::Posix,
        )
    };
    let send = match launch {
        Launch::Agent { kind, resume, .. } => Some(format!(
            "{}\r",
            command_line_with(*kind, resume.as_deref(), flavor, prefs.agent(*kind))
                .ok_or(LaunchError::UnknownAgent(*kind))?
        )),
        Launch::Prompted { kind, prompt, .. } => Some(format!(
            "{}\r",
            command_line_prompted(
                kind.spec().ok_or(LaunchError::UnknownAgent(*kind))?,
                prompt,
                flavor,
                prefs.agent(*kind),
            )?
        )),
        Launch::Shell => None,
    };
    Ok(Plan {
        spawn,
        send,
        can_detect_foreground: local,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::MachineId;
    use leon_remote::RemoteOs;

    struct Fake {
        programs: Vec<(&'static str, &'static str)>,
        folders: Vec<&'static str>,
    }

    impl System for Fake {
        fn find_program(&self, name: &str) -> Option<PathBuf> {
            self.programs
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, path)| PathBuf::from(path))
        }
        fn login_shell(&self) -> (String, Vec<String>) {
            (
                "/bin/zsh".to_owned(),
                vec!["-l".to_owned(), "-i".to_owned()],
            )
        }
        fn dir_exists(&self, path: &str) -> bool {
            self.folders.contains(&path)
        }
    }

    fn fake() -> Fake {
        Fake {
            programs: vec![("claude", "/home/me/.local/bin/claude")],
            folders: vec!["/srv/api"],
        }
    }

    fn local() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "This machine".into(),
            kind: MachineKind::Local,
        }
    }

    fn remote() -> Machine {
        Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: "build.example".into(),
                user: Some("dev".into()),
                port: None,
                identity_file: None,
            },
        }
    }

    fn report(claude: Option<&str>) -> ProbeReport {
        ProbeReport {
            os: RemoteOs::Linux,
            arch: None,
            home: None,
            git: None,
            tools: claude
                .map(|path| ("claude".to_owned(), path.to_owned()))
                .into_iter()
                .collect(),
        }
    }

    fn agent(kind: AgentId, resume: Option<&str>) -> Launch {
        Launch::Agent {
            kind,
            resume: resume.map(str::to_owned),
            account: None,
        }
    }

    fn ssh() -> SshOptions {
        SshOptions::without_multiplexing()
    }

    #[test]
    fn a_local_terminal_is_the_interactive_login_shell_in_the_folder() {
        let plan = plan(&local(), None, "/srv/api", &Launch::Shell, &ssh(), &fake()).unwrap();
        assert_eq!(plan.spawn.program, "/bin/zsh");
        assert_eq!(plan.spawn.args, ["-l", "-i"]);
        assert_eq!(plan.spawn.cwd.as_deref(), Some("/srv/api"));
        assert_eq!(plan.send, None, "a shell has nothing typed into it");
        assert!(plan.can_detect_foreground);
    }

    #[test]
    fn an_agent_is_typed_into_the_shell_not_started_as_the_terminals_process() {
        let plan = plan(
            &local(),
            None,
            "/srv/api",
            &agent(AgentId::CLAUDE, None),
            &ssh(),
            &fake(),
        )
        .unwrap();
        assert_eq!(plan.spawn.program, "/bin/zsh", "the terminal is the shell");
        assert_eq!(
            plan.send.as_deref(),
            Some("claude\r"),
            "by its bare name: the shell finds it"
        );
    }

    #[test]
    fn resuming_types_the_agents_own_resume_arguments() {
        let sent = |kind, id| {
            let fake = Fake {
                programs: vec![("claude", "/x"), ("codex", "/x"), ("opencode", "/x")],
                folders: vec!["/srv/api"],
            };
            plan(
                &local(),
                None,
                "/srv/api",
                &agent(kind, Some(id)),
                &ssh(),
                &fake,
            )
            .unwrap()
            .send
            .unwrap()
        };
        assert_eq!(
            sent(AgentId::CLAUDE, "abc-123"),
            "claude --resume abc-123\r"
        );
        assert_eq!(sent(AgentId::CODEX, "abc-123"), "codex resume abc-123\r");
        assert_eq!(
            sent(AgentId::OPENCODE, "abc-123"),
            "opencode --session abc-123\r"
        );
    }

    #[test]
    fn an_id_with_odd_characters_is_quoted_for_the_shell() {
        let line = command_line(AgentId::CLAUDE, Some("it's; rm -rf ~"), Flavor::Posix);
        assert_eq!(line, r"claude --resume 'it'\''s; rm -rf ~'");
        let line = command_line(AgentId::CLAUDE, Some("it's x"), Flavor::PowerShell);
        assert_eq!(line, "claude --resume 'it''s x'");
        let line = command_line(AgentId::CODEX, Some("a b"), Flavor::Cmd);
        assert_eq!(line, "codex resume \"a b\"");
    }

    #[test]
    fn the_shell_decides_the_quoting_flavour() {
        assert_eq!(flavor_of("/bin/zsh"), Flavor::Posix);
        assert_eq!(flavor_of("/usr/bin/fish"), Flavor::Posix);
        assert_eq!(
            flavor_of("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            Flavor::PowerShell
        );
        assert_eq!(flavor_of("powershell"), Flavor::PowerShell);
        assert_eq!(flavor_of("C:\\Windows\\System32\\cmd.exe"), Flavor::Cmd);
    }

    #[test]
    fn an_agent_that_is_not_installed_here_is_refused_with_its_name() {
        let error = plan(
            &local(),
            None,
            "/srv/api",
            &agent(AgentId::CODEX, None),
            &ssh(),
            &fake(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            LaunchError::NotInstalled {
                agent: AgentId::CODEX,
                machine: "This machine".into()
            }
        );
        assert_eq!(error.to_string(), "Codex is not installed on This machine.");
    }

    #[test]
    fn a_folder_that_does_not_exist_here_is_refused() {
        let error = plan(&local(), None, "/nope", &Launch::Shell, &ssh(), &fake()).unwrap_err();
        assert_eq!(error, LaunchError::NoSuchFolder("/nope".into()));
    }

    #[test]
    fn a_remote_terminal_is_an_interactive_ssh_into_the_remote_login_shell_in_the_folder() {
        let plan = plan(
            &remote(),
            None,
            "/srv/my api",
            &Launch::Shell,
            &ssh(),
            &fake(),
        )
        .unwrap();
        assert_eq!(plan.spawn.program, "ssh");
        assert!(
            plan.spawn.args.contains(&"-t".to_owned()),
            "a terminal is requested"
        );
        let remote_command = plan.spawn.args.last().unwrap();
        assert!(
            remote_command.starts_with("cd '/srv/my api' && exec sh -c "),
            "{remote_command}"
        );
        assert!(
            remote_command.contains("${SHELL:-/bin/sh}"),
            "{remote_command}"
        );
        assert!(remote_command.contains("-l -i"), "{remote_command}");
        assert!(
            !plan.can_detect_foreground,
            "the terminal's process is ssh, always"
        );
    }

    #[test]
    fn a_remote_agent_is_typed_into_that_shell_and_checked_against_the_probe() {
        let found = report(Some("/home/dev/.local/bin/claude"));
        let plan = plan(
            &remote(),
            Some(&found),
            "/srv/api",
            &agent(AgentId::CLAUDE, Some("abc")),
            &ssh(),
            &fake(),
        )
        .unwrap();
        assert_eq!(plan.send.as_deref(), Some("claude --resume abc\r"));
        let missing = report(None);
        let error = plan_for_missing(&missing);
        assert_eq!(
            error.to_string(),
            "Claude Code is not installed on build box."
        );
    }

    fn plan_for_missing(report: &ProbeReport) -> LaunchError {
        plan(
            &remote(),
            Some(report),
            "/srv/api",
            &agent(AgentId::CLAUDE, None),
            &ssh(),
            &fake(),
        )
        .unwrap_err()
    }

    #[test]
    fn an_unprobed_remote_is_tried_anyway() {
        let plan = plan(
            &remote(),
            None,
            "/srv/api",
            &agent(AgentId::CLAUDE, None),
            &ssh(),
            &fake(),
        )
        .unwrap();
        assert_eq!(plan.send.as_deref(), Some("claude\r"));
    }

    #[test]
    fn a_remote_folder_is_not_checked_from_here() {
        assert!(plan(
            &remote(),
            None,
            "/not/on/this/computer",
            &Launch::Shell,
            &ssh(),
            &fake()
        )
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn a_program_is_found_in_the_first_directory_that_has_it_runnable() {
        use std::os::unix::fs::PermissionsExt;
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let make = |dir: &tempfile::TempDir, name: &str, mode: u32| {
            let path = dir.path().join(name);
            std::fs::write(&path, "").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        make(&first, "agent", 0o644); // there, but not runnable
        let wanted = make(&second, "agent", 0o755);
        let dirs = [first.path().to_path_buf(), second.path().to_path_buf()];
        assert_eq!(find_in(&dirs, "agent"), Some(wanted));
        assert_eq!(find_in(&dirs, "leon-no-such-program-anywhere"), None);
        assert_eq!(find_in(&[], "agent"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_login_shell_is_the_users_started_as_a_login_interactive_shell() {
        let login = ["-l", "-i"].map(str::to_owned).to_vec();
        assert_eq!(
            unix_login_shell(Some("/bin/zsh".into())),
            ("/bin/zsh".to_owned(), login.clone())
        );
        assert_eq!(
            unix_login_shell(None),
            ("/bin/sh".to_owned(), login.clone())
        );
        assert_eq!(
            unix_login_shell(Some(String::new())),
            ("/bin/sh".to_owned(), login)
        );
    }

    #[test]
    fn a_folder_exists_only_when_it_is_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(RealSystem.dir_exists(&dir.path().to_string_lossy()));
        assert!(!RealSystem.dir_exists(&dir.path().join("missing").to_string_lossy()));
    }

    #[test]
    fn arguments_are_split_like_a_shell_would() {
        assert_eq!(split_words("--model opus"), ["--model", "opus"]);
        assert_eq!(
            split_words(r#"--prompt "a b" 'c d' e\ f"#),
            ["--prompt", "a b", "c d", "e f"]
        );
        assert_eq!(split_words("  "), Vec::<String>::new());
        assert_eq!(split_words(r#"x "" y"#), ["x", "", "y"]);
    }

    #[test]
    fn the_extra_arguments_follow_the_command_of_a_new_or_a_resumed_session() {
        let prefs = AgentPrefs {
            executable: Some("/opt/bin/claude".into()),
            args: vec!["--model".into(), "opus".into()],
            resume_args: vec!["--verbose".into()],
        };
        assert_eq!(
            command_line_with(AgentId::CLAUDE, None, Flavor::Posix, &prefs),
            Some("/opt/bin/claude --model opus".to_owned())
        );
        assert_eq!(
            command_line_with(AgentId::CLAUDE, Some("abc"), Flavor::Posix, &prefs),
            Some("/opt/bin/claude --resume abc --verbose".to_owned())
        );
    }

    #[test]
    fn a_chosen_shell_and_environment_replace_the_login_shell() {
        let prefs = LaunchPrefs {
            shell: Some(("/bin/fish".into(), vec!["-i".into()])),
            env: vec![("EDITOR".into(), "nvim".into())],
            ..LaunchPrefs::default()
        };
        let planned = plan_with(
            &local(),
            None,
            "/srv/api",
            &Launch::Shell,
            &ssh(),
            &fake(),
            &prefs,
        )
        .unwrap();
        assert_eq!(planned.spawn.program, "/bin/fish");
        assert_eq!(planned.spawn.args, ["-i"]);
        assert_eq!(
            planned.spawn.env,
            [("EDITOR".to_owned(), "nvim".to_owned())]
        );
    }

    #[test]
    fn leons_own_variables_come_first_so_the_users_win() {
        let prefs = LaunchPrefs {
            env: vec![
                ("EDITOR".into(), "nvim".into()),
                ("LEON_MEMORY".into(), "/my/own.md".into()),
            ],
            leon_env: vec![
                ("LEON_BIN".into(), "/opt/leon".into()),
                ("LEON_MEMORY".into(), "/data/memory/api.md".into()),
            ],
            ..LaunchPrefs::default()
        };
        let planned = plan_with(
            &local(),
            None,
            "/srv/api",
            &agent(AgentId::CLAUDE, None),
            &ssh(),
            &fake(),
            &prefs,
        )
        .unwrap();
        let names: Vec<&str> = planned.spawn.env.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["LEON_BIN", "LEON_MEMORY", "EDITOR", "LEON_MEMORY"]);
        // The terminal sets them in this order: the last one stays.
        let command = leon_pty::spec::command_builder(&planned.spawn);
        let value = |name: &str| command.get_env(name).and_then(|v| v.to_str());
        assert_eq!(value("LEON_BIN"), Some("/opt/leon"));
        assert_eq!(value("LEON_MEMORY"), Some("/my/own.md"));
        // Nothing of it is typed: the agent's line is as before.
        assert_eq!(planned.send.as_deref(), Some("claude\r"));
    }

    #[test]
    fn a_remote_terminal_is_told_nothing_of_leons_memory() {
        let prefs = LaunchPrefs {
            leon_env: vec![("LEON_MEMORY".into(), "/data/memory/api.md".into())],
            ..LaunchPrefs::default()
        };
        let planned = plan_with(
            &remote(),
            Some(&report(Some("/usr/bin/claude"))),
            "/srv/api",
            &Launch::Shell,
            &ssh(),
            &fake(),
            &prefs,
        )
        .unwrap();
        assert!(planned.spawn.env.is_empty());
        assert!(planned.spawn.args.iter().all(|arg| !arg.contains("LEON_")));
    }

    fn custom(name: &str, command: &str, args: &str, resume: &str) -> AgentSpec {
        leon_core::CustomAgent::new(name, command, args, resume, &[])
            .unwrap()
            .to_spec(&[])
            .unwrap()
    }

    #[test]
    fn a_custom_agent_is_quoted_for_every_shell_flavour() {
        let spec = custom(
            "My Tool",
            "my-tool",
            "--name 'Ana Smith' --fast",
            "--resume {id}",
        );
        let none = AgentPrefs::default();
        let line = |resume, flavor| command_line_for(&spec, resume, flavor, &none).unwrap();
        assert_eq!(
            line(None, Flavor::Posix),
            r"my-tool --name 'Ana Smith' --fast"
        );
        assert_eq!(
            line(Some("it's; rm -rf ~"), Flavor::Posix),
            r"my-tool --resume 'it'\''s; rm -rf ~'"
        );
        assert_eq!(
            line(Some("it's x"), Flavor::PowerShell),
            "my-tool --resume 'it''s x'"
        );
        assert_eq!(line(Some("a b"), Flavor::Cmd), "my-tool --resume \"a b\"");
        assert_eq!(
            line(None, Flavor::PowerShell),
            "my-tool --name 'Ana Smith' --fast"
        );
    }

    #[test]
    fn a_custom_agent_that_continues_the_latest_session_resumes_without_an_id() {
        let spec = custom("Latest Tool", "lt", "", "--continue");
        let none = AgentPrefs::default();
        assert_eq!(
            command_line_for(&spec, Some("ignored"), Flavor::Posix, &none).as_deref(),
            Some("lt --continue")
        );
        let launch_only = custom("Only Tool", "ot", "", "");
        assert_eq!(
            command_line_for(&launch_only, Some("x"), Flavor::Posix, &none),
            None
        );
        assert_eq!(
            command_line_for(&launch_only, None, Flavor::Posix, &none).as_deref(),
            Some("ot")
        );
    }

    #[test]
    fn a_catalogue_agent_starts_with_its_own_command_and_a_launch_only_one_is_never_resumed() {
        let grok = AgentId::GROK;
        assert_eq!(
            command_line_with(grok, None, Flavor::Posix, &AgentPrefs::default()).as_deref(),
            Some("grok")
        );
        let kiro = AgentId::parse("kiro").unwrap();
        assert_eq!(
            command_line_with(kiro, None, Flavor::Posix, &AgentPrefs::default()).as_deref(),
            Some("kiro-cli chat --tui")
        );
        let amp = AgentId::parse("amp").unwrap();
        assert_eq!(
            command_line_with(amp, Some("x"), Flavor::Posix, &AgentPrefs::default()),
            None
        );
        let copilot = AgentId::parse("copilot").unwrap();
        assert_eq!(
            command_line_with(copilot, Some("s1"), Flavor::Posix, &AgentPrefs::default())
                .as_deref(),
            Some("copilot '--resume=s1'")
        );
        // An id that is not in the catalogue has no command at all.
        let gone = AgentId::parse("never-in-the-catalogue").unwrap();
        assert_eq!(
            command_line_with(gone, None, Flavor::Posix, &AgentPrefs::default()),
            None
        );
    }

    #[test]
    fn resuming_a_launch_only_agent_or_an_unknown_one_is_refused_before_anything_starts() {
        let machine = Machine {
            id: leon_core::MachineId::local(),
            name: "This machine".into(),
            kind: MachineKind::Local,
        };
        let planned = |kind, resume: Option<&str>| {
            plan(
                &machine,
                None,
                "/srv/api",
                &Launch::Agent {
                    kind,
                    resume: resume.map(str::to_owned),
                    account: None,
                },
                &SshOptions::without_multiplexing(),
                &fake(),
            )
        };
        let amp = AgentId::parse("amp").unwrap();
        assert_eq!(
            planned(amp, Some("x")),
            Err(LaunchError::CannotResume { agent: amp })
        );
        let gone = AgentId::parse("never-in-the-catalogue").unwrap();
        assert_eq!(planned(gone, None), Err(LaunchError::UnknownAgent(gone)));
        assert!(LaunchError::CannotResume { agent: amp }
            .to_string()
            .contains("new sessions only"));
    }
    #[test]
    fn a_script_alone_is_typed_as_it_is() {
        for flavor in [Flavor::Posix, Flavor::PowerShell, Flavor::Cmd] {
            assert_eq!(
                chain("npm test", First::Script, None, flavor),
                "npm test\r",
                "{flavor:?}"
            );
        }
    }

    #[test]
    fn a_setup_alone_is_handed_to_sh_on_a_posix_shell_and_typed_as_it_is_elsewhere() {
        assert_eq!(
            chain("npm install", First::Setup, None, Flavor::Posix),
            "sh -c 'npm install'\r"
        );
        // Single quotes keep `!` away from history expansion.
        assert_eq!(
            chain("echo hi!! && make", First::Setup, None, Flavor::Posix),
            "sh -c 'echo hi!! && make'\r"
        );
        for flavor in [Flavor::PowerShell, Flavor::Cmd] {
            assert_eq!(
                chain("npm install", First::Setup, None, flavor),
                "npm install\r",
                "{flavor:?}"
            );
        }
    }

    #[test]
    fn the_agent_follows_the_command_only_when_it_succeeded() {
        assert_eq!(
            chain(
                "npm install",
                First::Setup,
                Some("claude --resume x\r"),
                Flavor::Posix
            ),
            "sh -c 'npm install' && claude --resume x\r"
        );
        assert_eq!(
            chain(
                "npm install",
                First::Setup,
                Some("claude"),
                Flavor::PowerShell
            ),
            "npm install; if ($?) { claude }\r"
        );
        assert_eq!(
            chain("npm install", First::Setup, Some("claude"), Flavor::Cmd),
            "npm install && claude\r"
        );
    }

    #[test]
    fn a_posix_command_is_quoted_whole() {
        // A quote in the command cannot end the quoting of the line.
        assert_eq!(
            chain(
                "echo 'a b' && make",
                First::Setup,
                Some("claude"),
                Flavor::Posix
            ),
            "sh -c 'echo '\\''a b'\\'' && make' && claude\r"
        );
        // And `;` or `||` inside it stay inside it.
        assert_eq!(
            chain("a; b || c", First::Setup, Some("claude"), Flavor::Posix),
            "sh -c 'a; b || c' && claude\r"
        );
        // A backslash is written outside the quotes, where fish reads it as
        // bash does.
        assert_eq!(
            chain("echo a\\nb", First::Setup, None, Flavor::Posix),
            "sh -c 'echo a'\\\\'nb'\r"
        );
    }

    #[test]
    fn the_flavour_of_a_terminal_is_its_shells_or_posix_away_from_here() {
        let own = LaunchPrefs {
            shell: Some(("C:\\Windows\\pwsh.exe".to_owned(), Vec::new())),
            ..LaunchPrefs::default()
        };
        assert_eq!(
            shell_flavor(&local(), &fake(), &LaunchPrefs::default()),
            Flavor::Posix
        );
        assert_eq!(shell_flavor(&local(), &fake(), &own), Flavor::PowerShell);
        assert_eq!(shell_flavor(&remote(), &fake(), &own), Flavor::Posix);
    }

    /// The sequencing is the shell's, so it is proved with one. Only where
    /// there is a POSIX shell to ask.
    #[cfg(leon_posix_tests)]
    #[test]
    fn a_real_shell_runs_the_agent_only_after_the_command_succeeded() {
        let run = |first: &str| {
            let line = chain(
                first,
                First::Setup,
                Some("echo AGENT-STARTED"),
                Flavor::Posix,
            );
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", line.trim_end_matches('\r')])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert_eq!(run("echo set-up"), "set-up\nAGENT-STARTED\n");
        assert_eq!(
            run("echo set-up; exit 3"),
            "set-up\n",
            "a failed setup stops the agent"
        );
        assert_eq!(
            run("false || false"),
            "",
            "its own operators stay inside it"
        );
        assert_eq!(
            run("false; true"),
            "AGENT-STARTED\n",
            "the last status is the command's"
        );
    }

    // ----- a first prompt on the launch line ------------------------------------------------------

    fn prompted(kind: AgentId, prompt: &str) -> Launch {
        Launch::Prompted {
            kind,
            prompt: prompt.to_owned(),
            account: None,
        }
    }

    fn line_of(kind: AgentId, prompt: &str) -> String {
        command_line_prompted(
            kind.spec().unwrap(),
            prompt,
            Flavor::Posix,
            &AgentPrefs::default(),
        )
        .unwrap()
    }

    #[test]
    fn each_agent_with_a_verified_form_gets_the_prompt_in_that_form() {
        let prompt = "fix the login bug";
        assert_eq!(
            line_of(AgentId::CLAUDE, prompt),
            "claude 'fix the login bug'"
        );
        assert_eq!(line_of(AgentId::CODEX, prompt), "codex 'fix the login bug'");
        assert_eq!(line_of(AgentId::GROK, prompt), "grok 'fix the login bug'");
        assert_eq!(
            line_of(AgentId::CURSOR, prompt),
            "cursor-agent 'fix the login bug'"
        );
        assert_eq!(
            line_of(AgentId::OPENCODE, prompt),
            "opencode --prompt 'fix the login bug'"
        );
        let gemini = AgentId::parse("gemini").unwrap();
        assert_eq!(
            line_of(gemini, prompt),
            "gemini --prompt-interactive 'fix the login bug'"
        );
    }

    // ----- accounts -----------------------------------------------------------

    fn work() -> Account {
        Account {
            id: "claude-work".into(),
            agent: AgentId::CLAUDE,
            name: "Work".into(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), "~/.claude work".into())],
        }
    }

    fn lab() -> Account {
        Account {
            id: "codex-lab".into(),
            agent: AgentId::CODEX,
            name: "Lab".into(),
            env: vec![("CODEX_HOME".into(), "/data/codex-lab".into())],
        }
    }

    fn with_accounts() -> LaunchPrefs {
        LaunchPrefs {
            env: vec![("FROM_SETTINGS".into(), "1".into())],
            accounts: vec![work(), lab()],
            ..LaunchPrefs::default()
        }
    }

    fn as_account(kind: AgentId, account: &str) -> Launch {
        Launch::Agent {
            kind,
            resume: None,
            account: Some(account.to_owned()),
        }
    }

    struct Homed(Fake);
    impl System for Homed {
        fn find_program(&self, name: &str) -> Option<PathBuf> {
            self.0.find_program(name)
        }
        fn login_shell(&self) -> (String, Vec<String>) {
            self.0.login_shell()
        }
        fn dir_exists(&self, path: &str) -> bool {
            self.0.dir_exists(path)
        }
        fn home_dir(&self) -> Option<String> {
            Some("/home/me".into())
        }
    }

    #[test]
    fn the_variables_of_an_account_are_resolved_for_the_machine_they_run_on() {
        let accounts = [work(), lab()];
        let env = |account: Option<&str>, kind, home: Option<&str>| {
            account_env(account, kind, &accounts, home)
        };
        // No account, no variable.
        assert_eq!(env(None, AgentId::CLAUDE, None), Ok(Vec::new()));
        // `~` is the home folder of the machine, whichever it is.
        assert_eq!(
            env(Some("claude-work"), AgentId::CLAUDE, Some("/home/me")),
            Ok(vec![(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/home/me/.claude work".to_owned()
            )])
        );
        assert_eq!(
            env(Some("claude-work"), AgentId::CLAUDE, Some("/Users/dev")),
            Ok(vec![(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/Users/dev/.claude work".to_owned()
            )])
        );
        // A value that needs no home does not ask for one.
        assert_eq!(
            env(Some("codex-lab"), AgentId::CODEX, None),
            Ok(vec![(
                "CODEX_HOME".to_owned(),
                "/data/codex-lab".to_owned()
            )])
        );
        // A `~` that cannot be expanded is said, not passed on as a literal.
        assert_eq!(
            env(Some("claude-work"), AgentId::CLAUDE, None),
            Err(LaunchError::NoHome {
                account: "Work".into()
            })
        );
    }

    #[test]
    fn the_extra_arguments_of_the_settings_come_before_the_prompt() {
        let prefs = AgentPrefs {
            executable: Some("/opt/bin/claude".into()),
            args: vec!["--model".into(), "opus".into()],
            resume_args: vec!["--ignored".into()],
        };
        let line =
            command_line_prompted(AgentId::CLAUDE.spec().unwrap(), "hi", Flavor::Posix, &prefs)
                .unwrap();
        assert_eq!(line, "/opt/bin/claude --model opus hi");
    }

    #[test]
    fn an_agent_without_a_verified_form_or_a_posix_shell_is_refused() {
        let aider = AgentId::parse("aider").unwrap();
        assert_eq!(
            command_line_prompted(
                aider.spec().unwrap(),
                "hi",
                Flavor::Posix,
                &AgentPrefs::default()
            ),
            Err(LaunchError::NoPromptForm { agent: aider })
        );
        for flavor in [Flavor::PowerShell, Flavor::Cmd] {
            assert_eq!(
                command_line_prompted(
                    AgentId::CLAUDE.spec().unwrap(),
                    "hi",
                    flavor,
                    &AgentPrefs::default()
                ),
                Err(LaunchError::PromptNeedsPosixShell)
            );
        }
    }

    /// Reads a line made of plain words and single-quoted ones back into its
    /// words, as a POSIX shell would (the line has no other quoting).
    fn words_of(line: &str) -> Vec<String> {
        let (mut words, mut word, mut open, mut any) = (Vec::new(), String::new(), false, false);
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match (open, c) {
                (true, '\'') => open = false,
                (true, c) => word.push(c),
                (false, '\'') => (open, any) = (true, true),
                (false, '\\') => {
                    word.push(chars.next().unwrap());
                    any = true;
                }
                (false, ' ') => {
                    if any {
                        words.push(std::mem::take(&mut word));
                        any = false;
                    }
                }
                (false, c) => {
                    word.push(c);
                    any = true;
                }
            }
        }
        assert!(!open, "{line}");
        if any {
            words.push(word);
        }
        words
    }

    #[test]
    fn a_prompt_with_quotes_dollars_backticks_and_bangs_is_one_argument_on_one_line() {
        for prompt in [
            "it's \"quoted\" and 'single'",
            "price $5, $HOME and $(reboot) and `reboot`",
            "wow! !! !$ !history",
            r"C:\Users\me and \' and \\",
            "a;b && c | d > e",
        ] {
            let line = line_of(AgentId::CLAUDE, prompt);
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(words_of(&line), ["claude", prompt], "{line}");
        }
    }

    #[test]
    fn a_prompt_of_several_lines_is_typed_on_a_single_line_without_a_bang() {
        let line = line_of(AgentId::CODEX, "first!\nsecond's $x `y`\n\nfourth");
        assert!(
            !line.contains('\n'),
            "a line break would run the line: {line}"
        );
        assert!(!line.contains('!'), "history expansion reads it: {line}");
        assert!(line.starts_with("codex \"$(printf '%b' "), "{line}");
        assert!(line.ends_with(")\""), "{line}");
    }

    #[test]
    fn a_prompt_is_typed_as_given_to_a_local_shell_and_over_ssh() {
        let prompt = "refactor it's \"parser\"\nthen $HOME!";
        let launch = prompted(AgentId::CLAUDE, prompt);
        let local = plan(&local(), None, "/srv/api", &launch, &ssh(), &fake()).unwrap();
        let remote = plan(
            &remote(),
            Some(&report(Some("/usr/bin/claude"))),
            "/srv/api",
            &launch,
            &ssh(),
            &fake(),
        )
        .unwrap();
        let expected = format!("{}\r", line_of(AgentId::CLAUDE, prompt));
        assert_eq!(local.send.as_deref(), Some(expected.as_str()));
        assert_eq!(
            remote.send.as_deref(),
            Some(expected.as_str()),
            "the remote login shell is typed into just the same"
        );
        // The prompt is in the typed line, not in what ssh runs.
        assert!(!remote.spawn.args.iter().any(|arg| arg.contains("refactor")));
    }

    #[test]
    fn a_prompted_agent_is_checked_for_like_any_other() {
        let launch = prompted(AgentId::CODEX, "hi");
        assert_eq!(
            plan(&local(), None, "/srv/api", &launch, &ssh(), &fake()).unwrap_err(),
            LaunchError::NotInstalled {
                agent: AgentId::CODEX,
                machine: "This machine".into()
            }
        );
        assert_eq!(
            plan(
                &local(),
                None,
                "/nowhere",
                &prompted(AgentId::CLAUDE, "hi"),
                &ssh(),
                &fake()
            )
            .unwrap_err(),
            LaunchError::NoSuchFolder("/nowhere".into())
        );
    }

    #[test]
    fn a_prompt_is_cleaned_before_it_is_typed() {
        assert_eq!(clean_prompt("  fix it \n"), Ok("fix it".into()));
        assert_eq!(
            clean_prompt("one\r\ntwo\rthree\tfour"),
            Ok("one\ntwo\nthree four".into())
        );
        assert_eq!(clean_prompt("é ✓ 日本語"), Ok("é ✓ 日本語".into()));
        assert_eq!(clean_prompt("refactor!"), Ok("refactor!".into()));
        assert_eq!(clean_prompt("fix it"), Ok("fix it".into()));
    }

    #[test]
    fn a_prompt_that_cannot_be_typed_safely_is_refused_with_the_reason() {
        for (text, why) in [
            ("", "Type a prompt."),
            (" \n\t ", "Type a prompt."),
            ("a\u{1b}[31mb", "control characters"),
            ("a\u{3}b", "control characters"),
            ("a\u{7f}b", "control characters"),
            ("a\u{85}b", "control characters"),
            ("--help", "start with a dash"),
            ("- a list item", "start with a dash"),
            ("apply", "one word"),
            ("  update\n", "one word"),
            ("log_out-now", "one word"),
        ] {
            let error = clean_prompt(text).unwrap_err();
            assert!(error.contains(why), "{text:?}: {error}");
        }
        let sentence = |chars: usize| format!("x {}", "y".repeat(chars - 2));
        assert!(clean_prompt(&sentence(MAX_PROMPT + 1))
            .unwrap_err()
            .contains("too long"));
        assert!(clean_prompt(&sentence(MAX_PROMPT)).is_ok());
    }

    /// The quoting is proved with a shell where there is one to ask: what an
    /// agent would receive as its argument is exactly the prompt.
    #[cfg(leon_posix_tests)]
    #[test]
    fn a_real_shell_hands_the_agent_exactly_the_prompt() {
        for prompt in [
            "it's \"quoted\"",
            "$HOME `id` $(id) !! done\\",
            "line one\nline two!\n\nit's line four",
        ] {
            let line = line_of(AgentId::CLAUDE, prompt).replacen("claude", "printf '%s'", 1);
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &line])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), prompt);
        }
    }

    #[test]
    fn an_account_that_is_gone_or_is_another_agents_refuses_instead_of_falling_back() {
        let accounts = [work()];
        let gone = LaunchError::UnknownAccount {
            account: "claude-old".into(),
        };
        assert_eq!(
            account_env(Some("claude-old"), AgentId::CLAUDE, &accounts, Some("/h")),
            Err(gone.clone())
        );
        assert_eq!(
            account_env(Some("claude-work"), AgentId::CODEX, &accounts, Some("/h")),
            Err(LaunchError::UnknownAccount {
                account: "claude-work".into()
            })
        );
        assert!(gone.to_string().contains("claude-old"));
        // The plan refuses before anything starts.
        let planned = plan_with(
            &local(),
            None,
            "/srv/api",
            &as_account(AgentId::CLAUDE, "claude-old"),
            &ssh(),
            &fake(),
            &LaunchPrefs::default(),
        );
        assert_eq!(planned, Err(gone));
    }

    #[test]
    fn a_local_terminal_gets_the_variables_of_its_account_after_the_settings_ones() {
        let plan = plan_with(
            &local(),
            None,
            "/srv/api",
            &as_account(AgentId::CLAUDE, "claude-work"),
            &ssh(),
            &Homed(fake()),
            &with_accounts(),
        )
        .unwrap();
        assert_eq!(
            plan.spawn.env,
            [
                ("FROM_SETTINGS".to_owned(), "1".to_owned()),
                (
                    "CLAUDE_CONFIG_DIR".to_owned(),
                    "/home/me/.claude work".to_owned()
                )
            ]
        );
        // The line typed is the agent's, as it always was: the variables are
        // the terminal's, not part of what is typed.
        assert_eq!(plan.send.as_deref(), Some("claude\r"));
        // Another terminal of the same agent is not touched.
        let own = plan_with(
            &local(),
            None,
            "/srv/api",
            &agent(AgentId::CLAUDE, None),
            &ssh(),
            &Homed(fake()),
            &with_accounts(),
        )
        .unwrap();
        assert_eq!(
            own.spawn.env,
            [("FROM_SETTINGS".to_owned(), "1".to_owned())]
        );
    }

    #[test]
    fn an_ssh_terminal_sets_the_variables_on_the_remote_line_each_quoted_as_one_word() {
        let mut hostile = work();
        hostile.env = vec![
            ("CLAUDE_CONFIG_DIR".into(), "~/.claude-work".into()),
            ("NOTE".into(), "it's $(touch /tmp/x); `id` \"q\"".into()),
        ];
        let prefs = LaunchPrefs {
            accounts: vec![hostile],
            ..LaunchPrefs::default()
        };
        let mut found = report(Some("/home/dev/.local/bin/claude"));
        found.home = Some("/home/dev".into());
        let plan = plan_with(
            &remote(),
            Some(&found),
            "/srv/api",
            &as_account(AgentId::CLAUDE, "claude-work"),
            &ssh(),
            &fake(),
            &prefs,
        )
        .unwrap();
        assert_eq!(plan.spawn.program, "ssh");
        let line = plan.spawn.args.last().unwrap();
        assert!(
            line.starts_with("cd /srv/api && exec env 'CLAUDE_CONFIG_DIR=/home/dev/.claude-work' "),
            "{line}"
        );
        assert!(
            line.contains(r#"'NOTE=it'\''s $(touch /tmp/x); `id` "q"'"#),
            "{line}"
        );
        // Nothing of the variables is typed into the shell.
        assert_eq!(plan.send.as_deref(), Some("claude\r"));
        // The settings' own environment is for this computer's terminals only.
        assert!(!line.contains("FROM_SETTINGS"));
    }

    #[test]
    fn a_remote_account_with_a_home_relative_folder_waits_for_the_machine_to_be_known() {
        let prefs = with_accounts();
        let asked = |report: Option<&ProbeReport>| {
            plan_with(
                &remote(),
                report,
                "/srv/api",
                &as_account(AgentId::CLAUDE, "claude-work"),
                &ssh(),
                &fake(),
                &prefs,
            )
        };
        // Never probed: the home is unknown.
        assert_eq!(
            asked(None),
            Err(LaunchError::NoHome {
                account: "Work".into()
            })
        );
        let mut found = report(Some("/x/claude"));
        assert!(matches!(
            asked(Some(&found)),
            Err(LaunchError::NoHome { .. })
        ));
        found.home = Some("/home/dev".into());
        assert!(asked(Some(&found)).is_ok());
    }

    #[test]
    fn a_relay_terminal_carries_the_variables_in_the_spec_the_host_runs() {
        let relayed = Machine {
            id: MachineId::from_string("r1"),
            name: "their box".into(),
            kind: MachineKind::Relay {
                host_id: "host-id".into(),
                host_key: "00".repeat(32),
                relay_url: "wss://relay.example".into(),
                name: "Their computer".into(),
            },
        };
        let mut found = report(Some("/x/claude"));
        found.home = Some("/home/them".into());
        let plan = plan_with(
            &relayed,
            Some(&found),
            "/srv/api",
            &as_account(AgentId::CLAUDE, "claude-work"),
            &ssh(),
            &fake(),
            &with_accounts(),
        )
        .unwrap();
        assert!(plan.spawn.route.is_some(), "the host runs it");
        assert_eq!(
            plan.spawn.env,
            [(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/home/them/.claude work".to_owned()
            )]
        );
    }

    #[test]
    fn resuming_with_an_account_keeps_the_resume_form_and_the_account() {
        let launch = Launch::Agent {
            kind: AgentId::CODEX,
            resume: Some("abc".into()),
            account: Some("codex-lab".into()),
        };
        let fake = Fake {
            programs: vec![("codex", "/x/codex")],
            folders: vec!["/srv/api"],
        };
        let plan = plan_with(
            &local(),
            None,
            "/srv/api",
            &launch,
            &ssh(),
            &fake,
            &with_accounts(),
        )
        .unwrap();
        assert_eq!(plan.send.as_deref(), Some("codex resume abc\r"));
        assert_eq!(
            plan.spawn.env,
            [
                ("FROM_SETTINGS".to_owned(), "1".to_owned()),
                ("CODEX_HOME".to_owned(), "/data/codex-lab".to_owned())
            ]
        );
    }
}
