//! Running a command for a client.
//!
//! The semantics match local commands in `leon-remote`: the program starts
//! with the host's environment plus the request's, in the requested directory,
//! with the request's standard input, closed after it (none at all when the
//! request carries none, so a command that unexpectedly asks a question fails
//! instead of hanging), and is killed if it outlives its time limit or the
//! request is dropped. Unlike a local run, each output stream is capped
//! ([`MAX_EXEC_OUTPUT`]): the excess is read and discarded and the result is
//! marked truncated.

use std::process::Stdio;
use std::time::Duration;

use leon_wire::{ErrorCode, ExecOutput, ExecSpec, WireError, MAX_EXEC_OUTPUT};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// The longest a command may run when the client asks for no limit.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a client may ask for.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(900);

/// The command a client's command is started from.
///
/// Windows gives every console program that a console-less process starts a
/// console of its own, so without this each command a client asks for flashes a
/// window. `CREATE_NO_WINDOW` asks it for none; the flag changes the window and
/// nothing else, and these commands talk through pipes anyway.
///
/// This is the same guard as [`leon_remote::spawn`], repeated rather than
/// imported: this crate deliberately re-implements the local command semantics
/// of `leon-remote` instead of depending on it, and it is three lines.
fn child(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[cfg(windows)]
    let mut command = tokio::process::Command::new(program);
    #[cfg(not(windows))]
    let command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
}

async fn capped<R: AsyncRead + Unpin>(mut reader: R) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_EXEC_OUTPUT.saturating_sub(kept.len());
                if n > room {
                    truncated = true;
                }
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
    (kept, truncated)
}

/// Runs `spec` to completion.
pub async fn run(spec: &ExecSpec, timeout_ms: Option<u32>) -> Result<ExecOutput, WireError> {
    let limit = timeout_ms
        .map(|ms| Duration::from_millis(u64::from(ms)))
        .unwrap_or(DEFAULT_TIMEOUT)
        .min(MAX_TIMEOUT);
    let mut command = child(&spec.program);
    command
        .args(&spec.args)
        .envs(spec.env.iter().map(|(name, value)| (name, value)))
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let mut child = command.spawn().map_err(|error| WireError {
        code: ErrorCode::SpawnFailed,
        message: format!("cannot start {}: {}", spec.program, error.kind()),
    })?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let mut input = child.stdin.take();
    let work = async {
        // Written while the output is read, so a command that answers before
        // it has read everything cannot block on a full pipe. A command that
        // stops reading closes the pipe, which is its choice; dropping the
        // pipe afterwards is what ends its input.
        let write = async {
            if let (Some(mut pipe), Some(bytes)) = (input.take(), &spec.stdin) {
                let _ = pipe.write_all(bytes).await;
            }
        };
        let (_, (out, out_cut), (err, err_cut), status) =
            tokio::join!(write, capped(stdout), capped(stderr), child.wait());
        (out, err, out_cut || err_cut, status)
    };
    match tokio::time::timeout(limit, work).await {
        Err(_) => Err(WireError {
            code: ErrorCode::Timeout,
            message: format!("{} did not finish within {:?}", spec.program, limit),
        }),
        Ok((stdout, stderr, truncated, status)) => {
            let status = status.map_err(|error| WireError {
                code: ErrorCode::Internal,
                message: format!("cannot wait for {}: {}", spec.program, error.kind()),
            })?;
            Ok(ExecOutput {
                status: status.code(),
                stdout,
                stderr,
                truncated,
            })
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> ExecSpec {
        ExecSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            ..ExecSpec::default()
        }
    }

    #[tokio::test]
    async fn output_status_and_stderr_are_captured() {
        let out = run(&sh("echo out; echo err >&2; exit 4"), None)
            .await
            .unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
        assert_eq!(out.status, Some(4));
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn standard_input_is_piped_in_and_closed() {
        let mut spec = sh("cat");
        spec.stdin = Some(b"typed in\n".to_vec());
        let out = run(&spec, None).await.unwrap();
        assert_eq!(out.stdout, b"typed in\n");
        assert_eq!(out.status, Some(0));
    }

    #[tokio::test]
    async fn a_command_without_input_sees_end_of_file() {
        let out = run(&sh("read line; echo \"got:$line\""), None)
            .await
            .unwrap();
        assert_eq!(out.stdout, b"got:\n");
    }

    #[tokio::test]
    async fn input_larger_than_the_pipe_with_output_larger_than_the_pipe_does_not_deadlock() {
        let mut spec = sh("cat");
        spec.stdin = Some(vec![b'y'; MAX_EXEC_OUTPUT / 2]);
        let out = run(&spec, Some(30_000)).await.unwrap();
        assert_eq!(out.stdout.len(), MAX_EXEC_OUTPUT / 2);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn a_command_that_never_reads_its_input_is_not_a_failure() {
        let mut spec = sh("echo done");
        spec.stdin = Some(vec![0; 1024 * 1024]);
        let out = run(&spec, None).await.unwrap();
        assert_eq!(out.stdout, b"done\n");
    }

    #[tokio::test]
    async fn the_environment_and_directory_of_the_request_apply() {
        let dir = std::env::temp_dir();
        let mut spec = sh("echo $LEON_TEST_VAR; pwd -P");
        spec.env.push(("LEON_TEST_VAR".into(), "set".into()));
        spec.cwd = Some(dir.to_string_lossy().into_owned());
        let out = run(&spec, None).await.unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(text.starts_with("set\n"), "{text}");
        let real = std::fs::canonicalize(dir).unwrap();
        assert!(text.contains(real.to_str().unwrap()), "{text}");
    }

    #[tokio::test]
    async fn standard_input_is_closed_so_a_question_fails_instead_of_hanging() {
        let out = run(&sh("read x || exit 7"), Some(20_000)).await.unwrap();
        assert_eq!(out.status, Some(7));
    }

    #[tokio::test]
    async fn a_command_over_its_time_limit_is_stopped() {
        let error = run(&sh("sleep 30"), Some(200)).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Timeout);
    }

    #[tokio::test]
    async fn a_missing_program_is_a_typed_error() {
        let spec = ExecSpec {
            program: "/no/such/program".into(),
            ..ExecSpec::default()
        };
        assert_eq!(
            run(&spec, None).await.unwrap_err().code,
            ErrorCode::SpawnFailed
        );
    }

    #[tokio::test]
    async fn output_beyond_the_cap_is_dropped_and_flagged() {
        let script = format!("head -c {} /dev/zero", MAX_EXEC_OUTPUT + 100_000);
        let out = run(&sh(&script), Some(60_000)).await.unwrap();
        assert_eq!(out.stdout.len(), MAX_EXEC_OUTPUT);
        assert!(out.truncated);
    }
}
