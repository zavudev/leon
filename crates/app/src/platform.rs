//! Which computer this is, as the decisions of the application see it.
//!
//! Everything that depends on the operating system and can be decided as
//! plain logic asks here instead of writing `cfg!(target_os = ...)`: the
//! chord table and the way keys are routed (macOS has Cmd, the others Ctrl
//! and Ctrl+Shift), and whether this computer has a POSIX shell of its own
//! (Windows has none). In a test build the answer can be overridden with
//! `LEON_SIMULATE_OS=linux|windows|macos`, which is how `scripts/check.sh`
//! runs the suites a second time "as" the platforms the developer's computer
//! is not. A release build never reads it.
//!
//! The simulation covers decisions; it does not turn the computer into
//! another one: real files, processes, clipboards and windows stay the real
//! ones (see the skip list in `scripts/check.sh`).

/// The operating system a test run pretends to be.
#[cfg(test)]
fn simulated() -> Option<String> {
    std::env::var("LEON_SIMULATE_OS")
        .ok()
        .map(|os| os.trim().to_ascii_lowercase())
        .filter(|os| !os.is_empty())
}

/// Whether this computer is a Mac: Cmd is the secondary key, and a bare Ctrl
/// chord belongs to a program in a terminal.
pub fn is_mac() -> bool {
    #[cfg(test)]
    if let Some(os) = simulated() {
        return matches!(os.as_str(), "macos" | "mac" | "darwin");
    }
    cfg!(target_os = "macos")
}

/// Whether this computer is Windows.
pub fn is_windows() -> bool {
    #[cfg(test)]
    if let Some(os) = simulated() {
        return os == "windows";
    }
    cfg!(windows)
}

/// Whether this computer has a POSIX shell of its own, which the usage
/// collection needs for its local machine. Remote machines are POSIX
/// wherever Leon runs, and do not ask.
pub fn local_has_posix_shell() -> bool {
    !is_windows()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_computer_is_one_platform_at_a_time() {
        assert!(!(is_mac() && is_windows()));
        assert_eq!(local_has_posix_shell(), !is_windows());
    }
}
