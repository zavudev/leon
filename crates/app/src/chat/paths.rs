//! The files an agent's message names, so that they can be opened from it.
//!
//! An agent writes "marketing/out/video.mp4: 52 s" as often as it writes
//! `[the video](marketing/out/video.mp4)` or `` `src/main.rs:12` ``. This
//! finds those places in a Markdown text ([`find`]), says what each one
//! means on disk (a relative path is the folder the conversation works in,
//! `~/` is the home, `file://` is read as a URL, and a trailing `:12` or
//! `:12:3` is a line and a column), and, once something outside this module
//! knows which of them exist, rewrites the text so that each of those is a
//! link whose address is a `file://` URL ([`rewrite`]). The view then draws
//! and opens them as it does any web link, and [`parse_url`] reads the
//! address back.
//!
//! Pure: text and paths in, text and paths out. Whether a file exists is the
//! caller's to know, and to look at off the interface thread.

use std::ops::Range;
use std::path::{Component, Path, PathBuf};

/// Where a path is written in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Plain text: the range is the path as written (with its line).
    Plain,
    /// Inline code: the range holds the backticks.
    Code,
    /// The address of a Markdown link: the range is the address.
    Link,
}

/// A place of a message that names a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    /// Where it is, in bytes of the text.
    pub range: Range<usize>,
    /// How it is written.
    pub how: How,
    /// The path it means, from the root, without `.` or `..`.
    pub path: PathBuf,
    /// The line, from 1, when it names one.
    pub line: Option<u32>,
    /// The column, from 1, when it names one.
    pub col: Option<u32>,
}

/// What a name resolves to: the path, the line and the column.
type Spot = (PathBuf, Option<u32>, Option<u32>);

/// What `token` names: `~/x`, `/x`, `file:///x` or a path from `cwd`, with a
/// `:line` or `:line:col` at its end. `None` when it is a web address, or
/// when it is relative and the conversation has no folder.
pub fn resolve(token: &str, cwd: Option<&Path>, home: Option<&Path>) -> Option<Spot> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(rest) = token.strip_prefix("file://") {
        let (path, line, col) = parse_url_rest(rest)?;
        return Some((normal(&path), line, col));
    }
    if token.contains("://") || token.starts_with("mailto:") {
        return None;
    }
    let (token, line, col) = split_line(token);
    if token.is_empty() {
        return None;
    }
    let path = if token == "~" || token.starts_with("~/") {
        home?.join(token.trim_start_matches('~').trim_start_matches('/'))
    } else if Path::new(token).is_absolute() {
        PathBuf::from(token)
    } else {
        cwd?.join(token)
    };
    Some((normal(&path), line, col))
}

/// `path:12` and `path:12:3` as the path, the line and the column. A colon
/// that is not followed by digits to the end (a drive, a sentence) is part of
/// the path.
fn split_line(token: &str) -> (&str, Option<u32>, Option<u32>) {
    let number = |text: &str| -> Option<u32> {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
            .filter(|number| *number > 0)
    };
    if let Some((rest, last)) = token.rsplit_once(':') {
        if let Some(last) = number(last) {
            if let Some((path, middle)) = rest.rsplit_once(':') {
                if let Some(line) = number(middle) {
                    return (path, Some(line), Some(last));
                }
            }
            return (rest, Some(last), None);
        }
    }
    (token, None, None)
}

/// The path with its `.` and `..` taken out, as written, not looked up.
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The address of a file: `file://` and its path, percent-encoded, with
/// `#L12` or `#L12C3` for a line and a column.
pub fn url_of(path: &Path, line: Option<u32>, col: Option<u32>) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    };
    let mut url = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    match (line, col) {
        (Some(line), Some(col)) => url.push_str(&format!("#L{line}C{col}")),
        (Some(line), None) => url.push_str(&format!("#L{line}")),
        _ => {}
    }
    url
}

/// The path, line and column a `file://` address of [`url_of`] names; `None`
/// for any other address.
pub fn parse_url(url: &str) -> Option<Spot> {
    let rest = url.strip_prefix("file://")?;
    parse_url_rest(rest)
}

/// What follows `file://`: an optional host, the path and the fragment.
fn parse_url_rest(rest: &str) -> Option<Spot> {
    let (path, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    // `file://localhost/x` and `file:///x`.
    let path = path.strip_prefix("localhost").unwrap_or(path);
    if !path.starts_with('/') {
        return None;
    }
    let decoded = decode(path)?;
    // `/C:/x`: a drive.
    let drive = decoded.as_bytes().get(2) == Some(&b':')
        && decoded
            .as_bytes()
            .get(1)
            .is_some_and(u8::is_ascii_alphabetic);
    let decoded = if drive {
        decoded[1..].to_owned()
    } else {
        decoded
    };
    // A line may also be written at the end of the path, as in an editor.
    let (decoded, line, col) = {
        let (path, line, col) = split_line(&decoded);
        (path.to_owned(), line, col)
    };
    let (line, col) = match (line, fragment_line(fragment)) {
        (None, from_fragment) => from_fragment,
        (some, _) => (some, col),
    };
    Some((PathBuf::from(decoded), line, col))
}

/// `L12` and `L12C3` as a line and a column.
fn fragment_line(fragment: &str) -> (Option<u32>, Option<u32>) {
    let Some(rest) = fragment.strip_prefix('L') else {
        return (None, None);
    };
    let (line, col) = rest.split_once('C').unwrap_or((rest, ""));
    (line.parse().ok(), col.parse().ok())
}

/// `%XX` read back; `None` when it is not UTF-8.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What a plain word has to look like to be looked for on disk: a path with
/// a folder in it, `~`, or a file name with an extension.
fn looks_like_a_path(word: &str) -> bool {
    let (path, ..) = split_line(word);
    if path.is_empty() || path.contains("://") || path.starts_with('-') {
        return false;
    }
    if path == "~" || path.starts_with("~/") {
        return true;
    }
    if path.contains('/') {
        // A word of only slashes and dots is no file.
        return path.chars().any(char::is_alphanumeric);
    }
    // A file name: letters, digits, `_`, `-`, `.`, and a letter-started
    // extension (not `1.2.3`, not `e.g`).
    let Some((stem, ext)) = path.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && ext.len() >= 2
        && ext.len() <= 8
        && ext.starts_with(|c: char| c.is_ascii_alphabetic())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && path
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Every place of `text` (Markdown) that names a file: addresses of links,
/// inline code and plain words. Fenced code and web addresses are left
/// alone. In the order of the text.
pub fn find(text: &str, cwd: Option<&Path>, home: Option<&Path>) -> Vec<Named> {
    let mut named = Vec::new();
    let mut fenced: Option<(char, usize)> = None;
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start_matches(' ');
        let indent = body.len() - trimmed.len();
        let fence = ['`', '~'].into_iter().find_map(|mark| {
            let run = trimmed.chars().take_while(|c| *c == mark).count();
            (run >= 3 && indent <= 3).then_some((mark, run))
        });
        match (fenced, fence) {
            (None, Some(open)) => fenced = Some(open),
            (Some((mark, run)), Some((closing, len)))
                if closing == mark && len >= run && trimmed[len..].trim().is_empty() =>
            {
                fenced = None
            }
            (None, None) => scan_line(body, start, cwd, home, &mut named),
            _ => {}
        }
        start += line.len();
    }
    named
}

/// The places of one line, `at` bytes into the text.
fn scan_line(
    line: &str,
    at: usize,
    cwd: Option<&Path>,
    home: Option<&Path>,
    named: &mut Vec<Named>,
) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut word_from: Option<usize> = None;
    let flush = |from: &mut Option<usize>, to: usize, named: &mut Vec<Named>| {
        if let Some(begin) = from.take() {
            plain(&line[begin..to], at + begin, cwd, home, named);
        }
    };
    while i < bytes.len() {
        match bytes[i] {
            b'`' => {
                flush(&mut word_from, i, named);
                let run = bytes[i..].iter().take_while(|byte| **byte == b'`').count();
                let content = i + run;
                match close_code(line, content, run) {
                    Some(end) => {
                        let inner = line[content..end].trim();
                        if let Some((path, line_no, col)) =
                            resolve(inner, cwd, home).filter(|_| code_names_a_file(inner))
                        {
                            named.push(Named {
                                range: at + i..at + end + run,
                                how: How::Code,
                                path,
                                line: line_no,
                                col,
                            });
                        }
                        i = end + run;
                    }
                    None => i = content,
                }
            }
            b'[' => {
                flush(&mut word_from, i, named);
                match link(line, i) {
                    Some((dest, end)) => {
                        let written = line[dest.clone()].trim().trim_matches(['<', '>']);
                        if let Some((path, line_no, col)) = resolve(written, cwd, home) {
                            named.push(Named {
                                range: at + dest.start..at + dest.end,
                                how: How::Link,
                                path,
                                line: line_no,
                                col,
                            });
                        }
                        i = end;
                    }
                    None => i += 1,
                }
            }
            b' ' | b'\t' => {
                flush(&mut word_from, i, named);
                i += 1;
            }
            _ => {
                if word_from.is_none() {
                    word_from = Some(i);
                }
                i += 1;
            }
        }
    }
    flush(&mut word_from, bytes.len(), named);
}

/// Whether the content of an inline code span may be a file: a path with a
/// folder in it (a space is allowed then), or a file name with an extension.
/// `cargo test` and `x` are not.
fn code_names_a_file(inner: &str) -> bool {
    if inner.contains(char::is_whitespace) {
        inner.contains('/') && !inner.contains("://")
    } else {
        looks_like_a_path(inner)
    }
}

/// Where the code span that opened with `run` backticks and whose content
/// begins at `from` ends (the start of the closing backticks).
fn close_code(line: &str, from: usize, run: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let len = bytes[i..].iter().take_while(|byte| **byte == b'`').count();
            if len == run {
                return Some(i);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

/// The link `[label](address)` that opens at `open`: where its address is
/// and where the link ends.
fn link(line: &str, open: usize) -> Option<(Range<usize>, usize)> {
    let bytes = line.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    let mut close = None;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let close = close?;
    if bytes.get(close + 1) != Some(&b'(') {
        return None;
    }
    let from = close + 2;
    let mut parens = 1usize;
    let mut i = from;
    let mut end = None;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'(' => parens += 1,
            b')' => {
                parens -= 1;
                if parens == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let end = end?;
    // A title follows the address after a space, unless the address is in
    // angle brackets.
    let inner = &line[from..end];
    let dest_len = if inner.trim_start().starts_with('<') {
        inner.find('>').map_or(inner.len(), |at| at + 1)
    } else {
        inner.find(char::is_whitespace).unwrap_or(inner.len())
    };
    Some((from..from + dest_len, end + 1))
}

/// A plain word (`at` bytes into the text): the part of it that is a path,
/// without what a sentence puts around it.
fn plain(word: &str, at: usize, cwd: Option<&Path>, home: Option<&Path>, named: &mut Vec<Named>) {
    let start = word.len() - word.trim_start_matches(['(', '<', '"', '\'', '*']).len();
    let trimmed = word[start..]
        .trim_end_matches([')', '>', '"', '\'', '*', ',', '.', ';', ':', '!', '?', ']']);
    if trimmed.is_empty() || trimmed.contains(['*', '`', '[', '\\']) || !looks_like_a_path(trimmed)
    {
        return;
    }
    let Some((path, line, col)) = resolve(trimmed, cwd, home) else {
        return;
    };
    named.push(Named {
        range: at + start..at + start + trimmed.len(),
        how: How::Plain,
        path,
        line,
        col,
    });
}

/// `text` with each of `named` whose path `exists` as a link to its file:
/// plain words and inline code become the label of one, and the address of
/// a link is replaced by the file's.
pub fn rewrite(text: &str, named: &[Named], exists: impl Fn(&Path) -> bool) -> String {
    let mut out = text.to_owned();
    for place in named.iter().rev().filter(|place| exists(&place.path)) {
        let url = url_of(&place.path, place.line, place.col);
        let written = &text[place.range.clone()];
        let replacement = match place.how {
            How::Link => url,
            How::Code => format!("[{written}]({url})"),
            How::Plain => format!("[{}]({url})", written.replace('_', "\\_")),
        };
        out.replace_range(place.range.clone(), &replacement);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cwd() -> Option<&'static Path> {
        Some(Path::new("/work/app"))
    }

    fn home() -> Option<&'static Path> {
        Some(Path::new("/home/me"))
    }

    fn spot(token: &str) -> Option<(String, Option<u32>, Option<u32>)> {
        resolve(token, cwd(), home()).map(|(path, line, col)| (slashed(&path), line, col))
    }

    /// A path as the tests spell it: Windows joins with `\`, the tests with `/`.
    fn slashed(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    fn at(
        path: &str,
        line: Option<u32>,
        col: Option<u32>,
    ) -> Option<(String, Option<u32>, Option<u32>)> {
        Some((path.to_owned(), line, col))
    }

    #[test]
    fn a_path_is_what_it_says_from_the_folder_the_home_or_the_root() {
        assert_eq!(spot("src/main.rs"), at("/work/app/src/main.rs", None, None));
        assert_eq!(spot("./src/../lib.rs"), at("/work/app/lib.rs", None, None));
        assert_eq!(spot("/etc/hosts"), at("/etc/hosts", None, None));
        assert_eq!(spot("~/notes/a.md"), at("/home/me/notes/a.md", None, None));
        assert_eq!(spot("~"), at("/home/me", None, None));
        assert_eq!(
            spot("file:///tmp/a%20b.txt"),
            at("/tmp/a b.txt", None, None)
        );
        assert_eq!(
            spot("file://localhost/tmp/a.txt"),
            at("/tmp/a.txt", None, None)
        );
    }

    #[test]
    fn a_line_and_a_column_at_the_end_are_not_part_of_the_path() {
        assert_eq!(
            spot("src/main.rs:12"),
            at("/work/app/src/main.rs", Some(12), None)
        );
        assert_eq!(
            spot("src/main.rs:12:7"),
            at("/work/app/src/main.rs", Some(12), Some(7))
        );
        assert_eq!(
            spot("file:///tmp/a.rs#L3C4"),
            at("/tmp/a.rs", Some(3), Some(4))
        );
        assert_eq!(spot("file:///tmp/a.rs:9"), at("/tmp/a.rs", Some(9), None));
        // Not numbers: part of the name (on Windows `a:` is a drive instead).
        if !cfg!(windows) {
            assert_eq!(spot("a:b/c.rs"), at("/work/app/a:b/c.rs", None, None));
        }
        assert_eq!(spot("src/a.rs:0"), at("/work/app/src/a.rs:0", None, None));
    }

    #[test]
    fn web_addresses_and_paths_without_a_folder_are_not_files() {
        assert_eq!(spot("https://example.com/a/b.html"), None);
        assert_eq!(spot("mailto:a@b.c"), None);
        assert_eq!(spot(""), None);
        assert_eq!(
            resolve("src/a.rs", None, home()),
            None,
            "no folder to start from"
        );
        let absolute = if cfg!(windows) {
            "C:/abs/a.rs"
        } else {
            "/abs/a.rs"
        };
        assert!(resolve(absolute, None, None).is_some());
        assert_eq!(resolve("~/a", cwd(), None), None, "no home");
    }

    #[test]
    fn an_address_is_percent_encoded_and_read_back() {
        let url = url_of(Path::new("/tmp/a b/(c)#d.txt"), Some(3), None);
        assert_eq!(url, "file:///tmp/a%20b/%28c%29%23d.txt#L3");
        assert_eq!(
            parse_url(&url),
            Some((PathBuf::from("/tmp/a b/(c)#d.txt"), Some(3), None))
        );
        assert_eq!(
            url_of(Path::new("/x/y.rs"), Some(1), Some(2)),
            "file:///x/y.rs#L1C2"
        );
        assert_eq!(parse_url("https://example.com"), None);
        assert_eq!(parse_url("file://relative/x"), None);
    }

    fn found(text: &str) -> Vec<(How, String, String)> {
        find(text, cwd(), home())
            .into_iter()
            .map(|named| {
                (
                    named.how,
                    text[named.range].to_owned(),
                    slashed(&named.path),
                )
            })
            .collect()
    }

    #[test]
    fn plain_words_lose_what_a_sentence_puts_around_them() {
        let text = "marketing/out/nely-social.mp4: dura 52 s (see ./notes.md, and `x`).";
        assert_eq!(
            found(text),
            [
                (
                    How::Plain,
                    "marketing/out/nely-social.mp4".to_owned(),
                    "/work/app/marketing/out/nely-social.mp4".to_owned()
                ),
                (
                    How::Plain,
                    "./notes.md".to_owned(),
                    "/work/app/notes.md".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn words_that_are_not_paths_are_not_looked_for() {
        for text in [
            "version 1.2.3 is out",
            "e.g. this and i.e. that",
            "see https://example.com/a/b.html now",
            "it costs 3.50 today",
            "---",
            "run `cargo test` or `x` or `build`",
        ] {
            let places = found(text);
            assert!(places.is_empty(), "{text}: {places:?}");
        }
        // `and/or` has a folder in it: it is looked for (and not found).
        assert_eq!(found("and/or maybe").len(), 1);
    }

    #[test]
    fn code_links_and_lines_are_found_where_they_are() {
        let text =
            "Open `src/main.rs:12` or [the file](docs/a.md) or [site](https://x.dev/a/b.md).";
        let places = found(text);
        assert_eq!(
            places,
            [
                (
                    How::Code,
                    "`src/main.rs:12`".to_owned(),
                    "/work/app/src/main.rs".to_owned()
                ),
                (
                    How::Link,
                    "docs/a.md".to_owned(),
                    "/work/app/docs/a.md".to_owned()
                ),
            ]
        );
        let all = find(text, cwd(), home());
        assert_eq!(all[0].line, Some(12));
    }

    #[test]
    fn fenced_code_is_left_alone() {
        let text =
            "before src/a.rs\n```sh\ncat src/b.rs\n```\nafter src/c.rs\n~~~\nsrc/d.rs\n~~~\n";
        let words: Vec<String> = found(text).into_iter().map(|(_, word, _)| word).collect();
        assert_eq!(words, ["src/a.rs", "src/c.rs"]);
    }

    #[test]
    fn only_what_exists_becomes_a_link_and_the_rest_of_the_text_stays() {
        let text = "Wrote out/v_1.mp4: 52 s, not out/none.mp4. See `a b/c.rs:3` and [it](docs/a.md) or [no](nope.md).";
        let named = find(text, cwd(), home());
        let exists = |path: &Path| {
            path != Path::new("/work/app/out/none.mp4") && path != Path::new("/work/app/nope.md")
        };
        let out = rewrite(text, &named, exists);
        assert_eq!(
            out,
            "Wrote [out/v\\_1.mp4](file:///work/app/out/v_1.mp4): 52 s, not out/none.mp4. \
             See [`a b/c.rs:3`](file:///work/app/a%20b/c.rs#L3) and [it](file:///work/app/docs/a.md) \
             or [no](nope.md)."
        );
        // Nothing exists: nothing changes.
        assert_eq!(rewrite(text, &named, |_| false), text);
    }

    #[test]
    fn a_link_keeps_its_title_and_an_angle_address_is_replaced_whole() {
        let text = "[a](docs/a.md \"the doc\") and [b](<my docs/b.md>)";
        let named = find(text, cwd(), home());
        let out = rewrite(text, &named, |_| true);
        assert_eq!(
            out,
            "[a](file:///work/app/docs/a.md \"the doc\") and [b](file:///work/app/my%20docs/b.md)"
        );
    }

    #[test]
    fn a_bare_file_name_and_home_paths_count() {
        let places = found("edit README.md and ~/notes.txt, not 4.5");
        let words: Vec<&str> = places.iter().map(|(_, word, _)| word.as_str()).collect();
        assert_eq!(words, ["README.md", "~/notes.txt"]);
        assert_eq!(places[1].2, "/home/me/notes.txt");
    }
}
