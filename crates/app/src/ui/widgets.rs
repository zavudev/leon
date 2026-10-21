//! Small shared pieces: mono labels, key caps, status lights, the mark, the
//! focus rule under a pane's header.

use crate::theme::{fonts, metrics, px, Palette};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, img, AnyElement, App, ClickEvent, Context, Div, FontWeight, Hsla, ObjectFit, Pixels,
    ScrollHandle, SharedString, Stateful, Window,
};
use leon_mark::{AnimatedMark, Mood};
use std::path::PathBuf;

use super::activity::Activity;
use super::shell::Shell;
use crate::icons::{icon, IconName};

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

/// The colour of a part of a checkout's state.
pub fn checkout_colour(tone: super::checkout::Tone, palette: &Palette) -> Hsla {
    use super::checkout::Tone;
    match tone {
        Tone::Muted => palette.text_muted,
        Tone::Warning => palette.warning,
        Tone::Good => palette.success,
        Tone::Bad => palette.error,
        Tone::Faint => palette.text_faint,
    }
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
/// green square, working the same square in a ring, an agent that finished its
/// turn in the info colour, waiting in the warning colour, an agent that needs an
/// answer the same square in a ring and failed red.
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
        Activity::TurnOver => slot.child(div().size(px(7.)).bg(palette.info)),
        Activity::Waiting => slot.child(div().size(px(7.)).bg(palette.warning)),
        Activity::NeedsYou => slot
            .border_1()
            .border_color(palette.warning)
            .child(div().size(px(4.)).bg(palette.warning)),
        Activity::Failed => slot.child(div().size(px(7.)).bg(palette.error)),
    }
}

/// A bare text field for a box of the interface: no border or background of
/// its own, and the Small size, whose 24 px box holds the 20 px line with 2 px
/// above and below. The default Medium box (32 px with 8 px above and below)
/// leaves 16 px for the same line and cuts the bottom of the letters.
pub fn text_input(state: &gpui_kit::Entity<InputState>) -> Input {
    Input::new(state).with_size(Size::Small).appearance(false)
}

/// Whether what `scroll` shows reaches its end, near enough.
pub(super) fn at_end(scroll: &ScrollHandle) -> bool {
    scroll.max_offset().y + scroll.offset().y < px(48.)
}

/// A conversation's `scrolling` list, with a button over its foot that goes
/// to the newest of it while it is scrolled up; `name` is the button's
/// selector.
pub(super) fn with_latest(
    scrolling: Stateful<Div>,
    scroll: &ScrollHandle,
    name: String,
    c: &Palette,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let hover = c.surface_2;
    let away = !at_end(scroll);
    let to = scroll.clone();
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        // Drawn again as it scrolls, so the button comes and goes.
        .child(scrolling.on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify())))
        .when(away, |this| {
            this.child(
                div()
                    .absolute()
                    .bottom(px(12.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id(SharedString::from(name.clone()))
                            .debug_selector(move || name.clone())
                            .h(px(26.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(metrics::RADIUS())
                            .border_1()
                            .border_color(c.elevated_border)
                            .bg(c.surface)
                            .shadow_md()
                            .text_color(c.text_muted)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                // At once, not at the next layout, so the
                                // button is gone when it is drawn again.
                                to.set_offset(gpui_kit::point(px(0.), -to.max_offset().y));
                                cx.notify();
                            }))
                            .child(icon(IconName::ChevronDown, px(12.), c.text_muted))
                            .child(mono("LATEST")),
                    ),
            )
        })
        .into_any_element()
}

/// Files attached to what is said next, over the field they go with: an
/// image as a thumbnail, any other file as its name, each with the button
/// that takes it off (`remove`). `names` are the selectors of the row and
/// the start of each button's; `None` with no files.
pub(super) fn attached_files(
    files: &[PathBuf],
    names: (String, String),
    c: &Palette,
    cx: &mut Context<Shell>,
    remove: impl Fn(&mut Shell, PathBuf, &mut Window, &mut Context<Shell>) + Clone + 'static,
) -> Option<AnyElement> {
    if files.is_empty() {
        return None;
    }
    let (name, remove_name) = names;
    let hover = c.surface_2;
    let mut row = div()
        .debug_selector({
            let name = name.clone();
            move || name.clone()
        })
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .gap(px(6.));
    for (place, path) in files.iter().enumerate() {
        let label = path
            .file_name()
            .map(|label| label.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let image = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ["png", "jpg", "jpeg", "webp", "gif", "bmp"]
                    .contains(&ext.to_ascii_lowercase().as_str())
            });
        let gone = path.clone();
        let remove = remove.clone();
        let item = format!("{name}-{place}");
        let button = format!("{remove_name}-{place}");
        row = row.child(
            div()
                .debug_selector(move || item.clone())
                .flex_none()
                .max_w(px(220.))
                .h(px(40.))
                .pl(px(4.))
                .pr(px(2.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(c.border)
                .bg(c.background)
                .child(if image {
                    img(path.clone())
                        .flex_none()
                        .size(px(32.))
                        .rounded(px(2.))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element()
                } else {
                    div()
                        .flex_none()
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::File, px(14.), c.text_muted))
                        .into_any_element()
                })
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(c.text_muted)
                        .child(label),
                )
                .child(
                    div()
                        .id(SharedString::from(button.clone()))
                        .debug_selector(move || button.clone())
                        .flex_none()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(metrics::RADIUS())
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            remove(this, gone.clone(), window, cx);
                            cx.notify();
                        }))
                        .child(icon(IconName::X, px(11.), c.text_faint)),
                ),
        );
    }
    Some(row.into_any_element())
}
