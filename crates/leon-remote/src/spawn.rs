//! A child process, started without a window of its own.
//!
//! A release build of Leon is a Windows *GUI* program: it has no console.
//! Windows answers that by giving every console program such a process starts
//! a console of its own, so `git`, `curl` or `ssh` each flash a black window
//! while it runs. One window per command is already noise; Leon starts a
//! handful of commands the moment a project opens (`git worktree list`, `git
//! branch`) and again on every usage or avatar refresh, so a launch turns
//! into a burst of windows that steal the focus from the window the user is
//! typing in.
//!
//! [`creation_flags`] asks Windows for the one flag that prevents it
//! (`CREATE_NO_WINDOW`) and [`child`] is the [`Command`] every such process is
//! started from. It changes the window and nothing else: these children talk
//! through pipes, never through a console, so there is nothing to lose. It
//! must never be applied to a child that needs a terminal of its own; those
//! live in `leon-pty`, which attaches them to a pseudo-terminal instead.

use std::ffi::OsStr;

use tokio::process::Command;

/// Windows' `CREATE_NO_WINDOW`: run the child without a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The creation flags a child of Leon's runs with: `CREATE_NO_WINDOW` on
/// Windows and nothing anywhere else. A plain value, so a test on any computer
/// can pin it.
pub const fn creation_flags() -> u32 {
    #[cfg(windows)]
    {
        CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// A command that will not open a console window on Windows. Arguments,
/// environment, streams and directory are the caller's business, as usual.
pub fn child(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    let mut command = Command::new(program);
    #[cfg(not(windows))]
    let command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(creation_flags());
    command
}

/// [`child`] for the few places that start a short command and wait for it
/// without an async runtime (the macOS `security` and `open` tools): the same
/// flag, a `std` command.
pub fn std_child(program: impl AsRef<OsStr>) -> std::process::Command {
    #[cfg(windows)]
    let mut command = std::process::Command::new(program);
    #[cfg(not(windows))]
    let command = std::process::Command::new(program);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, creation_flags());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No part of Leon that is not a pseudo-terminal child may start a process
    /// by hand: each would flash a console window on Windows. Every
    /// `Command::new` of the sources is either in this module (the helper), in
    /// `leon-host`'s own copy of it (that crate does not depend on this one),
    /// in `leon-pty` (a PTY child keeps its pseudo-console), in `xtask`
    /// (a developer tool), or in test code.
    #[test]
    fn no_source_starts_a_process_without_the_helper() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut offenders = Vec::new();
        let mut stack = vec![crates];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if path.is_dir() {
                    if !matches!(
                        name.as_str(),
                        "target" | "xtask" | "leon-pty" | "tests" | "examples"
                    ) {
                        stack.push(path);
                    }
                    continue;
                }
                let text_path = path.to_string_lossy().replace('\\', "/");
                let allowed = text_path.ends_with("leon-remote/src/spawn.rs")
                    || text_path.ends_with("leon-host/src/exec.rs")
                    || name == "tests.rs"
                    || name.starts_with("tests_")
                    || path.extension().is_none_or(|e| e != "rs");
                if allowed {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                for (number, line) in source.lines().enumerate() {
                    // Test modules sit at the end of a file.
                    if line.trim() == "#[cfg(test)]" {
                        break;
                    }
                    let code = line.split("//").next().unwrap_or_default();
                    if code.contains("Command::new(") && !code.contains("CommandSpec::new(") {
                        offenders.push(format!("{}:{}: {}", text_path, number + 1, line.trim()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "start the process with leon_remote::spawn::child or std_child:\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn the_std_variant_keeps_its_program_and_arguments() {
        let mut command = std_child("security");
        command.arg("find-generic-password");
        assert_eq!(command.get_program(), OsStr::new("security"));
        assert_eq!(command.get_args().count(), 1);
    }

    #[test]
    fn the_flag_is_asked_for_on_windows_and_nowhere_else() {
        let flags = creation_flags();
        if cfg!(windows) {
            // The value Windows documents for CREATE_NO_WINDOW, pinned here so
            // a quiet change to it cannot come back unnoticed.
            assert_eq!(flags, 0x0800_0000);
        } else {
            assert_eq!(flags, 0);
        }
    }

    #[test]
    fn a_child_keeps_its_program_and_its_arguments() {
        let mut command = child("git");
        let command = command.arg("--version").arg("status").as_std();
        assert_eq!(command.get_program(), OsStr::new("git"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("--version"), OsStr::new("status")]
        );
    }

    /// The flag must not cost the child anything it needs to run. `CREATE_NO_WINDOW`
    /// is invisible to the program itself, so a child asked for it still reads
    /// its pipes and reports its status; this is what would break if the flag
    /// were ever set together with a flag that does matter (`DETACHED_PROCESS`,
    /// say) or on the wrong stream.
    #[tokio::test]
    async fn a_child_still_runs_and_still_streams() {
        #[cfg(windows)]
        let (program, args, expected): (&str, &[&str], &str) =
            ("cmd", &["/C", "echo hello"], "hello\r\n");
        #[cfg(not(windows))]
        let (program, args, expected): (&str, &[&str], &str) =
            ("sh", &["-c", "printf hello"], "hello");

        let output = child(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
        assert!(output.status.success());
    }
}
