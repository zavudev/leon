//! Looking for text in the files of a project on this computer: the local twin
//! of [`leon_remote::search`].
//!
//! It answers in the same shapes ([`SearchHit`], [`SearchOutcome`]) without a
//! shell or any tool, so it works on Windows too. The files are the project's
//! file list (what git tracks and has not ignored, or a bounded walk), so
//! `.gitignore` is honoured exactly as the file tree and quick-open honour it.
//! The scan skips files above [`MAX_FILE_BYTES`] and the ones that look binary
//! (a NUL byte in the first [`SNIFF_BYTES`]), stops at [`MAX_HITS`] hits, and
//! looks at a flag between files so that a newer query ends it at once.

use std::io::Read as _;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::{Regex, RegexBuilder};

pub use leon_remote::search::{clip, ProjectQuery, SearchHit, SearchOutcome, MAX_HITS};

/// The biggest file that is searched.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// How much of a file is looked at to tell whether it is binary.
const SNIFF_BYTES: usize = 8 * 1024;

/// A query ready to be matched against lines: also what draws the highlight
/// of a hit, whichever tool found it.
#[derive(Debug, Clone)]
pub struct Matcher {
    pattern: Regex,
}

impl Matcher {
    /// The matcher of `query`, or what is wrong with a regular expression.
    pub fn new(query: &ProjectQuery) -> Result<Self, String> {
        let source = if query.regex {
            query.text.clone()
        } else {
            regex::escape(&query.text)
        };
        RegexBuilder::new(&source)
            .case_insensitive(!query.case_sensitive)
            .multi_line(false)
            .size_limit(1 << 22)
            .build()
            .map(|pattern| Self { pattern })
            .map_err(|error| match error {
                regex::Error::Syntax(text) => text
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map_or(text.clone(), |line| line.trim().to_owned()),
                other => other.to_string(),
            })
    }

    /// Where the query matches in `line`, never an empty match.
    pub fn ranges(&self, line: &str) -> Vec<Range<usize>> {
        self.pattern
            .find_iter(line)
            .filter(|found| !found.is_empty())
            .map(|found| found.range())
            .collect()
    }

    /// Where the query first matches in `line`.
    fn first(&self, line: &str) -> Option<Range<usize>> {
        self.pattern
            .find_iter(line)
            .find(|found| !found.is_empty())
            .map(|found| found.range())
    }
}

/// The hits of one file's text, appended to `hits` until it holds
/// [`MAX_HITS`]. `true` when there was a hit that did not fit.
pub fn scan_text(path: &str, text: &str, matcher: &Matcher, hits: &mut Vec<SearchHit>) -> bool {
    for (index, line) in text.lines().enumerate() {
        let Some(found) = matcher.first(line) else {
            continue;
        };
        if hits.len() >= MAX_HITS {
            return true;
        }
        hits.push(SearchHit {
            path: path.to_owned(),
            line: index as u32 + 1,
            col: Some(found.start as u32 + 1),
            text: clip(line),
        });
    }
    false
}

/// Searches `files` (relative paths, `/` separated) under `root`. `cancel`
/// ends it early with what it has found so far.
pub fn search_files(
    root: &Path,
    files: &[String],
    matcher: &Matcher,
    cancel: &AtomicBool,
) -> SearchOutcome {
    let mut hits = Vec::new();
    let mut truncated = false;
    for relative in files {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(text) = read_searchable(&root.join(relative)) else {
            continue;
        };
        if scan_text(relative, &text, matcher, &mut hits) {
            truncated = true;
            break;
        }
    }
    SearchOutcome { hits, truncated }
}

/// The text of a file that is worth searching: a regular file within the size
/// cap with no NUL near its start. Bytes that are not UTF-8 are replaced.
fn read_searchable(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES || bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn matcher(text: &str, regex: bool, case_sensitive: bool) -> Matcher {
        Matcher::new(&ProjectQuery {
            text: text.into(),
            regex,
            case_sensitive,
        })
        .unwrap()
    }

    #[test]
    fn plain_text_is_matched_as_it_is_even_with_regex_characters() {
        let m = matcher("a.b(c)", false, false);
        assert_eq!(m.ranges("x A.B(C) y a.b(c)"), [2..8, 11..17]);
        assert!(m.ranges("axb(c)").is_empty());
    }

    #[test]
    fn the_case_switch_decides_whether_case_matters() {
        assert_eq!(matcher("foo", false, false).ranges("Foo foo"), [0..3, 4..7]);
        assert_eq!(matcher("foo", false, true).ranges("Foo foo"), [4..7]);
        assert_eq!(matcher("F.o", true, true).ranges("Foo foo"), [0..3]);
    }

    #[test]
    fn a_regular_expression_is_one_and_a_bad_one_says_what_is_wrong() {
        assert_eq!(matcher(r"\bf\w+", true, false).ranges("a foo bar"), [2..5]);
        let error = Matcher::new(&ProjectQuery {
            text: "(".into(),
            regex: true,
            case_sensitive: false,
        })
        .unwrap_err();
        assert!(error.contains("unclosed group"), "{error}");
    }

    #[test]
    fn an_empty_match_is_not_a_hit() {
        let m = matcher("x*", true, false);
        assert!(m.ranges("abc").is_empty());
        let mut hits = Vec::new();
        assert!(!scan_text("a", "abc\nabxc\n", &m, &mut hits));
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].col), (2, Some(3)));
    }

    #[test]
    fn a_scan_reports_the_line_the_column_and_the_text() {
        let mut hits = Vec::new();
        scan_text(
            "src/a.rs",
            "fn main() {}\r\n  let foo = 1;\n",
            &matcher("foo", false, false),
            &mut hits,
        );
        assert_eq!(
            hits,
            [SearchHit {
                path: "src/a.rs".into(),
                line: 2,
                col: Some(7),
                text: "  let foo = 1;".into()
            }]
        );
    }

    #[test]
    fn hits_stop_at_the_cap_and_the_scan_says_there_was_more() {
        let text = "x\n".repeat(MAX_HITS + 3);
        let mut hits = Vec::new();
        let more = scan_text("a", &text, &matcher("x", false, false), &mut hits);
        assert!(more);
        assert_eq!(hits.len(), MAX_HITS);
        // Exactly the cap does not say so.
        let mut exact = Vec::new();
        let text = "x\n".repeat(MAX_HITS);
        assert!(!scan_text(
            "a",
            &text,
            &matcher("x", false, false),
            &mut exact
        ));
        assert_eq!(exact.len(), MAX_HITS);
    }

    #[test]
    fn the_files_searched_skip_binary_and_big_ones_and_follow_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let file = dir.path().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        };
        write("src/a.rs", b"one\nneedle here\n");
        write("b.txt", b"needle\n");
        write("bin.dat", b"\0needle\n");
        write("big.txt", &vec![b'n'; MAX_FILE_BYTES as usize + 1]);
        write("latin.txt", b"caf\xe9 needle\n");
        write("not-listed.txt", b"needle\n");
        let files: Vec<String> = [
            "src/a.rs",
            "b.txt",
            "bin.dat",
            "big.txt",
            "latin.txt",
            "gone",
        ]
        .map(String::from)
        .into();
        let outcome = search_files(
            dir.path(),
            &files,
            &matcher("needle", false, false),
            &AtomicBool::new(false),
        );
        let found: Vec<(&str, u32)> = outcome
            .hits
            .iter()
            .map(|hit| (hit.path.as_str(), hit.line))
            .collect();
        assert_eq!(found, [("src/a.rs", 2), ("b.txt", 1), ("latin.txt", 1)]);
        assert!(!outcome.truncated);
    }

    #[test]
    fn a_cancelled_scan_returns_what_it_had() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let files = vec!["a.txt".to_owned()];
        let cancel = AtomicBool::new(true);
        let outcome = search_files(
            dir.path(),
            &files,
            &matcher("needle", false, false),
            &cancel,
        );
        assert!(outcome.hits.is_empty());
    }

    #[test]
    fn the_cap_across_files_marks_the_outcome_truncated() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(dir.path().join(name), "x\n".repeat(MAX_HITS)).unwrap();
        }
        let files = vec!["a.txt".to_owned(), "b.txt".to_owned()];
        let outcome = search_files(
            dir.path(),
            &files,
            &matcher("x", false, false),
            &AtomicBool::new(false),
        );
        assert_eq!(outcome.hits.len(), MAX_HITS);
        assert!(outcome.truncated);
    }
}
