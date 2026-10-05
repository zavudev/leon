//! The GPUI-free part of Leon's terminals.
//!
//! * [`spec`]: what to spawn, as a neutral description, and the command
//!   builder that adds the terminal type every program expects;
//! * [`size`]: resize maths, from pixels and cell metrics to a grid;
//! * [`runner`]: a pseudo-terminal with a child in it, driven through
//!   channels, with no emulator and no UI. The embedded terminal of
//!   `leon-term` has its own, emulator-coupled PTY plumbing; the service that
//!   runs on a shared computer (`leon-host`) uses this runner and so does not
//!   depend on the GPU stack.
//!
//! `leon-term` re-exports [`spec`] and [`size`], so existing paths keep
//! working.

#![warn(missing_docs)]

pub mod runner;
pub mod size;
pub mod spec;

pub use runner::{PtyEvent, PtyExit, PtyProcess};
pub use size::GridSize;
pub use spec::SpawnSpec;
