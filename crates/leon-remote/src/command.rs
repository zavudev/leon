//! Command descriptions and their placement onto a machine.
//!
//! A [`CommandSpec`] says what to run: a program, its arguments, extra
//! environment variables and a working directory. Callers write it from the
//! point of view of the machine the command is meant for. [`run_on`] and
//! [`interactive_on`] then produce the spec of the process to start on the
//! machine Leon runs on:
//!
//! * for the local machine, the same spec;
//! * for an SSH machine, an `ssh` invocation whose last argument is the whole
//!   remote command as one POSIX shell string,
//!   `cd <dir> && exec env <vars> <program> <args...>`, with every part
//!   quoted.
//!
//! `ssh` always runs with `BatchMode=yes`, so it fails instead of prompting
//! for a password or a host-key confirmation that nobody would see.
//!
//! Connection multiplexing (`ControlMaster`) lets later commands reuse the
//! first connection to a host, which turns each remote command from a full
//! SSH handshake into a round trip. The OpenSSH build shipped with Windows
//! does not support it, so whether to use it is an explicit option;
//! [`SshOptions::for_this_platform`] picks the right value for the platform
//! Leon was built for.

use std::path::{Path, PathBuf};

use leon_core::{Machine, MachineKind};

use crate::quote::sh_quote;

/// How long an idle shared SSH connection is kept open by default, in minutes.
pub const DEFAULT_PERSIST_MINUTES: u32 = 10;

/// What to run and where, on some machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandSpec {
    /// Program name or path.
    pub program: String,
    /// Arguments, not including the program itself.
    pub args: Vec<String>,
    /// Environment variables to set in addition to the inherited ones.
    pub env: Vec<(String, String)>,
    /// Working directory; the default of the machine when absent.
    pub cwd: Option<String>,
    /// Standard input, closed after it is written; the command gets none
    /// (end of file at once) when absent.
    pub stdin: Option<Vec<u8>>,
    /// For a machine reached through a relay: where to send the command (see
    /// [`crate::relay::route`]). The runner that starts the process routes on
    /// it; `None` means "start it here".
    pub route: Option<String>,
}

impl CommandSpec {
    /// A command that runs `program` with no arguments.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    /// Adds one argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Adds several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets one environment variable.
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    /// Sets the working directory.
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets the standard input the command reads.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }
}

/// Options for the `ssh` invocations Leon builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshOptions {
    /// Directory for connection-sharing sockets. Multiplexing is used when
    /// this is set and not used when it is absent.
    ///
    /// The directory must exist, be private to the user and have a short
    /// path: the operating system limits socket paths to roughly a hundred
    /// bytes.
    pub control_dir: Option<PathBuf>,
    /// How long an idle shared connection stays open, in minutes. Only used
    /// with a `control_dir`.
    pub persist_minutes: u32,
    /// How long `ssh` waits to connect, in seconds; `ssh`'s own limit when
    /// absent.
    pub connect_timeout: Option<u32>,
}

impl Default for SshOptions {
    fn default() -> Self {
        Self {
            control_dir: None,
            persist_minutes: DEFAULT_PERSIST_MINUTES,
            connect_timeout: None,
        }
    }
}

impl SshOptions {
    /// Options with multiplexing explicitly switched on or off.
    pub fn new(runtime_dir: impl Into<PathBuf>, multiplex: bool) -> Self {
        Self {
            control_dir: multiplex.then(|| runtime_dir.into()),
            ..Self::default()
        }
    }

    /// Options suited to the platform Leon was built for: multiplexing on
    /// Unix, none on Windows, whose OpenSSH lacks it.
    pub fn for_this_platform(runtime_dir: impl Into<PathBuf>) -> Self {
        Self::new(runtime_dir, cfg!(unix))
    }

    /// Options without multiplexing.
    pub fn without_multiplexing() -> Self {
        Self::default()
    }
}

/// The process to start here so that `command` runs on `machine` without a
/// terminal. Use it for commands whose output Leon reads.
pub fn run_on(machine: &Machine, command: &CommandSpec, ssh: &SshOptions) -> CommandSpec {
    place(machine, command, ssh, false)
}

/// The process to start here so that `command` runs on `machine` attached to
/// a terminal. Use it for agent sessions hosted in a pseudo-terminal: for an
/// SSH machine it adds `-t` so the remote side allocates one too.
pub fn interactive_on(machine: &Machine, command: &CommandSpec, ssh: &SshOptions) -> CommandSpec {
    place(machine, command, ssh, true)
}

fn place(
    machine: &Machine,
    command: &CommandSpec,
    ssh: &SshOptions,
    interactive: bool,
) -> CommandSpec {
    let MachineKind::Ssh {
        host,
        user,
        port,
        identity_file,
    } = &machine.kind
    else {
        let mut placed = command.clone();
        if let MachineKind::Relay {
            host_id,
            host_key,
            relay_url,
            ..
        } = &machine.kind
        {
            // The host runs it as written; the runner sends it there.
            placed.route = Some(crate::relay::route(host_id, host_key, relay_url));
        }
        return placed;
    };

    let mut args: Vec<String> = vec!["-o".into(), "BatchMode=yes".into()];
    if interactive {
        args.push("-t".into());
    }
    if let Some(directory) = &ssh.control_dir {
        args.extend([
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", control_path(directory)),
            "-o".into(),
            format!("ControlPersist={}m", ssh.persist_minutes.max(1)),
        ]);
    }
    if let Some(seconds) = ssh.connect_timeout.filter(|seconds| *seconds > 0) {
        args.extend(["-o".into(), format!("ConnectTimeout={seconds}")]);
    }
    if let Some(port) = port {
        args.extend(["-p".into(), port.to_string()]);
    }
    if let Some(identity_file) = identity_file {
        args.extend(["-i".into(), identity_file.clone()]);
    }
    if let Some(user) = user {
        args.extend(["-l".into(), user.clone()]);
    }
    // Everything after "--" is the destination and the command, even if the
    // host name starts with a dash.
    args.extend(["--".into(), host.clone(), remote_shell_command(command)]);

    CommandSpec {
        program: "ssh".into(),
        args,
        env: Vec::new(),
        cwd: None,
        // `ssh` hands its own standard input to the remote command.
        stdin: command.stdin.clone(),
        route: None,
    }
}

/// The socket path pattern for shared connections. `%C` is expanded by `ssh`
/// to a hash of the connection parameters, which keeps the path short and
/// distinct per destination.
fn control_path(directory: &Path) -> String {
    directory.join("%C").to_string_lossy().into_owned()
}

/// Renders a command as the single POSIX shell string a remote shell
/// executes: `cd <dir> && exec env <vars> <program> <args...>`.
///
/// `exec` replaces the shell with the program, so signals and the exit status
/// belong to the program itself. The working directory is quoted, so a
/// leading `~` is not expanded: pass absolute paths.
pub fn remote_shell_command(command: &CommandSpec) -> String {
    let mut line = String::new();
    if let Some(cwd) = &command.cwd {
        line.push_str("cd ");
        line.push_str(&sh_quote(cwd));
        line.push_str(" && ");
    }
    line.push_str("exec");
    if !command.env.is_empty() {
        line.push_str(" env");
        for (name, value) in &command.env {
            line.push(' ');
            line.push_str(&sh_quote(&format!("{name}={value}")));
        }
    }
    line.push(' ');
    line.push_str(&sh_quote(&command.program));
    for arg in &command.args {
        line.push(' ');
        line.push_str(&sh_quote(arg));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::MachineId;

    fn local() -> Machine {
        Machine {
            id: MachineId::local(),
            name: "This machine".into(),
            kind: MachineKind::Local,
        }
    }

    fn ssh_machine(
        host: &str,
        user: Option<&str>,
        port: Option<u16>,
        identity_file: Option<&str>,
    ) -> Machine {
        Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: host.into(),
                user: user.map(str::to_owned),
                port,
                identity_file: identity_file.map(str::to_owned),
            },
        }
    }

    fn git_status() -> CommandSpec {
        CommandSpec::new("git").arg("status").cwd("/srv/my api")
    }

    #[test]
    fn a_command_spec_is_built_fluently() {
        let spec = CommandSpec::new("git")
            .arg("log")
            .args(["-n", "1"])
            .env("LC_ALL", "C")
            .cwd("/srv/api");
        assert_eq!(spec.program, "git");
        assert_eq!(spec.args, ["log", "-n", "1"]);
        assert_eq!(spec.env, [("LC_ALL".to_owned(), "C".to_owned())]);
        assert_eq!(spec.cwd.as_deref(), Some("/srv/api"));
    }

    #[test]
    fn standard_input_reaches_the_process_that_is_started() {
        let command = git_status().stdin("data");
        assert_eq!(command.stdin.as_deref(), Some(b"data".as_slice()));
        let ssh = ssh_machine("build.example", None, None, None);
        let placed = run_on(&ssh, &command, &SshOptions::without_multiplexing());
        assert_eq!(placed.stdin, command.stdin);
        let placed = run_on(&local(), &command, &SshOptions::without_multiplexing());
        assert_eq!(placed.stdin, command.stdin);
    }

    #[test]
    fn a_local_command_runs_directly_in_its_working_directory() {
        let command = git_status().env("LC_ALL", "C");
        let placed = run_on(&local(), &command, &SshOptions::without_multiplexing());
        assert_eq!(placed, command);
        assert_eq!(
            interactive_on(&local(), &command, &SshOptions::new("/run/leon", true)),
            command
        );
    }

    #[test]
    fn a_remote_command_becomes_a_batch_mode_ssh_invocation() {
        let machine = ssh_machine("build.example", None, None, None);
        let placed = run_on(&machine, &git_status(), &SshOptions::without_multiplexing());
        assert_eq!(placed.program, "ssh");
        assert_eq!(
            placed.args,
            [
                "-o",
                "BatchMode=yes",
                "--",
                "build.example",
                "cd '/srv/my api' && exec git status"
            ]
        );
        assert_eq!(placed.cwd, None, "the directory applies on the remote side");
        assert!(placed.env.is_empty());
    }

    #[test]
    fn port_identity_and_user_are_passed_when_set() {
        let machine = ssh_machine("build.example", Some("dev"), Some(2222), Some("/keys/id"));
        let placed = run_on(&machine, &git_status(), &SshOptions::without_multiplexing());
        assert_eq!(
            placed.args[..9],
            [
                "-o",
                "BatchMode=yes",
                "-p",
                "2222",
                "-i",
                "/keys/id",
                "-l",
                "dev",
                "--"
            ]
        );
        assert_eq!(placed.args[9], "build.example");
    }

    #[test]
    fn multiplexing_options_are_added_when_enabled() {
        let machine = ssh_machine("build.example", None, None, None);
        let options = SshOptions::new("run", true);
        let placed = run_on(&machine, &git_status(), &options);
        let expected_path = format!("ControlPath={}", Path::new("run").join("%C").display());
        assert_eq!(
            placed.args[..8],
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ControlMaster=auto",
                "-o",
                expected_path.as_str(),
                "-o",
                "ControlPersist=10m"
            ]
        );
    }

    #[test]
    fn multiplexing_options_are_absent_when_disabled() {
        let machine = ssh_machine("build.example", None, None, None);
        let placed = run_on(&machine, &git_status(), &SshOptions::new("run", false));
        assert!(!placed.args.iter().any(|arg| arg.starts_with("Control")));
    }

    #[test]
    fn the_platform_default_multiplexes_only_on_unix() {
        let options = SshOptions::for_this_platform("run");
        assert_eq!(options.control_dir.is_some(), cfg!(unix));
    }

    #[test]
    fn an_interactive_remote_command_requests_a_terminal() {
        let machine = ssh_machine("build.example", None, None, None);
        let command = CommandSpec::new("claude").cwd("/srv/api");
        let placed = interactive_on(&machine, &command, &SshOptions::without_multiplexing());
        assert_eq!(
            placed.args,
            [
                "-o",
                "BatchMode=yes",
                "-t",
                "--",
                "build.example",
                "cd /srv/api && exec claude"
            ]
        );
        let batch = run_on(&machine, &command, &SshOptions::without_multiplexing());
        assert!(!batch.args.contains(&"-t".to_owned()));
    }

    #[test]
    fn a_host_that_looks_like_an_option_cannot_become_one() {
        let machine = ssh_machine("-oProxyCommand=reboot", None, None, None);
        let placed = run_on(&machine, &git_status(), &SshOptions::without_multiplexing());
        let separator = placed.args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(placed.args[separator + 1], "-oProxyCommand=reboot");
    }

    #[test]
    fn the_remote_command_quotes_every_part() {
        let command = CommandSpec::new("my tool")
            .args(["it's", "$HOME", "", "a;b"])
            .cwd("/srv/it's here");
        assert_eq!(
            remote_shell_command(&command),
            r"cd '/srv/it'\''s here' && exec 'my tool' 'it'\''s' '$HOME' '' 'a;b'"
        );
    }

    #[test]
    fn the_remote_command_sets_environment_variables_through_env() {
        let command = CommandSpec::new("git")
            .arg("status")
            .env("LC_ALL", "C")
            .env("NOTE", "two words");
        assert_eq!(
            remote_shell_command(&command),
            "exec env 'LC_ALL=C' 'NOTE=two words' git status"
        );
    }

    #[test]
    fn a_remote_command_without_a_directory_has_no_cd() {
        assert_eq!(
            remote_shell_command(&CommandSpec::new("uname")),
            "exec uname"
        );
    }

    /// Runs the generated remote string through a real shell, standing in for
    /// the remote side of an SSH connection.
    #[cfg(unix)]
    #[test]
    fn a_real_shell_executes_the_remote_command_as_intended() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("it's a dir");
        std::fs::create_dir(&nested).unwrap();
        let command = CommandSpec::new("sh")
            .args([
                "-c",
                "printf '%s|%s|%s' \"$PWD\" \"$GREETING\" \"$1\"",
                "sh",
                "a b",
            ])
            .env("GREETING", "hello there")
            .cwd(nested.to_string_lossy());
        let Ok(output) = std::process::Command::new("sh")
            .arg("-c")
            .arg(remote_shell_command(&command))
            .output()
        else {
            eprintln!("skipped: no POSIX shell available");
            return;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let fields: Vec<_> = stdout.split('|').collect();
        assert!(fields[0].ends_with("it's a dir"), "ran in {stdout:?}");
        assert_eq!(fields[1..], ["hello there", "a b"]);
    }

    #[test]
    fn the_persist_time_and_the_connect_timeout_are_ssh_options() {
        let machine = ssh_machine("h", None, None, None);
        let options = SshOptions {
            control_dir: Some("run".into()),
            persist_minutes: 45,
            connect_timeout: Some(7),
        };
        let placed = run_on(&machine, &git_status(), &options);
        assert!(placed.args.contains(&"ControlPersist=45m".to_owned()));
        assert!(placed.args.contains(&"ConnectTimeout=7".to_owned()));
        let plain = run_on(&machine, &git_status(), &SshOptions::default());
        assert!(!plain
            .args
            .iter()
            .any(|arg| arg.starts_with("ConnectTimeout")));
        let none = SshOptions {
            connect_timeout: Some(0),
            ..SshOptions::default()
        };
        let zero = run_on(&machine, &git_status(), &none);
        assert!(!zero
            .args
            .iter()
            .any(|arg| arg.starts_with("ConnectTimeout")));
    }
}
