//! What to spawn, as a neutral description.
//!
//! The application maps its own command type onto [`SpawnSpec`], so this
//! crate does not depend on how commands are built. [`command_builder`] turns
//! the spec into the PTY library's command, adding the terminal type every
//! program expects to find.

use portable_pty::CommandBuilder;

/// The `TERM` value set for every child.
pub const TERM: &str = "xterm-256color";
/// The `COLORTERM` value set for every child: 24-bit colour is supported.
pub const COLORTERM: &str = "truecolor";

/// A program to run in a terminal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpawnSpec {
    /// Program name or path.
    pub program: String,
    /// Arguments, not including the program.
    pub args: Vec<String>,
    /// Environment variables set on top of the inherited ones.
    pub env: Vec<(String, String)>,
    /// Working directory; the inherited one when absent.
    pub cwd: Option<String>,
    /// For a machine reached through a relay: where to run it instead of on
    /// this computer (host id, key and relay), or `None` to start it here. The PTY layer ignores it; the application's
    /// backend routes on it.
    pub route: Option<String>,
}

impl SpawnSpec {
    /// A spec that runs `program` with no arguments.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }
}

/// The command to start: the spec's program, arguments and directory, its
/// environment on top of this process's, and `TERM` / `COLORTERM`. A variable
/// the spec sets wins over these two.
pub fn command_builder(spec: &SpawnSpec) -> CommandBuilder {
    command_builder_with(spec, false)
}

/// [`command_builder`], with the choice of the environment: `own_environment`
/// makes the spec's `env` the whole environment of the child (plus `TERM` and
/// `COLORTERM`, and `SHELL` when the spec has none), instead of this
/// process's with the spec's on top. A long-lived process that starts
/// terminals for someone else (the keeper of durable sessions) must not leak
/// its own, stale environment into them: the one who asks sends the
/// environment the terminal is to have.
pub fn command_builder_with(spec: &SpawnSpec, own_environment: bool) -> CommandBuilder {
    let mut command = CommandBuilder::new(&spec.program);
    if own_environment {
        // `SHELL` is the one variable the builder invents when it is unset;
        // keep that so a terminal still knows its shell.
        let shell = command.get_env("SHELL").map(std::ffi::OsStr::to_owned);
        command.env_clear();
        if let Some(shell) = shell {
            command.env("SHELL", shell);
        }
    }
    command.args(&spec.args);
    command.env("TERM", TERM);
    command.env("COLORTERM", COLORTERM);
    for (name, value) in &spec.env {
        command.env(name, value);
    }
    if let Some(cwd) = &spec.cwd {
        command.cwd(cwd);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn spec() -> SpawnSpec {
        SpawnSpec {
            program: "claude".into(),
            args: vec!["--resume".into(), "abc".into()],
            env: vec![("FOO".into(), "bar".into())],
            cwd: Some("/srv/api".into()),
            route: None,
        }
    }

    #[test]
    fn the_command_runs_the_program_with_its_arguments_in_its_directory() {
        let command = command_builder(&spec());
        assert_eq!(command.get_argv(), &["claude", "--resume", "abc"]);
        assert_eq!(
            command.get_cwd().map(|c| c.as_os_str()),
            Some(OsStr::new("/srv/api"))
        );
    }

    #[test]
    fn the_terminal_type_and_true_colour_are_always_announced() {
        let command = command_builder(&SpawnSpec::new("sh"));
        assert_eq!(command.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert_eq!(command.get_env("COLORTERM"), Some(OsStr::new("truecolor")));
    }

    #[test]
    fn the_specs_environment_is_added_and_may_override_term() {
        let mut spec = spec();
        spec.env.push(("TERM".into(), "screen".into()));
        let command = command_builder(&spec);
        assert_eq!(command.get_env("FOO"), Some(OsStr::new("bar")));
        assert_eq!(command.get_env("TERM"), Some(OsStr::new("screen")));
    }

    // An own environment is the keeper's, and `SHELL` is a unix variable.
    #[cfg(unix)]
    #[test]
    fn an_own_environment_replaces_ours_and_keeps_only_what_a_terminal_needs() {
        // `PATH` is in every test process's environment.
        let inherited = command_builder(&SpawnSpec::new("sh"));
        assert!(inherited.get_env("PATH").is_some());
        let mut spec = spec();
        spec.env = vec![("ONLY".into(), "this".into())];
        let own = command_builder_with(&spec, true);
        assert_eq!(own.get_env("ONLY"), Some(OsStr::new("this")));
        assert_eq!(
            own.get_env("PATH"),
            None,
            "the keeper's own PATH is not leaked"
        );
        assert_eq!(own.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert!(
            own.get_env("SHELL").is_some(),
            "a terminal still knows its shell"
        );
        // A shell the spec names wins over the invented one.
        spec.env.push(("SHELL".into(), "/bin/zsh".into()));
        let named = command_builder_with(&spec, true);
        assert_eq!(named.get_env("SHELL"), Some(OsStr::new("/bin/zsh")));
    }

    #[test]
    fn without_a_directory_the_child_inherits_ours() {
        let command = command_builder(&SpawnSpec::new("sh"));
        assert!(command.get_cwd().is_none());
    }
}
