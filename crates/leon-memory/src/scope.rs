//! Whose memory a folder shares, and the name of a memory file.
//!
//! A folder belongs to the project the store says owns it (the application
//! asks `Store::memory_root`). The store does not know every folder: an agent
//! may be started where Leon was never shown a project. [`git_root`] then
//! finds the repository the folder is in from git's own files, without
//! running git: the nearest folder above with a `.git`. When that `.git` is a
//! file, the folder is a linked worktree (or a submodule), and the file leads
//! to the repository's git folder, whose `commondir` leads to the main
//! checkout: its root is the answer, so a linked worktree shares the memory of
//! the checkout it belongs to. Everything read is asked of a [`Folders`], so
//! the walk is tested without a disk.
//!
//! [`file_name`] names the memory file of a root: a readable slug of the
//! folder's name and a short hash of the root's identity, so two projects
//! called `api` get two files and every spelling of one root gets one.

use sha2::{Digest as _, Sha256};
use std::path::{Component, Path, PathBuf};

/// The name of the memory file of the global scope.
pub const GLOBAL_FILE: &str = "global.md";

/// The folder below the data folder where the memory files are.
pub const FOLDER: &str = "memory";

/// What the walk asks of the disk.
pub trait Folders {
    /// Whether a folder exists at this path.
    fn is_dir(&self, path: &Path) -> bool;
    /// The text of a small file at this path, when there is one.
    fn read(&self, path: &Path) -> Option<String>;
}

/// A path without its `.` and `..` components, decided from the text alone.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// The path a line of git's own files names, relative to `base` when it is
/// not absolute.
fn named(base: &Path, text: &str) -> PathBuf {
    let path = Path::new(text.trim());
    if path.is_absolute() {
        tidy(path)
    } else {
        tidy(&base.join(path))
    }
}

/// The main checkout a `.git` file found in `folder` belongs to, when it is
/// a linked worktree's.
fn main_checkout(folders: &dyn Folders, folder: &Path, dot_git: &str) -> Option<PathBuf> {
    let git_dir = named(folder, dot_git.lines().next()?.strip_prefix("gitdir:")?);
    // A submodule's git folder has no `commondir`: it is its own repository.
    let common = named(&git_dir, &folders.read(&git_dir.join("commondir"))?);
    // A bare repository has no checkout to name.
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_path_buf))?
}

/// The root of the git repository `folder` is in: the main checkout's, also
/// from inside a linked worktree. `None` outside a repository.
pub fn git_root(folders: &dyn Folders, folder: &Path) -> Option<PathBuf> {
    for candidate in tidy(folder).ancestors() {
        let dot_git = candidate.join(".git");
        if folders.is_dir(&dot_git) {
            return Some(candidate.to_path_buf());
        }
        if let Some(text) = folders.read(&dot_git) {
            return Some(
                main_checkout(folders, candidate, &text).unwrap_or_else(|| candidate.to_path_buf()),
            );
        }
    }
    None
}

/// The top of the checkout `folder` is in: the nearest folder above with a
/// `.git`, be it a repository's, a linked worktree's or a submodule's. Where
/// a project's own files are, as opposed to whose memory it shares.
pub fn checkout_root(folders: &dyn Folders, folder: &Path) -> Option<PathBuf> {
    tidy(folder)
        .ancestors()
        .find(|candidate| {
            let dot_git = candidate.join(".git");
            folders.is_dir(&dot_git) || folders.read(&dot_git).is_some()
        })
        .map(Path::to_path_buf)
}

/// The readable part of a memory file's name: the folder's own name in lower
/// case, anything but letters and digits as `-`, at most 40 characters.
fn slug(root: &str) -> String {
    let name = root
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "project".to_owned()
    } else {
        out.to_owned()
    }
}

/// The name of the memory file of the project rooted at `root`.
pub fn file_name(root: &str) -> String {
    let digest = Sha256::digest(leon_core::path::key(root).as_bytes());
    let hash: String = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}-{hash}.md", slug(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk of a few folders and files.
    #[derive(Default)]
    struct Disk {
        dirs: Vec<&'static str>,
        files: Vec<(&'static str, &'static str)>,
    }

    impl Folders for Disk {
        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.iter().any(|dir| Path::new(dir) == path)
        }
        fn read(&self, path: &Path) -> Option<String> {
            self.files
                .iter()
                .find(|(file, _)| Path::new(file) == path)
                .map(|(_, text)| (*text).to_owned())
        }
    }

    /// Compared as paths, not as text: the separator is the platform's.
    fn root(disk: &Disk, folder: &str) -> Option<PathBuf> {
        git_root(disk, Path::new(folder))
    }

    fn at(path: &str) -> Option<PathBuf> {
        Some(PathBuf::from(path))
    }

    #[test]
    fn a_folder_of_a_checkout_belongs_to_its_root() {
        let disk = Disk {
            dirs: vec!["/code/api/.git"],
            ..Disk::default()
        };
        assert_eq!(root(&disk, "/code/api"), at("/code/api"));
        assert_eq!(root(&disk, "/code/api/src/deep"), at("/code/api"));
        assert_eq!(root(&disk, "/code/api/src/../src/./x"), at("/code/api"));
        assert_eq!(root(&disk, "/code/web"), None);
        assert_eq!(root(&disk, "/"), None);
    }

    #[test]
    fn a_linked_worktree_belongs_to_the_main_checkout() {
        let disk = Disk {
            dirs: vec!["/code/api/.git"],
            files: vec![
                (
                    "/trees/api-fix/.git",
                    "gitdir: /code/api/.git/worktrees/api-fix\n",
                ),
                ("/code/api/.git/worktrees/api-fix/commondir", "../..\n"),
            ],
        };
        assert_eq!(root(&disk, "/trees/api-fix"), at("/code/api"));
        assert_eq!(root(&disk, "/trees/api-fix/src"), at("/code/api"));
        // Its own files are in the worktree, not in the main checkout.
        let top = |folder: &str| checkout_root(&disk, Path::new(folder));
        assert_eq!(top("/trees/api-fix/src"), at("/trees/api-fix"));
        assert_eq!(top("/code/api/src"), at("/code/api"));
        assert_eq!(top("/elsewhere"), None);
    }

    #[test]
    fn relative_and_absolute_spellings_in_gits_files_are_followed() {
        let disk = Disk {
            dirs: vec![],
            files: vec![
                (
                    "/code/api-fix/.git",
                    "gitdir: ../api/.git/worktrees/api-fix",
                ),
                (
                    "/code/api/.git/worktrees/api-fix/commondir",
                    "/code/api/.git",
                ),
            ],
        };
        assert_eq!(root(&disk, "/code/api-fix"), at("/code/api"));
    }

    #[test]
    fn a_submodule_and_a_worktree_of_a_bare_repository_are_their_own_root() {
        let disk = Disk {
            dirs: vec!["/code/api/.git"],
            files: vec![
                (
                    "/code/api/vendor/lib/.git",
                    "gitdir: ../../.git/modules/lib\n",
                ),
                (
                    "/trees/bare-fix/.git",
                    "gitdir: /repos/api.git/worktrees/bare-fix\n",
                ),
                ("/repos/api.git/worktrees/bare-fix/commondir", "../..\n"),
                ("/odd/.git", "not what git writes"),
            ],
        };
        assert_eq!(
            root(&disk, "/code/api/vendor/lib/src"),
            at("/code/api/vendor/lib")
        );
        assert_eq!(root(&disk, "/trees/bare-fix"), at("/trees/bare-fix"));
        assert_eq!(root(&disk, "/odd/x"), at("/odd"));
    }

    #[test]
    fn a_memory_file_is_named_by_the_folder_and_a_hash_of_the_root() {
        let name = file_name("/home/me/code/My API (v2)");
        assert!(name.starts_with("my-api-v2-"), "{name}");
        assert!(name.ends_with(".md"));
        assert_eq!(name.len(), "my-api-v2-".len() + 12 + 3);
        // The same for every spelling of one root, different for another root
        // with the same name.
        assert_eq!(
            file_name("/home/me/code/api/"),
            file_name("/home/me/code/api")
        );
        assert_eq!(file_name("C:\\Code\\Api"), file_name("c:/code/api/"));
        assert_ne!(file_name("/home/me/code/api"), file_name("/work/api"));
        assert_ne!(file_name("/a/global"), GLOBAL_FILE);
    }

    #[test]
    fn a_name_is_always_a_plain_file_name() {
        for root in [
            "/",
            "",
            "/..",
            "/a/..",
            "/日本語",
            "/a/-- --",
            "/x/.hidden",
            &"/n".repeat(300),
        ] {
            let name = file_name(root);
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.'),
                "{root:?} -> {name}"
            );
            assert!(!name.starts_with('.') && !name.starts_with('-'), "{name}");
            assert!(name.len() <= 40 + 1 + 12 + 3, "{name}");
        }
        assert!(file_name("/").starts_with("project-"));
    }
}
