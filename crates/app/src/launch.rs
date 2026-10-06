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

use leon_core::{AgentId, AgentSpec, Machine, MachineKind};
use leon_remote::{agent_launch, interactive_on, sh_quote, CommandSpec, ProbeReport, SshOptions};
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
    /// How each agent starts, by agent; an agent without an entry starts as
    /// the catalogue says.
    pub agents: std::collections::HashMap<AgentId, AgentPrefs>,
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
    if let Launch::Agent { kind, resume } = launch {
        let spec = kind.spec().ok_or(LaunchError::UnknownAgent(*kind))?;
        if resume.is_some() && !spec.can_resume() {
            return Err(LaunchError::CannotResume { agent: *kind });
        }
        let own = prefs.agent(*kind).executable.as_deref();
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
                agent: *kind,
                machine: machine.name.clone(),
            });
        }
    }
    if local && !system.dir_exists(cwd) {
        return Err(LaunchError::NoSuchFolder(cwd.to_owned()));
    }
    let (spawn, flavor) = if local {
        let (program, args) = prefs.shell.clone().unwrap_or_else(|| system.login_shell());
        let flavor = flavor_of(&program);
        let mut command = CommandSpec::new(program).args(args).cwd(cwd);
        command.env = prefs.env.clone();
        (spawn_spec(command), flavor)
    } else {
        let command = CommandSpec::new("sh").args(["-c", REMOTE_SHELL]).cwd(cwd);
        (
            spawn_spec(interactive_on(machine, &command, ssh)),
            Flavor::Posix,
        )
    };
    let send = match launch {
        Launch::Agent { kind, resume } => Some(format!(
            "{}\r",
            command_line_with(*kind, resume.as_deref(), flavor, prefs.agent(*kind))
                .ok_or(LaunchError::UnknownAgent(*kind))?
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
}
