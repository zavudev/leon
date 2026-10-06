//! Reading, writing and listing text files on a machine.
//!
//! A document is small enough (at most [`MAX_FILE_BYTES`]) to travel inside
//! one command: reading runs a short POSIX script that checks the size,
//! reports a `cksum` revision and prints the bytes base64-encoded; writing
//! sends the new bytes as the command's standard input, after checking that
//! the revision is still the one that was read. A revision that moved means
//! somebody else changed the file, and the write is refused instead of
//! overwriting it.
//!
//! The same scripts run over SSH and through a relay, because both execute a
//! [`CommandSpec`] as written on the machine. The local machine does not come
//! through here: `crates/app`'s engine reads and writes it directly.
//!
//! Listing is the third command: tracked and untracked-not-ignored files of a
//! git repository (so what git ignores — `target/`, `node_modules/` — stays
//! out of the tree), or a plain bounded `find` when the folder is not a
//! repository.

use crate::command::CommandSpec;

/// The most bytes of a file Leon reads or writes. Twice this in base64 plus
/// the command's own output stays below [`leon_wire::MAX_EXEC_OUTPUT`].
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// The most paths one listing returns.
pub const MAX_LIST_ENTRIES: usize = 50_000;

/// The marker every script prints before its answer, so a login banner that
/// an SSH server prints first is skipped.
const MARKER: &str = "LEON-FILE 1";

/// The command that reads one file, size-checked and base64-encoded.
///
/// The answer is read with [`parse_read`].
pub fn read_command(path: &str) -> CommandSpec {
    let script = READ_SCRIPT.replace("@CAP@", &MAX_FILE_BYTES.to_string());
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", path])
}

const READ_SCRIPT: &str = r#"f=$1
if [ ! -f "$f" ]; then
  printf 'LEON-FILE 1\nMISSING\n'
  exit 3
fi
s=$(wc -c < "$f" | tr -d ' ')
if [ "$s" -gt @CAP@ ]; then
  printf 'LEON-FILE 1\nTOO-BIG %s\n' "$s"
  exit 5
fi
c=$(cksum < "$f") || exit 6
printf 'LEON-FILE 1\nCHECK %s\n' "$c"
base64 < "$f" | tr -d '\n\r'
printf '\n'
"#;

/// What the read command found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// The file's text, with the revision ([`FileRevision`]) it had when it
    /// was read.
    Contents {
        /// The file's bytes as UTF-8 text.
        contents: String,
        /// The revision the write command will insist on.
        revision: String,
    },
    /// There is no such file.
    Missing,
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooBig {
        /// How big it says it is.
        size: u64,
    },
    /// The bytes are not UTF-8 text.
    NotText,
}

/// What the write command did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    /// The file now holds the new bytes.
    Saved,
    /// The file changed since it was read; nothing was written.
    Changed,
    /// The file is gone; nothing was written.
    Missing,
    /// The path is a symbolic link; nothing was written.
    Symlink,
}

/// The command that writes one file, refusing to overwrite a revision that
/// moved. The new bytes travel as the command's standard input, base64
/// encoded.
///
/// The answer is read with [`parse_write`].
pub fn write_command(path: &str, revision: &str) -> CommandSpec {
    CommandSpec::new("sh").args(["-c", WRITE_SCRIPT, "sh", path, revision])
}

const WRITE_SCRIPT: &str = r#"f=$1
want=$2
if [ -L "$f" ]; then printf 'LEON-FILE 1\nSYMLINK\n'; exit 7; fi
have=$(cksum < "$f" 2>/dev/null) || { printf 'LEON-FILE 1\nMISSING\n'; exit 3; }
if [ "$have" != "$want" ]; then printf 'LEON-FILE 1\nCHANGED\n'; exit 4; fi
tmp="$f.leon-tmp.$$"
cp "$f" "$tmp" || { printf 'LEON-FILE 1\nTMP\n'; exit 5; }
if ! base64 -d > "$tmp" 2>/dev/null; then
  if ! base64 -D > "$tmp" 2>/dev/null; then
    rm -f "$tmp"
    printf 'LEON-FILE 1\nDECODE\n'
    exit 6
  fi
fi
mv "$tmp" "$f" || { rm -f "$tmp"; printf 'LEON-FILE 1\nTMP\n'; exit 5; }
printf 'LEON-FILE 1\nSAVED\n'
"#;

/// The command that lists the files under a folder: a git repository's
/// tracked and untracked-not-ignored files, or a bounded `find` otherwise.
///
/// The answer is read with [`parse_listing`].
pub fn list_command(root: &str) -> CommandSpec {
    let script = LIST_SCRIPT.replace("@CAP@", &MAX_LIST_ENTRIES.to_string());
    CommandSpec::new("sh").args(["-c", script.as_str(), "sh", root])
}

const LIST_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 3
if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files --cached --others --exclude-standard -z
  exit 0
fi
find . -path ./.git -prune -o -type f -print | head -n @CAP@
"#;

/// Reads what the read command printed. `None` when the output is not the
/// script's answer at all.
pub fn parse_read(output: &str) -> Option<Read> {
    let mut lines = output.lines().map(|line| line.trim_end_matches('\r'));
    // A login banner may come first.
    if !lines.by_ref().any(|line| line == MARKER) {
        return None;
    }
    let head = lines.next().unwrap_or_default();
    if let Some(revision) = head.strip_prefix("CHECK ") {
        let payload = lines.next().unwrap_or_default();
        let bytes = base64_decode(payload)?;
        let contents = String::from_utf8(bytes).ok()?;
        return Some(Read::Contents {
            contents,
            revision: revision.to_owned(),
        });
    }
    if head == "MISSING" {
        return Some(Read::Missing);
    }
    if let Some(size) = head.strip_prefix("TOO-BIG ") {
        return Some(Read::TooBig {
            size: size.parse().unwrap_or(0),
        });
    }
    None
}

/// Reads what the write command printed. `None` when the output is not the
/// script's answer at all.
pub fn parse_write(output: &str) -> Option<Write> {
    let mut lines = output.lines().map(|line| line.trim_end_matches('\r'));
    if !lines.by_ref().any(|line| line == MARKER) {
        return None;
    }
    match lines.next().unwrap_or_default() {
        "SAVED" => Some(Write::Saved),
        "CHANGED" => Some(Write::Changed),
        "MISSING" => Some(Write::Missing),
        "SYMLINK" => Some(Write::Symlink),
        _ => None,
    }
}

/// The paths of a listing: NUL-separated as git prints them, or
/// newline-separated as `find` does. Anything absolute, above the root or
/// empty is dropped, and at most [`MAX_LIST_ENTRIES`] survive.
pub fn parse_listing(output: &str) -> Vec<String> {
    let separator = if output.contains('\0') { '\0' } else { '\n' };
    output
        .split(separator)
        .map(|path| path.trim_end_matches('\r'))
        .map(|path| path.strip_prefix("./").unwrap_or(path))
        .filter(|path| safe_relative(path))
        .take(MAX_LIST_ENTRIES)
        .map(str::to_owned)
        .collect()
}

/// Whether `path` is a relative path inside the root, with no way up.
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Standard base64, whitespace ignored; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Encodes `bytes` as standard base64 without line breaks.
pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_parser_understands_a_file() {
        let payload = base64_encode(b"# Title\n\ntext\n");
        let output = format!(
            "rubish banner\n{marker}\nCHECK 123 12\n{payload}\n",
            marker = MARKER
        );
        assert_eq!(
            parse_read(&output),
            Some(Read::Contents {
                contents: "# Title\n\ntext\n".to_owned(),
                revision: "123 12".to_owned(),
            })
        );
    }

    #[test]
    fn the_read_parser_understands_every_refusal() {
        assert_eq!(parse_read("LEON-FILE 1\nMISSING\n"), Some(Read::Missing));
        assert_eq!(
            parse_read("LEON-FILE 1\nTOO-BIG 999\n"),
            Some(Read::TooBig { size: 999 })
        );
        assert_eq!(parse_read("something else"), None);
        assert_eq!(parse_read("LEON-FILE 1\nWHAT\n"), None);
    }

    #[test]
    fn a_file_whose_bytes_are_not_utf8_is_not_text() {
        let payload = base64_encode(&[0xff, 0xfe, 0x00]);
        let output = format!("{MARKER}\nCHECK 1 3\n{payload}\n");
        assert_eq!(parse_read(&output), None);
    }

    #[test]
    fn the_write_parser_understands_every_answer() {
        assert_eq!(parse_write("LEON-FILE 1\nSAVED\n"), Some(Write::Saved));
        assert_eq!(parse_write("LEON-FILE 1\nCHANGED\n"), Some(Write::Changed));
        assert_eq!(parse_write("LEON-FILE 1\nMISSING\n"), Some(Write::Missing));
        assert_eq!(parse_write("LEON-FILE 1\nSYMLINK\n"), Some(Write::Symlink));
        assert_eq!(parse_write("LEON-FILE 1\nTMP\n"), None);
    }

    #[test]
    fn a_listing_is_read_as_nul_separated_git_paths_or_lines() {
        assert_eq!(
            parse_listing("src/lib.rs\0README.md\0docs/README.md\0"),
            ["src/lib.rs", "README.md", "docs/README.md"]
        );
        assert_eq!(parse_listing("./a b/c.md\n./d.md\n"), ["a b/c.md", "d.md"]);
        assert_eq!(
            parse_listing("/etc/passwd\0../up\0./fine.md\0"),
            ["fine.md"]
        );
        assert_eq!(parse_listing(""), Vec::<String>::new());
    }

    #[test]
    fn too_many_listed_paths_are_cut_at_the_limit() {
        let many = (0..MAX_LIST_ENTRIES + 10)
            .map(|n| format!("f{n}"))
            .collect::<Vec<_>>()
            .join("\0");
        assert_eq!(parse_listing(&many).len(), MAX_LIST_ENTRIES);
    }

    #[test]
    fn base64_and_its_decoder_round_trip() {
        let bytes = b"# Title\n\n- a\n- b\n";
        assert_eq!(
            base64_decode(&base64_encode(bytes)).as_deref(),
            Some(&bytes[..])
        );
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64_decode("a!b"), None);
    }
}

/// The scripts against a real shell, on this computer.
#[cfg(all(test, unix))]
mod shell_tests {
    use super::*;
    use crate::command::remote_shell_command;

    /// Runs a placed command through `sh`, the way an SSH machine would.
    fn run(command: &CommandSpec, stdin: &[u8]) -> std::process::Output {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(remote_shell_command(command))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh starts");
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(stdin)
            .expect("stdin accepts the bytes");
        child.wait_with_output().expect("the command finishes")
    }

    #[test]
    fn the_read_command_round_trips_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("README.md");
        let text = "# Título\n\ntext with 'quotes' and $dollars\n";
        std::fs::write(&path, text).unwrap();
        let output = run(&read_command(&path.to_string_lossy()), b"");
        assert!(output.status.success(), "{:?}", output);
        let read = parse_read(&String::from_utf8_lossy(&output.stdout)).expect("an answer");
        let Read::Contents { contents, revision } = read else {
            panic!("expected contents, got {read:?}");
        };
        assert_eq!(contents, text);
        assert!(!revision.is_empty());

        // And the write command accepts the same revision and stores new text.
        let newer = "# Otro\n";
        let payload = base64_encode(newer.as_bytes());
        let output = run(
            &write_command(&path.to_string_lossy(), &revision),
            payload.as_bytes(),
        );
        assert_eq!(
            parse_write(&String::from_utf8_lossy(&output.stdout)),
            Some(Write::Saved)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
    }

    #[test]
    fn the_write_command_refuses_a_revision_that_moved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "first\n").unwrap();
        let output = run(&read_command(&path.to_string_lossy()), b"");
        let Read::Contents { revision, .. } =
            parse_read(&String::from_utf8_lossy(&output.stdout)).unwrap()
        else {
            panic!("a file");
        };
        std::fs::write(&path, "somebody else\n").unwrap();
        let payload = base64_encode(b"mine\n");
        let output = run(
            &write_command(&path.to_string_lossy(), &revision),
            payload.as_bytes(),
        );
        assert_eq!(
            parse_write(&String::from_utf8_lossy(&output.stdout)),
            Some(Write::Changed)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "somebody else\n");
    }

    #[test]
    fn reading_reports_a_file_that_is_not_there_and_one_too_big() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.md");
        let output = run(&read_command(&missing.to_string_lossy()), b"");
        assert_eq!(
            parse_read(&String::from_utf8_lossy(&output.stdout)),
            Some(Read::Missing)
        );

        let big = dir.path().join("big.md");
        std::fs::write(&big, vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
        let output = run(&read_command(&big.to_string_lossy()), b"");
        assert_eq!(
            parse_read(&String::from_utf8_lossy(&output.stdout)),
            Some(Read::TooBig {
                size: (MAX_FILE_BYTES + 1) as u64
            })
        );
    }

    #[test]
    fn the_write_command_refuses_a_symbolic_link() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("real.md");
        std::fs::write(&path, "real\n").unwrap();
        let link = dir.path().join("link.md");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let output = run(&read_command(&link.to_string_lossy()), b"");
        let Read::Contents { revision, .. } =
            parse_read(&String::from_utf8_lossy(&output.stdout)).unwrap()
        else {
            panic!("a file follows the link and reads");
        };
        let payload = base64_encode(b"new\n");
        let output = run(
            &write_command(&link.to_string_lossy(), &revision),
            payload.as_bytes(),
        );
        assert_eq!(
            parse_write(&String::from_utf8_lossy(&output.stdout)),
            Some(Write::Symlink)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "real\n");
    }

    #[test]
    fn the_write_command_answers_missing_for_a_file_that_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.md");
        let payload = base64_encode(b"new\n");
        let output = run(
            &write_command(&path.to_string_lossy(), "1 2"),
            payload.as_bytes(),
        );
        assert_eq!(
            parse_write(&String::from_utf8_lossy(&output.stdout)),
            Some(Write::Missing)
        );
    }

    #[test]
    fn the_list_command_lists_a_non_repository_with_find() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "a").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.md"), "b").unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/hidden"), "x").unwrap();
        let output = run(&list_command(&dir.path().to_string_lossy()), b"");
        assert!(output.status.success(), "{:?}", output);
        let mut paths = parse_listing(&String::from_utf8_lossy(&output.stdout));
        paths.sort();
        assert_eq!(paths, ["a.md", "sub/b.md"]);
    }

    #[test]
    fn the_list_command_asks_git_when_the_folder_is_a_repository() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipped: git is not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let in_dir = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .expect("git runs");
            assert!(output.status.success(), "git {args:?}: {output:?}");
        };
        in_dir(&["init", "-q"]);
        std::fs::write(dir.path().join("tracked.md"), "t").unwrap();
        std::fs::write(dir.path().join("untracked.md"), "u").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(dir.path().join("ignored.md"), "i").unwrap();
        in_dir(&["add", "tracked.md", ".gitignore"]);
        let output = run(&list_command(&dir.path().to_string_lossy()), b"");
        let mut paths = parse_listing(&String::from_utf8_lossy(&output.stdout));
        paths.sort();
        assert_eq!(paths, [".gitignore", "tracked.md", "untracked.md"]);
    }
}
