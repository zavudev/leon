//! The messages between a Leon client and a Leon host.
//!
//! Leon drives a machine through two primitives: run a command, and an
//! interactive terminal. A host offers exactly those, and everything above
//! them (git, probing, agents) keeps working unchanged.
//!
//! A host may also speak about its own Leon: the projects it holds and the
//! sessions of its unified history (`ShareState`, `ShareTranscript`). That is
//! how a paired device mirrors this machine's sidebar in its own; it is
//! sharing that adds no rights, because a paired device already holds a full
//! terminal as the host's user.
//!
//! Requests carry an `id` the answer repeats. Terminal output flows as
//! [`Message::PtyData`] with a monotonically increasing byte `offset`, which
//! is what makes re-attaching exact: a client that saw bytes up to offset
//! `n` asks to continue `from_offset: n` and receives each later byte exactly
//! once, or is told ([`Message::PtyAttached`]'s `gap`) that the host's bounded
//! buffer no longer reaches back that far.

use serde::{Deserialize, Serialize};

use crate::frame::FrameError;

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u16 = 2;
/// The most terminal bytes one [`Message::PtyData`] carries.
pub const MAX_PTY_CHUNK: usize = 32 * 1024;
/// The most bytes kept of a command's output, per stream; the rest is
/// dropped and `truncated` is set.
pub const MAX_EXEC_OUTPUT: usize = 4 * 1024 * 1024;
/// The most bytes of standard input one [`Message::Exec`] carries.
pub const MAX_EXEC_INPUT: usize = 4 * 1024 * 1024;
/// How many bytes of recent output a host keeps for each terminal.
pub const REPLAY_BUFFER_BYTES: usize = 2 * 1024 * 1024;
/// How many history sessions a host offers at most in one
/// [`Message::ShareStateData`]: the newest ones, older ones left out with
/// `truncated` set.
pub const SHARE_SESSIONS_LIMIT: usize = 500;

const MAX_STRING: usize = 64 * 1024;
/// The longest name of a program a [`Message::PtyProbed`] carries.
const MAX_COMMAND: usize = 256;
const MAX_ITEMS: usize = 4096;

/// A command to run, written from the point of view of the host.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecSpec {
    /// Program name or path.
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// Working directory.
    pub cwd: Option<String>,
    /// Standard input for [`Message::Exec`] (at most [`MAX_EXEC_INPUT`]
    /// bytes), closed after it is written; a command without any gets none.
    /// A terminal ([`Message::PtyOpen`]) ignores it.
    pub stdin: Option<Vec<u8>>,
}

/// A program to run in a terminal.
pub type SpawnSpec = ExecSpec;

/// The size of a terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Cell width in pixels.
    pub cell_width: u16,
    /// Cell height in pixels.
    pub cell_height: u16,
}

/// How a terminal's program ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    /// Exit code.
    pub code: u32,
    /// Signal name, when a signal ended it.
    pub signal: Option<String>,
}

/// What a finished command produced.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecOutput {
    /// Exit status; absent when a signal ended it.
    pub status: Option<i32>,
    /// Standard output (at most [`MAX_EXEC_OUTPUT`] bytes).
    pub stdout: Vec<u8>,
    /// Standard error (at most [`MAX_EXEC_OUTPUT`] bytes).
    pub stderr: Vec<u8>,
    /// Whether either stream was cut at the limit.
    pub truncated: bool,
}

/// What a host says about one of its terminals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyInfo {
    /// The terminal's id on the host.
    pub pty: u64,
    /// What the client called it when it opened it.
    pub label: String,
    /// The program it runs.
    pub program: String,
    /// Its working directory.
    pub cwd: Option<String>,
    /// Its current size.
    pub size: Grid,
    /// Seconds since the Unix epoch when it started.
    pub started_unix: u64,
    /// How it ended; absent while it runs.
    pub exit: Option<Exit>,
    /// The offset of the oldest byte still buffered.
    pub first_offset: u64,
    /// The offset one past the newest byte.
    pub end_offset: u64,
    /// How many clients are attached.
    pub attached: u32,
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// The request is not understood or breaks a limit.
    BadRequest,
    /// The peer does not speak this protocol version.
    Unsupported,
    /// No such terminal.
    NotFound,
    /// Too large.
    TooLarge,
    /// A command ran out of time.
    Timeout,
    /// A program could not be started.
    SpawnFailed,
    /// Too many of something.
    Busy,
    /// Not allowed.
    Forbidden,
    /// The host failed.
    Internal,
}

/// A typed error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    /// What kind of failure.
    pub code: ErrorCode,
    /// A sentence for a person; never carries secrets.
    pub message: String,
}

// ----- what a host's own Leon shares ---------------------------------------------------

/// A project of the host's own Leon, offered to a paired device. A paired
/// device already holds a full terminal as the host's user, so this adds no
/// exposure: it is what it could read with that terminal, handed over well.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedProject {
    /// The name the host shows it under.
    pub name: String,
    /// Absolute path of its root on the host.
    pub root: String,
}

/// One session of the host's unified history, without its transcript. The
/// pair `(agent, external_id)` names it on the host, and its transcript is
/// asked for by those two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedSession {
    /// The agent that ran it: its catalogue tag, such as `claude`.
    pub agent: String,
    /// The agent's own id of the session; what its `resume` accepts.
    pub external_id: String,
    /// Working directory of the session on the host.
    pub cwd: String,
    /// Short human-readable title.
    pub title: String,
    /// Model name reported by the agent, when known.
    pub model: Option<String>,
    /// Time of the first message: milliseconds since the Unix epoch.
    pub started_ms: i64,
    /// Time of the latest message: milliseconds since the Unix epoch.
    pub updated_ms: i64,
    /// How many messages the session holds on the host.
    pub messages: u32,
}

/// One entry of a shared transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedEntry {
    /// Who produced it: the role tag the messages use — `user`, `assistant`,
    /// `tool` or `system`.
    pub role: String,
    /// Plain text.
    pub text: String,
    /// When it was produced: milliseconds since the Unix epoch.
    pub at_ms: i64,
}

/// The transcript of one session, sent on demand because transcripts are the
/// bulky part of sharing. `(agent, external_id)` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedTranscript {
    /// The agent that ran the session.
    pub agent: String,
    /// The agent's own id of the session.
    pub external_id: String,
    /// The transcript, in order.
    pub messages: Vec<SharedEntry>,
}

/// One message of the protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// The first message each way after the secure channel is up.
    Hello {
        /// The protocol version spoken.
        protocol: u16,
        /// The application's version.
        app_version: String,
        /// The device's display name.
        device_name: String,
        /// An opaque optional token a relay operator may later require
        /// (account or plan). Hosts ignore it.
        token: Option<Vec<u8>>,
    },
    /// Runs a command.
    Exec {
        /// Request id.
        id: u64,
        /// What to run.
        spec: ExecSpec,
        /// Stop it after this many milliseconds.
        timeout_ms: Option<u32>,
    },
    /// The result of an [`Message::Exec`].
    ExecOutput {
        /// The request id.
        id: u64,
        /// The result.
        output: ExecOutput,
    },
    /// Opens a terminal.
    PtyOpen {
        /// Request id.
        id: u64,
        /// What to run in it.
        spec: SpawnSpec,
        /// Its size.
        size: Grid,
        /// A label the client can find it by later.
        label: String,
    },
    /// A terminal was opened (and the opener is attached to it).
    PtyOpened {
        /// The request id.
        id: u64,
        /// The terminal.
        pty: u64,
    },
    /// Terminal bytes. Host to client: `offset` is the stream offset of the
    /// first byte. Client to host: keystrokes; `offset` is ignored.
    PtyData {
        /// The terminal.
        pty: u64,
        /// Stream offset.
        offset: u64,
        /// The bytes (at most [`MAX_PTY_CHUNK`]).
        bytes: Vec<u8>,
    },
    /// Resizes a terminal.
    PtyResize {
        /// The terminal.
        pty: u64,
        /// The new size.
        size: Grid,
    },
    /// Closes a terminal: its program is hung up.
    PtyClose {
        /// The terminal.
        pty: u64,
    },
    /// A terminal's program ended.
    PtyExited {
        /// The terminal.
        pty: u64,
        /// How it ended.
        exit: Exit,
    },
    /// Asks for the host's terminals.
    PtyList {
        /// Request id.
        id: u64,
    },
    /// The host's terminals.
    PtyListing {
        /// The request id.
        id: u64,
        /// The terminals, oldest first.
        ptys: Vec<PtyInfo>,
    },
    /// Attaches to a running terminal and continues its output.
    PtyAttach {
        /// Request id.
        id: u64,
        /// The terminal.
        pty: u64,
        /// The first offset the client has not seen.
        from_offset: u64,
    },
    /// An attach succeeded. Output follows as [`Message::PtyData`] from
    /// `replay_from`.
    PtyAttached {
        /// The request id.
        id: u64,
        /// The terminal.
        pty: u64,
        /// Where the replay starts (`from_offset`, or later after a gap).
        replay_from: u64,
        /// The client asked for bytes the host no longer has.
        gap: bool,
        /// The offset one past the newest byte at this moment.
        end_offset: u64,
        /// The terminal's current size.
        size: Grid,
        /// How it ended, if it has.
        exit: Option<Exit>,
    },
    /// A liveness probe.
    Ping {
        /// Echoed in the [`Message::Pong`].
        nonce: u64,
    },
    /// The answer to a [`Message::Ping`].
    Pong {
        /// The probe's nonce.
        nonce: u64,
    },
    /// Asks for the host's own Leon: what projects it holds and what sessions
    /// its unified history holds, both for the computer the host runs on.
    /// Hosts that share nothing answer with a [`Message::ShareStateData`]
    /// that holds nothing.
    ShareState {
        /// Request id.
        id: u64,
    },
    /// The host's projects and history sessions, newest first. Older sessions
    /// past [`SHARE_SESSIONS_LIMIT`] are left out and `truncated` says so.
    ShareStateData {
        /// The request id.
        id: u64,
        /// The host's projects, in the order the host shows them.
        projects: Vec<SharedProject>,
        /// The host's history sessions, newest first.
        sessions: Vec<SharedSession>,
        /// Whether `sessions` was cut at [`SHARE_SESSIONS_LIMIT`].
        truncated: bool,
    },
    /// Asks for one history session's transcript.
    ShareTranscript {
        /// Request id.
        id: u64,
        /// The agent that ran the session.
        agent: String,
        /// The agent's own id of the session.
        external_id: String,
    },
    /// The transcript, or `None` when the host holds no session by that
    /// `(agent, external_id)` any more.
    ShareTranscriptData {
        /// The request id.
        id: u64,
        /// What was asked for.
        transcript: Option<SharedTranscript>,
    },
    /// A request failed.
    Error {
        /// The request it answers, when there was one.
        id: Option<u64>,
        /// What went wrong.
        error: WireError,
    },
    /// Asks who is in front of a terminal. Only the keeper of durable local
    /// sessions answers it (a client that holds the terminal's own
    /// pseudo-terminal can look for itself; one that does not cannot).
    /// Variants are only ever appended, so every message above keeps its
    /// encoding and the protocol version stays what it was: a peer that does
    /// not know this one drops the connection, so it is only sent to a peer
    /// known to be a keeper.
    PtyProbe {
        /// The terminal.
        pty: u64,
    },
    /// Who is in front of a terminal.
    PtyProbed {
        /// The terminal.
        pty: u64,
        /// The pid of the terminal's own process (the shell), when the system
        /// has one.
        pid: Option<u32>,
        /// Whether the shell, and not a program it started, leads the
        /// terminal's foreground process group; absent where the system
        /// cannot say.
        shell_in_front: Option<bool>,
        /// The program in front of the shell, where the system names it.
        command: Option<String>,
    },
    /// Asks the program in front of a terminal's shell to end: SIGTERM to
    /// the terminal's foreground process group. Nothing is sent when the
    /// shell itself is in front. No answer.
    PtyTerminate {
        /// The terminal.
        pty: u64,
    },
}

impl Message {
    /// Checks the sizes the protocol bounds. Called when encoding and when
    /// decoding, so neither side ever holds a message that breaks them.
    pub fn validate(&self) -> Result<(), FrameError> {
        fn spec(spec: &ExecSpec) -> Result<(), FrameError> {
            let strings = std::iter::once(&spec.program)
                .chain(spec.args.iter())
                .chain(spec.cwd.iter())
                .chain(spec.env.iter().flat_map(|(a, b)| [a, b]));
            for text in strings {
                if text.len() > MAX_STRING {
                    return Err(FrameError::OverLimit("string"));
                }
            }
            if spec.args.len() > MAX_ITEMS || spec.env.len() > MAX_ITEMS {
                return Err(FrameError::OverLimit("too many arguments"));
            }
            if spec
                .stdin
                .as_ref()
                .is_some_and(|input| input.len() > MAX_EXEC_INPUT)
            {
                return Err(FrameError::OverLimit("command input"));
            }
            Ok(())
        }
        match self {
            Message::PtyData { bytes, .. } if bytes.len() > MAX_PTY_CHUNK => {
                Err(FrameError::OverLimit("terminal chunk"))
            }
            Message::ExecOutput { output, .. }
                if output.stdout.len() > MAX_EXEC_OUTPUT
                    || output.stderr.len() > MAX_EXEC_OUTPUT =>
            {
                Err(FrameError::OverLimit("command output"))
            }
            Message::PtyOpen { label, .. } if label.len() > 1024 => {
                Err(FrameError::OverLimit("label"))
            }
            Message::Exec { spec: s, .. } | Message::PtyOpen { spec: s, .. } => spec(s),
            Message::Hello {
                app_version,
                device_name,
                token,
                ..
            } => {
                if app_version.len() > 256
                    || device_name.len() > 256
                    || token.as_ref().is_some_and(|t| t.len() > 4096)
                {
                    Err(FrameError::OverLimit("hello"))
                } else {
                    Ok(())
                }
            }
            Message::PtyProbed {
                command: Some(command),
                ..
            } if command.len() > MAX_COMMAND => Err(FrameError::OverLimit("program name")),
            Message::PtyListing { ptys, .. } if ptys.len() > MAX_ITEMS => {
                Err(FrameError::OverLimit("terminal list"))
            }
            Message::ShareStateData {
                projects, sessions, ..
            } => {
                for (label, count) in [("projects", projects.len()), ("sessions", sessions.len())] {
                    if count > MAX_ITEMS {
                        return Err(FrameError::OverLimit(label));
                    }
                }
                for project in projects {
                    for text in [&project.name, &project.root] {
                        if text.len() > MAX_STRING {
                            return Err(FrameError::OverLimit("shared project"));
                        }
                    }
                }
                shared_sessions(sessions)
            }
            Message::ShareTranscript {
                agent, external_id, ..
            } => {
                if agent.len() > MAX_STRING || external_id.len() > MAX_STRING {
                    Err(FrameError::OverLimit("shared session request"))
                } else {
                    Ok(())
                }
            }
            Message::ShareTranscriptData { transcript, .. } => {
                let Some(transcript) = transcript else {
                    return Ok(());
                };
                if transcript.messages.len() > MAX_ITEMS {
                    return Err(FrameError::OverLimit("transcript"));
                }
                bounded(&transcript.agent)?;
                bounded(&transcript.external_id)?;
                for entry in &transcript.messages {
                    bounded(&entry.role)?;
                    bounded(&entry.text)?;
                }
                Ok(())
            }
            Message::Error { error, .. } if error.message.len() > MAX_STRING => {
                Err(FrameError::OverLimit("error text"))
            }
            _ => Ok(()),
        }
    }
}

/// Whether one bounded string is short enough.
fn bounded(text: &str) -> Result<(), FrameError> {
    if text.len() > MAX_STRING {
        Err(FrameError::OverLimit("shared"))
    } else {
        Ok(())
    }
}

/// The bounds every shared session's fields must keep.
fn shared_sessions(sessions: &[SharedSession]) -> Result<(), FrameError> {
    for session in sessions {
        for text in [
            &session.agent,
            &session.external_id,
            &session.cwd,
            &session.title,
        ] {
            if text.len() > MAX_STRING {
                return Err(FrameError::OverLimit("shared session"));
            }
        }
        if let Some(model) = &session.model {
            if model.len() > MAX_STRING {
                return Err(FrameError::OverLimit("shared session"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{decode_frame, encode_frame, HEADER_LEN};

    fn grid() -> Grid {
        Grid {
            cols: 120,
            rows: 40,
            cell_width: 8,
            cell_height: 16,
        }
    }

    fn spec() -> ExecSpec {
        ExecSpec {
            program: "git".into(),
            args: vec!["status".into(), "--porcelain".into()],
            env: vec![("LANG".into(), "C".into())],
            cwd: Some("/srv/api".into()),
            stdin: None,
        }
    }

    fn exit() -> Exit {
        Exit {
            code: 1,
            signal: Some("Hangup".into()),
        }
    }

    fn info() -> PtyInfo {
        PtyInfo {
            pty: 3,
            label: "agent".into(),
            program: "claude".into(),
            cwd: None,
            size: grid(),
            started_unix: 1_700_000_000,
            exit: Some(exit()),
            first_offset: 10,
            end_offset: 99,
            attached: 2,
        }
    }

    fn shared_session() -> SharedSession {
        SharedSession {
            agent: "claude".into(),
            external_id: "a1b2".into(),
            cwd: "/srv/api".into(),
            title: "fix the login bug".into(),
            model: Some("claude-opus".into()),
            started_ms: 1_700_000_000_000,
            updated_ms: 1_700_000_100_000,
            messages: 12,
        }
    }

    /// One of every message, with edge-case values.
    fn every_message() -> Vec<Message> {
        vec![
            Message::Hello {
                protocol: PROTOCOL_VERSION,
                app_version: "0.1.0".into(),
                device_name: "Ana's laptop".into(),
                token: None,
            },
            Message::Hello {
                protocol: PROTOCOL_VERSION,
                app_version: String::new(),
                device_name: "é日本".into(),
                token: Some(vec![0, 255]),
            },
            Message::Exec {
                id: u64::MAX,
                spec: spec(),
                timeout_ms: Some(30_000),
            },
            Message::Exec {
                id: 0,
                spec: ExecSpec::default(),
                timeout_ms: None,
            },
            Message::ExecOutput {
                id: 1,
                output: ExecOutput {
                    status: Some(-1),
                    stdout: vec![0, 1, 2, 255],
                    stderr: b"err".to_vec(),
                    truncated: true,
                },
            },
            Message::PtyOpen {
                id: 2,
                spec: spec(),
                size: grid(),
                label: "shell".into(),
            },
            Message::PtyOpened { id: 2, pty: 9 },
            Message::PtyData {
                pty: 9,
                offset: u64::MAX,
                bytes: vec![0x1b, b'[', b'0', b'm'],
            },
            Message::PtyData {
                pty: 0,
                offset: 0,
                bytes: Vec::new(),
            },
            Message::PtyResize {
                pty: 9,
                size: grid(),
            },
            Message::PtyClose { pty: 9 },
            Message::PtyExited {
                pty: 9,
                exit: exit(),
            },
            Message::PtyList { id: 4 },
            Message::PtyListing {
                id: 4,
                ptys: vec![info(), info()],
            },
            Message::PtyAttach {
                id: 5,
                pty: 9,
                from_offset: 1234,
            },
            Message::PtyAttached {
                id: 5,
                pty: 9,
                replay_from: 1000,
                gap: true,
                end_offset: 5000,
                size: grid(),
                exit: None,
            },
            Message::Ping { nonce: 1 },
            Message::Pong { nonce: 1 },
            Message::ShareState { id: 7 },
            Message::ShareStateData {
                id: 7,
                projects: vec![SharedProject {
                    name: "api".into(),
                    root: "/srv/api".into(),
                }],
                sessions: vec![shared_session()],
                truncated: false,
            },
            Message::ShareTranscript {
                id: 8,
                agent: "claude".into(),
                external_id: "abc".into(),
            },
            Message::ShareTranscriptData {
                id: 8,
                transcript: Some(SharedTranscript {
                    agent: "claude".into(),
                    external_id: "abc".into(),
                    messages: vec![SharedEntry {
                        role: "user".into(),
                        text: "hello".into(),
                        at_ms: 1_700_000_000_000,
                    }],
                }),
            },
            Message::ShareTranscriptData {
                id: 8,
                transcript: None,
            },
            Message::PtyProbe { pty: 9 },
            Message::PtyProbed {
                pty: 9,
                pid: Some(4242),
                shell_in_front: Some(false),
                command: Some("claude".into()),
            },
            Message::PtyProbed {
                pty: 0,
                pid: None,
                shell_in_front: None,
                command: None,
            },
            Message::PtyTerminate { pty: u64::MAX },
            Message::Error {
                id: Some(3),
                error: WireError {
                    code: ErrorCode::Forbidden,
                    message: "no".into(),
                },
            },
            Message::Error {
                id: None,
                error: WireError {
                    code: ErrorCode::Internal,
                    message: String::new(),
                },
            },
        ]
    }

    #[test]
    fn every_message_round_trips_through_a_frame() {
        for message in every_message() {
            let bytes = encode_frame(&message).unwrap();
            let (back, used) = decode_frame(&bytes).unwrap();
            assert_eq!(back, message);
            assert_eq!(used, bytes.len());
        }
    }

    #[test]
    fn every_truncation_of_every_message_is_refused_without_a_panic() {
        for message in every_message() {
            let bytes = encode_frame(&message).unwrap();
            for cut in 0..bytes.len() {
                assert!(decode_frame(&bytes[..cut]).is_err(), "{message:?} at {cut}");
            }
        }
    }

    /// The index of a variant is its encoding. An old peer reads these as it
    /// always did, so none may move: new variants go at the end.
    #[test]
    fn the_messages_that_existed_keep_their_encoding_and_new_ones_come_after() {
        let tag = |message: &Message| {
            let bytes = encode_frame(message).unwrap();
            bytes[HEADER_LEN]
        };
        assert_eq!(tag(&Message::Ping { nonce: 1 }), 13);
        assert_eq!(tag(&Message::Pong { nonce: 1 }), 14);
        assert_eq!(tag(&Message::PtyClose { pty: 1 }), 7);
        assert_eq!(
            tag(&Message::Error {
                id: None,
                error: WireError {
                    code: ErrorCode::Internal,
                    message: String::new()
                }
            }),
            19
        );
        assert_eq!(tag(&Message::PtyProbe { pty: 1 }), 20);
        assert_eq!(
            tag(&Message::PtyProbed {
                pty: 1,
                pid: None,
                shell_in_front: None,
                command: None
            }),
            21
        );
        assert_eq!(tag(&Message::PtyTerminate { pty: 1 }), 22);
        assert_eq!(PROTOCOL_VERSION, 2, "the new messages need no new version");
    }

    #[test]
    fn a_program_name_over_the_limit_is_refused_both_ways() {
        let long = Message::PtyProbed {
            pty: 1,
            pid: Some(1),
            shell_in_front: Some(false),
            command: Some("x".repeat(MAX_COMMAND + 1)),
        };
        assert!(encode_frame(&long).is_err());
        let payload = postcard::to_allocvec(&long).unwrap();
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.push(PROTOCOL_VERSION as u8);
        frame.extend(payload);
        assert!(decode_frame(&frame).is_err());
        // At the limit it is fine.
        let at_limit = Message::PtyProbed {
            pty: 1,
            pid: Some(1),
            shell_in_front: Some(false),
            command: Some("x".repeat(MAX_COMMAND)),
        };
        assert!(encode_frame(&at_limit).is_ok());
    }

    #[test]
    fn a_terminal_chunk_over_the_limit_is_refused_both_ways() {
        let big = Message::PtyData {
            pty: 1,
            offset: 0,
            bytes: vec![0; MAX_PTY_CHUNK + 1],
        };
        assert!(encode_frame(&big).is_err());
        // A peer that skips the check on its side is caught on ours.
        let payload = postcard::to_allocvec(&big).unwrap();
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.push(PROTOCOL_VERSION as u8);
        frame.extend(payload);
        assert!(decode_frame(&frame).is_err());
    }

    #[test]
    fn an_absurd_claimed_length_inside_a_valid_header_does_not_allocate_it() {
        // A Vec<u8> claiming 4 GiB with three bytes behind it.
        let mut payload = vec![5u8]; // PtyData variant tag
        payload.push(1); // pty
        payload.push(0); // offset
        payload.extend([0xFF, 0xFF, 0xFF, 0xFF, 0x0F]); // length: ~4 GiB
        payload.extend([1, 2, 3]);
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.push(PROTOCOL_VERSION as u8);
        frame.extend(payload);
        assert!(decode_frame(&frame).is_err());
    }

    #[test]
    fn an_exec_with_too_many_arguments_is_refused() {
        let mut spec = spec();
        spec.args = vec!["x".into(); 5000];
        let message = Message::Exec {
            id: 1,
            spec,
            timeout_ms: None,
        };
        assert!(encode_frame(&message).is_err());
    }

    #[test]
    fn an_exec_carrying_standard_input_round_trips() {
        let mut spec = spec();
        spec.stdin = Some(b"line one\nline two\n".to_vec());
        let message = Message::Exec {
            id: 1,
            spec,
            timeout_ms: Some(5000),
        };
        let bytes = encode_frame(&message).unwrap();
        let (back, used) = decode_frame(&bytes).unwrap();
        assert_eq!(back, message);
        assert_eq!(used, bytes.len());
    }

    #[test]
    fn standard_input_over_the_limit_is_refused_both_ways() {
        let mut spec = spec();
        spec.stdin = Some(vec![0; MAX_EXEC_INPUT]);
        let at_limit = Message::Exec {
            id: 1,
            spec: spec.clone(),
            timeout_ms: None,
        };
        assert!(encode_frame(&at_limit).is_ok());
        spec.stdin = Some(vec![0; MAX_EXEC_INPUT + 1]);
        let big = Message::Exec {
            id: 1,
            spec,
            timeout_ms: None,
        };
        assert!(encode_frame(&big).is_err());
        // A peer that skips the check on its side is caught on ours.
        let payload = postcard::to_allocvec(&big).unwrap();
        let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
        frame.push(PROTOCOL_VERSION as u8);
        frame.extend(payload);
        assert!(decode_frame(&frame).is_err());
    }

    #[test]
    fn a_frame_from_the_previous_protocol_version_is_refused() {
        let mut bytes = encode_frame(&Message::Pong { nonce: 7 }).unwrap();
        bytes[4] = 1;
        assert_eq!(decode_frame(&bytes), Err(FrameError::UnsupportedVersion(1)));
    }

    #[test]
    fn a_shared_state_over_the_limits_is_refused() {
        let one_project = SharedProject {
            name: "p".into(),
            root: "/p".into(),
        };
        let huge = Message::ShareStateData {
            id: 1,
            projects: vec![one_project; MAX_ITEMS + 1],
            sessions: vec![],
            truncated: false,
        };
        assert!(encode_frame(&huge).is_err());
        let mut session = shared_session();
        session.title = "x".repeat(MAX_STRING + 1);
        session.model = None;
        let wide = Message::ShareStateData {
            id: 1,
            projects: vec![],
            sessions: vec![session],
            truncated: false,
        };
        assert!(encode_frame(&wide).is_err());
    }

    #[test]
    fn a_transcript_over_the_limits_is_refused() {
        let mut transcript = SharedTranscript {
            agent: "claude".into(),
            external_id: "abc".into(),
            messages: vec![
                SharedEntry {
                    role: "user".into(),
                    text: String::new(),
                    at_ms: 0,
                };
                MAX_ITEMS + 1
            ],
        };
        assert!(encode_frame(&Message::ShareTranscriptData {
            id: 1,
            transcript: Some(transcript.clone()),
        })
        .is_err());
        transcript.messages.truncate(256);
        transcript.messages[0].text = "x".repeat(MAX_STRING + 1);
        assert!(encode_frame(&Message::ShareTranscriptData {
            id: 1,
            transcript: Some(transcript),
        })
        .is_err());
    }
}
