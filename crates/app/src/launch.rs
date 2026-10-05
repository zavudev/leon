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

use leon_core::{AgentKind, Machine, MachineKind};
use leon_remote::{agent_launch, interactive_on, sh_quote, CommandSpec, ProbeReport, SshOptions};
use leon_term::SpawnSpec;
use std::path::{Path, PathBuf};

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// An agent, resuming the session with this id of its own when given.
    Agent {
        /// Which agent.
        kind: AgentKind,
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
        agent: AgentKind,
        /// The machine's name.
        machine: String,
    },
    /// The folder does not exist on this computer.
    NoSuchFolder(String),
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

/// The agent's path in a probe report: `Some(None)` when the machine was
/// probed and does not have it.
pub fn probed(report: &ProbeReport, kind: AgentKind) -> Option<&str> {
    match kind {
        AgentKind::Claude => report.claude.as_deref(),
        AgentKind::Codex => report.codex.as_deref(),
        AgentKind::Opencode => report.opencode.as_deref(),
    }
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
    /// How each agent starts, in the order of [`AgentKind::ALL`].
    pub agents: [AgentPrefs; 3],
}

impl LaunchPrefs {
    /// How `kind` is started.
    pub fn agent(&self, kind: AgentKind) -> &AgentPrefs {
        let place = AgentKind::ALL
            .iter()
            .position(|candidate| *candidate == kind)
            .unwrap_or(0);
        &self.agents[place]
    }
}

/// Splits what a person typed into arguments the way a shell would: spaces
/// separate, single and double quotes keep words together and a backslash
/// escapes the next character outside single quotes.
pub fn split_words(text: &str) -> Vec<String> {
    let (mut words, mut word, mut any) = (Vec::new(), String::new(), false);
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    any = true;
                }
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            (None, c) => {
                word.push(c);
                any = true;
            }
        }
    }
    if any {
        words.push(word);
    }
    words
}

/// The command line that starts an agent in a shell, without the Enter.
#[cfg(test)]
pub fn command_line(kind: AgentKind, resume: Option<&str>, flavor: Flavor) -> String {
    command_line_with(kind, resume, flavor, &AgentPrefs::default())
}

/// [`command_line`] with the executable and the extra arguments the settings
/// give: the arguments follow the command (`claude --resume <id> <extra>`).
pub fn command_line_with(
    kind: AgentKind,
    resume: Option<&str>,
    flavor: Flavor,
    prefs: &AgentPrefs,
) -> String {
    let (program, args) = agent_launch(kind, resume);
    let program = prefs.executable.clone().unwrap_or(program);
    let extra = if resume.is_some() {
        &prefs.resume_args
    } else {
        &prefs.args
    };
    std::iter::once(program)
        .chain(args)
        .chain(extra.iter().cloned())
        .map(|word| quote(&word, flavor))
        .collect::<Vec<_>>()
        .join(" ")
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
    if let Launch::Agent { kind, .. } = launch {
        let (name, _) = agent_launch(*kind, None);
        let own = prefs.agent(*kind).executable.as_deref();
        let installed = if local {
            match own {
                // A chosen program is a file, or a name the shell finds.
                Some(program) if Path::new(program).components().count() > 1 => {
                    is_runnable(Path::new(program))
                }
                Some(program) => system.find_program(program).is_some(),
                None => system.find_program(&name).is_some(),
            }
        } else {
            report.is_none_or(|report| probed(report, *kind).is_some())
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
            claude: claude.map(str::to_owned),
            codex: None,
            opencode: None,
        }
    }

    fn agent(kind: AgentKind, resume: Option<&str>) -> Launch {
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
            &agent(AgentKind::Claude, None),
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
            sent(AgentKind::Claude, "abc-123"),
            "claude --resume abc-123\r"
        );
        assert_eq!(sent(AgentKind::Codex, "abc-123"), "codex resume abc-123\r");
        assert_eq!(
            sent(AgentKind::Opencode, "abc-123"),
            "opencode --session abc-123\r"
        );
    }

    #[test]
    fn an_id_with_odd_characters_is_quoted_for_the_shell() {
        let line = command_line(AgentKind::Claude, Some("it's; rm -rf ~"), Flavor::Posix);
        assert_eq!(line, r"claude --resume 'it'\''s; rm -rf ~'");
        let line = command_line(AgentKind::Claude, Some("it's x"), Flavor::PowerShell);
        assert_eq!(line, "claude --resume 'it''s x'");
        let line = command_line(AgentKind::Codex, Some("a b"), Flavor::Cmd);
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
            &agent(AgentKind::Codex, None),
            &ssh(),
            &fake(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            LaunchError::NotInstalled {
                agent: AgentKind::Codex,
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
            &agent(AgentKind::Claude, Some("abc")),
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
            &agent(AgentKind::Claude, None),
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
            &agent(AgentKind::Claude, None),
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
            command_line_with(AgentKind::Claude, None, Flavor::Posix, &prefs),
            "/opt/bin/claude --model opus"
        );
        assert_eq!(
            command_line_with(AgentKind::Claude, Some("abc"), Flavor::Posix, &prefs),
            "/opt/bin/claude --resume abc --verbose"
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
}
