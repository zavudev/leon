//! A conversation as the window shows it: the events of the unified protocol
//! reduced to a list of things to draw.
//!
//! The model and its reducer live in `leon-conversation`, so every front end
//! of a conversation shares them; this module keeps the old path.

pub use leon_conversation::model::*;
