//! Versioned, length-prefixed frames.
//!
//! ```text
//! +----------------+---------+--------------------------+
//! | length: u32 BE | version | payload: postcard message |
//! +----------------+---------+--------------------------+
//! ```
//!
//! `length` counts the payload only. A frame whose length exceeds
//! [`MAX_FRAME_LEN`] is refused as soon as its header is seen, so a hostile
//! peer cannot make a decoder buffer more than that plus the header.
//!
//! [`encode_frame`] and [`decode_frame`] speak [`PROTOCOL_VERSION`] only, and
//! are what a host uses. A client that must reach an older host says which
//! version a frame is for ([`encode_frame_for`], [`decode_frame_as`]), down
//! to [`MIN_CLIENT_PROTOCOL`]; the older layouts live in this file.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::message::{ExecSpec, Grid, Message, MIN_CLIENT_PROTOCOL, PROTOCOL_VERSION};

/// The largest payload of one frame: a little over two maximal command
/// outputs.
pub const MAX_FRAME_LEN: usize = 10 * 1024 * 1024;
/// The size of the header.
pub const HEADER_LEN: usize = 5;

/// Why a frame could not be read or written.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum FrameError {
    /// More bytes are needed.
    #[error("the frame is incomplete")]
    Incomplete,
    /// The header announces more than [`MAX_FRAME_LEN`].
    #[error("the frame is larger than the limit ({0} bytes)")]
    TooLarge(usize),
    /// The peer speaks another protocol version.
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    /// The payload is not a valid message.
    #[error("malformed message: {0}")]
    Malformed(String),
    /// The message breaks a size limit.
    #[error("message over a limit: {0}")]
    OverLimit(&'static str),
    /// The message cannot be written for the version asked for: it carries
    /// something that only exists from this version on.
    #[error("this needs protocol version {0} on the other side")]
    NeedsVersion(u16),
}

// ----- version 1 --------------------------------------------------------------------------
//
// Version 2 added `stdin` to `ExecSpec`, which `postcard` cannot read around.
// The two messages that carry a spec are therefore mirrored here with the
// layout of version 1; every other message has the same bytes in both.

/// The variant indices of the two messages below, as `postcard` writes them.
const V1_EXEC: u8 = 1;
const V1_PTY_OPEN: u8 = 3;

/// `ExecSpec` as version 1 had it.
#[derive(Serialize, Deserialize)]
struct SpecV1 {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<String>,
}

/// The messages of version 1 that carry a spec, at their variant indices;
/// the unit variants only hold the places of the ones in between.
#[derive(Serialize, Deserialize)]
enum SpecMessageV1 {
    Hello,
    Exec {
        id: u64,
        spec: SpecV1,
        timeout_ms: Option<u32>,
    },
    ExecOutput,
    PtyOpen {
        id: u64,
        spec: SpecV1,
        size: Grid,
        label: String,
    },
}

fn spec_v1(spec: &ExecSpec) -> Result<SpecV1, FrameError> {
    if spec.stdin.is_some() {
        return Err(FrameError::NeedsVersion(2));
    }
    Ok(SpecV1 {
        program: spec.program.clone(),
        args: spec.args.clone(),
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
    })
}

fn spec_from_v1(spec: SpecV1) -> ExecSpec {
    ExecSpec {
        program: spec.program,
        args: spec.args,
        env: spec.env,
        cwd: spec.cwd,
        stdin: None,
    }
}

fn malformed(error: postcard::Error) -> FrameError {
    FrameError::Malformed(error.to_string())
}

fn payload_for(message: &Message, version: u16) -> Result<Vec<u8>, FrameError> {
    if version == PROTOCOL_VERSION {
        return postcard::to_allocvec(message).map_err(malformed);
    }
    let mirrored = match message {
        Message::Exec {
            id,
            spec,
            timeout_ms,
        } => SpecMessageV1::Exec {
            id: *id,
            spec: spec_v1(spec)?,
            timeout_ms: *timeout_ms,
        },
        Message::PtyOpen {
            id,
            spec,
            size,
            label,
        } => SpecMessageV1::PtyOpen {
            id: *id,
            spec: spec_v1(spec)?,
            size: *size,
            label: label.clone(),
        },
        other => return postcard::to_allocvec(other).map_err(malformed),
    };
    postcard::to_allocvec(&mirrored).map_err(malformed)
}

fn message_from(payload: &[u8], version: u16) -> Result<Message, FrameError> {
    if version == PROTOCOL_VERSION || !matches!(payload.first(), Some(&(V1_EXEC | V1_PTY_OPEN))) {
        return postcard::from_bytes(payload).map_err(malformed);
    }
    match postcard::from_bytes(payload).map_err(malformed)? {
        SpecMessageV1::Exec {
            id,
            spec,
            timeout_ms,
        } => Ok(Message::Exec {
            id,
            spec: spec_from_v1(spec),
            timeout_ms,
        }),
        SpecMessageV1::PtyOpen {
            id,
            spec,
            size,
            label,
        } => Ok(Message::PtyOpen {
            id,
            spec: spec_from_v1(spec),
            size,
            label,
        }),
        SpecMessageV1::Hello | SpecMessageV1::ExecOutput => {
            Err(FrameError::Malformed("not a message with a spec".into()))
        }
    }
}

fn spoken(version: u16) -> Result<u8, FrameError> {
    if (MIN_CLIENT_PROTOCOL..=PROTOCOL_VERSION).contains(&version) {
        Ok(version as u8)
    } else {
        Err(FrameError::UnsupportedVersion(version.min(255) as u8))
    }
}

/// Encodes `message` as one frame of this crate's version.
pub fn encode_frame(message: &Message) -> Result<Vec<u8>, FrameError> {
    encode_frame_for(message, PROTOCOL_VERSION)
}

/// Encodes `message` as one frame of `version`, for a client talking to a
/// host of that version. A message that version cannot express is refused
/// with [`FrameError::NeedsVersion`].
pub fn encode_frame_for(message: &Message, version: u16) -> Result<Vec<u8>, FrameError> {
    let tag = spoken(version)?;
    message.validate()?;
    let payload = payload_for(message, version)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.push(tag);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decodes the frame at the start of `bytes`, returning the message and how
/// many bytes it used. Only this crate's version is accepted.
pub fn decode_frame(bytes: &[u8]) -> Result<(Message, usize), FrameError> {
    decode_frame_as(bytes, PROTOCOL_VERSION)
}

/// Decodes the frame at the start of `bytes` as one of `version`: a frame of
/// any other version is refused, the current one included.
pub fn decode_frame_as(bytes: &[u8], version: u16) -> Result<(Message, usize), FrameError> {
    let tag = spoken(version)?;
    let header = bytes.get(..HEADER_LEN).ok_or(FrameError::Incomplete)?;
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if length > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(length));
    }
    if header[4] != tag {
        return Err(FrameError::UnsupportedVersion(header[4]));
    }
    let payload = bytes
        .get(HEADER_LEN..HEADER_LEN + length)
        .ok_or(FrameError::Incomplete)?;
    let message = message_from(payload, version)?;
    message.validate()?;
    Ok((message, HEADER_LEN + length))
}

/// Reads frames from a byte stream that arrives in arbitrary pieces.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    failed: Option<FrameError>,
}

impl FrameDecoder {
    /// An empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds received bytes. After an error the decoder stays failed: the
    /// stream cannot be resynchronised and the connection should be closed.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.failed.is_none() {
            self.buffer.extend_from_slice(bytes);
        }
    }

    /// The next complete message, if there is one.
    pub fn next_message(&mut self) -> Result<Option<Message>, FrameError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        match decode_frame(&self.buffer) {
            Ok((message, used)) => {
                self.buffer.drain(..used);
                Ok(Some(message))
            }
            Err(FrameError::Incomplete) => Ok(None),
            Err(error) => {
                self.failed = Some(error.clone());
                self.buffer = Vec::new();
                Err(error)
            }
        }
    }

    /// How many bytes are waiting for the rest of their frame.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    fn sample() -> Message {
        Message::Ping { nonce: 42 }
    }

    #[test]
    fn a_frame_round_trips_and_reports_its_length() {
        let bytes = encode_frame(&sample()).unwrap();
        let (message, used) = decode_frame(&bytes).unwrap();
        assert_eq!(message, sample());
        assert_eq!(used, bytes.len());
    }

    #[test]
    fn every_truncation_of_a_frame_is_incomplete_not_a_panic() {
        let bytes = encode_frame(&Message::Pong { nonce: 7 }).unwrap();
        for cut in 0..bytes.len() {
            assert_eq!(
                decode_frame(&bytes[..cut]),
                Err(FrameError::Incomplete),
                "{cut}"
            );
        }
    }

    #[test]
    fn an_oversized_header_is_refused_before_any_payload_is_read() {
        let mut bytes = ((MAX_FRAME_LEN + 1) as u32).to_be_bytes().to_vec();
        bytes.push(PROTOCOL_VERSION as u8);
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::TooLarge(MAX_FRAME_LEN + 1))
        );
    }

    #[test]
    fn another_protocol_version_is_refused() {
        let mut bytes = encode_frame(&sample()).unwrap();
        bytes[4] = 99;
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::UnsupportedVersion(99))
        );
    }

    #[test]
    fn two_frames_back_to_back_decode_one_at_a_time() {
        let mut bytes = encode_frame(&sample()).unwrap();
        bytes.extend(encode_frame(&Message::Pong { nonce: 1 }).unwrap());
        let (first, used) = decode_frame(&bytes).unwrap();
        assert_eq!(first, sample());
        let (second, _) = decode_frame(&bytes[used..]).unwrap();
        assert_eq!(second, Message::Pong { nonce: 1 });
    }

    #[test]
    fn the_streaming_decoder_copes_with_one_byte_at_a_time() {
        let mut stream = encode_frame(&sample()).unwrap();
        stream.extend(encode_frame(&Message::Pong { nonce: 5 }).unwrap());
        let mut decoder = FrameDecoder::new();
        let mut got = Vec::new();
        for byte in stream {
            decoder.push(&[byte]);
            while let Some(message) = decoder.next_message().unwrap() {
                got.push(message);
            }
        }
        assert_eq!(got, vec![sample(), Message::Pong { nonce: 5 }]);
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn the_streaming_decoder_stays_failed_after_garbage_and_buffers_nothing_more() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&[0xFF; 5]);
        assert!(matches!(
            decoder.next_message(),
            Err(FrameError::TooLarge(_))
        ));
        decoder.push(&[0; 1000]);
        assert!(decoder.next_message().is_err());
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn a_payload_that_is_not_a_message_is_malformed() {
        let mut bytes = vec![0, 0, 0, 3, PROTOCOL_VERSION as u8, 0xEE, 0xEE, 0xEE];
        assert!(matches!(
            decode_frame(&bytes),
            Err(FrameError::Malformed(_))
        ));
        bytes.truncate(5);
        assert_eq!(decode_frame(&bytes), Err(FrameError::Incomplete));
    }

    /// A tiny deterministic generator, so the randomised tests are repeatable.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    #[test]
    fn random_garbage_never_panics_and_never_decodes_past_its_end() {
        let mut rng = Lcg(0x1eed);
        for _ in 0..4000 {
            let len = (rng.next() % 200) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            if len >= 5 && rng.next() % 2 == 0 {
                // Make the header plausible so the payload decoder is reached.
                let payload = (len - 5) as u32;
                bytes[..4].copy_from_slice(&payload.to_be_bytes());
                bytes[4] = PROTOCOL_VERSION as u8;
            }
            if let Ok((_, used)) = decode_frame(&bytes) {
                assert!(used <= bytes.len());
            }
        }
    }

    #[test]
    fn flipping_any_single_byte_of_a_valid_frame_never_panics() {
        let message = Message::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: "0.1.0".into(),
            device_name: "laptop".into(),
            token: Some(vec![1, 2, 3]),
        };
        let bytes = encode_frame(&message).unwrap();
        for at in 0..bytes.len() {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut copy = bytes.clone();
                copy[at] ^= flip;
                let _ = decode_frame(&copy);
            }
        }
    }
    // ----- version 1 ----------------------------------------------------------------------

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
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

    fn grid() -> Grid {
        Grid {
            cols: 120,
            rows: 40,
            cell_width: 9,
            cell_height: 18,
        }
    }

    /// Every message a client sends, with the frame the 0.5.0 release
    /// (protocol 1) writes for it. The bytes were printed by `encode_frame`
    /// of `leon-wire` at the `v0.5.0` commit, not derived from this file.
    fn golden_v1() -> Vec<(Message, &'static str)> {
        vec![
            (
                Message::Hello {
                    protocol: 1,
                    app_version: "0.5.0".into(),
                    device_name: "phone".into(),
                    token: None,
                },
                "0000000f01000105302e352e300570686f6e6500",
            ),
            (
                Message::Exec {
                    id: 7,
                    spec: spec(),
                    timeout_ms: Some(5000),
                },
                "0000002f0101070367697402067374617475730b2d2d706f7263656c61696e01044c414e47014301082f7372762f617069018827",
            ),
            (
                Message::Exec {
                    id: 300,
                    spec: ExecSpec {
                        program: "sh".into(),
                        ..Default::default()
                    },
                    timeout_ms: None,
                },
                "0000000a0101ac0202736800000000",
            ),
            (
                Message::PtyOpen {
                    id: 9,
                    spec: spec(),
                    size: grid(),
                    label: "git @ /srv/api".into(),
                },
                "0000003f0103090367697402067374617475730b2d2d706f7263656c61696e01044c414e47014301082f7372762f617069782809120e6769742040202f7372762f617069",
            ),
            (
                Message::PtyData {
                    pty: 3,
                    offset: 1024,
                    bytes: vec![104, 105],
                },
                "000000070105038008026869",
            ),
            (
                Message::PtyResize {
                    pty: 3,
                    size: grid(),
                },
                "0000000601060378280912",
            ),
            (Message::PtyClose { pty: 3 }, "00000002010703"),
            (Message::PtyList { id: 11 }, "0000000201090b"),
            (
                Message::PtyAttach {
                    id: 12,
                    pty: 3,
                    from_offset: 77,
                },
                "00000004010b0c034d",
            ),
            (Message::Ping { nonce: 5 }, "00000002010d05"),
            (Message::ShareState { id: 13 }, "00000002010f0d"),
            (
                Message::ShareTranscript {
                    id: 14,
                    agent: "claude".into(),
                    external_id: "abc".into(),
                },
                "0000000d01110e06636c6175646503616263",
            ),
        ]
    }

    #[test]
    fn what_a_client_sends_as_version_1_is_what_the_previous_release_wrote() {
        for (message, frame) in golden_v1() {
            let bytes = encode_frame_for(&message, 1).unwrap();
            assert_eq!(bytes, unhex(frame), "{message:?}");
            // And it reads back as the same message.
            let (back, used) = decode_frame_as(&bytes, 1).unwrap();
            assert_eq!(back, message);
            assert_eq!(used, bytes.len());
        }
    }

    #[test]
    fn a_spec_with_standard_input_cannot_be_written_as_version_1() {
        let mut with_input = spec();
        with_input.stdin = Some(b"y\n".to_vec());
        let exec = Message::Exec {
            id: 1,
            spec: with_input.clone(),
            timeout_ms: None,
        };
        assert_eq!(encode_frame_for(&exec, 1), Err(FrameError::NeedsVersion(2)));
        let open = Message::PtyOpen {
            id: 1,
            spec: with_input,
            size: grid(),
            label: String::new(),
        };
        assert_eq!(encode_frame_for(&open, 1), Err(FrameError::NeedsVersion(2)));
        assert!(encode_frame_for(&exec, 2).is_ok());
    }

    #[test]
    fn what_a_host_sends_has_the_same_payload_in_both_versions() {
        use crate::message::{ErrorCode, ExecOutput, Exit, PtyInfo, WireError};
        use crate::message::{SharedEntry, SharedProject, SharedSession, SharedTranscript};
        let from_host = vec![
            Message::Hello {
                protocol: 1,
                app_version: "0.5.0".into(),
                device_name: "desk".into(),
                token: None,
            },
            Message::ExecOutput {
                id: 1,
                output: ExecOutput {
                    status: Some(0),
                    stdout: b"ok".to_vec(),
                    stderr: Vec::new(),
                    truncated: false,
                },
            },
            Message::PtyOpened { id: 2, pty: 9 },
            Message::PtyData {
                pty: 9,
                offset: 4,
                bytes: vec![1, 2, 3],
            },
            Message::PtyExited {
                pty: 9,
                exit: Exit {
                    code: 1,
                    signal: Some("TERM".into()),
                },
            },
            Message::PtyListing {
                id: 3,
                ptys: vec![PtyInfo {
                    pty: 9,
                    label: "sh".into(),
                    program: "sh".into(),
                    cwd: None,
                    size: grid(),
                    started_unix: 5,
                    exit: None,
                    first_offset: 0,
                    end_offset: 7,
                    attached: 1,
                }],
            },
            Message::PtyAttached {
                id: 4,
                pty: 9,
                replay_from: 0,
                gap: true,
                end_offset: 7,
                size: grid(),
                exit: None,
            },
            Message::Pong { nonce: 5 },
            Message::ShareStateData {
                id: 6,
                projects: vec![SharedProject {
                    name: "leon".into(),
                    root: "/code/leon".into(),
                }],
                sessions: vec![SharedSession {
                    agent: "claude".into(),
                    external_id: "abc".into(),
                    cwd: "/code/leon".into(),
                    title: "t".into(),
                    model: None,
                    started_ms: 1,
                    updated_ms: 2,
                    messages: 3,
                }],
                truncated: false,
                pinned: vec![],
                den: false,
                king: false,
            },
            Message::ShareTranscriptData {
                id: 7,
                transcript: Some(SharedTranscript {
                    agent: "claude".into(),
                    external_id: "abc".into(),
                    messages: vec![SharedEntry {
                        role: "user".into(),
                        text: "hi".into(),
                        at_ms: 1,
                    }],
                }),
            },
            Message::Error {
                id: Some(8),
                error: WireError {
                    code: ErrorCode::NotFound,
                    message: "gone".into(),
                },
            },
        ];
        for message in from_host {
            let new = encode_frame_for(&message, 2).unwrap();
            let old = encode_frame_for(&message, 1).unwrap();
            assert_eq!(new[4], 2);
            assert_eq!(old[4], 1);
            assert_eq!(new[HEADER_LEN..], old[HEADER_LEN..], "{message:?}");
            assert_eq!(decode_frame_as(&old, 1).unwrap().0, message);
            assert_eq!(decode_frame_as(&new, 2).unwrap().0, message);
        }
    }

    #[test]
    fn a_frame_is_only_read_as_the_version_asked_for() {
        let old = encode_frame_for(&Message::Pong { nonce: 7 }, 1).unwrap();
        let new = encode_frame_for(&Message::Pong { nonce: 7 }, 2).unwrap();
        // The strict reader, which is the host's, takes the current one only.
        assert_eq!(decode_frame(&old), Err(FrameError::UnsupportedVersion(1)));
        assert_eq!(
            decode_frame_as(&new, 1),
            Err(FrameError::UnsupportedVersion(2))
        );
        assert_eq!(
            decode_frame_as(&old, 2),
            Err(FrameError::UnsupportedVersion(1))
        );
    }

    #[test]
    fn versions_outside_the_supported_range_are_refused_both_ways() {
        let message = Message::Ping { nonce: 1 };
        let frame = encode_frame(&message).unwrap();
        for version in [0u16, 3, 300] {
            assert!(matches!(
                encode_frame_for(&message, version),
                Err(FrameError::UnsupportedVersion(_))
            ));
            assert!(matches!(
                decode_frame_as(&frame, version),
                Err(FrameError::UnsupportedVersion(_))
            ));
        }
    }

    #[test]
    fn a_truncated_or_mangled_version_1_spec_message_never_panics() {
        let (message, _) = golden_v1().remove(3);
        let bytes = encode_frame_for(&message, 1).unwrap();
        for cut in 0..bytes.len() {
            assert!(decode_frame_as(&bytes[..cut], 1).is_err());
        }
        for index in HEADER_LEN..bytes.len() {
            let mut mangled = bytes.clone();
            mangled[index] ^= 0xff;
            let _ = decode_frame_as(&mangled, 1);
        }
    }
}
