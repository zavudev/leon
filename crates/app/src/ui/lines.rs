//! The blueprint lines: crosshairs where rules meet, corner ticks on framed
//! surfaces and the frame of an empty state.
//!
//! This is wuapi's line system (its brand book, "La grilla de líneas") as a
//! property of a *theme*: `theme::Lines` says which pieces a theme draws and how
//! heavy they are, the palette says in which colour (`grid_mark`, `guide`), and
//! this module draws them. Nothing here is wallpaper: a mark sits on a point
//! where two rules meet, a tick on a corner of something that has a frame, and
//! the frame of an empty state surrounds its text and never lies behind it.
//!
//! There are two layers. The *geometry* ([`marks`], [`dividers`]) is pure
//! numbers: where the rules of the window are, derived from the same metrics
//! and the same pane layout that lay the panes out, so a mark cannot drift when
//! the interface is scaled or a pane is split or dragged. The *drawing*
//! ([`crosshair`], [`corner_ticks`], [`empty_frame`]) turns those numbers into a
//! few absolutely positioned boxes with no handlers: they take no mouse event,
//! so a click on a mark reaches what is under it, and they cost nothing per
//! frame beyond the boxes themselves.
//!
//! Positions are on whole device pixels (see [`snap`]) and every line is a
//! hairline of the theme's weight at every interface size, so a rule that is
//! one pixel stays one pixel.

use super::panes::{Axis, Layout};
use crate::theme::{self, hairline, metrics, px, Crosshairs, Palette};
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, Div, Hsla, Pixels, SharedString};

/// A point where two rules meet, named for the test that finds it.
///
/// `x` is the right edge of the vertical rule and `y` the bottom edge of the
/// horizontal one: the rule occupies the pixel just before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// What it marks: `header`, `footer`, `tabs`, `divider-<path>-start`...
    pub name: String,
    /// Right edge of the vertical rule, from the window's left.
    pub x: f32,
    /// Bottom edge of the horizontal rule, from the window's top.
    pub y: f32,
}

/// What is laid out in the window: the numbers the rules are drawn from.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// The window's size.
    pub viewport: (f32, f32),
    /// Width of the sidebar, its rule included.
    pub sidebar: f32,
    /// Width of the files panel on the right, its rule included; zero while
    /// it is hidden.
    pub files: f32,
    /// Height of the header strips, their rule included.
    pub header: f32,
    /// Height of the status strip, its rule included.
    pub status: f32,
    /// Height of the sidebar's tools, their rule included.
    pub tools: f32,
    /// Height of the strip of terminal tabs, when one is shown.
    pub tabs: Option<f32>,
    /// The split panes on screen, when a terminal is.
    pub layout: Option<&'a Layout>,
    /// Device pixels per point.
    pub scale_factor: f32,
}

/// `value` on the nearest device pixel.
pub fn snap(value: f32, scale_factor: f32) -> f32 {
    let factor = scale_factor.max(1.0);
    (value * factor).round() / factor
}

/// A divider between two panes: a rule one pixel wide.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// The path of its split, written as `divider-` plus 0 and 1.
    pub path: String,
    /// Which way the panes are divided: a row's divider is vertical.
    pub axis: Axis,
    /// The area the split divides, rules not included.
    pub area: [f32; 4],
    /// Left edge of a vertical divider, top edge of a horizontal one.
    pub at: f32,
}

/// Every divider of `layout` inside `area` (`x`, `y`, `w`, `h`), laid out the
/// way `render_layout` lays the panes out: the first half is `ratio` of the
/// area, the divider one pixel, the second half the rest.
pub fn dividers(layout: &Layout, area: [f32; 4]) -> Vec<Divider> {
    let mut all = Vec::new();
    collect(layout, area, String::new(), &mut all);
    all
}

fn collect(layout: &Layout, area: [f32; 4], path: String, all: &mut Vec<Divider>) {
    let Layout::Split { axis, ratio, a, b } = layout else {
        return;
    };
    let [x, y, w, h] = area;
    let (first, second, at) = match axis {
        Axis::Row => {
            let first = w * ratio;
            (
                [x, y, first, h],
                [x + first + 1.0, y, w - first - 1.0, h],
                x + first,
            )
        }
        Axis::Column => {
            let first = h * ratio;
            (
                [x, y, w, first],
                [x, y + first + 1.0, w, h - first - 1.0],
                y + first,
            )
        }
    };
    all.push(Divider {
        path: path.clone(),
        axis: *axis,
        area,
        at,
    });
    collect(a, first, format!("{path}0"), all);
    collect(b, second, format!("{path}1"), all);
}

/// Where the rules of the window and of its split panes meet, for the
/// crosshairs a theme draws. [`Crosshairs::Header`] is the one under the
/// sidebar's header, [`Crosshairs::All`] every intersection.
pub fn marks(frame: &Frame, set: Crosshairs) -> Vec<Mark> {
    let sf = frame.scale_factor;
    let mut all: Vec<Mark> = Vec::new();
    let mut put = |name: String, x: f32, y: f32| {
        let (x, y) = (snap(x, sf), snap(y, sf));
        if !all
            .iter()
            .any(|mark| (mark.x - x).abs() < 0.01 && (mark.y - y).abs() < 0.01)
        {
            all.push(Mark { name, x, y });
        }
    };
    if set == Crosshairs::Off {
        return Vec::new();
    }
    // With the sidebar hidden there is no rule of its to meet.
    let ruled = frame.sidebar > 0.0;
    // The sidebar's rule meets the header rule that runs across the window.
    if ruled {
        put("header".to_owned(), frame.sidebar, frame.header);
    }
    if set == Crosshairs::Header {
        return all;
    }
    let (width, height) = frame.viewport;
    // The right edge of the main area: the window's, or the files panel's
    // rule.
    let right = width - frame.files;
    // ...the rule over the status strip and the one over the sidebar's tools,
    // which are one rule where the theme aligns them...
    if ruled {
        put(
            "footer".to_owned(),
            frame.sidebar,
            height - frame.status + 1.0,
        );
        put(
            "footer-tools".to_owned(),
            frame.sidebar,
            height - frame.tools + 1.0,
        );
    }
    // ...and the rule under the strip of terminal tabs.
    let top = match frame.tabs {
        Some(tabs) => {
            if ruled {
                put("tabs".to_owned(), frame.sidebar, frame.header + tabs);
            }
            frame.header + tabs
        }
        None => frame.header,
    };
    // The files panel's rule meets the header, tab and status rules.
    if frame.files > 0.0 {
        put("header-files".to_owned(), right, frame.header);
        put(
            "footer-files".to_owned(),
            right,
            height - frame.status + 1.0,
        );
        if let Some(tabs) = frame.tabs {
            put("tabs-files".to_owned(), right, frame.header + tabs);
        }
    }
    // Split panes: each divider ends on a rule (the header or tab rule, the
    // status rule, the sidebar's rule or the divider of a split around it).
    if let Some(layout) = frame.layout {
        let area = [
            frame.sidebar,
            top,
            right - frame.sidebar,
            height - frame.status - top,
        ];
        for divider in dividers(layout, area) {
            let [x, y, w, h] = divider.area;
            let name = format!("divider-{}", divider.path);
            match divider.axis {
                Axis::Row => {
                    put(format!("{name}-start"), divider.at + 1.0, y);
                    put(format!("{name}-end"), divider.at + 1.0, y + h + 1.0);
                }
                Axis::Column => {
                    // Its left end meets the sidebar's rule or a divider; at
                    // the window's own edge, with no sidebar, nothing.
                    if ruled || x > 0.5 {
                        put(format!("{name}-start"), x, divider.at + 1.0);
                    }
                    // The window's own edge has no rule to meet.
                    if x + w < width - 0.5 {
                        put(format!("{name}-end"), x + w + 1.0, divider.at + 1.0);
                    }
                }
            }
        }
    }
    all
}

/// The side of a crosshair: an odd number of pixels, so its arms cross on one.
fn odd(length: Pixels) -> Pixels {
    gpui_kit::px((length.as_f32() / 2.).floor() * 2. + 1.)
}

/// A crosshair on the crossing of the rule whose right edge is `x` and the
/// rule whose bottom edge is `y`, centred on it. `name` is its test selector
/// (`crosshair-<name>`).
pub fn crosshair(name: &str, x: Pixels, y: Pixels, palette: &Palette) -> Div {
    let weight = gpui_kit::px(theme::lines().weight);
    let length = odd(metrics::CROSSHAIR());
    let arm = (length - hairline()) / 2.;
    let selector = format!("crosshair-{name}");
    div()
        .debug_selector(move || selector.clone())
        .absolute()
        .left(x - hairline() - arm)
        .top(y - hairline() - arm)
        .size(length)
        .child(
            div()
                .absolute()
                .left_0()
                .top(arm)
                .w(length)
                .h(weight)
                .bg(palette.grid_mark),
        )
        .child(
            div()
                .absolute()
                .left(arm)
                .top_0()
                .w(weight)
                .h(length)
                .bg(palette.grid_mark),
        )
}

/// The crosshairs of the window, from the numbers it is laid out with. Empty
/// where the theme draws none.
pub fn crosshairs(frame: &Frame, palette: &Palette) -> Vec<Div> {
    marks(frame, theme::lines().crosshairs)
        .into_iter()
        .map(|mark| {
            crosshair(
                &mark.name,
                gpui_kit::px(mark.x),
                gpui_kit::px(mark.y),
                palette,
            )
        })
        .collect()
}

/// The four corner ticks of the box they are children of, which must be
/// `relative`: an L of `length` on each corner, drawn just inside the box's
/// border so that it reinforces the frame. `name` prefixes their test
/// selectors (`<name>-tick-tl` and so on).
pub fn corner_ticks(name: &str, colour: Hsla, weight: Pixels) -> Vec<Div> {
    let length = metrics::TICK();
    [
        ("tl", true, true),
        ("tr", true, false),
        ("bl", false, true),
        ("br", false, false),
    ]
    .into_iter()
    .map(|(corner, top, left)| {
        let selector = format!("{name}-tick-{corner}");
        div()
            .debug_selector(move || selector.clone())
            .absolute()
            .size(length)
            .map(|this| if top { this.top_0() } else { this.bottom_0() })
            .map(|this| if left { this.left_0() } else { this.right_0() })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .w(length)
                    .h(weight)
                    .map(|this| if top { this.top_0() } else { this.bottom_0() })
                    .bg(colour),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h(length)
                    .w(weight)
                    .map(|this| if left { this.left_0() } else { this.right_0() })
                    .bg(colour),
            )
    })
    .collect()
}

/// The ticks of a framed surface in the quiet colour, when the theme draws
/// them; nothing otherwise.
pub fn frame_ticks(name: &str, palette: &Palette) -> Vec<Div> {
    let lines = theme::lines();
    if !lines.corner_ticks {
        return Vec::new();
    }
    corner_ticks(name, palette.grid_mark, gpui_kit::px(lines.weight))
}

/// The ticks of the pane that has the keyboard: the accent, twice as heavy as
/// the quiet ones.
pub fn focus_ticks(name: &str, palette: &Palette) -> Vec<Div> {
    let lines = theme::lines();
    if !lines.corner_ticks {
        return Vec::new();
    }
    corner_ticks(name, palette.signal, gpui_kit::px(lines.weight * 2.))
}

/// An empty state: `content` inside a frame of corner ticks, with a dimension
/// line under it that carries `caption`, as a drawing is dimensioned. In a
/// theme without the motif it is `content` as it is.
pub fn empty_frame(
    name: &str,
    content: impl IntoElement,
    caption: Option<String>,
    palette: &Palette,
) -> AnyElement {
    if !theme::lines().empty_motif {
        return content.into_any_element();
    }
    let line = || div().flex_1().h(hairline()).bg(palette.guide);
    let end = || div().flex_none().w(hairline()).h(px(9.)).bg(palette.guide);
    let dimension = caption.map(|text| {
        div()
            .debug_selector({
                let selector = format!("{name}-dimension");
                move || selector.clone()
            })
            .flex()
            .items_center()
            .gap(px(8.))
            .pt(px(8.))
            .child(end())
            .child(line())
            .child(super::widgets::mono(SharedString::from(text)).text_color(palette.text_faint))
            .child(line())
            .child(end())
    });
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .debug_selector({
                    let selector = format!("{name}-frame");
                    move || selector.clone()
                })
                .relative()
                .px(px(20.))
                .py(px(16.))
                .child(content)
                .children(corner_ticks(
                    name,
                    palette.grid_mark,
                    gpui_kit::px(theme::lines().weight),
                )),
        )
        .children(dimension)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::live::LiveId;

    fn frame(layout: Option<&Layout>) -> Frame<'_> {
        Frame {
            viewport: (1000.0, 600.0),
            sidebar: 320.0,
            files: 0.0,
            header: 48.0,
            status: 41.0,
            tools: 41.0,
            tabs: None,
            layout,
            scale_factor: 1.0,
        }
    }

    fn at(marks: &[Mark], name: &str) -> (f32, f32) {
        let mark = marks
            .iter()
            .find(|mark| mark.name == name)
            .unwrap_or_else(|| panic!("no mark {name}: {marks:?}"));
        (mark.x, mark.y)
    }

    fn two() -> Layout {
        Layout::Split {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(Layout::Leaf(LiveId(1))),
            b: Box::new(Layout::Leaf(LiveId(2))),
        }
    }

    #[test]
    fn a_theme_with_no_crosshairs_has_no_marks_and_the_header_set_has_one() {
        assert!(marks(&frame(None), Crosshairs::Off).is_empty());
        let header = marks(&frame(None), Crosshairs::Header);
        assert_eq!(header.len(), 1);
        assert_eq!(at(&header, "header"), (320.0, 48.0));
    }

    #[test]
    fn the_full_set_marks_the_header_and_the_footer_rules_on_the_sidebars_rule() {
        let all = marks(&frame(None), Crosshairs::All);
        assert_eq!(at(&all, "header"), (320.0, 48.0));
        // Status strip and tools share one rule: one mark.
        assert_eq!(at(&all, "footer"), (320.0, 600.0 - 41.0 + 1.0));
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn a_footer_whose_two_rules_differ_gets_a_mark_for_each() {
        let mut unequal = frame(None);
        unequal.status = 28.0;
        let all = marks(&unequal, Crosshairs::All);
        assert_eq!(at(&all, "footer"), (320.0, 600.0 - 28.0 + 1.0));
        assert_eq!(at(&all, "footer-tools"), (320.0, 600.0 - 41.0 + 1.0));
    }

    #[test]
    fn the_strip_of_tabs_adds_the_mark_under_its_rule() {
        let mut tabbed = frame(None);
        tabbed.tabs = Some(30.0);
        assert_eq!(at(&marks(&tabbed, Crosshairs::All), "tabs"), (320.0, 78.0));
    }

    #[test]
    fn the_files_panel_rule_is_marked_where_it_meets_the_others() {
        let mut panel = frame(None);
        panel.files = 272.0;
        panel.tabs = Some(30.0);
        let all = marks(&panel, Crosshairs::All);
        assert_eq!(at(&all, "header-files"), (1000.0 - 272.0, 48.0));
        assert_eq!(
            at(&all, "footer-files"),
            (1000.0 - 272.0, 600.0 - 41.0 + 1.0)
        );
        assert_eq!(at(&all, "tabs-files"), (1000.0 - 272.0, 78.0));
    }

    #[test]
    fn a_divider_ends_on_the_files_panels_rule_when_it_is_open() {
        let layout = two();
        let mut panel = frame(Some(&layout));
        panel.files = 272.0;
        let all = marks(&panel, Crosshairs::All);
        // The area is now 408 wide: the first pane is 204, the divider's
        // right edge is at 525.
        assert_eq!(at(&all, "divider--start"), (525.0, 48.0));
        assert_eq!(at(&all, "divider--end"), (525.0, 600.0 - 41.0 + 1.0));
    }

    #[test]
    fn a_divider_is_marked_where_it_meets_the_rule_above_and_the_one_below() {
        let layout = two();
        let all = marks(&frame(Some(&layout)), Crosshairs::All);
        // The area is 680 wide: the first pane is 340, the divider is the
        // pixel at 660 and its right edge is 661.
        assert_eq!(at(&all, "divider--start"), (661.0, 48.0));
        assert_eq!(at(&all, "divider--end"), (661.0, 600.0 - 41.0 + 1.0));
    }

    #[test]
    fn a_split_inside_a_pane_is_marked_where_it_meets_the_divider_beside_it() {
        let layout = Layout::Split {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(Layout::Leaf(LiveId(1))),
            b: Box::new(Layout::Split {
                axis: Axis::Column,
                ratio: 0.5,
                a: Box::new(Layout::Leaf(LiveId(2))),
                b: Box::new(Layout::Leaf(LiveId(3))),
            }),
        };
        let all = marks(&frame(Some(&layout)), Crosshairs::All);
        // The horizontal divider of the right pane starts on the vertical one
        // (right edge 661) and ends at the window's edge, where no rule is.
        let (x, y) = at(&all, "divider-1-start");
        assert_eq!(x, 661.0);
        assert!(y > 48.0 && y < 559.0);
        assert!(all.iter().all(|mark| mark.name != "divider-1-end"));
    }

    #[test]
    fn a_split_in_the_first_pane_ends_on_the_divider_to_its_right() {
        let layout = Layout::Split {
            axis: Axis::Row,
            ratio: 0.5,
            a: Box::new(Layout::Split {
                axis: Axis::Column,
                ratio: 0.5,
                a: Box::new(Layout::Leaf(LiveId(1))),
                b: Box::new(Layout::Leaf(LiveId(2))),
            }),
            b: Box::new(Layout::Leaf(LiveId(3))),
        };
        let all = marks(&frame(Some(&layout)), Crosshairs::All);
        let (x, _) = at(&all, "divider-0-end");
        assert_eq!(x, 661.0, "the horizontal divider reaches the vertical one");
        assert_eq!(at(&all, "divider-0-start").0, 320.0);
    }

    #[test]
    fn with_the_sidebar_hidden_no_mark_stays_where_its_rule_was() {
        let layout = Layout::Split {
            axis: Axis::Column,
            ratio: 0.5,
            a: Box::new(Layout::Leaf(LiveId(1))),
            b: Box::new(Layout::Leaf(LiveId(2))),
        };
        let mut bare = frame(Some(&layout));
        bare.sidebar = 0.0;
        bare.tabs = Some(30.0);
        let all = marks(&bare, Crosshairs::All);
        assert!(all.is_empty(), "{all:?}");
        assert!(marks(&bare, Crosshairs::Header).is_empty());
    }

    #[test]
    fn marks_land_on_device_pixels() {
        let layout = two();
        let mut sharp = frame(Some(&layout));
        sharp.scale_factor = 2.0;
        sharp.viewport = (1001.0, 601.0);
        for mark in marks(&sharp, Crosshairs::All) {
            assert!((mark.x * 2.0).fract().abs() < 1e-3, "{mark:?}");
            assert!((mark.y * 2.0).fract().abs() < 1e-3, "{mark:?}");
        }
    }

    #[test]
    fn dividers_follow_the_ratio_of_their_split() {
        let mut layout = two();
        if let Layout::Split { ratio, .. } = &mut layout {
            *ratio = 0.25;
        }
        let found = dividers(&layout, [0.0, 0.0, 400.0, 100.0]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].at, 100.0);
    }
}
