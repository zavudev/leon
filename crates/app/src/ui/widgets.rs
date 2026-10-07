//! Small shared pieces: mono labels, key caps, status lights, the mark, the
//! focus rule under a pane's header.

use crate::theme::{fonts, metrics, px, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, App, Div, FontWeight, Hsla, Pixels, SharedString};
use leon_mark::{AnimatedMark, Mood};

use super::activity::Activity;

/// A mono label: the theme's mono font at the label size, semibold as the
/// brand sets small caps. Callers pass the text already in the case they want.
pub fn mono(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .font_family(fonts::mono())
        .font_features(fonts::mono_features())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(metrics::TEXT_LABEL())
        .child(text.into())
}

/// A section label: mono, uppercase, muted, in the `[ NAME ]` form.
pub fn section_label(title: &str, palette: &Palette) -> Div {
    mono(format!("[ {} ]", title.to_uppercase())).text_color(palette.text_muted)
}

/// A key as it is printed on the key: mono, outlined by a hairline.
pub fn key_cap(text: impl Into<SharedString>, palette: &Palette) -> Div {
    mono(text)
        .px(px(5.))
        .py(px(1.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(palette.elevated_border)
        .text_color(palette.text_muted)
}

/// How one lion of the interface behaves: its identity, what it feels, and
/// whether it answers the pointer and plays its entrance.
#[derive(Clone, Copy)]
pub struct Lion {
    id: &'static str,
    mood: Mood,
    hover: bool,
    intro: bool,
}

impl Lion {
    /// A lion with an identity (its element id and its debug selector): idle,
    /// deaf to the pointer, no entrance.
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            mood: Mood::Idle,
            hover: false,
            intro: false,
        }
    }

    /// How it feels.
    pub fn mood(mut self, mood: Mood) -> Self {
        self.mood = mood;
        self
    }

    /// It narrows its eyes when the pointer arrives.
    pub fn hover(mut self) -> Self {
        self.hover = true;
        self
    }

    /// Whether it plays its entrance when it first appears.
    pub fn intro(mut self, intro: bool) -> Self {
        self.intro = intro;
        self
    }

    /// A number that is the same for the same identity and differs between
    /// identities, so that two lions on one screen do not blink together.
    fn seed(&self) -> u64 {
        self.id.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }
}

/// The Leon glare at `size`, the product's mark in every theme: always the
/// animated lion, in the palette's `logo` colour, so it follows the theme in
/// either appearance and its cuts show the surface behind it. At 20 px and
/// below (after scaling) it is the fitted drawing. "Animate the lion" and
/// "Reduce motion" decide whether it moves; still, it is the rest pose, the
/// owner's drawing exactly.
pub fn mark(size: Pixels, palette: &Palette, cx: &App, lion: Lion) -> Div {
    let id = lion.id;
    div()
        .debug_selector(move || id.to_owned())
        .flex_none()
        .size(size)
        .child(
            AnimatedMark::new(size)
                .id(id)
                .seed(lion.seed())
                .color(palette.logo)
                .mood(lion.mood)
                .intro(lion.intro)
                .play_on_hover(lion.hover)
                .animate(crate::settings::flag(cx, "animate_lion"))
                .reduced_motion(crate::settings::reduce_motion(cx)),
        )
}

/// A status light: a small square of one state colour.
pub fn led(colour: Hsla) -> Div {
    div().flex_none().size(px(6.)).bg(colour)
}

/// The mark of a session that another terminal runs: a filled square in the
/// `elsewhere` colour when a process certainly holds it, an outline when that
/// is only the best fit. Never the green of a session live in Leon.
pub fn elsewhere_mark(certain: bool, palette: &Palette) -> Div {
    let mark = div().flex_none().size(px(6.)).border_1();
    if certain {
        mark.border_color(palette.elsewhere).bg(palette.elsewhere)
    } else {
        mark.border_color(palette.elsewhere)
    }
}

/// The two-pixel line of the accent under the header of the pane that has the
/// keyboard.
pub fn focus_rule(palette: &Palette) -> Div {
    div()
        .debug_selector(|| "pane-focus".into())
        .absolute()
        .left_0()
        .right_0()
        .bottom(gpui_kit::px(-1.))
        .h(px(2.))
        .bg(palette.signal)
}

/// The light of a project or a worktree row: how the agents in it are doing,
/// the same colours as the light of a session. Nothing live draws nothing (the
/// slot stays, so the rows line up): an empty grey square told nothing.
pub fn activity_light(activity: Activity, palette: &Palette) -> Div {
    match activity {
        Activity::Off => div().flex_none().size(px(10.)),
        _ => activity_dot(activity, palette),
    }
}

/// The dot of a worktree or a project: how its terminals are doing, in the
/// state colours (see `activity.rs`). Every state fits the same ten pixel
/// square so that labels line up: nothing live is a grey outline, idle a solid
/// green square, working the same square in a ring, waiting in the warning colour and failed
/// red.
pub fn activity_dot(activity: Activity, palette: &Palette) -> Div {
    let slot = div()
        .flex_none()
        .size(px(10.))
        .flex()
        .items_center()
        .justify_center();
    match activity {
        Activity::Off => slot.child(
            div()
                .size(px(7.))
                .border_1()
                .border_color(palette.text_faint),
        ),
        Activity::Idle => slot.child(div().size(px(7.)).bg(palette.success)),
        Activity::Working => slot
            .border_1()
            .border_color(palette.success)
            .child(div().size(px(4.)).bg(palette.success)),
        Activity::Waiting => slot.child(div().size(px(7.)).bg(palette.warning)),
        Activity::Failed => slot.child(div().size(px(7.)).bg(palette.error)),
    }
}
