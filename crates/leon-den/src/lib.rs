//! The Den: every live agent session as a pixel-art lion at work in a den
//! furnished as an office, with a narrator.
//!
//! It is the one comic view of Leon, and it never lies to be funny. Each
//! lion is one session and does what its agent really does: it types at its
//! desk when the agent edits, empties the shelf when it searches, minds the
//! rack while a command runs, stands on the rug at the entrance with a `!`
//! when it waits for the user. The feed beside the room tells the same facts
//! in the cadence of the 8-bit creature games (`brand/voice.md`), shows
//! what the agents wrote to the user in their own words, and the plain
//! truth, the file and the command as they are, is one hover away on the
//! truth card.
//!
//! The room is pixel art from files, compiled in ([`assets`]): the furniture
//! and the tiles of pixel-agents (MIT), and lions made from its character
//! sheets by [`atelier`]. It has its own colours. What is around and over it,
//! the boxes, the name plates, the bubbles, follows the host's theme
//! ([`palette::DenPalette`]); this crate holds no colour of a theme.
//!
//! The crate has layers, each usable without the next:
//!
//! * [`model`]: what the host tells the Den, as plain data: the [`Cub`]s
//!   (the lions; a sub-agent is a little one) and the [`Happening`]s;
//! * [`layout`]: the room as data, [`DenLayout`]: its size, floor and walls,
//!   and the pieces of the [`catalogue`] put in it; JSON that is read
//!   without ever failing. [`prefabs`] are the built-in ones;
//! * [`world`]: what a layout means: its places and spots (a seat that
//!   faces a computer is a place to work), the way from one tile to another;
//! * [`editor`]: every change a user can make to a layout, pure, with undo;
//! * [`sim`]: [`Den`], a pure function from what it was told and a time to a
//!   [`Frame`], with the [`Wake`] that says when the picture changes next;
//! * [`narrator`]: [`narrate`], from a happening to a line, and its plain
//!   twin;
//! * [`feed`]: the log of what was told and of what the agents themselves
//!   said to the user, with its history, its rows for a width and where the
//!   reader is;
//! * [`pose`]: from what a lion does to the frame of its sheet;
//! * [`paint`]: the room at a moment as one picture, composited in software
//!   ([`bitmap`]) at the size of its art;
//! * [`scene`]: the layout of the view and its chrome, as rectangles and
//!   text in device pixels, still with no window;
//! * [`view`]: [`DenView`], the GPUI view that paints the picture and the
//!   chrome and follows the wake with one timer.
//!
//! # Use
//!
//! ```no_run
//! use leon_den::{Cub, DenPalette, DenStyle, DenView, Tokens};
//! use gpui_kit::{px, App, AppContext, Entity};
//!
//! fn open(tokens: &Tokens, cubs: &[Cub], cx: &mut App) -> Entity<DenView> {
//!     let style = DenStyle {
//!         palette: DenPalette::from_tokens(tokens),
//!         font_family: "JetBrains Mono".into(),
//!         font_size: px(13.),
//!     };
//!     let den = cx.new(|cx| DenView::new(style, cx));
//!     den.update(cx, |den, cx| den.set_cubs(cubs, cx));
//!     den
//! }
//! ```
#![warn(missing_docs)]

pub mod assets;
pub mod atelier;
pub mod bitmap;
pub mod catalogue;
pub mod editor;
pub mod feed;
pub mod glyphs;
pub mod layout;
pub mod model;
pub mod narrator;
pub mod paint;
pub mod palette;
pub mod pose;
pub mod prefabs;
pub mod scene;
pub mod sim;
pub mod sprite;
pub mod view;
pub mod world;

pub use layout::DenLayout;
pub use model::{happenings_between, Cub, CubState, Event, Happening, Species, Status, ToolKind};
pub use narrator::{narrate, Line};
pub use palette::{DenPalette, Tokens};
pub use sim::{Den, Frame, RosterEntry, TruthCard, Wake};
pub use view::{DenEvent, DenStyle, DenView};

#[cfg(test)]
mod atelier_tests;
#[cfg(test)]
mod bitmap_tests;
#[cfg(test)]
mod catalogue_tests;
#[cfg(test)]
mod editor_tests;
#[cfg(test)]
mod feed_tests;
#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod narrator_tests;
#[cfg(test)]
mod paint_tests;
#[cfg(test)]
mod palette_tests;
#[cfg(test)]
mod pose_tests;
#[cfg(test)]
mod prefabs_tests;
#[cfg(test)]
mod scene_tests;
#[cfg(test)]
mod sim_tests;
#[cfg(test)]
mod sprite_tests;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod view_tests;
#[cfg(test)]
mod world_tests;
