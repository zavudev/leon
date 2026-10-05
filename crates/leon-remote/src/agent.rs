//! How each coding agent is started and resumed.
//!
//! The command lines here were checked against the `--help` output of the
//! agents' own CLIs:
//!
//! | Agent    | New session | Resume a session          |
//! |----------|-------------|---------------------------|
//! | Claude   | `claude`    | `claude --resume <id>`    |
//! | Codex    | `codex`     | `codex resume <id>`       |
//! | opencode | `opencode`  | `opencode --session <id>` |
//!
//! The id is the agent's own session id, stored by Leon as
//! `Session::external_id`.

use leon_core::{AgentKind, Machine};

use crate::command::{interactive_on, CommandSpec, SshOptions};

/// The program and arguments that start `kind`, resuming the session
/// `resume` when given.
pub fn agent_launch(kind: AgentKind, resume: Option<&str>) -> (String, Vec<String>) {
    let (program, resume_args): (&str, &[&str]) = match kind {
        AgentKind::Claude => ("claude", &["--resume"]),
        AgentKind::Codex => ("codex", &["resume"]),
        AgentKind::Opencode => ("opencode", &["--session"]),
    };
    let args = match resume {
        Some(id) => resume_args
            .iter()
            .map(|arg| (*arg).to_owned())
            .chain([id.to_owned()])
            .collect(),
        None => Vec::new(),
    };
    (program.to_owned(), args)
}

/// The complete interactive command that hosts an agent session in `cwd` on
/// `machine`. The result is meant to be spawned inside a pseudo-terminal.
pub fn session_command(
    machine: &Machine,
    cwd: &str,
    kind: AgentKind,
    resume: Option<&str>,
    ssh: &SshOptions,
) -> CommandSpec {
    let (program, args) = agent_launch(kind, resume);
    let command = CommandSpec::new(program).args(args).cwd(cwd);
    interactive_on(machine, &command, ssh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::{MachineId, MachineKind};

    #[test]
    fn a_new_session_starts_the_bare_program() {
        assert_eq!(
            agent_launch(AgentKind::Claude, None),
            ("claude".to_owned(), vec![])
        );
        assert_eq!(
            agent_launch(AgentKind::Codex, None),
            ("codex".to_owned(), vec![])
        );
        assert_eq!(
            agent_launch(AgentKind::Opencode, None),
            ("opencode".to_owned(), vec![])
        );
    }

    #[test]
    fn each_agent_resumes_with_its_own_syntax() {
        assert_eq!(
            agent_launch(AgentKind::Claude, Some("abc")).1,
            ["--resume", "abc"]
        );
        assert_eq!(
            agent_launch(AgentKind::Codex, Some("abc")).1,
            ["resume", "abc"]
        );
        assert_eq!(
            agent_launch(AgentKind::Opencode, Some("abc")).1,
            ["--session", "abc"]
        );
    }

    #[test]
    fn a_local_session_runs_the_agent_in_the_working_directory() {
        let machine = Machine {
            id: MachineId::local(),
            name: "This machine".into(),
            kind: MachineKind::Local,
        };
        let spec = session_command(
            &machine,
            "/srv/api",
            AgentKind::Codex,
            Some("abc"),
            &SshOptions::without_multiplexing(),
        );
        assert_eq!(spec.program, "codex");
        assert_eq!(spec.args, ["resume", "abc"]);
        assert_eq!(spec.cwd.as_deref(), Some("/srv/api"));
    }

    #[test]
    fn a_remote_session_is_an_interactive_ssh_command() {
        let machine = Machine {
            id: MachineId::from_string("m1"),
            name: "build box".into(),
            kind: MachineKind::Ssh {
                host: "build.example".into(),
                user: Some("dev".into()),
                port: None,
                identity_file: None,
            },
        };
        let spec = session_command(
            &machine,
            "/srv/my api",
            AgentKind::Claude,
            Some("abc"),
            &SshOptions::without_multiplexing(),
        );
        assert_eq!(spec.program, "ssh");
        assert_eq!(
            spec.args,
            [
                "-o",
                "BatchMode=yes",
                "-t",
                "-l",
                "dev",
                "--",
                "build.example",
                "cd '/srv/my api' && exec claude --resume abc"
            ]
        );
    }
}
