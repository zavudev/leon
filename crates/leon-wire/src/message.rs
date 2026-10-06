//! The messages between a Leon client and a Leon host.
//!
//! Leon drives a machine through two primitives: run a command, and an
//! interactive terminal. A host offers exactly those, and everything above
//! them (git, probing, agents) keeps working unchanged.
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
///
/// 2 added standard input to [`ExecSpec`]: a 1 client and a 2 host (or the
/// other way round) meet, compare versions and refuse each other, rather than
/// misreading each other's frames.
pub const PROTOCOL_VERSION: u16 = 2;
/// The most terminal bytes one [`Message::PtyData`] carries.
pub const MAX_PTY_CHUNK: usize = 32 * 1024;
/// The most bytes one [`ExecSpec::stdin`] carries.
pub const MAX_EXEC_INPUT: usize = 4 * 1024 * 1024;
/// The most bytes kept of a command's output, per stream; the rest is
/// dropped and `truncated` is set.
pub const MAX_EXEC_OUTPUT: usize = 4 * 1024 * 1024;
/// How many bytes of recent output a host keeps for each terminal.
pub const REPLAY_BUFFER_BYTES: usize = 2 * 1024 * 1024;

const MAX_STRING: usize = 64 * 1024;
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
    /// Bytes written to the program's standard input, up to
    /// [`MAX_EXEC_INPUT`], and then closed. `None` closes it at once, so a
    /// command that asks a question sees end of file and fails instead of
    /// hanging. A terminal's spec ([`Message::PtyOpen`]) ignores this: input
    /// reaches a terminal through [`Message::PtyData`].
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
    /// A request failed.
    Error {
        /// The request it answers, when there was one.
        id: Option<u64>,
        /// What went wrong.
        error: WireError,
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
                .is_some_and(|bytes| bytes.len() > MAX_EXEC_INPUT)
            {
                return Err(FrameError::OverLimit("standard input"));
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
            Message::PtyListing { ptys, .. } if ptys.len() > MAX_ITEMS => {
                Err(FrameError::OverLimit("terminal list"))
            }
            Message::Error { error, .. } if error.message.len() > MAX_STRING => {
                Err(FrameError::OverLimit("error text"))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{decode_frame, encode_frame};

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
            stdin: Some(b"a line\n".to_vec()),
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
                protocol: 1,
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
    fn an_exec_with_more_standard_input_than_the_limit_is_refused() {
        let mut spec = spec();
        spec.stdin = Some(vec![0; MAX_EXEC_INPUT + 1]);
        let message = Message::Exec {
            id: 1,
            spec,
            timeout_ms: None,
        };
        assert!(encode_frame(&message).is_err());
    }
}
