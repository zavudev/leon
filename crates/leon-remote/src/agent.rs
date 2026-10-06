//! How each coding agent is started and resumed.
//!
//! The command lines come from the catalogue ([`leon_core::agent`]): the
//! program, the arguments of a new session and the resume form of each agent.
//! The three agents whose history Leon imports were checked against the
//! `--help` output of their own CLIs:
//!
//! | Agent    | New session | Resume a session          |
//! |----------|-------------|---------------------------|
//! | Claude   | `claude`    | `claude --resume <id>`    |
//! | Codex    | `codex`     | `codex resume <id>`       |
//! | opencode | `opencode`  | `opencode --session <id>` |
//!
//! The id is the agent's own session id, stored by Leon as
//! `Session::external_id`.

use leon_core::{AgentSpec, Machine};

use crate::command::{interactive_on, CommandSpec, SshOptions};

/// The program and arguments that start `spec`, resuming the session
/// `resume` when given. `None` when a resume was asked of an agent that is
/// launch only.
pub fn agent_launch(spec: &AgentSpec, resume: Option<&str>) -> Option<(String, Vec<String>)> {
    spec.launch(resume)
}

/// The complete interactive command that hosts an agent session in `cwd` on
/// `machine`. The result is meant to be spawned inside a pseudo-terminal.
/// `None` when a resume was asked of an agent that is launch only.
pub fn session_command(
    machine: &Machine,
    cwd: &str,
    spec: &AgentSpec,
    resume: Option<&str>,
    ssh: &SshOptions,
) -> Option<CommandSpec> {
    let (program, args) = agent_launch(spec, resume)?;
    let command = CommandSpec::new(program).args(args).cwd(cwd);
    Some(interactive_on(machine, &command, ssh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::{AgentId, MachineId, MachineKind};

    fn spec(id: AgentId) -> &'static AgentSpec {
        id.spec().unwrap()
    }

    #[test]
    fn a_new_session_starts_the_bare_program() {
        assert_eq!(
            agent_launch(spec(AgentId::CLAUDE), None).unwrap(),
            ("claude".to_owned(), vec![])
        );
        assert_eq!(
            agent_launch(spec(AgentId::CODEX), None).unwrap(),
            ("codex".to_owned(), vec![])
        );
        assert_eq!(
            agent_launch(spec(AgentId::OPENCODE), None).unwrap(),
            ("opencode".to_owned(), vec![])
        );
    }

    #[test]
    fn each_agent_resumes_with_its_own_syntax() {
        assert_eq!(
            agent_launch(spec(AgentId::CLAUDE), Some("abc")).unwrap().1,
            ["--resume", "abc"]
        );
        assert_eq!(
            agent_launch(spec(AgentId::CODEX), Some("abc")).unwrap().1,
            ["resume", "abc"]
        );
        assert_eq!(
            agent_launch(spec(AgentId::OPENCODE), Some("abc"))
                .unwrap()
                .1,
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
            spec(AgentId::CODEX),
            Some("abc"),
            &SshOptions::without_multiplexing(),
        )
        .unwrap();
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
            spec(AgentId::CLAUDE),
            Some("abc"),
            &SshOptions::without_multiplexing(),
        )
        .unwrap();
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
