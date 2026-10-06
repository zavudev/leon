//! What is kept between runs, in the `updates` folder of the data folder.
//!
//! ```text
//! updates/
//!   state.json         when it last looked, the ETag, what was skipped or refused,
//!                      and the update that is downloaded and ready
//!   pending.json       an update that is installed and not yet seen to start
//!   <version>/         a download: the archive, SHA256SUMS and the unpacked program
//! ```
//!
//! Everything is read leniently (a file that is missing or damaged is the
//! empty state) and written whole through a temporary file and a rename, so a
//! crash never leaves half a file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A release the person can be told about.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// The version, as `0.2.1`.
    pub version: String,
    /// The release notes as written.
    pub notes: String,
    /// The release's page.
    pub page: String,
    /// The size of the archive for this platform, in bytes.
    pub size: u64,
    /// Whether the release is a pre-release.
    pub prerelease: bool,
}

/// An update that is downloaded, verified and unpacked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    /// What it is.
    pub offer: Offer,
    /// The archive's file name.
    pub archive: String,
    /// The archive's SHA-256 as it was verified, in hex.
    pub sha256: String,
    /// Who signed the new program, if anyone.
    pub identity: Option<String>,
}

/// What is kept about the checks and the downloads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// When the last check that got an answer was made, in Unix seconds.
    pub last_check: Option<i64>,
    /// The address the ETag belongs to.
    pub etag_url: Option<String>,
    /// The ETag GitHub sent with it.
    pub etag: Option<String>,
    /// The body that came with that ETag, to read again on a `304`.
    pub cached: Option<String>,
    /// Do not offer again until it is manually asked for: a version.
    pub skipped: Option<String>,
    /// Versions that were downloaded and refused, or installed and did not
    /// start. They are not tried again until a newer release exists.
    pub refused: Vec<String>,
    /// Do not ask GitHub before this time: it asked to come back later.
    pub retry_after: Option<i64>,
    /// The update that is ready.
    pub staged: Option<Staged>,
    /// A line for the person about something that happened without them
    /// (an update that did not start and was undone).
    pub notice: Option<String>,
}

/// An update that is installed and has not yet been seen to start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    /// The version that was installed.
    pub version: String,
    /// The version it replaced.
    pub previous: String,
    /// How many times a process of the new version has started without
    /// reaching its window.
    pub starts: u32,
}

/// Where the files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// The layout under `root` (`<data folder>/updates`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The folder itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state_file(&self) -> PathBuf {
        self.root.join("state.json")
    }

    fn pending_file(&self) -> PathBuf {
        self.root.join("pending.json")
    }

    /// The folder of a version's download.
    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.root.join(version)
    }

    /// Where the program of a version is unpacked to.
    pub fn payload_dir(&self, version: &str) -> PathBuf {
        self.version_dir(version).join("payload")
    }

    /// Reads what is kept; an empty state when there is none.
    pub fn load(&self) -> Saved {
        read_json(&self.state_file()).unwrap_or_default()
    }

    /// Writes what is kept.
    pub fn save(&self, saved: &Saved) -> std::io::Result<()> {
        write_json(&self.state_file(), saved)
    }

    /// The installed-but-unconfirmed update, if there is one.
    pub fn pending(&self) -> Option<Pending> {
        read_json(&self.pending_file())
    }

    /// Records that an update was installed.
    pub fn set_pending(&self, pending: &Pending) -> std::io::Result<()> {
        write_json(&self.pending_file(), pending)
    }

    /// Forgets the pending update: it was confirmed, or undone.
    pub fn clear_pending(&self) {
        let _ = std::fs::remove_file(self.pending_file());
    }

    /// Removes the downloads of every version but `keep`.
    pub fn drop_downloads_except(&self, keep: Option<&str>) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && Some(name.as_str()) != keep {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path().join("updates"));
        assert_eq!(layout.load(), Saved::default());
        let saved = Saved {
            last_check: Some(5),
            etag: Some("W/\"x\"".into()),
            refused: vec!["0.2.1".into()],
            notice: Some("n".into()),
            ..Saved::default()
        };
        layout.save(&saved).unwrap();
        assert_eq!(layout.load(), saved);
    }

    #[test]
    fn a_damaged_file_is_an_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        std::fs::write(dir.path().join("state.json"), "{ not json").unwrap();
        assert_eq!(layout.load(), Saved::default());
        std::fs::write(dir.path().join("pending.json"), "").unwrap();
        assert_eq!(layout.pending(), None);
    }

    #[test]
    fn the_pending_mark_is_set_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        let pending = Pending {
            version: "0.2.1".into(),
            previous: "0.2.0".into(),
            starts: 0,
        };
        layout.set_pending(&pending).unwrap();
        assert_eq!(layout.pending(), Some(pending));
        layout.clear_pending();
        assert_eq!(layout.pending(), None);
        layout.clear_pending();
    }

    #[test]
    fn old_downloads_are_dropped_but_the_one_kept_stays() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        for version in ["0.2.1", "0.2.2"] {
            std::fs::create_dir_all(layout.version_dir(version)).unwrap();
        }
        layout.save(&Saved::default()).unwrap();
        layout.drop_downloads_except(Some("0.2.2"));
        assert!(!layout.version_dir("0.2.1").exists());
        assert!(layout.version_dir("0.2.2").exists());
        assert!(dir.path().join("state.json").exists());
    }
}
