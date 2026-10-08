//! Unsaved text kept on disk, so that a crash or a power cut loses nothing.
//!
//! While a file has changes that were not saved, its text is written (a pause
//! after the last change) to `drafts/<hash>.json` beside the settings, where
//! the hash is of the machine and the path. Saving, closing with "discard" and
//! reloading remove it; opening the file again finds it and puts the text
//! back as unsaved changes ([`super::actions`]). A draft carries the revision
//! the file had when the text began to differ, so that a file that changed on
//! disk in the meantime is told from one that did not.
//!
//! A draft that cannot be read or written is logged and otherwise ignored:
//! it is a safety net, not the file.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The text of a file that was not saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// The machine the file is on.
    pub machine: String,
    /// Its path there.
    pub path: String,
    /// The text of the editor.
    pub text: String,
    /// The revision the file had when it was read or last saved.
    pub revision: Option<String>,
}

/// The name of a draft's file: the same for the same machine and path.
pub fn file_name(machine: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(machine.as_bytes());
    hasher.update([0]);
    hasher.update(path.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{hex}.json")
}

fn path_of(dir: &Path, machine: &str, path: &str) -> PathBuf {
    dir.join(file_name(machine, path))
}

/// Writes a draft, through a temporary file so a crash leaves the old one.
pub fn write(dir: &Path, draft: &Draft) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let target = path_of(dir, &draft.machine, &draft.path);
    let temp = target.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec(draft)?)?;
    std::fs::rename(&temp, &target)
}

/// The draft of a file, if there is one that can be read.
pub fn read(dir: &Path, machine: &str, path: &str) -> Option<Draft> {
    let bytes = std::fs::read(path_of(dir, machine, path)).ok()?;
    match serde_json::from_slice::<Draft>(&bytes) {
        // A name that collides is not this file's draft.
        Ok(draft) if draft.machine == machine && draft.path == path => Some(draft),
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(%error, "a draft is unreadable and is ignored");
            None
        }
    }
}

/// Removes the draft of a file; there being none is not a failure.
pub fn remove(dir: &Path, machine: &str, path: &str) {
    if let Err(error) = std::fs::remove_file(path_of(dir, machine, path)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, "a draft could not be removed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(path: &str, text: &str) -> Draft {
        Draft {
            machine: "local".into(),
            path: path.into(),
            text: text.into(),
            revision: Some("1 2".into()),
        }
    }

    #[test]
    fn a_draft_is_found_by_machine_and_path_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let a = draft("/p/a.rs", "fn a() {}\n");
        write(dir.path(), &a).unwrap();
        assert_eq!(read(dir.path(), "local", "/p/a.rs"), Some(a.clone()));
        assert_eq!(read(dir.path(), "other", "/p/a.rs"), None);
        assert_eq!(read(dir.path(), "local", "/p/b.rs"), None);
        // Writing again replaces; no temporary file is left.
        let again = draft("/p/a.rs", "fn a() { 1 }\n");
        write(dir.path(), &again).unwrap();
        assert_eq!(read(dir.path(), "local", "/p/a.rs"), Some(again));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        remove(dir.path(), "local", "/p/a.rs");
        assert_eq!(read(dir.path(), "local", "/p/a.rs"), None);
        remove(dir.path(), "local", "/p/a.rs");
    }

    #[test]
    fn the_name_depends_on_both_the_machine_and_the_path() {
        assert_eq!(file_name("m", "/a"), file_name("m", "/a"));
        assert_ne!(file_name("m", "/a"), file_name("n", "/a"));
        assert_ne!(file_name("m", "/a"), file_name("m", "/b"));
        assert_ne!(file_name("m", "a/b"), file_name("ma", "/b"));
    }

    #[test]
    fn an_unreadable_draft_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(file_name("local", "/p/a.rs")), b"{ nope").unwrap();
        assert_eq!(read(dir.path(), "local", "/p/a.rs"), None);
    }
}
