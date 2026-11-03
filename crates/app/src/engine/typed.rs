//! A command the person typed in a conversation after `!`: run once, to the
//! end, in the conversation's folder on its machine, and its output given
//! back whole. No terminal: what it prints is read, not shown live, and it
//! gets no input, so a command that asks something fails instead of waiting.
//!
//! It goes through the slow runner (the one that commits and pushes), so it
//! is the same on this computer, over SSH and over the relay, and it is
//! stopped when it outlives that runner's limit or when the task is dropped.

use std::sync::atomic::Ordering;

use leon_core::{MachineId, MachineKind};
use leon_remote::{CommandSpec, Output, RunError, Runner};
use tokio::task::JoinHandle;

use super::Engine;

impl Engine {
    /// Runs `command`, as the person wrote it, in `cwd` on `machine`. The
    /// error says why it could not be run, or that it outlived the runner's
    /// limit; a command that ran and failed is an [`Output`] with its status.
    pub fn run_typed(
        &self,
        machine: MachineId,
        cwd: String,
        command: String,
    ) -> JoinHandle<Result<Output, RunError>> {
        let engine = self.clone();
        self.inner.handle.spawn(async move {
            let machine =
                engine
                    .inner
                    .store
                    .machine(&machine)
                    .map_err(|error| RunError::Spawn {
                        program: "sh".to_owned(),
                        source: std::io::Error::other(error.to_string()),
                    })?;
            // Every remote machine is POSIX; this computer is unless it is
            // Windows without a shell of its own.
            let posix = machine.kind != MachineKind::Local
                || engine.inner.local_posix_shell.load(Ordering::Relaxed);
            let spec = typed_command(&command, posix).cwd(cwd);
            let placed = leon_remote::run_on(&machine, &spec, &engine.ssh());
            engine.slow().run(&placed).await
        })
    }
}

/// What runs a typed command: the POSIX shell, or PowerShell on a Windows
/// computer that has none.
pub fn typed_command(command: &str, posix: bool) -> CommandSpec {
    if posix {
        CommandSpec::new("sh").args(["-c", command])
    } else {
        CommandSpec::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", command])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_command_goes_to_the_shell_whole() {
        let posix = typed_command("ls ~ | wc -l", true);
        assert_eq!(posix.program, "sh");
        assert_eq!(posix.args, ["-c", "ls ~ | wc -l"]);
        let windows = typed_command("dir", false);
        assert_eq!(windows.program, "powershell");
        assert_eq!(windows.args.last().map(String::as_str), Some("dir"));
    }
}
