//! Looking for text in the files of a project on a machine.
//!
//! Like the other file operations this is a short POSIX `sh` script described
//! once as a [`CommandSpec`]: the root, the query and the two switches travel
//! as its arguments (`$1` to `$4`), never as part of its text, so a query with
//! quotes, `$(...)` or a leading dash is only a string. One execute does it
//! all; the script picks the best tool the machine has:
//!
//! 1. `rg` (ripgrep): honours `.gitignore`, skips binary files and those above
//!    1 MiB, prints `path NUL line:column:text`;
//! 2. `git grep` inside a work tree: tracked and untracked-not-ignored files,
//!    binary files skipped, prints `path NUL line NUL column NUL text`;
//! 3. `grep -r`, which leaves out `.git` and the usual dependency and build
//!    folders, prints `path NUL line:text`.
//!
//! The script prints [`MARKER`](crate::files) and the tool's name before the
//! tool's own output, which is cut at [`MAX_LIST_BYTES`](crate::files) so that
//! the answer stays below `leon_wire::MAX_EXEC_OUTPUT`; a record cut by that
//! is dropped. The parsers ([`parse_search`]) normalise the three formats to
//! [`SearchHit`]s, at most [`MAX_HITS`] of them.
//!
//! The switches mean what they say for `rg` and the literal mode of the others;
//! a regular expression is the tool's own dialect (Rust's for `rg`, POSIX
//! extended for `git grep -E` and `grep -E`).

use crate::command::CommandSpec;
use crate::files::{after_marker, inside_root, MAX_LIST_BYTES};

/// The most hits a search returns; the answer says when there were more.
pub const MAX_HITS: usize = 1000;

/// The most characters of a matching line that are kept.
pub const MAX_LINE_CHARS: usize = 400;

/// What to look for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectQuery {
    /// The text, or the regular expression.
    pub text: String,
    /// Whether `text` is a regular expression rather than plain text.
    pub regex: bool,
    /// Whether upper and lower case letters differ.
    pub case_sensitive: bool,
}

impl ProjectQuery {
    /// A search for plain text, ignoring case.
    pub fn literal(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            regex: false,
            case_sensitive: false,
        }
    }
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// The file, relative to the root, with `/` separators.
    pub path: String,
    /// The line, from 1.
    pub line: u32,
    /// The byte column of the match in the line, from 1, when the tool says.
    pub col: Option<u32>,
    /// The line, cut at [`MAX_LINE_CHARS`] characters.
    pub text: String,
}

/// What a search found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchOutcome {
    /// The matching lines, in the order the tool printed them.
    pub hits: Vec<SearchHit>,
    /// Whether there was more than [`MAX_HITS`] (or than the output allowed).
    pub truncated: bool,
}

/// How the search script answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchReply {
    /// The search ran; there may be no hits.
    Found(SearchOutcome),
    /// The root does not exist.
    Missing,
}

/// The command that searches the files under `root` for `query`.
///
/// The answer is read with [`parse_search`]. A query that is not a valid
/// regular expression prints nothing on standard output and the tool's
/// complaint on standard error.
pub fn search_command(root: &str, query: &ProjectQuery) -> CommandSpec {
    let script = SEARCH_SCRIPT.replace("@BYTES@", &MAX_LIST_BYTES.to_string());
    CommandSpec::new("sh").args([
        "-c",
        script.as_str(),
        "sh",
        root,
        query.text.as_str(),
        if query.regex { "E" } else { "F" },
        if query.case_sensitive { "s" } else { "i" },
    ])
}

const SEARCH_SCRIPT: &str = r#"cd "$1" 2>/dev/null || { printf 'LEON-FILE 1\nMISSING\n'; exit 0; }
t=; command -v timeout >/dev/null 2>&1 && t='timeout 25'
ci=; [ "$4" = i ] && ci=-i
if command -v rg >/dev/null 2>&1; then
  printf 'LEON-FILE 1\nRG\n'
  m=; [ "$3" = F ] && m=-F
  $t rg --null --no-heading --line-number --column --color never --no-config --hidden -g '!.git' --max-filesize 1M $m $ci -e "$2" -- . | head -c @BYTES@
elif git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  printf 'LEON-FILE 1\nGIT\n'
  m=-E; [ "$3" = F ] && m=-F
  $t git grep -n -I -z --column --untracked --exclude-standard $m $ci -e "$2" | head -c @BYTES@
else
  printf 'LEON-FILE 1\nGREP\n'
  m=-E; [ "$3" = F ] && m=-F
  $t grep -rnI --null --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=target --exclude-dir=dist --exclude-dir=build --exclude-dir=.venv --exclude-dir=venv --exclude-dir=__pycache__ --exclude-dir=.next --exclude-dir=.cache $m $ci -e "$2" -- . | head -c @BYTES@
fi
exit 0
"#;

/// Reads what the search script printed. `None` when the output is not the
/// script's answer at all (no marker, or a tool this version does not know).
pub fn parse_search(output: &str) -> Option<SearchReply> {
    let body = after_marker(output)?;
    let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
    let parse: fn(&str) -> Option<SearchHit> = match head.trim_end_matches('\r') {
        "RG" => parse_rg_line,
        "GIT" => parse_git_line,
        "GREP" => parse_grep_line,
        "MISSING" => return Some(SearchReply::Missing),
        _ => return None,
    };
    Some(SearchReply::Found(collect(rest, parse)))
}

/// The hits of `records`, one per line. A last line with no newline after it
/// was cut short and is dropped, and says so; so does a hit past the cap.
fn collect(records: &str, parse: fn(&str) -> Option<SearchHit>) -> SearchOutcome {
    let mut lines: Vec<&str> = records.split('\n').collect();
    // Whatever follows the last newline is empty for a complete answer.
    let cut = lines.pop().is_some_and(|last| !last.is_empty());
    let mut hits = Vec::new();
    let mut truncated = cut;
    for line in lines {
        let Some(hit) = parse(line.strip_suffix('\r').unwrap_or(line)) else {
            continue;
        };
        if hits.len() == MAX_HITS {
            truncated = true;
            break;
        }
        hits.push(hit);
    }
    SearchOutcome { hits, truncated }
}

/// `path NUL line:column:text` (ripgrep with `--null`).
fn parse_rg_line(line: &str) -> Option<SearchHit> {
    let (path, rest) = line.split_once('\0')?;
    let (number, rest) = rest.split_once(':')?;
    let (column, text) = rest.split_once(':')?;
    hit(path, number, column.parse().ok(), text)
}

/// `path NUL line NUL column NUL text` (`git grep -z --column`).
fn parse_git_line(line: &str) -> Option<SearchHit> {
    let mut parts = line.splitn(4, '\0');
    let (path, number, column, text) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    hit(path, number, column.parse().ok(), text)
}

/// `path NUL line:text` (`grep -rn --null`); without the NUL (a `grep` that
/// does not know the option) `path:line:text`, where the path ends at the
/// first `:digits:`, which a path with such a part in it fools.
fn parse_grep_line(line: &str) -> Option<SearchHit> {
    if let Some((path, rest)) = line.split_once('\0') {
        let (number, text) = rest.split_once(':')?;
        return hit(path, number, None, text);
    }
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(at) = line[from..].find(':').map(|at| at + from) {
        let digits = bytes[at + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits > 0 && bytes.get(at + 1 + digits) == Some(&b':') {
            return hit(
                &line[..at],
                &line[at + 1..at + 1 + digits],
                None,
                &line[at + 2 + digits..],
            );
        }
        from = at + 1;
    }
    None
}

fn hit(path: &str, number: &str, column: Option<u32>, text: &str) -> Option<SearchHit> {
    let path = path.strip_prefix("./").unwrap_or(path);
    if !inside_root(path) {
        return None;
    }
    let line: u32 = number.parse().ok().filter(|line| *line > 0)?;
    Some(SearchHit {
        path: path.to_owned(),
        line,
        col: column.filter(|column| *column > 0),
        text: clip(text),
    })
}

/// `text` cut at [`MAX_LINE_CHARS`] characters, with an ellipsis when it was.
pub fn clip(text: &str) -> String {
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => format!("{}\u{2026}", &text[..at]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::remote_shell_command;

    fn found(output: &str) -> SearchOutcome {
        match parse_search(output) {
            Some(SearchReply::Found(outcome)) => outcome,
            other => panic!("expected hits, got {other:?}"),
        }
    }

    fn plain(path: &str, line: u32, col: Option<u32>, text: &str) -> SearchHit {
        SearchHit {
            path: path.into(),
            line,
            col,
            text: text.into(),
        }
    }

    #[test]
    fn the_query_and_the_root_are_arguments_never_part_of_the_script() {
        let query = ProjectQuery {
            text: "$(rm -rf ~) 'x' \"y\" -v".into(),
            regex: true,
            case_sensitive: true,
        };
        let command = search_command("/srv/my api's", &query);
        assert_eq!(command.program, "sh");
        assert_eq!(command.args[0], "-c");
        let script = &command.args[1];
        assert!(!script.contains("rm -rf") && !script.contains("my api"));
        assert_eq!(
            command.args[2..],
            ["sh", "/srv/my api's", "$(rm -rf ~) 'x' \"y\" -v", "E", "s"]
        );
        // Over SSH the whole thing is one quoted string; the query is inside
        // it only as a quoted argument.
        let line = remote_shell_command(&command);
        assert!(line.contains(r#"'$(rm -rf ~) '\''x'\'' "y" -v'"#), "{line}");
        let literal = search_command("/w", &ProjectQuery::literal("a"));
        assert_eq!(literal.args[4..], ["a", "F", "i"]);
    }

    #[test]
    fn the_script_prefers_ripgrep_then_git_grep_then_grep_and_bounds_its_output() {
        let script = &search_command("/w", &ProjectQuery::literal("a")).args[1];
        let rg = script.find("rg --null").unwrap();
        let git = script.find("git grep").unwrap();
        let grep = script.find("grep -rnI").unwrap();
        assert!(rg < git && git < grep);
        assert_eq!(script.matches("head -c 3500000").count(), 3);
        assert!(script.contains(" -e \"$2\" "), "the query follows -e");
        assert!(script.contains("--exclude-dir=.git"));
        const { assert!(MAX_LIST_BYTES < leon_wire::MAX_EXEC_OUTPUT) };
    }

    #[test]
    fn ripgrep_output_is_read_after_a_login_banner() {
        let output = "Last login: Mon\nLEON-FILE 1\nRG\n\
            ./src/a.rs\u{0}2:5:let foo = 1; foo\n\
            ./we ird/b:3: c.txt\u{0}1:1:foo:12:bar\n";
        let outcome = found(output);
        assert!(!outcome.truncated);
        assert_eq!(
            outcome.hits,
            [
                plain("src/a.rs", 2, Some(5), "let foo = 1; foo"),
                plain("we ird/b:3: c.txt", 1, Some(1), "foo:12:bar"),
            ]
        );
    }

    #[test]
    fn git_grep_output_has_the_path_line_and_column_between_nuls() {
        let output = "LEON-FILE 1\nGIT\nsrc/a.rs\u{0}2\u{0}5\u{0}let foo = 1; foo\n\
            we ird/b:3: c.txt\u{0}1\u{0}1\u{0}foo:12:bar\n";
        assert_eq!(
            found(output).hits,
            [
                plain("src/a.rs", 2, Some(5), "let foo = 1; foo"),
                plain("we ird/b:3: c.txt", 1, Some(1), "foo:12:bar"),
            ]
        );
    }

    #[test]
    fn grep_output_has_no_column_and_reads_both_forms() {
        let with_nul = "LEON-FILE 1\nGREP\n./src/a.rs\u{0}2:let foo = 1; foo\n\
            ./we ird/b:3: c.txt\u{0}1:foo:12:bar\n";
        assert_eq!(
            found(with_nul).hits,
            [
                plain("src/a.rs", 2, None, "let foo = 1; foo"),
                plain("we ird/b:3: c.txt", 1, None, "foo:12:bar"),
            ]
        );
        // No NUL (a grep without --null): the path ends at the first :digits:.
        let without = "LEON-FILE 1\nGREP\n./src/a.rs:2:let foo = 1; foo\n./x.txt:10:a:3:b\n";
        assert_eq!(
            found(without).hits,
            [
                plain("src/a.rs", 2, None, "let foo = 1; foo"),
                plain("x.txt", 10, None, "a:3:b"),
            ]
        );
    }

    #[test]
    fn a_line_cut_by_the_byte_cap_is_dropped_and_the_answer_says_so() {
        let output = "LEON-FILE 1\nRG\n./a\u{0}1:1:one\n./b\u{0}2:1:tw";
        let outcome = found(output);
        assert_eq!(outcome.hits, [plain("a", 1, Some(1), "one")]);
        assert!(outcome.truncated);
    }

    #[test]
    fn more_than_the_cap_is_cut_and_says_so() {
        let mut output = String::from("LEON-FILE 1\nGIT\n");
        for line in 1..=MAX_HITS + 5 {
            output.push_str(&format!("f.rs\u{0}{line}\u{0}1\u{0}x\n"));
        }
        let outcome = found(&output);
        assert_eq!(outcome.hits.len(), MAX_HITS);
        assert!(outcome.truncated);
        // Exactly the cap is not truncated.
        let mut exact = String::from("LEON-FILE 1\nGIT\n");
        for line in 1..=MAX_HITS {
            exact.push_str(&format!("f.rs\u{0}{line}\u{0}1\u{0}x\n"));
        }
        assert!(!found(&exact).truncated);
    }

    #[test]
    fn no_hits_a_missing_root_and_foreign_output_are_told_apart() {
        assert_eq!(found("LEON-FILE 1\nRG\n"), SearchOutcome::default());
        assert_eq!(
            parse_search("LEON-FILE 1\nMISSING\n"),
            Some(SearchReply::Missing)
        );
        assert_eq!(parse_search("ssh: connect to host box: refused"), None);
        assert_eq!(parse_search("LEON-FILE 1\nAG\n"), None);
    }

    #[test]
    fn paths_that_leave_the_root_and_lines_that_are_not_hits_are_ignored() {
        let output = "LEON-FILE 1\nGIT\n../x\u{0}1\u{0}1\u{0}a\n/etc/passwd\u{0}1\u{0}1\u{0}a\n\
            ok\u{0}0\u{0}1\u{0}zero line\nok\u{0}x\u{0}1\u{0}not a number\nwarning: something\n\
            ok\u{0}7\u{0}0\u{0}fine\n";
        assert_eq!(found(output).hits, [plain("ok", 7, None, "fine")]);
    }

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_LINE_CHARS + 10);
        let output = format!("LEON-FILE 1\nGIT\na\u{0}1\u{0}1\u{0}{long}\n");
        let text = &found(&output).hits[0].text;
        assert_eq!(text.chars().count(), MAX_LINE_CHARS + 1);
        assert!(text.ends_with('\u{2026}'));
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn carriage_returns_at_the_end_of_a_line_are_not_part_of_the_text() {
        let output = "LEON-FILE 1\r\nRG\r\n./a\u{0}1:1:crlf\r\n";
        assert_eq!(found(output).hits, [plain("a", 1, Some(1), "crlf")]);
    }

    /// Runs the script here, with whatever tools this machine has.
    #[cfg(unix)]
    #[test]
    fn the_script_finds_the_same_lines_with_whatever_tool_the_machine_has() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("src/a.rs", &b"fn main() {}\nlet Foo = 1; foo\n"[..]),
            ("we ird/b c.txt", b"nothing\nFOO bar\n"),
            ("bin.dat", b"\0foo\n"),
            (".git/config", b"foo\n"),
        ] {
            let file = dir.path().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        let run = |query: &ProjectQuery| {
            let command = search_command(dir.path().to_str().unwrap(), query);
            let output = std::process::Command::new(&command.program)
                .args(&command.args)
                .output()
                .unwrap();
            let mut hits = found(&String::from_utf8_lossy(&output.stdout)).hits;
            hits.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
            hits.into_iter()
                .map(|hit| (hit.path, hit.line))
                .collect::<Vec<_>>()
        };
        let both = [("src/a.rs".to_owned(), 2), ("we ird/b c.txt".to_owned(), 2)];
        assert_eq!(run(&ProjectQuery::literal("foo")), both);
        let sensitive = ProjectQuery {
            case_sensitive: true,
            ..ProjectQuery::literal("Foo")
        };
        assert_eq!(run(&sensitive), [("src/a.rs".to_owned(), 2)]);
        let regex = ProjectQuery {
            regex: true,
            ..ProjectQuery::literal("f(o)+")
        };
        assert_eq!(run(&regex), both);
    }
}
