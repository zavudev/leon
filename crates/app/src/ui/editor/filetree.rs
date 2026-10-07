//! The file tree of a folder, as pure data: what is listed, what is open, what
//! is selected, and the flat list of rows that follows from them.
//!
//! The tree is lazy. A folder's entries are asked for (through the engine)
//! the first time it is opened, and kept; [`rows`] only reads what was
//! already listed. Paths in here are relative to the tree's root, `/`
//! separated, with `""` for the root itself, which is also what git's marks
//! use. The expansion and the selection are remembered per root, so coming
//! back to a worktree finds the tree as it was left.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::files::{EntryKind, FileEntry, GitMark, GitMarks};
use leon_core::MachineId;

/// How deep [`rows`] goes: a guard against a listing that loops through
/// links, never reached by a real tree.
const MAX_DEPTH: u16 = 64;

/// What is known of one folder that was asked for (a folder not in the map
/// has not been answered yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// The entries, folders first.
    Loaded(Vec<FileEntry>),
    /// Why it could not be listed.
    Failed(String),
}

/// What a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A folder.
    Dir,
    /// A file (or whatever else is not a folder).
    File,
    /// A line of text under an open folder: it is loading, empty or failed.
    Note,
}

/// One visible line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The path from the root; for a note, that of its folder.
    pub path: String,
    /// The text to show.
    pub name: String,
    /// How many folders deep the row is: 0 under the root.
    pub depth: u16,
    /// What it is.
    pub kind: RowKind,
    /// For a folder, whether it is open.
    pub open: bool,
    /// What git says about it. A folder carries the strongest mark of what it
    /// holds.
    pub mark: Option<GitMark>,
    /// For a link, that it is one.
    pub symlink: bool,
}

/// The flat list of rows: the root's entries, and under each open folder
/// whose listing is known its own, indented. A folder that is open and not
/// listed yet has a note saying so, an empty one has a note saying that and a
/// failed one has the reason.
pub fn rows(
    listings: &HashMap<String, Listing>,
    open: &BTreeSet<String>,
    marks: &GitMarks,
) -> Vec<Row> {
    let mut out = Vec::new();
    match listings.get("") {
        Some(Listing::Loaded(entries)) => {
            push_entries("", entries, 0, listings, open, marks, &mut out)
        }
        Some(Listing::Failed(why)) => out.push(note("", why, 0)),
        None => out.push(note("", "Loading\u{2026}", 0)),
    }
    out
}

fn note(path: &str, text: &str, depth: u16) -> Row {
    Row {
        path: path.to_owned(),
        name: text.to_owned(),
        depth,
        kind: RowKind::Note,
        open: false,
        mark: None,
        symlink: false,
    }
}

fn push_entries(
    folder: &str,
    entries: &[FileEntry],
    depth: u16,
    listings: &HashMap<String, Listing>,
    open: &BTreeSet<String>,
    marks: &GitMarks,
    out: &mut Vec<Row>,
) {
    for entry in entries {
        let path = join(folder, &entry.name);
        let is_dir = entry.kind == EntryKind::Dir;
        let is_open = is_dir && open.contains(&path);
        out.push(Row {
            mark: marks.get(&path),
            path: path.clone(),
            name: entry.name.clone(),
            depth,
            kind: if is_dir { RowKind::Dir } else { RowKind::File },
            open: is_open,
            symlink: entry.symlink,
        });
        if !is_open || depth >= MAX_DEPTH {
            continue;
        }
        match listings.get(&path) {
            Some(Listing::Loaded(inner)) if inner.is_empty() => {
                out.push(note(&path, "Empty", depth + 1));
            }
            Some(Listing::Loaded(inner)) => {
                push_entries(&path, inner, depth + 1, listings, open, marks, out);
            }
            Some(Listing::Failed(why)) => out.push(note(&path, why, depth + 1)),
            None => {
                out.push(note(&path, "Loading\u{2026}", depth + 1));
            }
        }
    }
}

/// The tree of one folder of one machine: what was listed, which folders are
/// open and which row is selected, and the rows that follow. One is kept for
/// every folder that was shown, so coming back to it finds it as it was.
#[derive(Debug)]
pub struct Tree {
    /// The machine the folder is on.
    pub machine: MachineId,
    /// The folder, as the machine writes it.
    pub root: String,
    /// What is known of each folder, by path from the root.
    pub listings: HashMap<String, Listing>,
    /// The folders that are open.
    pub open: BTreeSet<String>,
    /// The selected path, if any. It is a path, not a row, so it survives the
    /// list changing under it.
    pub selected: Option<String>,
    /// What git says.
    pub marks: GitMarks,
    /// The visible rows.
    pub rows: Vec<Row>,
    /// The folders being listed right now, so a refresh does not ask twice.
    pub listing: HashSet<String>,
    /// Whether the marks are being read right now.
    pub marking: bool,
}

impl Tree {
    /// An empty tree of `root` on `machine`: nothing listed yet.
    pub fn new(machine: MachineId, root: String) -> Self {
        let mut tree = Self {
            machine,
            root,
            listings: HashMap::new(),
            open: BTreeSet::new(),
            selected: None,
            marks: GitMarks::default(),
            rows: Vec::new(),
            listing: HashSet::new(),
            marking: false,
        };
        tree.rebuild();
        tree
    }

    /// Makes the rows again, after anything changed.
    pub fn rebuild(&mut self) {
        self.rows = rows(&self.listings, &self.open, &self.marks);
    }

    /// The row of the selection, if it is shown.
    pub fn selected_index(&self) -> Option<usize> {
        position(&self.rows, self.selected.as_deref()?)
    }

    /// Puts the selection on the row at `index`.
    pub fn select(&mut self, index: usize) {
        if let Some(row) = self.rows.get(index).filter(|row| row.kind != RowKind::Note) {
            self.selected = Some(row.path.clone());
        }
    }

    /// Opens or closes a folder; `true` when it was not listed yet, so the
    /// caller has to ask for it.
    pub fn set_open(&mut self, path: &str, open: bool) -> bool {
        if open {
            self.open.insert(path.to_owned());
        } else {
            self.open.remove(path);
        }
        self.rebuild();
        open && !matches!(self.listings.get(path), Some(Listing::Loaded(_)))
    }

    /// Keeps what a listing answered. A folder that vanished since it was
    /// opened is closed.
    pub fn listed(&mut self, path: &str, answer: Result<Vec<FileEntry>, String>) {
        self.listing.remove(path);
        match answer {
            Ok(entries) => {
                self.listings
                    .insert(path.to_owned(), Listing::Loaded(entries));
            }
            // A refresh that fails keeps what was shown.
            Err(_) if matches!(self.listings.get(path), Some(Listing::Loaded(_))) => {}
            Err(why) => {
                self.listings.insert(path.to_owned(), Listing::Failed(why));
            }
        }
        self.rebuild();
    }
}

/// `name` inside `folder` (`""` for the root).
pub fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// `relative` under `root` as the machine writes it: the root as it is, with
/// a separator the root already uses.
pub fn absolute(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        return root.to_owned();
    }
    let separator = if root.contains('\\') && !root.contains('/') {
        '\\'
    } else {
        '/'
    };
    let relative = relative.replace('/', &separator.to_string());
    if root.ends_with(separator) {
        format!("{root}{relative}")
    } else {
        format!("{root}{separator}{relative}")
    }
}

/// `path` relative to `root`, written with `/` as the rows' paths are; `None`
/// when `path` is not under `root`.
pub fn relative(root: &str, path: &str) -> Option<String> {
    let root = root.replace('\\', "/");
    let path = path.replace('\\', "/");
    let rest = path.strip_prefix(root.trim_end_matches('/'))?;
    let rest = rest.strip_prefix('/')?;
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// Where the selection goes from `from` after `delta` rows, notes skipped and
/// the ends kept. `None` when no row can be selected.
pub fn step(rows: &[Row], from: Option<usize>, delta: isize) -> Option<usize> {
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.kind != RowKind::Note)
        .map(|(index, _)| index)
        .collect();
    if selectable.is_empty() {
        return None;
    }
    let at = from.and_then(|from| selectable.iter().position(|&index| index >= from));
    let target = match at {
        None if delta >= 0 && from.is_none() => 0,
        None => selectable.len() - 1,
        Some(at) => (at as isize + delta).clamp(0, selectable.len() as isize - 1) as usize,
    };
    Some(selectable[target])
}

/// The row of the folder that holds the row at `index`, if it is not at the
/// root.
pub fn parent_row(rows: &[Row], index: usize) -> Option<usize> {
    let row = rows.get(index)?;
    if row.depth == 0 {
        return None;
    }
    rows[..index]
        .iter()
        .rposition(|above| above.kind == RowKind::Dir && above.depth == row.depth - 1)
}

/// The row of `path`, if it is shown.
pub fn position(rows: &[Row], path: &str) -> Option<usize> {
    rows.iter()
        .position(|row| row.kind != RowKind::Note && row.path == path)
}

/// What Right does on the selected row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Right {
    /// Open the folder.
    Open,
    /// Move to the folder's first entry.
    Child(usize),
    /// Nothing: a file, or a folder with nothing in it.
    Nothing,
}

/// Right on the row at `index`: a closed folder opens, an open one goes to
/// its first entry.
pub fn right(rows: &[Row], index: usize) -> Right {
    let Some(row) = rows.get(index) else {
        return Right::Nothing;
    };
    match (row.kind, row.open) {
        (RowKind::Dir, false) => Right::Open,
        (RowKind::Dir, true) => match rows.get(index + 1) {
            Some(next) if next.kind != RowKind::Note && next.depth > row.depth => {
                Right::Child(index + 1)
            }
            _ => Right::Nothing,
        },
        _ => Right::Nothing,
    }
}

/// What Left does on the selected row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Left {
    /// Close the folder.
    Close,
    /// Move to the folder that holds the row.
    Parent(usize),
    /// Nothing: already at the top.
    Nothing,
}

/// Left on the row at `index`: an open folder closes, anything else goes to
/// its folder.
pub fn left(rows: &[Row], index: usize) -> Left {
    match rows.get(index) {
        Some(row) if row.kind == RowKind::Dir && row.open => Left::Close,
        Some(_) => parent_row(rows, index).map_or(Left::Nothing, Left::Parent),
        None => Left::Nothing,
    }
}

/// The folders the tree has to list to be complete: the root and every open
/// folder. What a refresh asks for.
pub fn to_list(open: &BTreeSet<String>) -> Vec<String> {
    std::iter::once(String::new())
        .chain(open.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            kind,
            symlink: false,
            size: (kind == EntryKind::File).then_some(1),
        }
    }

    fn listings() -> HashMap<String, Listing> {
        let mut all = HashMap::new();
        all.insert(
            String::new(),
            Listing::Loaded(vec![
                entry("src", EntryKind::Dir),
                entry("docs", EntryKind::Dir),
                entry("Cargo.toml", EntryKind::File),
            ]),
        );
        all.insert(
            "src".into(),
            Listing::Loaded(vec![
                entry("ui", EntryKind::Dir),
                entry("main.rs", EntryKind::File),
            ]),
        );
        all.insert(
            "src/ui".into(),
            Listing::Loaded(vec![entry("tree.rs", EntryKind::File)]),
        );
        all
    }

    fn shown(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                format!(
                    "{}{}{}",
                    "  ".repeat(usize::from(row.depth)),
                    row.name,
                    match (row.kind, row.open) {
                        (RowKind::Dir, true) => "/-",
                        (RowKind::Dir, false) => "/+",
                        _ => "",
                    }
                )
            })
            .collect()
    }

    fn open_of(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn a_closed_tree_is_the_roots_entries_and_an_open_folder_shows_its_own_indented() {
        let none = GitMarks::default();
        let closed = rows(&listings(), &BTreeSet::new(), &none);
        assert_eq!(shown(&closed), ["src/+", "docs/+", "Cargo.toml"]);

        let some = rows(&listings(), &open_of(&["src", "src/ui"]), &none);
        assert_eq!(
            shown(&some),
            [
                "src/-",
                "  ui/-",
                "    tree.rs",
                "  main.rs",
                "docs/+",
                "Cargo.toml"
            ]
        );
        assert_eq!(some[2].path, "src/ui/tree.rs");
        assert_eq!(some[2].depth, 2);
    }

    #[test]
    fn a_folder_open_but_not_listed_loading_empty_or_failed_says_so_in_a_note() {
        let mut all = listings();
        all.remove("src");
        let rows_ = rows(&all, &open_of(&["src", "docs"]), &GitMarks::default());
        assert_eq!(rows_[1].kind, RowKind::Note);
        assert_eq!(rows_[1].name, "Loading\u{2026}");
        assert_eq!(rows_[1].path, "src");

        all.insert("docs".into(), Listing::Loaded(Vec::new()));
        all.insert("src".into(), Listing::Failed("Permission denied.".into()));
        let rows_ = rows(&all, &open_of(&["src", "docs"]), &GitMarks::default());
        assert_eq!(
            shown(&rows_),
            [
                "src/-",
                "  Permission denied.",
                "docs/-",
                "  Empty",
                "Cargo.toml"
            ]
        );

        // The root itself.
        assert_eq!(
            shown(&rows(
                &HashMap::new(),
                &BTreeSet::new(),
                &GitMarks::default()
            )),
            ["Loading\u{2026}"]
        );
        let mut broken = HashMap::new();
        broken.insert(String::new(), Listing::Failed("gone".into()));
        assert_eq!(
            shown(&rows(&broken, &BTreeSet::new(), &GitMarks::default())),
            ["gone"]
        );
    }

    #[test]
    fn a_file_is_never_open_and_marks_follow_the_paths() {
        let marks = GitMarks::from_status(" M src/ui/tree.rs\0?? Cargo.toml\0");
        let all = rows(&listings(), &open_of(&["src", "Cargo.toml"]), &marks);
        let by = |name: &str| all.iter().find(|row| row.name == name).unwrap();
        assert!(!by("Cargo.toml").open, "a file does not open");
        assert_eq!(by("Cargo.toml").mark, Some(GitMark::Untracked));
        assert_eq!(by("src").mark, Some(GitMark::Modified), "a folder rolls up");
        assert_eq!(by("ui").mark, Some(GitMark::Modified));
        assert_eq!(by("main.rs").mark, None);
        assert_eq!(by("docs").mark, None);
    }

    #[test]
    fn the_selection_steps_over_notes_and_stays_between_the_ends() {
        let mut all = listings();
        all.insert("docs".into(), Listing::Loaded(Vec::new()));
        let list = rows(&all, &open_of(&["docs"]), &GitMarks::default());
        // src, docs, Empty (note), Cargo.toml
        assert_eq!(step(&list, None, 1), Some(0));
        assert_eq!(step(&list, Some(0), 1), Some(1));
        assert_eq!(step(&list, Some(1), 1), Some(3), "the note is skipped");
        assert_eq!(step(&list, Some(3), 1), Some(3), "the end stays");
        assert_eq!(step(&list, Some(3), -1), Some(1));
        assert_eq!(step(&list, Some(0), -1), Some(0));
        assert_eq!(step(&list, Some(0), 8), Some(3));
        assert_eq!(step(&[], Some(0), 1), None);
    }

    #[test]
    fn right_opens_then_enters_and_left_closes_then_leaves() {
        let list = rows(&listings(), &open_of(&["src"]), &GitMarks::default());
        // src(0) ui(1) main.rs(2) docs(3) Cargo.toml(4)
        assert_eq!(right(&list, 3), Right::Open);
        assert_eq!(right(&list, 0), Right::Child(1));
        assert_eq!(right(&list, 2), Right::Nothing);
        assert_eq!(left(&list, 0), Left::Close);
        assert_eq!(left(&list, 2), Left::Parent(0));
        assert_eq!(left(&list, 1), Left::Parent(0));
        assert_eq!(left(&list, 4), Left::Nothing, "already at the top");
        assert_eq!(left(&list, 3), Left::Nothing);

        // An open folder with only a note has nothing to enter.
        let mut all = listings();
        all.insert("docs".into(), Listing::Loaded(Vec::new()));
        let list = rows(&all, &open_of(&["docs"]), &GitMarks::default());
        assert_eq!(right(&list, 1), Right::Nothing);
    }

    #[test]
    fn a_tree_opens_folders_keeps_the_selection_by_path_and_survives_a_failed_refresh() {
        let mut tree = Tree::new(MachineId::local(), "/srv/api".into());
        assert_eq!(shown(&tree.rows), ["Loading\u{2026}"]);
        tree.listed(
            "",
            Ok(vec![
                entry("src", EntryKind::Dir),
                entry("a.rs", EntryKind::File),
            ]),
        );
        assert_eq!(shown(&tree.rows), ["src/+", "a.rs"]);

        tree.select(1);
        assert_eq!(tree.selected.as_deref(), Some("a.rs"));
        assert_eq!(tree.selected_index(), Some(1));

        // Opening an unlisted folder asks for it; a listed one does not.
        assert!(tree.set_open("src", true));
        assert_eq!(shown(&tree.rows), ["src/-", "  Loading\u{2026}", "a.rs"]);
        assert_eq!(
            tree.selected_index(),
            Some(2),
            "the selection followed its path"
        );
        tree.select(1);
        assert_eq!(
            tree.selected.as_deref(),
            Some("a.rs"),
            "a note cannot be selected"
        );
        tree.listed("src", Ok(vec![entry("lib.rs", EntryKind::File)]));
        assert!(!tree.set_open("src", false));
        assert!(!tree.set_open("src", true));
        assert_eq!(shown(&tree.rows), ["src/-", "  lib.rs", "a.rs"]);

        // A failed refresh keeps the listing; a first failure is shown.
        tree.listed("src", Err("timed out".into()));
        assert_eq!(shown(&tree.rows), ["src/-", "  lib.rs", "a.rs"]);
        tree.open.insert("other".into());
        tree.listed("other", Err("timed out".into()));
        assert_eq!(tree.listings["other"], Listing::Failed("timed out".into()));
        assert!(tree.listing.is_empty());
    }

    #[test]
    fn a_path_under_the_root_is_made_relative() {
        assert_eq!(relative("/p", "/p/src/a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(relative("/p/", "/p/a.rs").as_deref(), Some("a.rs"));
        assert_eq!(
            relative("C:\\p", "C:\\p\\src\\a.rs").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(relative("/p", "/pp/a.rs"), None);
        assert_eq!(relative("/p", "/q/a.rs"), None);
        assert_eq!(relative("/p", "/p"), None);
    }

    #[test]
    fn paths_join_split_and_become_the_machines_own() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a/b", "c"), "a/b/c");
        assert_eq!(absolute("/srv/api", ""), "/srv/api");
        assert_eq!(absolute("/srv/api", "src/a b.rs"), "/srv/api/src/a b.rs");
        assert_eq!(absolute("/", "etc"), "/etc");
        assert_eq!(
            absolute("C:\\work\\api", "src/a.rs"),
            "C:\\work\\api\\src\\a.rs"
        );
        assert_eq!(to_list(&open_of(&["src", "src/ui"])), ["", "src", "src/ui"]);
        assert_eq!(
            position(
                &rows(&listings(), &open_of(&["src"]), &GitMarks::default()),
                "src/main.rs"
            ),
            Some(2)
        );
    }
}
