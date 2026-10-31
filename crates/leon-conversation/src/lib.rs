//! A conversation as a list of things to draw: the events of Leon's unified
//! agent protocol ([`leon_turn`]) reduced to data, and the words that
//! describe it.
//!
//! The window draws one interface for every agent, so the conversation is
//! the same data whatever the agent is. This crate is the part of it without
//! a window:
//!
//! * [`model`]: the conversation, its items, and the reducer that builds
//!   them from the protocol's events;
//! * [`phrase`]: what a stretch of tools says of itself, in words;
//! * [`stamp`]: when a message was said, as a person reads it;
//! * [`format`]: counts, spans and speeds, spelled one way everywhere;
//! * [`harness`]: what a program wrote in the person's name, read back from
//!   a history as a note;
//! * [`handoff`]: the tag that marks the switch from one agent to another.
//!
//! Pure: events in, data out. It starts no process and opens no window.

pub mod format;
pub mod handoff;
pub mod harness;
pub mod model;
pub mod phrase;
pub mod stamp;
