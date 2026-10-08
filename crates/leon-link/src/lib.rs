//! Leon's end-to-end secure channel.
//!
//! Everything that keeps a remote session private lives here, in public code,
//! so it can be reviewed without trusting any server: the relay that carries
//! the bytes is untrusted by design.
//!
//! * [`identity`]: each installation's long-term keys (an X25519 static key
//!   for Noise and an Ed25519 key that proves ownership of its id to a relay),
//!   stored owner-only, never logged.
//! * [`code`]: the short one-time pairing code.
//! * [`pairing`]: pairing two computers with that code: SPAKE2 turns the
//!   low-entropy code into a strong shared secret that neither the relay nor a
//!   network attacker can guess offline, a Noise `XXpsk3` handshake bound to
//!   it exchanges and authenticates the long-term keys, and the host keeps a
//!   single-use, expiring, attempt-limited [`pairing::PairingOffer`].
//! * [`session`]: the Noise `IK` session used for every later connection,
//!   with pinned keys, fragmentation of large messages and rekeying.
//! * [`registry`]: the host's list of authorised devices and the client's
//!   list of known hosts.
//! * [`client`]: a durable connection to one host, with reconnection and
//!   exact terminal re-attach.
//! * [`local`]: the same protocol as plain frames on this computer's own
//!   socket, for the terminal keeper, where the file permissions are the
//!   authentication and the durable client is reused as it is.
//! * [`pipe`]: a transport-independent ordered message pipe, [`channel`]: the
//!   async handshakes and the encrypted message channel over it, and
//!   [`relay_client`]: the WebSocket adapter that dials a relay and yields
//!   such pipes.
//!
//! No cryptography is home-made: this crate only composes `snow` (Noise),
//! `spake2`, `ed25519-dalek`, `sha2` and `subtle`. See `docs/REMOTE.md` for
//! the versions, the threat model and the message flow.

#![warn(missing_docs)]

pub mod channel;
pub mod client;
pub mod code;
pub mod identity;
pub mod local;
pub mod pairing;
pub mod pipe;
pub mod registry;
pub mod relay_client;
pub mod session;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod tls;

pub use channel::{accept, connect, LinkError, Rejection, SecureChannel};
pub use code::{CodeShape, PairingCode};
pub use identity::{DeviceId, Identity, IdentityError};
pub use pairing::{
    pair_as_client, pair_as_host, PairError, PairRequest, PairedDevice, PairedHost, PairingOffer,
};
pub use pipe::Pipe;
pub use registry::{DeviceRecord, DeviceRegistry, RegistryError};
pub use session::{RekeyPolicy, Session, SessionError};
