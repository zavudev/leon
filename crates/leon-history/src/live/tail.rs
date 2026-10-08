//! The incremental reader: bytes of a growing transcript in, beats out.
//!
//! A [`Tail`] is fed whatever was appended to a transcript since the last
//! time, in pieces of any size. It keeps the unfinished last line until the
//! rest of it arrives, skips lines it cannot read, and never fails: a
//! transcript is written by another program, at its own pace, in a format
//! that grows new record types without notice.
//!
//! The reader does no I/O. [`read_appended`] is the one function here that
//! touches a file, and [`Follower`] joins the two for callers that follow a
//! file on disk.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use leon_core::AgentId;

use super::beat::Beat;
use super::{claude, codex};

/// The longest unfinished line kept while waiting for its end. A longer
/// line (a pasted picture can be several megabytes) is dropped whole.
const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;

/// The most bytes one [`read_appended`] call returns.
const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// Which transcript format a [`Tail`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// A Claude Code session file, `<projects>/<project>/<session>.jsonl`.
    /// Lines of sub-agents that older versions wrote into the same file are
    /// left out.
    Claude,
    /// A Claude Code sub-agent file,
    /// `<session>/subagents/agent-<id>.jsonl`.
    ClaudeSubagent,
    /// A Codex rollout file.
    Codex,
}

impl Format {
    /// The format of the session files of `agent`, when Leon can tail them.
    /// opencode keeps its sessions in a database and has none.
    pub fn of(agent: AgentId) -> Option<Self> {
        if agent == AgentId::CLAUDE {
            Some(Self::Claude)
        } else if agent == AgentId::CODEX {
            Some(Self::Codex)
        } else {
            None
        }
    }
}

/// An incremental parser of one transcript.
#[derive(Debug, Clone)]
pub struct Tail {
    format: Format,
    /// The bytes of the last line, still without its line break.
    rest: Vec<u8>,
    /// Whether bytes are being dropped up to the next line break.
    skipping: bool,
    /// Whether a turn is in progress, so its end is told once.
    turn_open: bool,
    /// The last token numbers told, so a repeat is not told again.
    usage: Option<(u64, u64)>,
    malformed: u64,
}

impl Tail {
    /// A reader for a transcript read from its first byte.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            rest: Vec::new(),
            skipping: false,
            turn_open: true,
            usage: None,
            malformed: 0,
        }
    }

    /// A reader for a transcript joined somewhere in the middle: everything
    /// up to the first line break is the end of a line whose start was not
    /// read, and is dropped.
    pub fn mid_file(format: Format) -> Self {
        Self {
            skipping: true,
            ..Self::new(format)
        }
    }

    /// The format this reader was made for.
    pub fn format(&self) -> Format {
        self.format
    }

    /// How many complete lines could not be read so far.
    pub fn malformed(&self) -> u64 {
        self.malformed
    }

    /// Reads the next bytes of the transcript and returns the beats of every
    /// line they complete, in order. Bytes after the last line break are
    /// kept for the next call.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Beat> {
        let mut beats = Vec::new();
        let mut bytes = bytes;
        while let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
            let (head, tail) = bytes.split_at(end);
            bytes = &tail[1..];
            if self.skipping {
                self.skipping = false;
                self.rest.clear();
                continue;
            }
            if self.rest.is_empty() {
                self.read_line(head, &mut beats);
            } else {
                let mut line = std::mem::take(&mut self.rest);
                line.extend_from_slice(head);
                self.read_line(&line, &mut beats);
            }
        }
        if !self.skipping {
            if self.rest.len() + bytes.len() > MAX_LINE_BYTES {
                self.rest = Vec::new();
                self.skipping = true;
                self.malformed += 1;
            } else {
                self.rest.extend_from_slice(bytes);
            }
        }
        beats
    }

    fn read_line(&mut self, line: &[u8], beats: &mut Vec<Beat>) {
        if line.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let mut found = Vec::new();
        let read = match self.format {
            Format::Claude => claude::read_line(line, false, &mut found),
            Format::ClaudeSubagent => claude::read_line(line, true, &mut found),
            Format::Codex => codex::read_line(line, &mut found),
        };
        if read.is_err() {
            self.malformed += 1;
            return;
        }
        for beat in found {
            if self.admit(&beat) {
                beats.push(beat);
            }
        }
    }

    /// Drops what a transcript tells twice: the end of a turn (the model's
    /// stop reason and the harness's own record say the same) and token
    /// numbers repeated on each line of one reply.
    fn admit(&mut self, beat: &Beat) -> bool {
        match beat {
            Beat::TurnEnded | Beat::Interrupted => std::mem::replace(&mut self.turn_open, false),
            Beat::Usage {
                context, output, ..
            } => self.usage.replace((*context, *output)) != Some((*context, *output)),
            other => {
                if other.opens_turn() {
                    self.turn_open = true;
                }
                true
            }
        }
    }
}

/// What [`read_appended`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    /// The bytes read.
    pub bytes: Vec<u8>,
    /// Where the next read starts.
    pub offset: u64,
    /// The file is shorter than it was: it was replaced or cut, and `bytes`
    /// starts at its first byte again. Whatever was built from the earlier
    /// bytes describes a file that is gone.
    pub restarted: bool,
    /// The file has more bytes than this read returned; read again.
    pub more: bool,
}

/// Reads what was appended to `path` after `offset`, at most 8 MiB a call.
///
/// A file shorter than `offset` was truncated or replaced by a new one, and
/// is read from its start. A file replaced by a longer one cannot be told
/// from a file that grew; transcripts are append-only, so this does not
/// happen to them.
pub fn read_appended(path: &Path, offset: u64) -> io::Result<Appended> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let restarted = length < offset;
    let start = if restarted { 0 } else { offset };
    let wanted = (length - start).min(MAX_READ_BYTES);
    let mut bytes = Vec::with_capacity(usize::try_from(wanted).unwrap_or(0));
    if wanted > 0 {
        file.seek(SeekFrom::Start(start))?;
        file.take(wanted).read_to_end(&mut bytes)?;
    }
    let offset = start + bytes.len() as u64;
    Ok(Appended {
        more: offset < length,
        bytes,
        offset,
        restarted,
    })
}

/// Where in a transcript a [`Follower`] starts reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// From the first byte: every beat of the session, at the cost of
    /// reading the whole file once.
    Beginning,
    /// From this many bytes before the end, at the next line break. Enough
    /// to see what the agent is doing now without reading a long history;
    /// a tool call started earlier and still running is not seen.
    Recent(u64),
}

/// What one [`Follower::poll`] found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Polled {
    /// The new beats, in order.
    pub beats: Vec<Beat>,
    /// The file started over (see [`Appended::restarted`]): the beats are
    /// those of the new file, and state reduced from earlier beats must be
    /// thrown away before applying them.
    pub restarted: bool,
}

/// A [`Tail`] with the file it reads and how far it has read.
///
/// Polling blocks on the file system for the time of one read; call it off
/// the thread that draws.
#[derive(Debug, Clone)]
pub struct Follower {
    path: PathBuf,
    offset: u64,
    start: Option<Start>,
    tail: Tail,
}

impl Follower {
    /// Follows the transcript at `path`. Nothing is read until the first
    /// [`poll`](Self::poll); the file need not exist yet.
    pub fn new(path: impl Into<PathBuf>, format: Format, start: Start) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            start: Some(start),
            tail: Tail::new(format),
        }
    }

    /// The file being followed.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many bytes of the file have been read.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The reader, for its count of unreadable lines.
    pub fn tail(&self) -> &Tail {
        &self.tail
    }

    /// Reads everything appended since the last poll. A file that does not
    /// exist yet has no beats; any other failure to read is the error.
    pub fn poll(&mut self) -> io::Result<Polled> {
        let mut polled = Polled::default();
        if let Some(start) = self.start {
            let length = match std::fs::metadata(&self.path) {
                Ok(metadata) => metadata.len(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(polled),
                Err(error) => return Err(error),
            };
            self.offset = start_offset(length, start);
            if self.offset > 0 {
                self.tail = Tail::mid_file(self.tail.format());
            }
            self.start = None;
        }
        loop {
            let appended = match read_appended(&self.path, self.offset) {
                Ok(appended) => appended,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(polled),
                Err(error) => return Err(error),
            };
            if appended.restarted {
                self.tail = Tail::new(self.tail.format());
                polled.beats.clear();
                polled.restarted = true;
            }
            self.offset = appended.offset;
            polled.beats.extend(self.tail.feed(&appended.bytes));
            if !appended.more {
                return Ok(polled);
            }
        }
    }
}

/// The byte a reader starts at in a file of `length` bytes.
fn start_offset(length: u64, start: Start) -> u64 {
    match start {
        Start::Beginning => 0,
        Start::Recent(window) => length.saturating_sub(window),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::beat::ToolKind;
    use crate::live::claude::fixture as claude_lines;
    use crate::live::codex::fixture as codex_lines;
    use serde_json::{json, Value};
    use std::fs;
    use std::io::Write;

    fn lines(values: &[Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.to_string().as_bytes());
            bytes.push(b'\n');
        }
        bytes
    }

    /// A short turn: a prompt, a reply that reads a file, the result, the
    /// closing words and the harness's record of the turn.
    fn turn() -> Vec<Value> {
        use claude_lines::*;
        vec![
            prompt("what does main do"),
            reply("m1", thinking(), None),
            reply(
                "m1",
                tool_use("t1", "Read", json!({"file_path": "/srv/api/src/main.rs"})),
                Some("tool_use"),
            ),
            result("t1", json!("fn main() {}"), false, json!({"type": "text"})),
            reply("m2", text("It starts the server."), Some("end_turn")),
            system("stop_hook_summary"),
            system("turn_duration"),
        ]
    }

    /// What the user wrote in a line of [`claude_lines::prompt`].
    fn heard(text: &str) -> Beat {
        Beat::Heard {
            text: text.into(),
            at: Some(1_772_359_200),
        }
    }

    fn turn_beats() -> Vec<Beat> {
        vec![
            Beat::Prompt,
            heard("what does main do"),
            Beat::Thinking,
            Beat::Usage {
                context: 1000,
                output: 42,
                window: None,
            },
            Beat::ToolStarted {
                id: "t1".into(),
                name: "Read".into(),
                kind: ToolKind::Read,
                detail: "main.rs".into(),
            },
            Beat::Brief {
                id: "t1".into(),
                text: "/srv/api/src/main.rs".into(),
                options: Vec::new(),
            },
            Beat::ToolFinished {
                id: "t1".into(),
                failed: false,
                refused: false,
            },
            Beat::Said {
                text: "It starts the server.".into(),
                at: Some(1_772_359_200),
            },
            Beat::TurnEnded,
        ]
    }

    #[test]
    fn a_turn_is_read_into_its_beats_once_each() {
        let mut tail = Tail::new(Format::Claude);
        assert_eq!(tail.feed(&lines(&turn())), turn_beats());
        assert_eq!(tail.malformed(), 0);
    }

    #[test]
    fn bytes_may_arrive_in_pieces_of_any_size() {
        let bytes = lines(&turn());
        for size in [1, 2, 7, 64, 1000] {
            let mut tail = Tail::new(Format::Claude);
            let beats: Vec<Beat> = bytes
                .chunks(size)
                .flat_map(|chunk| tail.feed(chunk))
                .collect();
            assert_eq!(beats, turn_beats(), "chunks of {size}");
        }
    }

    #[test]
    fn a_line_without_its_line_break_waits_for_the_rest() {
        let bytes = lines(&[claude_lines::prompt("hello")]);
        let (head, end) = bytes.split_at(bytes.len() - 1);
        let mut tail = Tail::new(Format::Claude);
        assert_eq!(tail.feed(head), []);
        assert_eq!(tail.feed(&[]), []);
        assert_eq!(tail.feed(end), [Beat::Prompt, heard("hello")]);
    }

    #[test]
    fn malformed_lines_are_counted_and_skipped() {
        let mut bytes = lines(&[claude_lines::prompt("first")]);
        bytes.extend_from_slice(b"this is not json\n");
        bytes.extend_from_slice(&[0xff, 0xfe, b'\n']);
        bytes.extend_from_slice(b"\n   \n\r\n");
        bytes.extend_from_slice(&lines(&[claude_lines::system("turn_duration")]));
        let mut tail = Tail::new(Format::Claude);
        assert_eq!(
            tail.feed(&bytes),
            [Beat::Prompt, heard("first"), Beat::TurnEnded]
        );
        assert_eq!(tail.malformed(), 2);
    }

    #[test]
    fn a_turn_that_ends_twice_in_the_file_ends_once_and_can_end_again() {
        use claude_lines::*;
        let mut tail = Tail::new(Format::Claude);
        let first = tail.feed(&lines(&[
            reply("m1", text("Done."), Some("end_turn")),
            system("turn_duration"),
            system("turn_duration"),
        ]));
        assert_eq!(
            first
                .iter()
                .filter(|beat| **beat == Beat::TurnEnded)
                .count(),
            1
        );
        let second = tail.feed(&lines(&[
            prompt("and now?"),
            reply("m2", text("Still done."), Some("end_turn")),
        ]));
        assert_eq!(
            second,
            [
                Beat::Prompt,
                heard("and now?"),
                Beat::Said {
                    text: "Still done.".into(),
                    at: Some(1_772_359_200),
                },
                Beat::TurnEnded
            ]
        );
    }

    #[test]
    fn a_reader_joining_mid_file_drops_the_line_it_landed_in() {
        let bytes = lines(&turn());
        let first_break = bytes.iter().position(|byte| *byte == b'\n').unwrap();
        let mut tail = Tail::mid_file(Format::Claude);
        let beats = tail.feed(&bytes[first_break / 2..]);
        assert_eq!(beats, turn_beats()[2..]);
        assert_eq!(tail.malformed(), 0);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_dropped_without_being_kept() {
        let mut tail = Tail::new(Format::Claude);
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..40 {
            assert_eq!(tail.feed(&chunk), []);
            assert!(tail.rest.len() <= MAX_LINE_BYTES);
        }
        assert!(tail.skipping);
        let mut rest = b"still the long line\n".to_vec();
        rest.extend_from_slice(&lines(&[claude_lines::prompt("after")]));
        assert_eq!(tail.feed(&rest), [Beat::Prompt, heard("after")]);
        assert_eq!(tail.malformed(), 1);
    }

    #[test]
    fn a_sub_agent_file_is_read_with_its_own_format() {
        use claude_lines::*;
        let bytes = lines(&[
            sidechain(prompt("map the parser"), "a1"),
            sidechain(
                reply(
                    "m1",
                    tool_use("t1", "Grep", json!({"pattern": "fn parse"})),
                    Some("tool_use"),
                ),
                "a1",
            ),
        ]);
        assert_eq!(Tail::new(Format::Claude).feed(&bytes), []);
        let beats = Tail::new(Format::ClaudeSubagent).feed(&bytes);
        assert_eq!(beats.len(), 4);
        assert_eq!(beats[1], heard("map the parser"));
        assert!(matches!(
            beats[2],
            Beat::ToolStarted {
                kind: ToolKind::Search,
                ..
            }
        ));
    }

    #[test]
    fn a_codex_rollout_is_read_into_the_same_beats() {
        use codex_lines::*;
        let bytes = lines(&[
            envelope("session_meta", json!({"id": "s", "cwd": "/srv/api"})),
            event("task_started"),
            item(json!({"type": "reasoning", "summary": []})),
            exec(
                "c1",
                "const r = await tools.exec_command({\"cmd\":\"cargo check\"});",
            ),
            exec_output("c1", "Script completed\nWall time 1 seconds"),
            item(json!({"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "Clean."}]})),
            event("task_complete"),
        ]);
        let beats = Tail::new(Format::Codex).feed(&bytes);
        assert_eq!(
            beats,
            [
                Beat::Prompt,
                Beat::Thinking,
                Beat::ToolStarted {
                    id: "c1".into(),
                    name: "exec_command".into(),
                    kind: ToolKind::Run,
                    detail: "cargo check".into()
                },
                Beat::ToolFinished {
                    id: "c1".into(),
                    failed: false,
                    refused: false
                },
                Beat::Said {
                    text: "Clean.".into(),
                    at: Some(1_772_359_200),
                },
                Beat::TurnEnded
            ]
        );
    }

    #[test]
    fn formats_are_known_for_the_agents_that_write_files() {
        assert_eq!(Format::of(AgentId::CLAUDE), Some(Format::Claude));
        assert_eq!(Format::of(AgentId::CODEX), Some(Format::Codex));
        assert_eq!(Format::of(AgentId::OPENCODE), None);
    }

    #[test]
    fn only_the_appended_bytes_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"first\n").unwrap();

        let read = read_appended(&path, 0).unwrap();
        assert_eq!(read.bytes, b"first\n");
        assert_eq!((read.offset, read.restarted, read.more), (6, false, false));

        let nothing = read_appended(&path, 6).unwrap();
        assert_eq!((nothing.bytes.len(), nothing.offset), (0, 6));

        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        let more = read_appended(&path, 6).unwrap();
        assert_eq!(more.bytes, b"second\n");
        assert_eq!(more.offset, 13);
    }

    #[test]
    fn a_file_that_became_shorter_is_read_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"a long first version\n").unwrap();
        let offset = read_appended(&path, 0).unwrap().offset;

        fs::write(&path, b"new\n").unwrap();
        let read = read_appended(&path, offset).unwrap();
        assert!(read.restarted);
        assert_eq!(read.bytes, b"new\n");
        assert_eq!(read.offset, 4);
    }

    #[test]
    fn a_missing_file_is_an_error_for_the_read_and_silence_for_a_follower() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-yet.jsonl");
        assert!(read_appended(&path, 0).is_err());

        let mut follower = Follower::new(&path, Format::Claude, Start::Beginning);
        assert_eq!(follower.poll().unwrap(), Polled::default());

        fs::write(&path, lines(&turn())).unwrap();
        assert_eq!(follower.poll().unwrap().beats, turn_beats());
        assert_eq!(follower.poll().unwrap().beats, []);
        assert_eq!(follower.offset(), fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn a_follower_reads_what_is_appended_and_starts_over_after_a_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let all = lines(&turn());
        let cut = all.len() / 2;
        fs::write(&path, &all[..cut]).unwrap();

        let mut follower = Follower::new(&path, Format::Claude, Start::Beginning);
        let mut beats = follower.poll().unwrap().beats;
        fs::write(&path, &all).unwrap();
        beats.extend(follower.poll().unwrap().beats);
        assert_eq!(beats, turn_beats());

        fs::write(&path, lines(&[claude_lines::prompt("a new file")])).unwrap();
        let polled = follower.poll().unwrap();
        assert!(polled.restarted);
        assert_eq!(polled.beats, [Beat::Prompt, heard("a new file")]);
    }

    #[test]
    fn a_follower_can_start_near_the_end_of_a_long_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut bytes = Vec::new();
        for _ in 0..50 {
            bytes.extend_from_slice(&lines(&turn()));
        }
        let last = lines(&[claude_lines::prompt("the newest prompt")]);
        bytes.extend_from_slice(&last);
        fs::write(&path, &bytes).unwrap();

        let window = last.len() as u64 + 10;
        let mut follower = Follower::new(&path, Format::Claude, Start::Recent(window));
        assert_eq!(
            follower.poll().unwrap().beats,
            [Beat::Prompt, heard("the newest prompt")]
        );
        assert_eq!(follower.tail().malformed(), 0);

        // A window larger than the file reads all of it.
        let mut whole = Follower::new(&path, Format::Claude, Start::Recent(u64::MAX));
        // Nine beats for the first turn, eight for each later one (its token
        // numbers repeat the previous ones), and the last prompt with its
        // words.
        assert_eq!(whole.poll().unwrap().beats.len(), 9 + 49 * 8 + 2);
    }

    #[test]
    fn the_start_offset_is_the_window_before_the_end() {
        assert_eq!(start_offset(1000, Start::Beginning), 0);
        assert_eq!(start_offset(1000, Start::Recent(100)), 900);
        assert_eq!(start_offset(50, Start::Recent(100)), 0);
    }
}
