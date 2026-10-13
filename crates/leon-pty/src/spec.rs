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
    let mut command = CommandBuilder::new(program(&spec.program));
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
        command.cwd(folder(cwd));
    }
    command
}

/// The program to start for `name`.
///
/// A client that drives agents (the phone, a relay) asks for `sh`: its
/// scripts are POSIX. Windows has no `sh` on `PATH` unless someone put it
/// there, but Git for Windows ships one with everything those scripts use
/// (`mkfifo`, `stty`), so on Windows `sh` is that one when `PATH` has none.
/// Any other name, and `sh` everywhere else, is started as it is.
pub fn program(name: &str) -> std::ffi::OsString {
    #[cfg(windows)]
    if name == "sh" {
        if let Some(sh) = git_sh(std::env::var_os("PATH"), std::env::var_os("ProgramFiles")) {
            return sh.into_os_string();
        }
    }
    name.into()
}

/// The folder to start in for `path`. On Windows a path that `sh` above
/// printed (`/c/Users/me`) is the drive's (`C:/Users/me`); everything else
/// is used as it is.
pub fn folder(path: &str) -> String {
    let b = path.as_bytes();
    if cfg!(windows)
        && b.len() >= 2
        && b[0] == b'/'
        && b[1].is_ascii_alphabetic()
        && (b.len() == 2 || b[2] == b'/')
    {
        let drive = char::from(b[1]).to_ascii_uppercase();
        return format!("{drive}:/{}", path.get(3..).unwrap_or(""));
    }
    path.to_owned()
}

/// `sh.exe` on `path`, else the one of the Git for Windows whose `git.exe`
/// is on `path` (`<Git>\cmd\git.exe` → `<Git>\bin\sh.exe`), else the one in
/// the default install folder under `program_files`.
#[cfg_attr(not(windows), allow(dead_code))]
fn git_sh(
    path: Option<std::ffi::OsString>,
    program_files: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    let dirs: Vec<_> = path.iter().flat_map(std::env::split_paths).collect();
    let on_path = dirs.iter().map(|dir| dir.join("sh.exe"));
    let beside_git = dirs
        .iter()
        .filter(|dir| dir.join("git.exe").is_file())
        .filter_map(|dir| Some(dir.parent()?.join("bin").join("sh.exe")));
    let installed = program_files.map(|dir| std::path::Path::new(&dir).join(r"Git\bin\sh.exe"));
    on_path
        .chain(beside_git)
        .chain(installed)
        .find(|sh| sh.is_file())
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
    fn sh_is_found_beside_git_or_in_its_install_folder() {
        let root = tempfile::tempdir().unwrap();
        let git = root.path().join("Git");
        std::fs::create_dir_all(git.join("cmd")).unwrap();
        std::fs::create_dir_all(git.join("bin")).unwrap();
        std::fs::write(git.join(r"cmd/git.exe"), "").unwrap();
        let sh = git.join("bin").join("sh.exe");
        let path = std::env::join_paths([git.join("cmd")]).unwrap();
        assert_eq!(git_sh(Some(path.clone()), None), None);
        std::fs::write(&sh, "").unwrap();
        assert_eq!(git_sh(Some(path), None), Some(sh.clone()));
        #[cfg(windows)]
        assert_eq!(git_sh(None, Some(root.path().into())), Some(sh));
        assert_eq!(git_sh(None, None), None);
    }

    #[test]
    fn a_folder_sh_printed_is_the_drives_on_windows() {
        let windows = cfg!(windows);
        assert_eq!(
            folder("/c/Users/me"),
            if windows {
                "C:/Users/me"
            } else {
                "/c/Users/me"
            }
        );
        assert_eq!(folder("/d"), if windows { "D:/" } else { "/d" });
        for kept in ["/srv/api", "C:/Code", "/", "rel"] {
            assert_eq!(folder(kept), kept);
        }
    }

    #[test]
    fn without_a_directory_the_child_inherits_ours() {
        let command = command_builder(&SpawnSpec::new("sh"));
        assert!(command.get_cwd().is_none());
    }
}
