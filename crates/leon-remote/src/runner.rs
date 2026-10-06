//! Executing commands and collecting their output.
//!
//! [`Runner`] is the one place where processes are started. Everything above
//! it (git operations, the machine probe) is written against the trait, so it
//! can be tested with [`ScriptedRunner`] and without `ssh`, `git` or a
//! network.
//!
//! A runner reports what happened; it does not judge it. A command that ran
//! and exited with a non-zero status is an `Ok` [`Output`]; [`RunError`] is
//! reserved for commands that could not be run at all.

use std::collections::VecDeque;
use std::future::Future;
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use thiserror::Error;

use crate::command::CommandSpec;

/// What a finished command produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// Exit status; absent when the process was ended by a signal.
    pub status: Option<i32>,
    /// Standard output, with invalid UTF-8 replaced.
    pub stdout: String,
    /// Standard error, with invalid UTF-8 replaced.
    pub stderr: String,
}

impl Output {
    /// Whether the command exited with status zero.
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// A successful output with the given standard output. Handy for
    /// scripting a [`ScriptedRunner`].
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            status: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    /// An unsuccessful output with the given status and standard error.
    pub fn failed(status: i32, stderr: impl Into<String>) -> Self {
        Self {
            status: Some(status),
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// Why a command could not be run to completion.
#[derive(Debug, Error)]
pub enum RunError {
    /// The program could not be started, for example because it is not
    /// installed.
    #[error("cannot start {program}: {source}")]
    Spawn {
        /// The program that was to be started.
        program: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The command did not finish within the runner's time limit and was
    /// stopped.
    #[error("{program} did not finish within {limit:?}")]
    TimedOut {
        /// The program that was running.
        program: String,
        /// The time limit that was exceeded.
        limit: Duration,
    },
}

/// Runs commands to completion and captures their output.
pub trait Runner: Send + Sync {
    /// Runs `spec` and waits for it to finish.
    fn run(&self, spec: &CommandSpec) -> impl Future<Output = Result<Output, RunError>> + Send;
}

/// The real runner: starts an operating-system process.
///
/// The process gets no standard input, so a command that unexpectedly asks a
/// question fails instead of hanging, and it is killed if the future driving
/// it is dropped.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner {
    time_limit: Option<Duration>,
}

impl ProcessRunner {
    /// A runner without a time limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// A runner that stops any command still running after `limit`.
    pub fn with_time_limit(limit: Duration) -> Self {
        Self {
            time_limit: Some(limit),
        }
    }
}

impl Runner for ProcessRunner {
    async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
        let mut command = crate::spawn::child(&spec.program);
        command
            .args(&spec.args)
            .envs(spec.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }

        let finished = match self.time_limit {
            Some(limit) => tokio::time::timeout(limit, command.output())
                .await
                .map_err(|_| RunError::TimedOut {
                    program: spec.program.clone(),
                    limit,
                })?,
            None => command.output().await,
        };
        let output = finished.map_err(|source| RunError::Spawn {
            program: spec.program.clone(),
            source,
        })?;
        tracing::trace!(program = %spec.program, status = ?output.status.code(), "command finished");
        Ok(Output {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A runner for tests: replies with pre-arranged results and records what it
/// was asked to run.
///
/// Replies are handed out in the order they were queued. A call made after
/// the queue is empty fails with a [`RunError::Spawn`], so a test notices a
/// command it did not expect.
#[derive(Debug, Default)]
pub struct ScriptedRunner {
    replies: Mutex<VecDeque<Result<Output, RunError>>>,
    calls: Mutex<Vec<CommandSpec>>,
}

impl ScriptedRunner {
    /// A runner with no replies queued.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues the output of the next command.
    pub fn reply(self, output: Output) -> Self {
        self.push(Ok(output));
        self
    }

    /// Queues the output of the next command, for a test that drives the
    /// runner after it was shared (the engine holds it).
    pub fn queue(&self, output: Output) {
        self.push(Ok(output));
    }

    /// Queues a failure to run the next command.
    pub fn fail(self, error: RunError) -> Self {
        self.push(Err(error));
        self
    }

    /// Every command run so far, in order.
    pub fn calls(&self) -> Vec<CommandSpec> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn push(&self, reply: Result<Output, RunError>) {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(reply);
    }
}

impl Runner for ScriptedRunner {
    async fn run(&self, spec: &CommandSpec) -> Result<Output, RunError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(spec.clone());
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| {
                Err(RunError::Spawn {
                    program: spec.program.clone(),
                    source: std::io::Error::other("no scripted reply left for this command"),
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_scripted_runner_replies_in_order_and_records_its_calls() {
        let runner = ScriptedRunner::new()
            .reply(Output::ok("first"))
            .reply(Output::failed(2, "second failed"));

        let first = runner.run(&CommandSpec::new("one")).await.unwrap();
        let second = runner.run(&CommandSpec::new("two").arg("x")).await.unwrap();

        assert!(first.success());
        assert_eq!(first.stdout, "first");
        assert!(!second.success());
        assert_eq!(second.status, Some(2));
        assert_eq!(second.stderr, "second failed");
        let programs: Vec<_> = runner.calls().into_iter().map(|c| c.program).collect();
        assert_eq!(programs, ["one", "two"]);
    }

    #[tokio::test]
    async fn a_scripted_runner_fails_a_command_it_was_not_told_about() {
        let runner = ScriptedRunner::new();
        let outcome = runner.run(&CommandSpec::new("surprise")).await;
        assert!(matches!(outcome, Err(RunError::Spawn { program, .. }) if program == "surprise"));
        assert_eq!(runner.calls().len(), 1);
    }

    #[tokio::test]
    async fn a_scripted_runner_can_report_a_failure_to_run() {
        let runner = ScriptedRunner::new().fail(RunError::TimedOut {
            program: "ssh".into(),
            limit: Duration::from_secs(1),
        });
        assert!(matches!(
            runner.run(&CommandSpec::new("ssh")).await,
            Err(RunError::TimedOut { .. })
        ));
    }

    #[tokio::test]
    async fn starting_a_program_that_does_not_exist_is_a_spawn_error() {
        let spec = CommandSpec::new("leon-test-program-that-does-not-exist");
        let outcome = ProcessRunner::new().run(&spec).await;
        assert!(matches!(outcome, Err(RunError::Spawn { .. })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_process_runner_captures_output_status_environment_and_directory() {
        let directory = tempfile::tempdir().unwrap();
        let spec = CommandSpec::new("sh")
            .args(["-c", "printf '%s' \"$GREETING\"; pwd >&2; exit 3"])
            .env("GREETING", "hello")
            .cwd(directory.path().to_string_lossy());
        let output = ProcessRunner::new().run(&spec).await.unwrap();

        assert_eq!(output.stdout, "hello");
        assert_eq!(output.status, Some(3));
        assert!(!output.success());
        let name = directory.path().file_name().unwrap().to_string_lossy();
        assert!(output.stderr.trim_end().ends_with(name.as_ref()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_reads_standard_input_sees_end_of_file() {
        let spec = CommandSpec::new("sh").args(["-c", "read line; echo \"got:$line\""]);
        let output = ProcessRunner::new().run(&spec).await.unwrap();
        assert_eq!(output.stdout.trim_end(), "got:");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_exceeding_the_time_limit_is_stopped() {
        let runner = ProcessRunner::with_time_limit(Duration::from_millis(100));
        let outcome = runner.run(&CommandSpec::new("sleep").arg("30")).await;
        assert!(matches!(outcome, Err(RunError::TimedOut { .. })));
    }
}
