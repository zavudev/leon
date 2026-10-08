//! Paths printed in the terminal that name a file on disk: `src/main.rs`,
//! `notes.md:12`, `~/todo.txt`. They are links like the web ones, but the
//! host opens them in its editor, so they carry a scheme of their own.
//!
//! [`file_refs`] finds what looks like a path in a line of text and is pure.
//! [`Files`] says which of those are real files of the terminal's folder; a
//! word such as `e.g.` or `v1.2` is not, and a remembered answer keeps the
//! disk from being asked on every repaint.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// What a file link starts with. The rest is the absolute path, then
/// optionally `#L` and a line number.
const SCHEME: &str = "leon-file://";

/// How long an answer about a path is remembered. A file made by the agent a
/// moment ago becomes a link on the next look after this.
const REMEMBER: Duration = Duration::from_secs(2);

/// The longest extension a file name may have to be taken for one.
const LONGEST_EXTENSION: usize = 10;

/// The files of one terminal's folder.
pub struct Files {
    cwd: PathBuf,
    home: Option<PathBuf>,
    seen: Mutex<HashMap<String, (Instant, Option<String>)>>,
}

impl Files {
    /// The files that relative paths printed in `cwd` mean.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        Self {
            cwd: cwd.into(),
            home,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// The link for `raw` (a path as printed) when it names a file that
    /// exists, or `None`.
    pub(crate) fn link(&self, raw: &str, line: Option<u32>) -> Option<String> {
        let path = self.resolve(raw)?;
        Some(match line {
            Some(line) => format!("{SCHEME}{path}#L{line}"),
            None => format!("{SCHEME}{path}"),
        })
    }

    fn resolve(&self, raw: &str) -> Option<String> {
        let now = Instant::now();
        if let Some((at, found)) = self.seen.lock().get(raw) {
            if now.duration_since(*at) < REMEMBER {
                return found.clone();
            }
        }
        let found = self.locate(raw);
        let mut seen = self.seen.lock();
        if seen.len() > 512 {
            seen.retain(|_, (at, _)| now.duration_since(*at) < REMEMBER);
        }
        seen.insert(raw.to_owned(), (now, found.clone()));
        found
    }

    fn locate(&self, raw: &str) -> Option<String> {
        let path = if let Some(rest) = raw.strip_prefix("~/") {
            self.home.as_ref()?.join(rest)
        } else if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            self.cwd.join(raw)
        };
        path.is_file().then(|| path.to_string_lossy().into_owned())
    }
}

/// The path and line a file link names, or `None` when `uri` is not one.
pub fn parse(uri: &str) -> Option<(String, Option<u32>)> {
    let rest = uri.strip_prefix(SCHEME)?;
    if let Some((path, line)) = rest.rsplit_once("#L") {
        if let Ok(line) = line.parse() {
            return Some((path.to_owned(), Some(line)));
        }
    }
    Some((rest.to_owned(), None))
}

/// What may end a path in running text.
fn separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '<' | '>' | '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '|'
        )
}

/// The things in `text` that look like a path to a file with an extension, as
/// character ranges with the path and the line after it (`:12` or `:12:3`).
/// Whether they exist is [`Files`]' to say.
pub(crate) fn file_refs(text: &[char]) -> Vec<(Range<usize>, String, Option<u32>)> {
    let mut found = Vec::new();
    let mut start = 0;
    while start < text.len() {
        if separator(text[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < text.len() && !separator(text[end]) {
            end += 1;
        }
        let token = &text[start..end];
        let first = start;
        start = end;

        let mut length = token.len();
        while length > 0 && matches!(token[length - 1], '.' | ',' | ';' | ':' | '!' | '?') {
            length -= 1;
        }
        let token: String = token[..length].iter().collect();
        if let Some((path, line)) = split_line(&token) {
            if looks_like_a_file(path) {
                found.push((first..first + length, path.to_owned(), line));
            }
        }
    }
    found
}

/// `path:12:3` as the path and `12`; a name that ends in other things is
/// itself the path.
fn split_line(token: &str) -> Option<(&str, Option<u32>)> {
    let mut path = token;
    let mut line = None;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((head, number))
                if !head.is_empty()
                    && !number.is_empty()
                    && number.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                // `path:12:3` is line 12: the number nearest the path.
                line = number.parse().ok();
                path = head;
            }
            _ => break,
        }
    }
    (!path.is_empty()).then_some((path, line))
}

/// Whether `path` is shaped like a file path: no scheme, and a name with an
/// extension that has a letter in it (so `1.2.3` and `3.14` are numbers).
fn looks_like_a_file(path: &str) -> bool {
    if path.contains("://") || path.chars().any(char::is_control) {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    let extension = &name[dot + 1..];
    dot > 0
        && (1..=LONGEST_EXTENSION).contains(&extension.len())
        && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
        && extension.chars().any(|ch| ch.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(text: &str) -> Vec<(String, Option<u32>)> {
        let chars: Vec<char> = text.chars().collect();
        file_refs(&chars)
            .into_iter()
            .map(|(_, path, line)| (path, line))
            .collect()
    }

    #[test]
    fn paths_with_an_extension_are_found_with_their_line() {
        assert_eq!(
            refs("edited src/main.rs:42:7, then notes.md."),
            vec![
                ("src/main.rs".to_owned(), Some(42)),
                ("notes.md".to_owned(), None)
            ]
        );
        assert_eq!(
            refs("see (./a/b.test.ts) and ~/todo.txt"),
            vec![
                ("./a/b.test.ts".to_owned(), None),
                ("~/todo.txt".to_owned(), None)
            ]
        );
    }

    #[test]
    fn numbers_words_and_urls_are_not_files() {
        assert!(refs("version 1.2.3 pi 3.14 https://example.com/a.html").is_empty());
        assert!(refs("Makefile and .gitignore and name.").is_empty());
    }

    #[test]
    fn the_range_covers_the_path_and_its_line_but_not_the_full_stop() {
        let text: Vec<char> = "in a/b.rs:3. ok".chars().collect();
        let found = file_refs(&text);
        assert_eq!(found.len(), 1);
        let shown: String = text[found[0].0.clone()].iter().collect();
        assert_eq!(shown, "a/b.rs:3");
    }

    #[test]
    fn only_a_file_that_exists_is_a_link_and_it_round_trips() {
        let dir = std::env::temp_dir().join(format!("leon-files-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "x").unwrap();
        let files = Files::new(&dir);
        let link = files.link("src/lib.rs", Some(9)).unwrap();
        let (path, line) = parse(&link).unwrap();
        assert_eq!(Path::new(&path), dir.join("src/lib.rs"));
        assert_eq!(line, Some(9));
        assert_eq!(files.link("src/gone.rs", None), None);
        assert_eq!(files.link("src", None), None);
        assert_eq!(parse("https://example.com"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
