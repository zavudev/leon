//! How much room a folder takes on its machine, for the cleaner.
//!
//! A worktree's weight is its files plus what its builds left behind: the
//! folders a package manager or a compiler fills ([`REBUILDABLE`]) are counted
//! apart, so the cleaner can say how much of a worktree is only build output.
//! A linked worktree shares the repository's objects with the main one, so
//! what it holds of its own is exactly what its folder holds.
//!
//! On this computer the folder is walked ([`measure_local`]); on another
//! machine `du` is asked ([`du_command`], read by [`parse_du`]), which looks
//! at the build folders at the top of the worktree only. Links are never
//! followed, and a file that cannot be read counts as nothing.

use leon_remote::CommandSpec;
use std::path::Path;

/// Folder names that hold build output or downloaded packages: removing them
/// loses nothing a build or an install cannot bring back.
pub const REBUILDABLE: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".gradle",
    ".pytest_cache",
    ".mypy_cache",
    "dist",
    "build",
];

/// What a folder takes on disk, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiskUse {
    /// Everything in it.
    pub total: u64,
    /// The part inside [`REBUILDABLE`] folders.
    pub rebuildable: u64,
}

/// Walks `root` on this computer and adds up its files.
pub fn measure_local(root: &Path) -> std::io::Result<DiskUse> {
    // The root itself must be there: a worktree whose folder is gone is not
    // an empty one.
    std::fs::symlink_metadata(root)?;
    let mut usage = DiskUse::default();
    let mut pending = vec![(root.to_path_buf(), false)];
    while let Some((folder, inside_build)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                let build = inside_build
                    || entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| REBUILDABLE.contains(&name));
                pending.push((entry.path(), build));
            } else if meta.is_file() {
                usage.total += meta.len();
                if inside_build {
                    usage.rebuildable += meta.len();
                }
            }
        }
    }
    Ok(usage)
}

/// The `du` that measures `path` and the build folders at its top, in KiB.
pub fn du_command(path: &str) -> CommandSpec {
    let path = path.trim_end_matches('/');
    let mut spec = CommandSpec::new("du").arg("-sk").arg("--").arg(path);
    for name in REBUILDABLE {
        spec = spec.arg(format!("{path}/{name}"));
    }
    spec
}

/// Reads what [`du_command`] printed for `path`. The build folders that do
/// not exist make `du` complain and fail; the lines it did print still count.
/// `None` when the worktree itself was not measured.
pub fn parse_du(stdout: &str, path: &str) -> Option<DiskUse> {
    let path = path.trim_end_matches('/');
    let mut total = None;
    let mut rebuildable = 0;
    for line in stdout.lines() {
        let Some((size, measured)) = line.split_once('\t') else {
            continue;
        };
        let Ok(kib) = size.trim().parse::<u64>() else {
            continue;
        };
        let bytes = kib * 1024;
        let measured = measured.trim_end_matches('/');
        if measured == path {
            total = Some(bytes);
        } else if measured
            .strip_prefix(path)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|name| REBUILDABLE.contains(&name))
        {
            rebuildable += bytes;
        }
    }
    total.map(|total| DiskUse {
        total,
        rebuildable: rebuildable.min(total),
    })
}

/// A size as a person reads it: "812 KB", "4.2 GB".
pub fn size_words(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < KB * KB {
        format!("{:.0} KB", value / KB)
    } else if value < KB * KB * KB {
        format!("{:.0} MB", value / (KB * KB))
    } else {
        format!("{:.1} GB", value / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn du_reads_the_worktree_and_its_build_folders() {
        let out = "2048\t/srv/w/node_modules\n100\t/srv/w/target\n4096\t/srv/w\n";
        assert_eq!(
            parse_du(out, "/srv/w/"),
            Some(DiskUse {
                total: 4096 * 1024,
                rebuildable: 2148 * 1024,
            })
        );
    }

    #[test]
    fn du_without_the_worktree_line_measured_nothing() {
        assert_eq!(parse_du("12\t/srv/w/target\n", "/srv/w"), None);
        assert_eq!(parse_du("", "/srv/w"), None);
    }

    #[test]
    fn a_folder_named_like_the_worktree_is_not_one_of_its_build_folders() {
        let out = "10\t/srv/w\n5\t/srv/w-old/target\n7\t/srv/w/src\n";
        assert_eq!(
            parse_du(out, "/srv/w"),
            Some(DiskUse {
                total: 10 * 1024,
                rebuildable: 0,
            })
        );
    }

    #[test]
    fn du_is_asked_about_the_worktree_and_each_build_folder() {
        let spec = du_command("/srv/w/");
        assert_eq!(spec.program, "du");
        assert_eq!(spec.args[..3], ["-sk", "--", "/srv/w"]);
        assert!(spec.args.contains(&"/srv/w/node_modules".to_owned()));
    }

    #[test]
    fn a_walk_counts_build_output_apart() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), [0u8; 10]).unwrap();
        let deep = root.path().join("web/node_modules/pkg");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("index.js"), [0u8; 30]).unwrap();
        assert_eq!(
            measure_local(root.path()).unwrap(),
            DiskUse {
                total: 40,
                rebuildable: 30,
            }
        );
    }

    #[test]
    fn a_folder_that_is_gone_is_not_measured_as_empty() {
        let root = tempfile::tempdir().unwrap();
        assert!(measure_local(&root.path().join("gone")).is_err());
    }

    #[test]
    fn sizes_read_in_the_largest_unit_that_fits() {
        assert_eq!(size_words(512), "512 B");
        assert_eq!(size_words(812 * 1024), "812 KB");
        assert_eq!(size_words(300 * 1024 * 1024), "300 MB");
        assert_eq!(size_words(4_509_715_660), "4.2 GB");
    }
}
