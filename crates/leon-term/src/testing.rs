//! Helpers shared by the tests of this crate.

use crate::colors::{from_rgb8, TerminalTheme};
#[cfg(unix)]
use crate::spec::SpawnSpec;
use std::time::{Duration, Instant};

/// The colours every test uses.
pub fn theme() -> TerminalTheme {
    TerminalTheme {
        foreground: from_rgb8(250, 250, 250),
        background: from_rgb8(0, 0, 0),
        cursor: from_rgb8(97, 95, 255),
        selection: from_rgb8(30, 30, 90),
        find_match: from_rgb8(60, 60, 20),
        find_match_current: from_rgb8(250, 200, 0),
        ansi: std::array::from_fn(|i| from_rgb8(i as u8 * 16, 0, 0)),
    }
}

/// A POSIX shell that reads no startup file, with a fixed search path: what a
/// test runs when it needs a real child, whatever the developer's own shell,
/// `PATH` and home directory are.
#[cfg(unix)]
pub fn sh(args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        program: "/bin/sh".into(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: vec![
            ("ENV".into(), "/dev/null".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("PS1".into(), "READY> ".into()),
        ],
        cwd: None,
    }
}

/// `sh -c script`.
#[cfg(unix)]
pub fn sh_c(script: &str) -> SpawnSpec {
    sh(&["-c", script])
}

/// How long a test waits for something that must happen: only a failure ever
/// waits this long.
#[cfg_attr(not(unix), allow(dead_code))]
pub const PATIENCE: Duration = Duration::from_secs(20);

/// Polls `condition` every millisecond until it holds. There is no fixed
/// wait: a passing test returns the moment the condition is true, and
/// [`PATIENCE`] only bounds a failing one.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}
