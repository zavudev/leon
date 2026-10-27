//! Files of the phone kept on the other computer, for an agent to open.
//!
//! The agent runs on the computer, so a photo, a screenshot or a document
//! picked on the phone has to be there before the turn names it. A host
//! offers commands and nothing else: the file goes as the standard input of
//! one (`cat`), in pieces of at most [`MAX_EXEC_INPUT`] bytes, the first
//! writing the file and the others adding to it. It lands where the desktop
//! keeps pasted images, `$TMPDIR/leon-king-attachments`, under `phone/` and
//! the chat it was attached in; the last command prints the path the turn
//! names.
//!
//! Everything here is pure: the names, the pieces and their commands. The
//! session runs them ([`crate::session::HostSession::keep_attachment`]).

use leon_wire::{ExecSpec, MAX_EXEC_INPUT};

use crate::agents::login_spec;

/// The most a file may weigh: the pieces it is sent in, and the time the
/// relay takes to carry them, stay reasonable.
pub const MAX_ATTACHMENT: usize = 20 * 1024 * 1024;

/// The command that keeps one piece of a file: the folder, the file's name
/// and whether the piece is added to what came before are its arguments,
/// never written into the script.
const KEEP: &str = r#"dir="${TMPDIR:-/tmp}"
dir="${dir%/}/leon-king-attachments/phone/$1"
mkdir -p "$dir" || exit 1
if [ "$3" = more ]; then cat >> "$dir/$2"; else cat > "$dir/$2"; fi || exit 1
printf '\n%s\n' "$dir/$2""#;

/// `text` as a safe name of a file or a folder: letters, digits, `.`, `_`
/// and `-` are kept, anything else becomes `-`; it never starts with a dot
/// (no hidden file, no `..`) and is at most `max` characters, the end (the
/// extension) kept when it is cut. `fallback` when nothing is left.
pub fn safe_name(text: &str, max: usize, fallback: &str) -> String {
    let kept: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let kept = kept.trim_start_matches(['.', '-']);
    if kept.is_empty() {
        return fallback.to_owned();
    }
    let chars: Vec<char> = kept.chars().collect();
    if chars.len() <= max {
        return kept.to_owned();
    }
    chars[chars.len() - max..].iter().collect()
}

/// The name a file is kept under: a stamp first, so two files of the same
/// name in one chat never overwrite each other, then its own name.
pub fn kept_name(name: &str, stamp: u64) -> String {
    format!("{stamp}-{}", safe_name(name, 80, "file"))
}

/// The commands that keep `bytes` as `name` in the folder of `chat`, in the
/// order they have to run. An empty file is one command that writes
/// nothing.
pub fn keep_specs(chat: &str, name: &str, bytes: &[u8]) -> Vec<ExecSpec> {
    let folder = safe_name(chat, 64, "chat");
    let pieces: Vec<&[u8]> = if bytes.is_empty() {
        vec![&[]]
    } else {
        bytes.chunks(MAX_EXEC_INPUT).collect()
    };
    pieces
        .into_iter()
        .enumerate()
        .map(|(at, piece)| {
            let how = if at == 0 { "first" } else { "more" };
            let argv = ["sh", "-c", KEEP, "leon", &folder, name, how]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect();
            // Straight to `sh`: a login shell's start files have no business
            // with the file's bytes on standard input.
            let mut spec = login_spec(argv, None, false);
            spec.stdin = Some(piece.to_vec());
            spec
        })
        .collect()
}

/// The path the last command printed: its last line that is not empty.
pub fn kept_path(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .rfind(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_keeps_what_is_safe_and_its_extension_when_cut() {
        assert_eq!(
            safe_name("Screenshot 2026-10-26.png", 80, "file"),
            "Screenshot-2026-10-26.png"
        );
        assert_eq!(safe_name("../../etc/passwd", 80, "file"), "etc-passwd");
        assert_eq!(safe_name(".env", 80, "file"), "env");
        assert_eq!(safe_name("ñandú.jpg", 80, "file"), "and-.jpg");
        assert_eq!(safe_name("///", 80, "file"), "file");
        assert_eq!(safe_name("abcdefghij.png", 8, "file"), "ghij.png");
        assert_eq!(kept_name("a b.pdf", 7), "7-a-b.pdf");
    }

    #[test]
    fn a_file_goes_in_pieces_the_first_writes_and_the_others_add() {
        let bytes = vec![7u8; MAX_EXEC_INPUT * 2 + 3];
        let specs = keep_specs("chat 1", "1-x.png", &bytes);
        assert_eq!(specs.len(), 3);
        for (spec, (how, len)) in specs.iter().zip([
            ("first", MAX_EXEC_INPUT),
            ("more", MAX_EXEC_INPUT),
            ("more", 3),
        ]) {
            assert_eq!(spec.program, "sh");
            assert_eq!(spec.args[0], "-c");
            assert_eq!(&spec.args[2..], ["leon", "chat-1", "1-x.png", how]);
            assert_eq!(spec.stdin.as_ref().map(Vec::len), Some(len));
        }
        let empty = keep_specs("c", "1-e.txt", &[]);
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].stdin.as_deref(), Some(&[][..]));
    }

    #[test]
    fn the_path_is_the_last_line_printed() {
        assert_eq!(
            kept_path(b"motd\n\n/tmp/leon-king-attachments/phone/c/1-x.png\n").as_deref(),
            Some("/tmp/leon-king-attachments/phone/c/1-x.png")
        );
        assert_eq!(kept_path(b"\n \n"), None);
    }

    /// The script itself, run by the system's `sh`: pieces land in one
    /// file under `$TMPDIR`, and the path printed is that file.
    #[cfg(unix)]
    #[test]
    fn the_script_writes_the_pieces_into_one_file_and_says_where() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let tmp = std::env::temp_dir().join(format!("leon-attach-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mut printed = None;
        for (spec, piece) in keep_specs("c", "1-x.bin", b"abc")
            .into_iter()
            .chain(
                keep_specs("c", "1-x.bin", b"de")
                    .into_iter()
                    .map(|mut spec| {
                        spec.args[5] = "more".to_owned();
                        spec
                    }),
            )
            .map(|spec| {
                let piece = spec.stdin.clone().unwrap();
                (spec, piece)
            })
        {
            let mut child = Command::new(&spec.program)
                .args(&spec.args)
                .env("TMPDIR", &tmp)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&piece).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            printed = kept_path(&output.stdout);
        }
        let path = tmp.join("leon-king-attachments/phone/c/1-x.bin");
        assert_eq!(printed.as_deref(), path.to_str());
        assert_eq!(std::fs::read(&path).unwrap(), b"abcde");
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
