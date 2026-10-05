//! Noticing that a theme file changed.
//!
//! **Why polling.** The themes folder holds a handful of small files, and a
//! change must be noticed within a second or two, not at once. Reading the
//! folder's listing (names, sizes, modification times) once a second costs
//! microseconds and needs nothing but the standard library; a file-watching
//! crate (`notify` and the platform backends under it) would add dependencies,
//! threads and a different behaviour per platform (editors that save by
//! renaming, network folders) to do the same job. So the application polls,
//! and the decision of *whether* something changed is this pure value, tested
//! without a clock.
//!
//! **Debounce.** An editor may write a file in several steps. A change counts
//! only once two consecutive listings agree on it: the first sees the file
//! changing, the second sees it settled, and only then is it read.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

/// What the folder holds: each theme file's name with its size and the time it
/// was modified, in nanoseconds since the epoch.
pub type Snapshot = BTreeMap<String, (u64, u128)>;

/// How often the application lists the folder.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Lists the theme files of a folder.
pub fn snapshot(folder: &Path) -> Snapshot {
    let mut files = Snapshot::new();
    let Ok(listing) = std::fs::read_dir(folder) else {
        return files;
    };
    for entry in listing.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        if let Ok(name) = entry.file_name().into_string() {
            files.insert(name, (meta.len(), modified));
        }
    }
    files
}

/// Decides, listing by listing, when the folder has changed for good.
#[derive(Debug, Default)]
pub struct Watcher {
    settled: Snapshot,
    pending: Option<Snapshot>,
}

impl Watcher {
    /// A watcher that takes `now` as how the folder is.
    pub fn new(now: Snapshot) -> Self {
        Self {
            settled: now,
            pending: None,
        }
    }

    /// Takes a new listing. `true` when the folder differs from what was last
    /// settled and two listings in a row agree on how.
    pub fn step(&mut self, now: Snapshot) -> bool {
        if now == self.settled {
            self.pending = None;
            return false;
        }
        if self.pending.as_ref() == Some(&now) {
            self.settled = now;
            self.pending = None;
            return true;
        }
        self.pending = Some(now);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(files: &[(&str, u64, u128)]) -> Snapshot {
        files
            .iter()
            .map(|(name, len, at)| ((*name).to_owned(), (*len, *at)))
            .collect()
    }

    #[test]
    fn an_unchanged_folder_is_never_a_change() {
        let mut watcher = Watcher::new(folder(&[("a.toml", 10, 1)]));
        for _ in 0..5 {
            assert!(!watcher.step(folder(&[("a.toml", 10, 1)])));
        }
    }

    #[test]
    fn a_change_counts_once_two_listings_in_a_row_agree_on_it() {
        let mut watcher = Watcher::new(folder(&[("a.toml", 10, 1)]));
        assert!(!watcher.step(folder(&[("a.toml", 12, 2)])), "first seen");
        assert!(watcher.step(folder(&[("a.toml", 12, 2)])), "settled");
        assert!(!watcher.step(folder(&[("a.toml", 12, 2)])), "and only once");
    }

    #[test]
    fn a_file_still_being_written_waits_until_it_stops_changing() {
        let mut watcher = Watcher::new(Snapshot::new());
        assert!(!watcher.step(folder(&[("a.toml", 5, 1)])));
        assert!(!watcher.step(folder(&[("a.toml", 90, 2)])), "still growing");
        assert!(
            !watcher.step(folder(&[("a.toml", 300, 3)])),
            "still growing"
        );
        assert!(watcher.step(folder(&[("a.toml", 300, 3)])), "done");
    }

    #[test]
    fn a_change_that_is_undone_before_it_settles_is_no_change() {
        let mut watcher = Watcher::new(folder(&[("a.toml", 10, 1)]));
        assert!(!watcher.step(folder(&[("a.toml", 11, 2)])));
        assert!(!watcher.step(folder(&[("a.toml", 10, 1)])));
        assert!(!watcher.step(folder(&[("a.toml", 10, 1)])));
    }

    #[test]
    fn a_file_added_or_removed_is_a_change() {
        let mut watcher = Watcher::new(folder(&[("a.toml", 1, 1)]));
        let two = folder(&[("a.toml", 1, 1), ("b.toml", 1, 1)]);
        assert!(!watcher.step(two.clone()));
        assert!(watcher.step(two));
        let one = folder(&[("a.toml", 1, 1)]);
        assert!(!watcher.step(one.clone()));
        assert!(watcher.step(one));
    }

    #[test]
    fn the_listing_of_a_real_folder_names_only_toml_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), "x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let listed = snapshot(dir.path());
        assert_eq!(listed.keys().collect::<Vec<_>>(), ["a.toml"]);
        assert!(snapshot(&dir.path().join("missing")).is_empty());
    }
}
