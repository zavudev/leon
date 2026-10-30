//! What a turn changed, as the card at its end and the diff panel say it.
//!
//! The first source is what the turn's tools were given and did
//! ([`FileChange`]); the better one, when the worktree was snapshotted
//! (`super::checkpoint`), is git's own count between two snapshots, which
//! also sees what a shell command did. Both end as [`Stat`]s: a file and how
//! many lines went and came.
//!
//! Pure: data in, data out.

use std::ops::Range;

use leon_remote::{DiffLine, LineKind as GitLine};
use leon_turn::{FileChange, LineKind};

use super::model::{Item, Tool};
use super::rewind::Mark;

/// One file a turn changed, and by how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    /// The file, from the root of the worktree where that is known.
    pub path: String,
    /// Lines that came.
    pub added: usize,
    /// Lines that went.
    pub removed: usize,
}

/// The path as the worktree at `root` names it: an agent writes the whole
/// path of a file, a card wants the part below the worktree.
pub fn relative(path: &str, root: &str) -> String {
    let root = root.trim_end_matches(['/', '\\']);
    if root.is_empty() {
        return path.to_owned();
    }
    match path.strip_prefix(root) {
        Some(rest) if rest.starts_with(['/', '\\']) => rest[1..].to_owned(),
        _ => path.to_owned(),
    }
}

/// Whether two spellings are the same file: git's path from the root of the
/// worktree, and an agent's, which may be whole or from the folder it works
/// in.
pub fn same_file(a: &str, b: &str) -> bool {
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// The files `changes` touched, in the order they first appear, a file
/// changed several times counted once with all its lines.
pub fn stats_of<'a>(changes: impl IntoIterator<Item = &'a FileChange>, root: &str) -> Vec<Stat> {
    let mut stats: Vec<Stat> = Vec::new();
    for change in changes {
        let path = relative(&change.path, root);
        match stats.iter_mut().find(|stat| stat.path == path) {
            Some(stat) => {
                stat.added += change.added();
                stat.removed += change.removed();
            }
            None => stats.push(Stat {
                path,
                added: change.added(),
                removed: change.removed(),
            }),
        }
    }
    stats
}

/// The changes the tools of `items` made.
pub fn changes_in(items: &[Item]) -> impl Iterator<Item = &FileChange> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Tool(Tool { changes, .. }) => Some(changes),
            _ => None,
        })
        .flatten()
}

/// What `items` (the work of one turn) changed, from what the tools say.
pub fn from_items(items: &[Item], root: &str) -> Vec<Stat> {
    stats_of(changes_in(items), root)
}

/// The lines that came and went, in all.
pub fn totals(stats: &[Stat]) -> (usize, usize) {
    stats.iter().fold((0, 0), |(added, removed), stat| {
        (added + stat.added, removed + stat.removed)
    })
}

/// "1 file changed" or "3 files changed".
pub fn headline(stats: &[Stat]) -> String {
    match stats.len() {
        1 => "1 file changed".to_owned(),
        n => format!("{n} files changed"),
    }
}

/// The card's line: "3 files changed (+12 -4)".
pub fn title(stats: &[Stat]) -> String {
    let (added, removed) = totals(stats);
    format!("{} ({})", headline(stats), counts(added, removed))
}

/// "+12 -4": what a file or a whole turn added and removed.
pub fn counts(added: usize, removed: usize) -> String {
    format!("+{added} -{removed}")
}

/// The items of the turn `mark` (the one at `at` in `marks`) began: from the
/// person's words to the next turn's, or to the end.
pub fn turn_range(marks: &[Mark], at: usize, items: usize) -> Range<usize> {
    let from = marks.get(at).map_or(items, |mark| mark.item.min(items));
    let to = marks.get(at + 1).map_or(items, |next| next.item.min(items));
    from..to.max(from)
}

/// What a file's several changes of one turn come to, as one change.
pub fn merged(
    changes: impl IntoIterator<Item = impl std::borrow::Borrow<FileChange>>,
    path: &str,
    root: &str,
) -> Option<FileChange> {
    let mut out: Option<FileChange> = None;
    for change in changes {
        let change = change.borrow();
        if !same_file(&relative(&change.path, root), path) {
            continue;
        }
        match &mut out {
            Some(merged) => merged.hunks.extend(change.hunks.iter().cloned()),
            None => out = Some(change.clone()),
        }
    }
    out
}

/// A change as the lines a git diff has, for the one view that draws them:
/// a header line before each hunk, the numbers where the agent said them.
pub fn diff_lines(change: &FileChange) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for hunk in &change.hunks {
        lines.push(DiffLine {
            kind: GitLine::Hunk,
            text: "@@".to_owned(),
            old: None,
            new: None,
        });
        for line in &hunk.lines {
            lines.push(DiffLine {
                kind: match line.kind {
                    LineKind::Context => GitLine::Context,
                    LineKind::Added => GitLine::Added,
                    LineKind::Removed => GitLine::Removed,
                },
                text: line.text.clone(),
                old: line.old,
                new: line.new,
            });
        }
    }
    lines
}

/// Reads `git diff --numstat -z`: `added TAB removed TAB path NUL` for each
/// file, `-` for both when it is not text.
pub fn parse_numstat(raw: &str) -> Vec<Stat> {
    raw.split('\0')
        .filter_map(|record| {
            let mut parts = record.trim_start_matches('\n').splitn(3, '\t');
            let added = parts.next()?;
            let removed = parts.next()?;
            let path = parts.next().filter(|path| !path.is_empty())?;
            Some(Stat {
                path: path.to_owned(),
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::rewind::Memory;
    use leon_turn::FileChange;

    fn mark(item: usize, turn: u32) -> Mark {
        Mark {
            item,
            turn,
            checkpoint: None,
            memory: Memory::Fresh,
        }
    }

    #[test]
    fn a_file_changed_twice_is_one_file_with_all_its_lines() {
        let changes = [
            FileChange::edited("/w/src/a.rs", &[("one", "two")]),
            FileChange::written("/w/b.rs", "x\ny\nz"),
            FileChange::edited("/w/src/a.rs", &[("p\nq", "r")]),
        ];
        let stats = stats_of(&changes, "/w");
        assert_eq!(
            stats,
            [
                Stat {
                    path: "src/a.rs".into(),
                    added: 2,
                    removed: 3
                },
                Stat {
                    path: "b.rs".into(),
                    added: 3,
                    removed: 0
                },
            ]
        );
        assert_eq!(totals(&stats), (5, 3));
        assert_eq!(headline(&stats), "2 files changed");
        assert_eq!(headline(&stats[..1]), "1 file changed");
        assert_eq!(counts(5, 3), "+5 -3");
        assert_eq!(title(&stats), "2 files changed (+5 -3)");
    }

    #[test]
    fn a_path_is_cut_to_the_worktree_only_when_it_is_inside() {
        assert_eq!(relative("/w/src/a.rs", "/w"), "src/a.rs");
        assert_eq!(relative("/w/src/a.rs", "/w/"), "src/a.rs");
        assert_eq!(relative("/wide/a.rs", "/w"), "/wide/a.rs");
        assert_eq!(relative("src/a.rs", "/w"), "src/a.rs");
        assert!(same_file("src/a.rs", "/w/src/a.rs"));
        assert!(!same_file("src/a.rs", "src/b.rs"));
    }

    #[test]
    fn a_turn_is_what_lies_between_its_words_and_the_next_turns() {
        let marks = [mark(0, 1), mark(4, 2), mark(9, 3)];
        assert_eq!(turn_range(&marks, 0, 12), 0..4);
        assert_eq!(turn_range(&marks, 1, 12), 4..9);
        assert_eq!(turn_range(&marks, 2, 12), 9..12);
        // A conversation cut shorter than its marks says.
        assert_eq!(turn_range(&marks, 2, 6), 6..6);
    }

    #[test]
    fn git_counts_what_was_added_and_removed_and_lists_what_is_not_text() {
        let raw = "3\t1\tsrc/a.rs\0-\t-\timg/logo.png\0";
        assert_eq!(
            parse_numstat(raw),
            [
                Stat {
                    path: "src/a.rs".into(),
                    added: 3,
                    removed: 1
                },
                Stat {
                    path: "img/logo.png".into(),
                    added: 0,
                    removed: 0
                },
            ]
        );
        assert!(parse_numstat("").is_empty());
    }

    #[test]
    fn several_edits_of_a_file_are_one_change_with_a_header_per_hunk() {
        let changes = [
            FileChange::edited("/w/a.rs", &[("one", "two")]),
            FileChange::edited("/w/b.rs", &[("x", "y")]),
            FileChange::edited("/w/a.rs", &[("three", "four")]),
        ];
        let merged = merged(&changes, "a.rs", "/w").expect("a.rs was changed");
        assert_eq!(merged.hunks.len(), 2);
        let lines = diff_lines(&merged);
        let kinds: Vec<GitLine> = lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            [
                GitLine::Hunk,
                GitLine::Removed,
                GitLine::Added,
                GitLine::Hunk,
                GitLine::Removed,
                GitLine::Added
            ]
        );
        assert_eq!(lines[1].text, "one");
        assert!(merged_none(&changes));
    }

    fn merged_none(changes: &[FileChange]) -> bool {
        merged(changes, "c.rs", "/w").is_none()
    }
}
