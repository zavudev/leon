//! Both directions of a turn's terminal: how a client writes to an agent
//! that reads its standard input, and how it knows what arrived.
//!
//! A turn runs in a terminal of the host because only a terminal streams and
//! outlives a dropped connection. A terminal is a poor pipe, though: it
//! echoes what is typed, hands a program its input one line at a time and
//! cuts a line at 4096 bytes, treats some bytes as keys, and cannot say "end
//! of input" without closing. And the host keeps a terminal's *output* only:
//! after the client was closed, nothing says what had been typed.
//!
//! So the agent does not read the terminal. [`BRIDGE`], a small POSIX shell
//! script, stands between them:
//!
//! ```text
//! client ── terminal input ──► bridge ── pipe ──► agent's standard input
//! client ◄── terminal output ◄──────────────────── agent's output
//!                              bridge's marks ──┘
//! ```
//!
//! * The bridge switches echo off, then says it is ready ([`Mark::Ready`]).
//!   Nothing is written before that mark: no input is echoed or lost to a
//!   shell that is still starting.
//! * A message for the agent (one line of JSON) travels as a **frame**: short
//!   terminal lines carrying pieces of it ([`frame`]). The bridge puts the
//!   pieces together and gives the agent the whole line at once, whatever its
//!   length, or nothing.
//! * Every frame has a number that only grows and a tag. The bridge passes a
//!   number on once (a frame sent twice arrives once) and answers each frame
//!   on the terminal's output with [`Mark::Ack`], number and tag. The mark is
//!   therefore in the output the host keeps: replaying a turn shows what the
//!   agent was told, which is how a question is known to be answered.
//! * [`end`] closes the agent's input, which is how an agent that waits for
//!   more is told there is no more.
//! * What the agent must read first (its handshake, the first prompt) can be
//!   given to the bridge as arguments: it is then delivered whether or not
//!   the client stays connected after starting the turn.
//!
//! Marks are written as an operating system command of their own
//! (`ESC ]5379;… BEL`), without a line end: ESC and BEL are bytes no JSON
//! line holds unescaped, terminals ignore a command they do not know, and
//! the Windows pseudo-console passes it on (it drops a lone control byte
//! such as a record separator). [`Marks`] takes them out of the output
//! wherever they fall, even inside a line the agent was printing.

/// The byte that opens a mark.
const OPEN: u8 = 0x1B;
/// The byte that closes a mark.
pub const CLOSE: u8 = 0x07;
/// What follows [`OPEN`] in a mark.
const PREFIX: &str = "]5379;";
/// A mark is short; an opening followed by more than this is not one.
const MARK_MAX: usize = 200;
/// The most bytes of a message on one terminal line. With the header it
/// stays far below the 4096 bytes a terminal hands over at once.
const PIECE_BYTES: usize = 1000;
/// The most bytes of first messages given to the bridge as arguments; more
/// is sent as frames once it is ready. (One argument may be 128 KiB on
/// Linux, and a wire string 64 KiB.)
pub const ARGUMENT_BYTES: usize = 48 * 1024;

/// The script between the terminal and the agent. Arguments: how many first
/// lines follow, those lines, then the agent's command.
pub const BRIDGE: &str = r#"n=$1; shift
init=
while [ "$n" -gt 0 ]; do
    init="$init$1
"
    shift; n=$((n - 1))
done
exec 3>&1 4<&0
stty -echo 2>/dev/null
umask 077
pipe="${TMPDIR:-/tmp}/leon-turn.$$"
rm -f "$pipe"
mkfifo "$pipe" || exit 70
trap 'rm -f "$pipe"' EXIT
trap 'exit 129' HUP
trap 'exit 143' TERM
(
    printf '%s' "$init"
    printf '\033]5379;leon-ready\007' >&3
    last=0; cur=; buf=; count=0
    while IFS= read -r line <&4; do
        kind=${line%% *}; rest=${line#* }
        seq=${rest%% *}
        case "$seq" in ''|*[!0-9]*) continue ;; esac
        case "$kind" in
        B) cur=$seq; buf=; count=0 ;;
        +) if [ "$seq" = "$cur" ]; then buf="$buf${rest#* }"; count=$((count + 1)); fi ;;
        E) rest=${rest#* }; want=${rest%% *}; tag=${rest#* }
           if [ "$seq" = "$cur" ] && [ "$count" = "$want" ]; then
               if [ "$seq" -gt "$last" ]; then printf '%s\n' "$buf"; last=$seq; fi
               printf '\033]5379;leon-ack:%s:%s\007' "$seq" "$tag" >&3
           else
               printf '\033]5379;leon-nak:%s\007' "$seq" >&3
           fi
           cur=; buf=; count=0 ;;
        X) printf '\033]5379;leon-ack:%s:end\007' "$seq" >&3; exit 0 ;;
        esac
    done
) > "$pipe" &
"$@" < "$pipe" 3>&- 4<&-
code=$?
kill $! 2>/dev/null
exit $code"#;

/// The command that starts the bridge: given `first` to hand to the agent
/// before anything else, then the agent's `argv` (program first).
pub fn argv(first: &[String], argv: Vec<String>) -> Vec<String> {
    let mut command = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        BRIDGE.to_owned(),
        "leon-bridge".to_owned(),
        first.len().to_string(),
    ];
    command.extend(first.iter().map(|line| typeable(line)));
    command.extend(argv);
    command
}

/// A line of JSON with nothing in it a terminal acts on: JSON already
/// escapes every control character below 0x20; DEL (the erase key) is the
/// one left, and inside a JSON string it may be written escaped.
fn typeable(line: &str) -> String {
    line.replace('\u{7f}', "\\u007f")
}

/// The terminal lines that carry `line` (one JSON value) to the agent as
/// frame number `seq`, answered by an ack carrying `tag`.
pub fn frame(seq: u64, tag: &str, line: &str) -> Vec<u8> {
    let line = typeable(line);
    let mut out = format!("B {seq}\n").into_bytes();
    let mut count = 0usize;
    let mut rest = line.as_str();
    while !rest.is_empty() {
        let mut cut = rest.len().min(PIECE_BYTES);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let (piece, tail) = rest.split_at(cut);
        out.extend_from_slice(format!("+ {seq} ").as_bytes());
        out.extend_from_slice(piece.as_bytes());
        out.push(b'\n');
        count += 1;
        rest = tail;
    }
    out.extend_from_slice(format!("E {seq} {count} {}\n", safe_tag(tag)).as_bytes());
    out
}

/// The terminal line that closes the agent's input, as frame number `seq`.
/// Its ack carries the tag [`END_TAG`].
pub fn end(seq: u64) -> Vec<u8> {
    format!("X {seq}\n").into_bytes()
}

/// The tag of the ack that answers [`end`].
pub const END_TAG: &str = "end";

/// A tag as it may be written on a terminal line: letters, digits, `.`, `-`
/// and `_`; anything else becomes `_`.
pub fn safe_tag(tag: &str) -> String {
    tag.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// What the bridge says on the terminal's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// Echo is off and the agent has its input: frames may be written.
    Ready,
    /// Frame `seq` arrived whole and the agent has it (or had it already).
    Ack {
        /// The frame's number.
        seq: u64,
        /// The frame's tag.
        tag: String,
    },
    /// Frame `seq` arrived damaged and was dropped: send it again.
    Nak {
        /// The frame's number.
        seq: u64,
    },
}

fn parse_mark(text: &[u8]) -> Option<Mark> {
    let text = std::str::from_utf8(text).ok()?.strip_prefix(PREFIX)?;
    if text == "leon-ready" {
        return Some(Mark::Ready);
    }
    if let Some(rest) = text.strip_prefix("leon-ack:") {
        let (seq, tag) = rest.split_once(':')?;
        return Some(Mark::Ack {
            seq: seq.parse().ok()?,
            tag: tag.to_owned(),
        });
    }
    let seq = text.strip_prefix("leon-nak:")?;
    Some(Mark::Nak {
        seq: seq.parse().ok()?,
    })
}

/// Takes the bridge's marks out of a terminal's output.
#[derive(Debug, Default)]
pub struct Marks {
    /// The bytes after an opening byte, while the closing one is awaited.
    open: Option<Vec<u8>>,
}

impl Marks {
    /// Splits `bytes` into what the agent printed (appended to `output`) and
    /// the marks among it (appended to `marks`), in order. Anything else
    /// between the two bytes (a terminal's own sequences) stays output.
    pub fn push(&mut self, bytes: &[u8], output: &mut Vec<u8>, marks: &mut Vec<Mark>) {
        for &byte in bytes {
            match (&mut self.open, byte) {
                (None, OPEN) => self.open = Some(Vec::new()),
                (None, _) => output.push(byte),
                (Some(inside), CLOSE) => {
                    match parse_mark(inside) {
                        Some(mark) => marks.push(mark),
                        None => {
                            output.push(OPEN);
                            output.extend_from_slice(inside);
                            output.push(CLOSE);
                        }
                    }
                    self.open = None;
                }
                // Not a mark: what was held is output, and this byte may
                // open a real one.
                (Some(inside), OPEN) => {
                    output.push(OPEN);
                    output.extend_from_slice(inside);
                    inside.clear();
                }
                (Some(inside), _) => {
                    inside.push(byte);
                    if inside.len() > MARK_MAX || byte == b'\n' {
                        output.push(OPEN);
                        output.extend_from_slice(inside);
                        self.open = None;
                    }
                }
            }
        }
    }

    /// Output was lost: a mark cut in two is forgotten.
    pub fn reset(&mut self) {
        self.open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(chunks: &[&[u8]]) -> (String, Vec<Mark>) {
        let (mut reader, mut output, mut marks) = (Marks::default(), Vec::new(), Vec::new());
        for chunk in chunks {
            reader.push(chunk, &mut output, &mut marks);
        }
        (String::from_utf8(output).unwrap(), marks)
    }

    #[test]
    fn a_short_message_is_one_piece_between_a_beginning_and_an_end() {
        let bytes = frame(7, "a.q-1", r#"{"a":1}"#);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "B 7\n+ 7 {\"a\":1}\nE 7 1 a.q-1\n"
        );
    }

    #[test]
    fn a_long_message_is_cut_into_lines_a_terminal_hands_over_whole() {
        let text = format!(r#"{{"text":"{}é"}}"#, "ñ".repeat(4000));
        let bytes = frame(1, "p", &text);
        let lines: Vec<&str> = std::str::from_utf8(&bytes).unwrap().lines().collect();
        assert!(lines.len() > 9, "{} lines", lines.len());
        assert!(lines.iter().all(|line| line.len() < 1100));
        // Put back together, it is the message: no character was cut in two.
        let pieces: String = lines
            .iter()
            .filter_map(|line| line.strip_prefix("+ 1 "))
            .collect();
        assert_eq!(pieces, text);
        let count = lines.len() - 2;
        assert_eq!(lines[lines.len() - 1], format!("E 1 {count} p"));
    }

    #[test]
    fn the_erase_key_is_never_typed() {
        let bytes = frame(1, "p", "{\"text\":\"a\u{7f}b\"}");
        assert!(!bytes.contains(&0x7f));
        assert!(String::from_utf8(bytes).unwrap().contains("a\\u007fb"));
        let command = argv(&["{\"t\":\"\u{7f}\"}".to_owned()], vec![]);
        assert!(command.iter().all(|arg| !arg.contains('\u{7f}')));
    }

    #[test]
    fn a_tag_holds_nothing_a_terminal_line_could_not() {
        use crate::tag::{id_tag, tag_id};
        assert_eq!(safe_tag("a.b c\n;$(x)"), "a.b_c____x_");
        assert_eq!(id_tag('a', "0f3c-11"), "a.0f3c-11");
        assert_eq!(tag_id('a', "a.0f3c-11").as_deref(), Some("0f3c-11"));
        for id in ["", "x1", "has space", "né", "7", "a.b"] {
            let tag = id_tag('q', id);
            assert_eq!(tag, safe_tag(&tag), "{id:?}");
            assert_eq!(tag_id('q', &tag).as_deref(), Some(id), "{id:?}");
            assert_eq!(tag_id('a', &tag), None);
        }
    }

    #[test]
    fn marks_are_taken_out_wherever_they_fall() {
        let (output, marks) = split(&[
            b"noise\n\x1b]5379;leon-ready\x07{\"a\":",
            b"\x1b]5379;leon-ack:12:a.q1\x071}\n\x1b]5379;leon-",
            b"nak:13\x07",
        ]);
        assert_eq!(output, "noise\n{\"a\":1}\n");
        assert_eq!(
            marks,
            [
                Mark::Ready,
                Mark::Ack {
                    seq: 12,
                    tag: "a.q1".into()
                },
                Mark::Nak { seq: 13 },
            ]
        );
    }

    #[test]
    fn what_opens_no_mark_is_output_whole() {
        let (output, marks) =
            split(&[b"a\x1b[6n\x1b]0;title\x07\x1b]5379;leon-ready\x07b\x1bline\nend"]);
        assert_eq!(output, "a\x1b[6n\x1b]0;title\x07b\x1bline\nend");
        assert_eq!(marks, [Mark::Ready]);
        let long = vec![b'z'; MARK_MAX + 50];
        let mut bytes = vec![OPEN];
        bytes.extend_from_slice(&long);
        let (output, marks) = split(&[&bytes]);
        assert_eq!(output.len(), long.len() + 1);
        assert!(marks.is_empty());
    }

    #[test]
    fn the_bridge_is_started_with_its_first_lines_before_the_agent() {
        let command = argv(
            &["{\"one\":1}".to_owned(), "{\"two\":2}".to_owned()],
            vec!["claude".to_owned(), "-p".to_owned()],
        );
        assert_eq!(
            command,
            [
                "sh",
                "-c",
                BRIDGE,
                "leon-bridge",
                "2",
                "{\"one\":1}",
                "{\"two\":2}",
                "claude",
                "-p"
            ]
        );
    }
}
