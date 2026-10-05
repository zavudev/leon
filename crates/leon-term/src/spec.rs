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
/// environment, and `TERM` / `COLORTERM`. A variable the spec sets wins over
/// these two.
pub fn command_builder(spec: &SpawnSpec) -> CommandBuilder {
    let mut command = CommandBuilder::new(&spec.program);
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

    #[test]
    fn without_a_directory_the_child_inherits_ours() {
        let command = command_builder(&SpawnSpec::new("sh"));
        assert!(command.get_cwd().is_none());
    }
}
