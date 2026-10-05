//! The Leon lion, always animated.
//!
//! The product's mark is the glare: one lion's plate with cuts, two slanted
//! slit eyes under a notched brow, a chevron nose with a short stem, in the
//! theme's logo colour. This crate makes it live without redrawing it: at
//! rest it is exactly the owner's SVG, point for point, and everything it
//! does is a small, restrained movement of its own parts. It blinks, narrows
//! its eyes into a glare, glances aside and leans a little with the glance,
//! breathes, twitches its nose now and then, and on launch resolves from two
//! slits opening in the dark. It is never cute: no bouncing, no smile.
//!
//! The crate has three layers, each usable without the next:
//!
//! * [`geometry`]: the lion's parts as point lists on the 64-unit grid, and
//!   [`geometry::shapes`], the polygons of a [`Pose`];
//! * [`motion`]: a pure, deterministic function from time to [`Pose`], with
//!   the body language of each [`Mood`], and a [`Wake`] that says when to ask
//!   for the next frame and when to sleep on a timer;
//! * [`element`]: [`AnimatedMark`], a GPUI element that follows a
//!   [`Timeline`] and costs nothing between gestures.
//!
//! [`svg`] writes the same idle gestures as a self-contained animated SVG
//! for the web, from the same geometry and the same timing constants.
//!
//! # Use
//!
//! ```no_run
//! use leon_mark::{AnimatedMark, Mood};
//! use gpui_kit::{px, rgb};
//!
//! let header = AnimatedMark::new(px(20.))
//!     .color(rgb(0xffea00))
//!     .mood(Mood::Working)
//!     .play_on_hover(true);
//! ```
#![warn(missing_docs)]

pub mod element;
pub mod geometry;
pub mod motion;
pub mod svg;

#[cfg(any(test, feature = "test-support"))]
pub use element::probe;
pub use element::{paint_pose, paint_shapes, AnimatedMark};
pub use geometry::{shapes, Shapes, Variant};
pub use motion::{Frame, Gesture, GestureSet, Mood, Pose, Side, Timeline, Wake, INTRO_LEN};

#[cfg(test)]
mod element_tests;
#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod motion_tests;
#[cfg(test)]
mod svg_tests;
