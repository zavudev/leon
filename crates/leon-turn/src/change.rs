//! What a tool changed in a file, as lines and not as text: the structure a
//! chat draws a diff from.
//!
//! An agent says a change three ways: as the old and new text of an edit, as
//! the whole content of a file it writes, and as a patch. Claude Code adds,
//! with the result of the tool, the patch it made with real line numbers.
//! Each becomes a [`FileChange`]: hunks of lines, each marked as kept, added
//! or removed, with its number in the old and in the new file when the agent
//! said where it is. A change drawn from what the agent was *given* has no
//! numbers (it does not know yet where the text is); the one it *made* has.
//!
//! Pure: JSON and text in, data out.

use serde_json::Value;

/// What was done to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The file did not exist before.
    Created,
    /// Lines of it were changed.
    Updated,
    /// The file was removed.
    Deleted,
}

/// What a line of a change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Around a change, unchanged.
    Context,
    /// Only the new file has it.
    Added,
    /// Only the old file had it.
    Removed,
}

/// One line of a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Kept, added or removed.
    pub kind: LineKind,
    /// The text, without the `+`, `-` or space that marks the kind.
    pub text: String,
    /// Its number in the old file, for a removed or kept line, when known.
    pub old: Option<u32>,
    /// Its number in the new file, for an added or kept line, when known.
    pub new: Option<u32>,
}

/// Lines that follow each other in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The lines, in the order of the file.
    pub lines: Vec<DiffLine>,
}

/// The change of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The file, as the agent names it.
    pub path: String,
    /// What was done to it.
    pub kind: ChangeKind,
    /// The lines that changed, with the ones around them.
    pub hunks: Vec<Hunk>,
}

impl FileChange {
    /// How many lines were added.
    pub fn added(&self) -> usize {
        self.count(LineKind::Added)
    }

    /// How many lines were removed.
    pub fn removed(&self) -> usize {
        self.count(LineKind::Removed)
    }

    fn count(&self, kind: LineKind) -> usize {
        self.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == kind)
            .count()
    }

    /// The lines of the change, hunk after hunk.
    pub fn lines(&self) -> impl Iterator<Item = &DiffLine> {
        self.hunks.iter().flat_map(|hunk| &hunk.lines)
    }

    /// A file written whole: every line is added, from the first.
    pub fn written(path: &str, content: &str) -> Self {
        Self::from_diff(path, ChangeKind::Created, content)
    }

    /// A file edited: each `(old, new)` pair is a hunk of the lines that
    /// went and the lines that came, without numbers: the lines both begin
    /// and end with are left out, and nothing says where in the file the
    /// pair is.
    pub fn edited(path: &str, edits: &[(&str, &str)]) -> Self {
        Self {
            path: path.to_owned(),
            kind: ChangeKind::Updated,
            hunks: edits
                .iter()
                .map(|(old, new)| changed_hunk(old, new))
                .collect(),
        }
    }

    /// The patch Claude Code made, as its result says it
    /// (`structuredPatch`): hunks that start at a line of each file.
    /// `None` when it holds no hunk.
    pub fn from_structured_patch(path: &str, patch: &Value) -> Option<Self> {
        let hunks: Vec<Hunk> = patch
            .as_array()?
            .iter()
            .map(|hunk| {
                let start = |key: &str| hunk[key].as_u64().and_then(|n| u32::try_from(n).ok());
                let mut numbers = Numbers::new(start("oldStart"), start("newStart"));
                let lines = hunk["lines"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|line| numbers.line(line))
                    .collect();
                Hunk { lines }
            })
            .filter(|hunk| !hunk.lines.is_empty())
            .collect();
        (!hunks.is_empty()).then(|| Self {
            path: path.to_owned(),
            kind: ChangeKind::Updated,
            hunks,
        })
    }

    /// The change a Claude Code tool made, from the `tool_use_result` that
    /// came with its result: a patch of the file, or the file created.
    pub fn from_tool_use_result(result: &Value) -> Option<Self> {
        let path = result["filePath"].as_str()?;
        if result["type"] == "create" {
            return Some(Self::written(path, result["content"].as_str()?));
        }
        Self::from_structured_patch(path, &result["structuredPatch"])
    }

    /// The changes a tool is *about* to make, from what it was given: the
    /// old and new text of `Edit` and `MultiEdit`, the content of `Write`, a
    /// patch of `apply_patch`. None for any other tool.
    pub fn from_input(name: &str, input: &Value) -> Vec<Self> {
        if let Some(patch) = ["patchText", "patch"]
            .iter()
            .find_map(|key| input[*key].as_str())
            .filter(|patch| patch.trim_start().starts_with("*** Begin Patch"))
        {
            return Self::from_patch(patch);
        }
        let text = |value: &Value, keys: &[&str]| {
            keys.iter()
                .find_map(|key| value[*key].as_str())
                .map(str::to_owned)
        };
        let Some(path) = text(input, &["file_path", "filePath", "path"]) else {
            return Vec::new();
        };
        let pair = |edit: &Value| {
            Some((
                text(edit, &["old_string", "oldString"])?,
                text(edit, &["new_string", "newString"])?,
            ))
        };
        let pairs: Vec<(String, String)> = match input["edits"].as_array() {
            Some(edits) => edits.iter().filter_map(pair).collect(),
            None => pair(input).into_iter().collect(),
        };
        if !pairs.is_empty() {
            let pairs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(old, new)| (old.as_str(), new.as_str()))
                .collect();
            return vec![Self::edited(&path, &pairs)];
        }
        match text(input, &["content"]) {
            Some(content) if name.eq_ignore_ascii_case("write") => {
                vec![Self::written(&path, &content)]
            }
            _ => Vec::new(),
        }
    }

    /// The files of a patch in the format of `apply_patch`
    /// (`*** Begin Patch`, `*** Update File: path`…), or a unified diff.
    pub fn from_patch(patch: &str) -> Vec<Self> {
        const FILES: [(&str, ChangeKind); 3] = [
            ("*** Update File: ", ChangeKind::Updated),
            ("*** Add File: ", ChangeKind::Created),
            ("*** Delete File: ", ChangeKind::Deleted),
        ];
        // The path, what was done and the lines of its section.
        let mut files: Vec<(String, ChangeKind, Vec<&str>)> = Vec::new();
        for line in patch.lines() {
            let named = FILES
                .iter()
                .find_map(|(mark, kind)| Some((line.strip_prefix(mark)?, *kind)));
            match (named, files.last_mut()) {
                (Some((path, kind)), _) => files.push((path.trim().to_owned(), kind, Vec::new())),
                (None, Some((_, _, lines))) if !line.starts_with("*** ") => lines.push(line),
                _ => {}
            }
        }
        if files.is_empty() {
            // A unified diff of one file, or nothing to read.
            return Vec::new();
        }
        files
            .into_iter()
            .map(|(path, kind, lines)| {
                match kind {
                    // The lines of a file added or removed are all marked.
                    ChangeKind::Created | ChangeKind::Deleted => {
                        let content: Vec<&str> =
                            lines.iter().filter_map(|line| line.get(1..)).collect();
                        Self::from_diff(&path, kind, &content.join("\n"))
                    }
                    ChangeKind::Updated => Self::from_diff(&path, kind, &lines.join("\n")),
                }
            })
            .collect()
    }

    /// One file's change as Codex says it: a unified diff for an update, the
    /// whole content for a file added or deleted.
    pub fn from_diff(path: &str, kind: ChangeKind, diff: &str) -> Self {
        let hunks = match kind {
            ChangeKind::Created | ChangeKind::Deleted => {
                let mut counter = 0;
                let lines = diff
                    .lines()
                    .map(|text| {
                        counter += 1;
                        marked(kind, text, Some(counter))
                    })
                    .collect::<Vec<_>>();
                vec![Hunk { lines }]
            }
            ChangeKind::Updated => unified_hunks(diff),
        };
        Self {
            path: path.to_owned(),
            kind,
            hunks: hunks
                .into_iter()
                .filter(|hunk| !hunk.lines.is_empty())
                .collect(),
        }
    }

    /// The `changes` of a Codex `fileChange` item: each file with what was
    /// done (`add`, `delete`, `update`) and its diff.
    pub fn from_codex(changes: &Value) -> Vec<Self> {
        changes
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|change| {
                let path = change["path"].as_str()?;
                let diff = change["diff"].as_str().unwrap_or_default();
                let kind = match change["kind"]["type"].as_str() {
                    Some("add") => ChangeKind::Created,
                    Some("delete") => ChangeKind::Deleted,
                    _ => ChangeKind::Updated,
                };
                Some(Self::from_diff(path, kind, diff))
            })
            .collect()
    }

    /// The `diff` blocks of an Agent Client Protocol tool call's content:
    /// the old and the new text of each file, the old one empty for a file
    /// that is new.
    pub fn from_acp(content: &Value) -> Vec<Self> {
        content
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter(|block| block["type"] == "diff")
            .filter_map(|block| {
                let path = block["path"].as_str()?;
                let new = block["newText"].as_str().unwrap_or_default();
                Some(match block["oldText"].as_str() {
                    Some(old) if !old.is_empty() => Self::edited(path, &[(old, new)]),
                    _ => Self::written(path, new),
                })
            })
            .collect()
    }
}

/// The numbers of the lines of a hunk as they are read: each side counts
/// from where its header says, or not at all.
struct Numbers {
    old: Option<u32>,
    new: Option<u32>,
}

impl Numbers {
    fn new(old: Option<u32>, new: Option<u32>) -> Self {
        Self { old, new }
    }

    /// The line `text` (marked with `+`, `-` or a space) as a [`DiffLine`],
    /// moving the counts on. `None` for what is not a line of a hunk.
    fn line(&mut self, text: &str) -> Option<DiffLine> {
        let (kind, rest) = match text.as_bytes().first() {
            Some(b'+') => (LineKind::Added, &text[1..]),
            Some(b'-') => (LineKind::Removed, &text[1..]),
            Some(b' ') => (LineKind::Context, &text[1..]),
            // An empty line stands for a kept empty line.
            None => (LineKind::Context, ""),
            _ => return None,
        };
        let (old, new) = (
            (kind != LineKind::Added).then_some(self.old).flatten(),
            (kind != LineKind::Removed).then_some(self.new).flatten(),
        );
        if kind != LineKind::Added {
            self.old = self.old.map(|n| n + 1);
        }
        if kind != LineKind::Removed {
            self.new = self.new.map(|n| n + 1);
        }
        Some(DiffLine {
            kind,
            text: rest.to_owned(),
            old,
            new,
        })
    }
}

/// A line of a file added (`number` in the new file) or removed (in the old).
fn marked(kind: ChangeKind, text: &str, number: Option<u32>) -> DiffLine {
    let removed = kind == ChangeKind::Deleted;
    DiffLine {
        kind: if removed {
            LineKind::Removed
        } else {
            LineKind::Added
        },
        text: text.to_owned(),
        old: number.filter(|_| removed),
        new: number.filter(|_| !removed),
    }
}

/// The hunks of a unified diff. The headers that name the files before the
/// first hunk are left out; a hunk's own header gives its numbers when it
/// has them (`@@ -10,3 +10,3 @@`) and none when it does not (`@@ fn a`).
fn unified_hunks(diff: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut numbers = Numbers::new(None, None);
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            numbers = header_numbers(header);
            hunks.push(Hunk { lines: Vec::new() });
            continue;
        }
        if hunks.is_empty() {
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                continue;
            }
            // A first hunk that has no header of its own.
            if !matches!(line.as_bytes().first(), Some(b'+' | b'-' | b' ')) {
                continue;
            }
            hunks.push(Hunk { lines: Vec::new() });
        }
        if let Some(line) = numbers.line(line) {
            if let Some(hunk) = hunks.last_mut() {
                hunk.lines.push(line);
            }
        }
    }
    hunks
}

/// Where a hunk starts in each file, from what follows its `@@`.
fn header_numbers(header: &str) -> Numbers {
    let start = |sign: char| {
        header
            .split_whitespace()
            .find_map(|word| word.strip_prefix(sign))
            .and_then(|range| range.split(',').next())
            .and_then(|n| n.parse().ok())
    };
    Numbers::new(start('-'), start('+'))
}

/// The lines only `old` has, then the lines only `new` has, without the
/// lines both begin and end with.
fn changed_hunk(old: &str, new: &str) -> Hunk {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let shorter = old.len().min(new.len());
    let head = (0..shorter).take_while(|&at| old[at] == new[at]).count();
    let tail = (0..shorter - head)
        .take_while(|&at| old[old.len() - 1 - at] == new[new.len() - 1 - at])
        .count();
    let line = |kind, text: &&str| DiffLine {
        kind,
        text: (*text).to_owned(),
        old: None,
        new: None,
    };
    let gone = old[head..old.len() - tail]
        .iter()
        .map(|text| line(LineKind::Removed, text));
    let come = new[head..new.len() - tail]
        .iter()
        .map(|text| line(LineKind::Added, text));
    Hunk {
        lines: gone.chain(come).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(kind: LineKind, text: &str, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            kind,
            text: text.into(),
            old,
            new,
        }
    }

    #[test]
    fn a_structured_patch_has_the_numbers_of_both_files() {
        // What Claude Code printed for an Edit, cut down.
        let result = json!({
            "filePath": "/work/a.py",
            "structuredPatch": [{
                "oldStart": 3, "oldLines": 3, "newStart": 3, "newLines": 4,
                "lines": [" keep", "-old", "+new", "+more", " tail"]
            }, {
                "oldStart": 20, "oldLines": 1, "newStart": 21, "newLines": 1,
                "lines": ["-x", "+y"]
            }]
        });
        let change = FileChange::from_tool_use_result(&result).unwrap();
        assert_eq!(change.path, "/work/a.py");
        assert_eq!(change.kind, ChangeKind::Updated);
        assert_eq!(change.hunks.len(), 2);
        assert_eq!(
            change.hunks[0].lines,
            vec![
                line(LineKind::Context, "keep", Some(3), Some(3)),
                line(LineKind::Removed, "old", Some(4), None),
                line(LineKind::Added, "new", None, Some(4)),
                line(LineKind::Added, "more", None, Some(5)),
                line(LineKind::Context, "tail", Some(5), Some(6)),
            ]
        );
        assert_eq!(
            change.hunks[1].lines[1],
            line(LineKind::Added, "y", None, Some(21))
        );
        assert_eq!((change.added(), change.removed()), (3, 2));
    }

    #[test]
    fn a_result_that_patched_nothing_changes_nothing() {
        let result = json!({"filePath": "/work/a.py", "structuredPatch": []});
        assert_eq!(FileChange::from_tool_use_result(&result), None);
        assert_eq!(
            FileChange::from_tool_use_result(&json!({"type": "text"})),
            None
        );
    }

    #[test]
    fn a_file_created_is_all_added_from_its_first_line() {
        let result = json!({
            "type": "create", "filePath": "/work/new.rs",
            "content": "fn a() {}\n\nfn b() {}\n", "structuredPatch": []
        });
        let change = FileChange::from_tool_use_result(&result).unwrap();
        assert_eq!(change.kind, ChangeKind::Created);
        assert_eq!((change.added(), change.removed()), (3, 0));
        assert_eq!(
            change.hunks[0].lines[2],
            line(LineKind::Added, "fn b() {}", None, Some(3))
        );
    }

    #[test]
    fn an_edit_not_yet_made_is_what_went_and_what_comes_without_numbers() {
        let input = json!({"file_path": "a.rs", "old_string": "a\nb\nc", "new_string": "a\nB\nc"});
        let changes = FileChange::from_input("Edit", &input);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "a.rs");
        assert_eq!(changes[0].kind, ChangeKind::Updated);
        assert_eq!(
            changes[0].hunks[0].lines,
            vec![
                line(LineKind::Removed, "b", None, None),
                line(LineKind::Added, "B", None, None),
            ]
        );
    }

    #[test]
    fn a_multi_edit_is_a_hunk_for_each_of_its_edits() {
        let input = json!({"file_path": "a.rs", "edits": [
            {"old_string": "one", "new_string": "two"},
            {"old_string": "x", "new_string": ""},
        ]});
        let changes = FileChange::from_input("MultiEdit", &input);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].hunks.len(), 2);
        assert_eq!((changes[0].added(), changes[0].removed()), (1, 2));
    }

    #[test]
    fn a_write_not_yet_made_is_a_file_created() {
        let input = json!({"file_path": "b.txt", "content": "x\ny"});
        let changes = FileChange::from_input("Write", &input);
        assert_eq!(changes[0].kind, ChangeKind::Created);
        assert_eq!(changes[0].added(), 2);
        assert_eq!(changes[0].hunks[0].lines[1].new, Some(2));
    }

    #[test]
    fn a_tool_that_changes_no_file_has_no_change() {
        assert!(FileChange::from_input("Bash", &json!({"command": "ls"})).is_empty());
        assert!(FileChange::from_input("Read", &json!({"file_path": "a"})).is_empty());
        // A cell of a notebook is not the lines of a file.
        let cell = json!({"notebook_path": "n.ipynb", "new_source": "x"});
        assert!(FileChange::from_input("NotebookEdit", &cell).is_empty());
    }

    #[test]
    fn a_codex_patch_is_one_change_for_each_file_it_names() {
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@ fn a\n keep\n-old\n+new\n*** Add File: b.txt\n+one\n+two\n*** Delete File: c.txt\n*** End Patch";
        let changes = FileChange::from_patch(patch);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].path, "src/a.rs");
        assert_eq!(changes[0].kind, ChangeKind::Updated);
        assert_eq!(
            changes[0].hunks[0].lines,
            vec![
                line(LineKind::Context, "keep", None, None),
                line(LineKind::Removed, "old", None, None),
                line(LineKind::Added, "new", None, None),
            ]
        );
        assert_eq!(changes[1].kind, ChangeKind::Created);
        assert_eq!(changes[1].added(), 2);
        assert_eq!(changes[2].kind, ChangeKind::Deleted);
        // It is also what a patch given as a tool's input says.
        let input = json!({ "patchText": patch });
        assert_eq!(FileChange::from_input("apply_patch", &input), changes);
    }

    #[test]
    fn a_unified_diff_has_the_numbers_its_headers_give() {
        let diff = "--- a/x\n+++ b/x\n@@ -10,3 +10,3 @@ fn f\n a\n-b\n+c\n d\n\\ No newline at end of file";
        let change = FileChange::from_diff("x", ChangeKind::Updated, diff);
        assert_eq!(
            change.hunks[0].lines,
            vec![
                line(LineKind::Context, "a", Some(10), Some(10)),
                line(LineKind::Removed, "b", Some(11), None),
                line(LineKind::Added, "c", None, Some(11)),
                line(LineKind::Context, "d", Some(12), Some(12)),
            ]
        );
        // A file added is its content, from line 1.
        let added = FileChange::from_diff("y", ChangeKind::Created, "p\nq");
        assert_eq!(
            added.hunks[0].lines[1],
            line(LineKind::Added, "q", None, Some(2))
        );
        let gone = FileChange::from_diff("y", ChangeKind::Deleted, "p\nq");
        assert_eq!(
            gone.hunks[0].lines[1],
            line(LineKind::Removed, "q", Some(2), None)
        );
    }

    #[test]
    fn a_big_write_keeps_every_line_and_its_count() {
        let content = (1..=500)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let change = FileChange::written("big", &content);
        assert_eq!(change.added(), 500);
        assert_eq!(change.lines().last().unwrap().new, Some(500));
    }

    #[test]
    fn what_codex_and_acp_say_of_a_change_is_read_as_the_same_lines() {
        let codex = json!([
            {"path": "a.rs", "kind": {"type": "update"}, "diff": "@@ -1,2 +1,2 @@\n k\n-o\n+n"},
            {"path": "b.rs", "kind": {"type": "add"}, "diff": "x\ny"},
            {"path": "c.rs", "kind": {"type": "delete"}, "diff": "z"},
        ]);
        let changes = FileChange::from_codex(&codex);
        assert_eq!(changes.len(), 3);
        assert_eq!(
            changes[0].hunks[0].lines[1],
            line(LineKind::Removed, "o", Some(2), None)
        );
        assert_eq!(
            (changes[1].kind, changes[1].added()),
            (ChangeKind::Created, 2)
        );
        assert_eq!(
            (changes[2].kind, changes[2].removed()),
            (ChangeKind::Deleted, 1)
        );

        let acp = json!([
            {"type": "content", "content": {"text": "hi"}},
            {"type": "diff", "path": "a.rs", "oldText": "a\nb", "newText": "a\nc"},
            {"type": "diff", "path": "n.rs", "newText": "q"},
        ]);
        let changes = FileChange::from_acp(&acp);
        assert_eq!(changes.len(), 2);
        assert_eq!((changes[0].added(), changes[0].removed()), (1, 1));
        assert_eq!(changes[1].kind, ChangeKind::Created);
        assert!(FileChange::from_acp(&json!(null)).is_empty());
    }
}
