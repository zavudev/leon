//! The lion's idle gestures as a self-contained animated SVG, for the
//! website, the README and the docs. The application does not use these
//! files; it paints the same gestures itself ([`crate::element`]).
//!
//! The SVG is written from the same geometry and the same gestures as the
//! app: each keyframe is [`shapes`] of the [`Pose`] of a [`Gesture`] at that
//! moment, so the two cannot drift. The animation is SMIL, no script: the
//! `d` of the one `evenodd` path is animated through a list of values, with
//! linear interpolation between keyframes sampled every [`FRAME_STEP`]
//! seconds from the eased curves of the gestures. Holds cost no keyframes.
//!
//! A reduced-motion visitor gets a still lion: the file carries two copies
//! of the path, the live one and a still one, and an embedded
//! `prefers-reduced-motion` media query swaps them. SMIL cannot be paused
//! from CSS, so hiding the live path is how the query is honoured. The
//! first frame, the last frame and the still path are all the owner's path
//! to the byte.

use crate::geometry::{path_data, shapes, Variant};
use crate::motion::{Gesture, Pose, Side};

/// How long the loop is, in seconds.
pub const LOOP_LEN: f32 = 26.;

/// The seconds between two keyframes inside a gesture: 24 a second.
pub const FRAME_STEP: f32 = 1. / 24.;

/// What the loop plays and when, in seconds from its start: the blink, the
/// double blink, the glance and the glare of the idle lion, one at a time
/// with a rest between them.
pub const SCRIPT: [(f32, Gesture); 8] = [
    (2.8, Gesture::Blink),
    (6.9, Gesture::DoubleBlink),
    (10.8, Gesture::Glance(Side::Right)),
    (15.2, Gesture::Blink),
    (18.4, Gesture::Narrow),
    (20.4, Gesture::Blink),
    (22.6, Gesture::Glance(Side::Left)),
    (24.9, Gesture::Blink),
];

/// The radius of the tile of the app icon, in the icon's 64-unit grid.
pub const TILE_RADIUS: u32 = 14;

/// Where the lion sits on the tile, and how large: `translate(12 12)
/// scale(0.62)` of the app icon.
pub const TILE_TRANSFORM: &str = "translate(12 12) scale(0.62)";

/// The keyframes of the loop: `(seconds, pose)`, in order, from rest at 0 to
/// rest at [`LOOP_LEN`]. Samples inside a hold are left out.
pub fn frames() -> Vec<(f32, Pose)> {
    let mut samples: Vec<(f32, Pose)> = vec![(0., Pose::REST)];
    for (start, gesture) in SCRIPT {
        let duration = gesture.duration();
        let mut at = FRAME_STEP;
        samples.push((start, Pose::REST));
        while at < duration - 0.001 {
            samples.push((start + at, gesture.pose(at)));
            at += FRAME_STEP;
        }
        samples.push((start + duration, Pose::REST));
    }
    samples.push((LOOP_LEN, Pose::REST));

    // A sample equal to both its neighbours says nothing the line between
    // them does not.
    let kept: Vec<(f32, Pose)> = samples
        .iter()
        .enumerate()
        .filter(|(index, (_, pose))| {
            *index == 0
                || *index == samples.len() - 1
                || *pose != samples[index - 1].1
                || *pose != samples[index + 1].1
        })
        .map(|(_, sample)| *sample)
        .collect();
    kept
}

fn animated_path(attributes: &str) -> String {
    let frames = frames();
    let d = |pose: &Pose| path_data(&shapes(pose, Variant::Full).contours());
    let values: Vec<String> = frames.iter().map(|(_, pose)| d(pose)).collect();
    let times: Vec<String> = frames
        .iter()
        .map(|(at, _)| {
            if *at <= 0. {
                "0".to_owned()
            } else if *at >= LOOP_LEN {
                "1".to_owned()
            } else {
                format!("{:.5}", at / LOOP_LEN)
            }
        })
        .collect();
    let rest = d(&Pose::REST);
    format!(
        "<path class=\"live\"{attributes} fill-rule=\"evenodd\" d=\"{rest}\">\
<animate attributeName=\"d\" dur=\"{LOOP_LEN}s\" repeatCount=\"indefinite\" calcMode=\"linear\" \
keyTimes=\"{}\" values=\"{}\"/></path>\
<path class=\"still\"{attributes} fill-rule=\"evenodd\" d=\"{rest}\"/>",
        times.join(";"),
        values.join(";"),
    )
}

const STYLE: &str = "<style>.still{display:none}\
@media (prefers-reduced-motion: reduce){.live{display:none}.still{display:inline}}</style>";

/// The animated mark: transparent, in `accent` (a CSS colour), looping the
/// idle gestures.
pub fn mark_animated(accent: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\" role=\"img\" aria-label=\"Leon\">{STYLE}{}</svg>\n",
        animated_path(&format!(" fill=\"{accent}\""))
    )
}

/// The animated app icon: the lion in `accent` on a tile of `ink`, as
/// `app-icon.svg` places it, looping the idle gestures.
pub fn icon_animated(accent: &str, ink: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\" role=\"img\" aria-label=\"Leon\">{STYLE}\
<rect width=\"64\" height=\"64\" rx=\"{TILE_RADIUS}\" fill=\"{ink}\"/>{}</svg>\n",
        animated_path(&format!(
            " transform=\"{TILE_TRANSFORM}\" fill=\"{accent}\""
        ))
    )
}
