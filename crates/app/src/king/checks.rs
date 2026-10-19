//! A project's check in a worker's worktree, and a look at a pull
//! request's CI: the processes behind [`leon_king::verify`].
//!
//! Both run on a thread of their own and end in an input of the service's,
//! so the service never waits on them. A check writes what it prints to
//! `king/checks/<worker>.log` (the whole of it stays there; the state keeps
//! its end), and is ended, with all it started, when it runs past
//! `[limits] check_timeout_secs`.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use leon_king::verify;

/// The command that checks the project in `cwd`: the one `king.toml` names
/// for it, else the one its files say.
pub fn command_for(
    configured: &std::collections::BTreeMap<String, String>,
    project: Option<&str>,
    cwd: &str,
) -> Result<String, String> {
    if let Some(command) = project.and_then(|name| configured.get(name)) {
        return Ok(command.clone());
    }
    let root = Path::new(cwd);
    verify::detect_check(
        |name| root.join(name).exists(),
        |name| std::fs::read_to_string(root.join(name)).ok(),
    )
    .ok_or_else(|| {
        format!(
            "Leon does not know how {} is checked: name its command in king.toml, under \
             [checks] ({} = \"…\")",
            project.unwrap_or("this project"),
            project.unwrap_or("<project>")
        )
    })
}

/// The commit `cwd` is on, and whether it has changes not committed.
pub fn head_of(cwd: &str) -> Result<(String, bool), String> {
    let commit = git(cwd, &["rev-parse", "HEAD"])?;
    let status = git(cwd, &["status", "--porcelain"])?;
    Ok((commit.trim().to_owned(), !status.trim().is_empty()))
}

fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let output = leon_remote::spawn::std_child("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not run: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs `command` with `sh -c` in `cwd`, what it prints going to `log`,
/// for at most `timeout`. Whether it passed, and what it printed.
pub fn run(command: &str, cwd: &str, log: &Path, timeout: Duration) -> (bool, String) {
    if let Some(folder) = log.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let file = match std::fs::File::create(log) {
        Ok(file) => file,
        Err(error) => return (false, format!("its log could not be written: {error}")),
    };
    let Ok(errors) = file.try_clone() else {
        return (false, "its log could not be written".to_owned());
    };
    let mut child = leon_remote::spawn::std_child("sh");
    child
        .args(["-c", command])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(errors);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut child, 0);
    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(error) => return (false, format!("`sh` could not start: {error}")),
    };
    let started = Instant::now();
    let passed = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.success()),
            Ok(None) if started.elapsed() >= timeout => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(_) => break Some(false),
        }
    };
    let printed = std::fs::read_to_string(log).unwrap_or_default();
    match passed {
        Some(passed) => (passed, printed),
        None => {
            end_group(child.id());
            let _ = child.wait();
            (
                false,
                format!(
                    "{printed}\n[Leon] It ran for {} and was ended ([limits] check_timeout_secs).",
                    verify::duration(timeout.as_millis() as u64)
                ),
            )
        }
    }
}

/// Ends a check and everything it started.
fn end_group(pid: u32) {
    #[cfg(unix)]
    // SAFETY: `kill` takes plain integers; the group is the one the check
    // was started in, of a process this service started.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        std::thread::sleep(Duration::from_millis(500));
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// What `gh` says of the pull request at `url`, as [`verify::read_ci`]
/// reads it.
pub fn look_at(url: &str, cwd: &str) -> Result<String, String> {
    let output = leon_remote::spawn::std_child("gh")
        .args(["pr", "view", url, "--json", "state,statusCheckRollup"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("gh could not run: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_check_passes_fails_or_is_ended_when_it_runs_too_long() {
        let folder = tempfile::tempdir().unwrap();
        let cwd = folder.path().to_str().unwrap();
        let log = folder.path().join("checks/1.log");
        let (passed, printed) = run(
            "echo fine; echo warned >&2",
            cwd,
            &log,
            Duration::from_secs(10),
        );
        assert!(passed);
        assert!(
            printed.contains("fine") && printed.contains("warned"),
            "{printed}"
        );
        let (passed, _) = run("exit 3", cwd, &log, Duration::from_secs(10));
        assert!(!passed);
        let (passed, printed) = run("sleep 30", cwd, &log, Duration::from_millis(300));
        assert!(!passed);
        assert!(printed.contains("was ended"), "{printed}");
    }

    #[test]
    fn the_command_named_for_a_project_comes_before_its_files() {
        let folder = tempfile::tempdir().unwrap();
        let cwd = folder.path().to_str().unwrap();
        let mut configured = std::collections::BTreeMap::new();
        assert!(command_for(&configured, Some("leon"), cwd)
            .unwrap_err()
            .contains("[checks]"));
        std::fs::write(folder.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(
            command_for(&configured, Some("leon"), cwd).unwrap(),
            "cargo test"
        );
        configured.insert("leon".to_owned(), "scripts/check.sh".to_owned());
        assert_eq!(
            command_for(&configured, Some("leon"), cwd).unwrap(),
            "scripts/check.sh"
        );
    }
}
