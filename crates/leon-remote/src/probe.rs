//! A one-round-trip inventory of a machine.
//!
//! Before offering to start agents on a machine, Leon needs to know what the
//! machine is and what is installed there. [`probe`] answers that with a
//! single command, so adding or refreshing an SSH machine costs one round
//! trip. The command is a small POSIX shell script that prints `key=value`
//! lines; [`parse_probe`] reads them.
//!
//! Scope: the script needs a POSIX `sh`. That covers Linux and macOS, local
//! or remote. A Windows machine cannot be probed yet; running the probe
//! locally on Windows fails with a [`ProbeError`].
//!
//! Non-interactive SSH sessions often start with a minimal `PATH` that lacks
//! the per-user directories agents are installed into. The script therefore
//! searches a few well-known directories in addition to `PATH` and reports
//! the absolute path of each tool it finds, which callers can use to start
//! the tool regardless of the session's `PATH`.

use leon_core::Machine;
use thiserror::Error;

use crate::command::{run_on, CommandSpec, SshOptions};
use crate::runner::{RunError, Runner};

/// The script run on the machine. Each line of output is `key=value`; a tool
/// line is `tool=<name>=<absolute path>` and is printed only for tools that
/// exist.
const PROBE_SCRIPT: &str = r#"PATH="$PATH:$HOME/.local/bin:$HOME/.opencode/bin:$HOME/.bun/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin"
printf 'os=%s\n' "$(uname -s)"
printf 'arch=%s\n' "$(uname -m)"
printf 'home=%s\n' "$HOME"
for tool in git claude codex opencode; do
  found=$(command -v "$tool" 2>/dev/null) && printf 'tool=%s=%s\n' "$tool" "$found"
done
exit 0"#;

/// The operating system of a probed machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteOs {
    /// Linux.
    Linux,
    /// macOS.
    MacOs,
    /// Anything else, with the name the machine reported (`uname -s`).
    Other(String),
}

/// What a machine reported about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// Operating system.
    pub os: RemoteOs,
    /// Processor architecture as reported by `uname -m`, such as `x86_64` or
    /// `arm64`.
    pub arch: Option<String>,
    /// Home directory of the login user.
    pub home: Option<String>,
    /// Absolute path of `git`, when installed.
    pub git: Option<String>,
    /// Absolute path of `claude`, when installed.
    pub claude: Option<String>,
    /// Absolute path of `codex`, when installed.
    pub codex: Option<String>,
    /// Absolute path of `opencode`, when installed.
    pub opencode: Option<String>,
}

/// Why a machine could not be probed.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// The probe command could not be run at all.
    #[error(transparent)]
    Run(#[from] RunError),
    /// The probe ran but failed, typically because the connection was
    /// refused or the machine has no POSIX shell.
    #[error("probe failed (status {status:?}): {stderr}")]
    Failed {
        /// Exit status, absent when the process was ended by a signal.
        status: Option<i32>,
        /// What was printed to standard error, trimmed.
        stderr: String,
    },
    /// The probe ran but its output did not name an operating system.
    #[error("probe output was not understood")]
    Unrecognised,
}

/// The probe as a command for the target machine.
pub fn probe_command() -> CommandSpec {
    CommandSpec::new("sh").args(["-c", PROBE_SCRIPT])
}

/// Parses the output of the probe script. Returns `None` when the output
/// does not name an operating system, which means the script did not run.
///
/// Lines that are not understood are ignored: login banners and shell
/// start-up noise commonly precede the real output on remote machines.
pub fn parse_probe(output: &str) -> Option<ProbeReport> {
    let mut os = None;
    let mut report = ProbeReport {
        os: RemoteOs::Other(String::new()),
        arch: None,
        home: None,
        git: None,
        claude: None,
        codex: None,
        opencode: None,
    };
    for line in output.lines() {
        let Some((key, value)) = line.trim_end_matches('\r').split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "os" if !value.is_empty() => {
                os = Some(match value {
                    "Linux" => RemoteOs::Linux,
                    "Darwin" => RemoteOs::MacOs,
                    other => RemoteOs::Other(other.to_owned()),
                });
            }
            "arch" if !value.is_empty() => report.arch = Some(value.to_owned()),
            "home" if !value.is_empty() => report.home = Some(value.to_owned()),
            "tool" => {
                if let Some((name, path)) = value.split_once('=') {
                    let slot = match name {
                        "git" => &mut report.git,
                        "claude" => &mut report.claude,
                        "codex" => &mut report.codex,
                        "opencode" => &mut report.opencode,
                        _ => continue,
                    };
                    if !path.is_empty() {
                        *slot = Some(path.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    report.os = os?;
    Some(report)
}

/// Probes `machine` with one command and reports what it found.
pub async fn probe<R: Runner>(
    runner: &R,
    machine: &Machine,
    ssh: &SshOptions,
) -> Result<ProbeReport, ProbeError> {
    let output = runner.run(&run_on(machine, &probe_command(), ssh)).await?;
    if !output.success() {
        return Err(ProbeError::Failed {
            status: output.status,
            stderr: output.stderr.trim().to_owned(),
        });
    }
    parse_probe(&output.stdout).ok_or(ProbeError::Unrecognised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{Output, ScriptedRunner};
    use leon_core::{MachineId, MachineKind};

    const LINUX: &str = "\
os=Linux
arch=x86_64
home=/home/dev
tool=git=/usr/bin/git
tool=claude=/home/dev/.local/bin/claude
";

    fn remote() -> Machine {
        Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: "build.example".into(),
                user: None,
                port: None,
                identity_file: None,
            },
        }
    }

    #[test]
    fn a_linux_machine_reports_its_details_and_installed_tools() {
        let report = parse_probe(LINUX).unwrap();
        assert_eq!(report.os, RemoteOs::Linux);
        assert_eq!(report.arch.as_deref(), Some("x86_64"));
        assert_eq!(report.home.as_deref(), Some("/home/dev"));
        assert_eq!(report.git.as_deref(), Some("/usr/bin/git"));
        assert_eq!(
            report.claude.as_deref(),
            Some("/home/dev/.local/bin/claude")
        );
        assert_eq!(report.codex, None);
        assert_eq!(report.opencode, None);
    }

    #[test]
    fn macos_and_unfamiliar_systems_are_told_apart() {
        assert_eq!(parse_probe("os=Darwin\n").unwrap().os, RemoteOs::MacOs);
        assert_eq!(
            parse_probe("os=FreeBSD\n").unwrap().os,
            RemoteOs::Other("FreeBSD".into())
        );
    }

    #[test]
    fn noise_around_the_output_is_ignored() {
        let noisy = "Welcome to the build box!\r\nLast login: yesterday\r\n\r\nos=Linux\r\nhome=/home/dev\r\nmotd=ignored\r\ntool=unknown=/bin/x\r\ntool=codex=/usr/local/bin/codex\r\ntool=broken\r\n";
        let report = parse_probe(noisy).unwrap();
        assert_eq!(report.os, RemoteOs::Linux);
        assert_eq!(report.home.as_deref(), Some("/home/dev"));
        assert_eq!(report.codex.as_deref(), Some("/usr/local/bin/codex"));
        assert_eq!(report.git, None);
    }

    #[test]
    fn a_tool_path_containing_an_equals_sign_is_kept_whole() {
        let report = parse_probe("os=Linux\ntool=git=/opt/a=b/git\n").unwrap();
        assert_eq!(report.git.as_deref(), Some("/opt/a=b/git"));
    }

    #[test]
    fn output_without_an_operating_system_is_not_a_report() {
        assert_eq!(parse_probe(""), None);
        assert_eq!(parse_probe("command not found: sh\n"), None);
        assert_eq!(parse_probe("os=\nhome=/home/dev\n"), None);
    }

    #[tokio::test]
    async fn probing_a_remote_machine_takes_a_single_ssh_command() {
        let runner = ScriptedRunner::new().reply(Output::ok(LINUX));
        let report = probe(&runner, &remote(), &SshOptions::without_multiplexing())
            .await
            .unwrap();
        assert_eq!(report.os, RemoteOs::Linux);

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].program, "ssh");
        let remote_command = calls[0].args.last().unwrap();
        assert!(remote_command.starts_with("exec sh -c '"));
        assert!(remote_command.contains("uname -s"));
    }

    #[tokio::test]
    async fn a_failed_connection_is_reported_with_its_message() {
        let runner = ScriptedRunner::new().reply(Output::failed(255, "Permission denied\n"));
        let outcome = probe(&runner, &remote(), &SshOptions::without_multiplexing()).await;
        assert!(matches!(
            outcome,
            Err(ProbeError::Failed { status: Some(255), stderr }) if stderr == "Permission denied"
        ));
    }

    #[tokio::test]
    async fn unintelligible_output_is_reported_as_unrecognised() {
        let runner = ScriptedRunner::new().reply(Output::ok("hello\n"));
        let outcome = probe(&runner, &remote(), &SshOptions::without_multiplexing()).await;
        assert!(matches!(outcome, Err(ProbeError::Unrecognised)));
    }

    /// Runs the real script on this machine and through a shell standing in
    /// for the remote side, to check the script and its quoting end to end.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_probe_script_runs_on_this_machine_directly_and_when_quoted_for_ssh() {
        use crate::command::remote_shell_command;
        use crate::runner::ProcessRunner;

        let runner = ProcessRunner::new();
        let local = Machine {
            id: MachineId::local(),
            name: "This machine".into(),
            kind: MachineKind::Local,
        };
        let direct = match probe(&runner, &local, &SshOptions::without_multiplexing()).await {
            Ok(report) => report,
            Err(ProbeError::Run(_)) => {
                eprintln!("skipped: no POSIX shell available");
                return;
            }
            Err(other) => panic!("probe failed: {other}"),
        };
        assert!(matches!(
            direct.os,
            RemoteOs::Linux | RemoteOs::MacOs | RemoteOs::Other(_)
        ));
        assert!(direct.arch.is_some());

        let as_remote =
            CommandSpec::new("sh").args(["-c", &remote_shell_command(&probe_command())]);
        let output = runner.run(&as_remote).await.unwrap();
        assert!(output.success(), "stderr: {}", output.stderr);
        assert_eq!(parse_probe(&output.stdout).unwrap(), direct);
    }
}
