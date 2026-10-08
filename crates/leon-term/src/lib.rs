//! An embedded terminal for GPUI: a pseudo-terminal, an emulator and a view
//! drawn on the GPU.
//!
//! ```text
//! child process <-> PTY <-> reader thread -> processor thread -> Term (grid)
//!                     ^                                            |
//!                     +-- writer thread <- keys, paste, replies    v
//!                                                          TerminalView (GPUI)
//! ```
//!
//! * [`Terminal`] owns the emulator ([`alacritty_terminal`]'s `Term`), the
//!   PTY ([`portable-pty`], ConPTY on Windows) and the child. It has no GPUI
//!   in it: output wakes the UI through a callback that fires at most once
//!   per repaint, however much the child prints.
//! * [`Backend`] is where terminals come from: [`Pty`] in the application,
//!   [`Scripted`] (no process, no threads, driven by the test) in tests of
//!   code that only uses a terminal. [`Timings`] shortens the few waits a
//!   real terminal makes so that its own tests do not sleep.
//! * [`TerminalView`] is the GPUI entity that draws a [`Terminal`] and turns
//!   keys, text input, the mouse and the clipboard into bytes for it.
//! * Everything that can be decided without a window is a pure function with
//!   its own tests: [`spec`] (what to spawn), [`size`] (resize maths),
//!   [`colors`] (colour resolution), [`keys`] (key to escape sequence, paste
//!   and mouse reports) and [`layout`] (cells to background rectangles and
//!   text runs).
//!
//! Colours never come from this crate: the application passes a
//! [`TerminalTheme`]. The only constants here are the standard xterm 256
//! colour cube.
//!
//! # Attribution
//!
//! The overall shape of this crate (an alacritty `Term` fed by a reader
//! thread, an event proxy, a GPUI view with an input handler, the key table
//! in [`keys`]) follows `gpui-terminal` 0.1.0 by Leonard Seibold
//! (<https://github.com/zortax/gpui-terminal>, MIT OR Apache-2.0), which
//! targets `gpui` 0.2 and `alacritty_terminal` 0.25 and so could not be used
//! as it is. It was ported to `gpui-kit` 0.7 / `alacritty_terminal` 0.26 and
//! extended (batched painting, selection, mouse reporting, scrollback,
//! replies to the child's queries, synchronized updates, process cleanup).
//! The licences of the original ship beside this file
//! (`LICENSE-gpui-terminal-MIT`, `LICENSE-gpui-terminal-APACHE`) and are
//! listed in the repository's `NOTICE`. No code from Zed's GPL-licensed
//! terminal crates is used.
//!
//! [`portable-pty`]: portable_pty

#![warn(missing_docs)]

pub mod buffer;
pub mod colors;
pub mod files;
pub mod find;
pub mod keys;
pub mod layout;
pub use leon_pty::{size, spec};
mod terminal;
#[cfg(test)]
mod testing;
mod view;

pub use alacritty_terminal::vte::ansi::CursorShape;
pub use buffer::{Cleared, Extent};
pub use colors::TerminalTheme;
pub use find::{FindOptions, FindState, Highlights};
pub use leon_pty::{GridSize, SpawnSpec};
pub use terminal::{
    feed, Backend, EventProxy, ExitInfo, GridPoint, Headless, Pty, RemoteFeed, RemoteLink, Script,
    Scripted, SpawnError, Terminal, TerminalEvent, Timings, Wake, SCROLLBACK_LINES,
};
pub use view::{FontSettings, TerminalView, ViewEvent};
