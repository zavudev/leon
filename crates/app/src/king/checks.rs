//! A project's check in a worker's worktree, and a look at a pull
//! request's CI: the processes behind [`leon_king::verify`].
//!
//! Both run on a thread of their own and end in an input of the service's,
//! so the service never waits on them. They run on the worker's machine
//! ([`Place`]). A check writes what it prints to `king/checks/<worker>.log`
//! here (the whole of it stays there; the state keeps its end), and is
//! ended, with all it started, when it runs past `[limits]
//! check_timeout_secs`. On another machine what ends is its connection: a
//! check over SSH may go on there after it, unseen.

use std::path::Path;
use std::time::{Duration, Instant};

use leon_king::verify;

use super::place::{process, Place};

/// The command that checks the project in `cwd`: the one `king.toml` names
/// for it, else the one its files say.
pub fn command_for(
    configured: &std::collections::BTreeMap<String, String>,
    project: Option<&str>,
    place: &Place,
    cwd: &str,
) -> Result<String, String> {
    if let Some(command) = project.and_then(|name| configured.get(name)) {
        return Ok(command.clone());
    }
    verify::detect_check(|name| place.exists(cwd, name), |name| place.read(cwd, name)).ok_or_else(
        || {
            format!(
                "Leon does not know how {} is checked: name its command in king.toml, under \
             [checks] ({} = \"…\")",
                project.unwrap_or("this project"),
                project.unwrap_or("<project>")
            )
        },
    )
}

/// The commit `cwd` is on, and whether it has changes not committed.
pub fn head_of(place: &Place, cwd: &str) -> Result<(String, bool), String> {
    let commit = git(place, cwd, &["rev-parse", "HEAD"])?;
    let status = git(place, cwd, &["status", "--porcelain"])?;
    Ok((commit.trim().to_owned(), !status.trim().is_empty()))
}

/// Commits everything not committed in `cwd`, with `message`: what a run
/// step's worker left. Whether there was anything to commit.
pub fn commit_all(place: &Place, cwd: &str, message: &str) -> Result<bool, String> {
    let (_, dirty) = head_of(place, cwd)?;
    if !dirty {
        return Ok(false);
    }
    git(place, cwd, &["add", "--all"])?;
    git(place, cwd, &["commit", "--quiet", "--message", message])?;
    Ok(true)
}

fn git(place: &Place, cwd: &str, args: &[&str]) -> Result<String, String> {
    place
        .run("git", args, cwd)
        .map_err(|error| format!("git: {error}"))
}

/// Runs `command` with `sh -c` in `cwd` on `place`, what it prints going to
/// `log`, for at most `timeout`. Whether it passed, and what it printed.
pub fn run(
    place: &Place,
    command: &str,
    cwd: &str,
    log: &Path,
    timeout: Duration,
) -> (bool, String) {
    if let Some(folder) = log.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let placed = place.placed("sh", &["-c", command], cwd);
    if placed.route.is_some() {
        // Through a relay: the host runs it and answers once it ended.
        let (passed, printed) = match place.output(&placed, timeout) {
            Ok(output) => (
                output.success(),
                format!("{}{}", output.stdout, output.stderr),
            ),
            Err(error) => (false, error),
        };
        let _ = std::fs::write(log, &printed);
        return (passed, printed);
    }
    let file = match std::fs::File::create(log) {
        Ok(file) => file,
        Err(error) => return (false, format!("its log could not be written: {error}")),
    };
    let Ok(errors) = file.try_clone() else {
        return (false, "its log could not be written".to_owned());
    };
    let mut child = process(&placed);
    child.stdout(file).stderr(errors);
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
pub fn look_at(place: &Place, url: &str, cwd: &str) -> Result<String, String> {
    place
        .run(
            "gh",
            &["pr", "view", url, "--json", "state,statusCheckRollup"],
            cwd,
        )
        .map_err(|error| format!("gh: {error}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_check_passes_fails_or_is_ended_when_it_runs_too_long() {
        let folder = tempfile::tempdir().unwrap();
        let cwd = folder.path().to_str().unwrap();
        let log = folder.path().join("checks/1.log");
        let here = Place::here();
        let (passed, printed) = run(
            &here,
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
        let (passed, _) = run(&here, "exit 3", cwd, &log, Duration::from_secs(10));
        assert!(!passed);
        let (passed, printed) = run(&here, "sleep 30", cwd, &log, Duration::from_millis(300));
        assert!(!passed);
        assert!(printed.contains("was ended"), "{printed}");
    }

    #[test]
    fn what_a_worker_left_is_committed_once() {
        let folder = tempfile::tempdir().unwrap();
        let cwd = folder.path().to_str().unwrap();
        let here = Place::here();
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "t"],
            &["config", "user.email", "t@t"],
            &["commit", "-q", "--allow-empty", "-m", "base"],
        ] {
            git(&here, cwd, args).unwrap();
        }
        assert!(!commit_all(&here, cwd, "Nothing").unwrap());
        std::fs::write(folder.path().join("new.txt"), "x").unwrap();
        assert!(commit_all(&here, cwd, "Billing schema").unwrap());
        assert!(!head_of(&here, cwd).unwrap().1);
        assert_eq!(
            git(&here, cwd, &["log", "-1", "--format=%s"])
                .unwrap()
                .trim(),
            "Billing schema"
        );
    }

    #[test]
    fn the_command_named_for_a_project_comes_before_its_files() {
        let folder = tempfile::tempdir().unwrap();
        let cwd = folder.path().to_str().unwrap();
        let mut configured = std::collections::BTreeMap::new();
        assert!(command_for(&configured, Some("leon"), &Place::here(), cwd)
            .unwrap_err()
            .contains("[checks]"));
        std::fs::write(folder.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(
            command_for(&configured, Some("leon"), &Place::here(), cwd).unwrap(),
            "cargo test"
        );
        configured.insert("leon".to_owned(), "scripts/check.sh".to_owned());
        assert_eq!(
            command_for(&configured, Some("leon"), &Place::here(), cwd).unwrap(),
            "scripts/check.sh"
        );
    }
}
