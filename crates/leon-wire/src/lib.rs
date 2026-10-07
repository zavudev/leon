//! The wire protocol Leon speaks between computers.
//!
//! This crate is pure: no I/O, no async, no cryptography beyond one hash. It
//! defines
//!
//! * [`frame`]: versioned, length-prefixed frames and a streaming decoder
//!   that never buffers more than [`MAX_FRAME_LEN`];
//! * [`message`]: the application messages between a Leon client and a Leon
//!   host (run a command, drive a terminal, re-attach to one), carried inside
//!   the end-to-end encrypted channel of `leon-link`;
//! * [`relay`]: the small rendezvous messages between a peer and a relay
//!   server, which pairs two peers and forwards opaque bytes;
//! * [`id`]: the identifiers both sides display and the relay routes on.
//!
//! Messages are encoded with [`postcard`]: a compact, deterministic,
//! non-self-describing binary format over `serde`. It was chosen over JSON
//! (terminal bytes would need base64, a 33 % tax on the hot path) and over
//! a schema compiler such as protobuf (a build step and a dependency for a
//! protocol of a few dozen messages). It has a stable, documented wire
//! format and does not allocate from untrusted lengths: a claimed length is
//! checked against the bytes that really follow.
//!
//! Every input is hostile. Decoding returns an error for anything malformed
//! and never panics; a frame larger than [`MAX_FRAME_LEN`] is refused before
//! any of it is read.

#![warn(missing_docs)]

pub mod frame;
pub mod id;
pub mod message;
pub mod relay;

pub use frame::{decode_frame, encode_frame, FrameDecoder, FrameError, MAX_FRAME_LEN};
pub use id::{base32, HostId, IdError};
pub use message::{
    ErrorCode, ExecOutput, ExecSpec, Exit, Grid, Message, PtyInfo, SharedEntry, SharedProject,
    SharedSession, SharedTranscript, SpawnSpec, WireError, MAX_EXEC_OUTPUT, MAX_PTY_CHUNK,
    PROTOCOL_VERSION, REPLAY_BUFFER_BYTES, SHARE_SESSIONS_LIMIT,
};
pub use relay::{RelayError, RelayErrorCode, RelayLimits};
