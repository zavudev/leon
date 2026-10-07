//! The service that runs on a shared computer.
//!
//! A host offers exactly two primitives to the devices paired with it, over an
//! end-to-end encrypted channel (see `leon-link`) that a relay merely carries:
//!
//! * **run a command** ([`exec`]): the same semantics as a local command
//!   (inherited environment plus the request's, no standard input, a time
//!   limit, bounded output);
//! * **a terminal** ([`ptys`]): a pseudo-terminal owned by the host. It keeps
//!   running when every client disconnects (the durable-session property: an
//!   agent keeps working), keeps a bounded ring of recent output with a
//!   monotonically increasing byte offset so that a client can re-attach and
//!   receive each missed byte exactly once ([`ring`]), and is reaped some time
//!   after it exits.
//!
//! It may also speak of itself: with a [`ShareSource`], the host lets a paired
//! device see what this Leon holds of this machine — its projects and the
//! sessions of its history — so the device's own Leon shows them in its
//! sidebar. Sharing adds no rights: the device already holds a full terminal
//! as this computer's user.
//!
//! [`Host`] ties them to a relay: it registers, accepts pairing attempts and
//! sessions, reconnects with backoff, and re-checks its device list so a
//! revoked device is cut off mid-session.
//!
//! **Safety.** The service runs as the invoking user and gives every paired
//! device that user's full shell. [`running_as_root`] lets a front end refuse
//! to start as root.

#![warn(missing_docs)]

pub mod cli;
pub mod exec;
mod host;
pub mod ptys;
pub mod ring;
mod session;
pub mod share;

pub use host::{
    running_as_root, Approval, ApprovalRequest, Backoff, Host, HostConfig, HostStatus, PairingInfo,
    RelayState, Shared,
};
pub use share::ShareSource;
