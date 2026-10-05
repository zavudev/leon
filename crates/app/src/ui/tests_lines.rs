//! Tests of the blueprint lines: where the marks are, that they follow the
//! layout, that a theme without them draws none, and that they take no clicks.

use super::live::{open_live, real_worktree, script_of, terminal_of, wait_until};
use super::*;
use crate::theme::ThemeId;
use gpui_kit::{Modifiers, MouseButton};

fn centre(h: &Harness, cx: &mut TestAppContext, selector: &str) -> (f32, f32) {
    let bounds = h
        .bounds_of(selector.to_owned(), cx)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    (bounds.center().x.as_f32(), bounds.center().y.as_f32())
}

fn near(a: (f32, f32), b: (f32, f32), what: &str) {
    assert!(
        (a.0 - b.0).abs() <= 0.5 && (a.1 - b.1).abs() <= 0.5,
        "{what}: mark at {a:?}, intersection at {b:?}"
    );
}

fn scale(cx: &mut TestAppContext, percent: u16) {
    cx.update(|cx| settings::update(cx, |s| s.interface_scale = percent));
}

/// The pixel where the sidebar's rule meets a horizontal rule whose pixel row
/// starts at `y`.
fn on_sidebar_rule(h: &Harness, cx: &mut TestAppContext, y: f32) -> (f32, f32) {
    let sidebar = h.bounds_of("sidebar".to_owned(), cx).unwrap();
    (sidebar.right().as_f32() - 0.5, y + 0.5)
}

#[gpui_kit::test]
fn each_crosshair_is_centred_on_the_intersection_it_marks_at_100_and_at_125_percent(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    for percent in [100, 125] {
        scale(cx, percent);
        h.settle(cx);
        let header = h.bounds_of("sidebar-header".to_owned(), cx).unwrap();
        near(
            centre(&h, cx, "crosshair-header"),
            on_sidebar_rule(&h, cx, header.bottom().as_f32() - 1.0),
            &format!("the header rule at {percent}%"),
        );
        let status = h.bounds_of("status-strip".to_owned(), cx).unwrap();
        near(
            centre(&h, cx, "crosshair-footer"),
            on_sidebar_rule(&h, cx, status.top().as_f32()),
            &format!("the footer rule at {percent}%"),
        );
        // The sidebar's tools and the status strip share one rule.
        let tools = h.bounds_of("sidebar-tools".to_owned(), cx).unwrap();
        assert_eq!(
            tools.top(),
            status.top(),
            "one rule across the window at {percent}%"
        );
    }
    scale(cx, 100);
}

#[gpui_kit::test]
fn the_mark_under_the_tab_strip_is_centred_on_its_rule(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    h.press_chord("cmd-t", "ctrl-shift-t", cx);
    assert!(h.shows("terminal-tabs", cx), "two tabs: a strip");
    let tabs = h.bounds_of("terminal-tabs".to_owned(), cx).unwrap();
    near(
        centre(&h, cx, "crosshair-tabs"),
        on_sidebar_rule(&h, cx, tabs.bottom().as_f32() - 1.0),
        "the tab strip's rule",
    );
}

#[gpui_kit::test]
fn a_dividers_ends_are_marked_and_the_marks_move_with_the_split(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    let check = |h: &Harness, cx: &mut TestAppContext, when: &str| {
        let divider = h.bounds_of("divider-".to_owned(), cx).unwrap();
        let header = h.bounds_of("main-header".to_owned(), cx).unwrap();
        let status = h.bounds_of("status-strip".to_owned(), cx).unwrap();
        let x = divider.left().as_f32() + 0.5;
        near(
            centre(h, cx, "crosshair-divider--start"),
            (x, header.bottom().as_f32() - 0.5),
            &format!("{when}: where the divider meets the header's rule"),
        );
        near(
            centre(h, cx, "crosshair-divider--end"),
            (x, status.top().as_f32() + 0.5),
            &format!("{when}: where it meets the status rule"),
        );
        x
    };
    let before = check(&h, cx, "at first");
    h.press_chord("ctrl-cmd-right", "ctrl-shift-alt-right", cx);
    let after = check(&h, cx, "after the divider moved");
    assert!(after > before, "the divider moved: {before} -> {after}");
}

#[gpui_kit::test]
fn the_marks_follow_the_sidebars_width(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    cx.update(|cx| h.shell.update(cx, |s, cx| s.set_sidebar_width(420, cx)));
    h.settle(cx);
    let header = h.bounds_of("sidebar-header".to_owned(), cx).unwrap();
    near(
        centre(&h, cx, "crosshair-header"),
        (419.5, header.bottom().as_f32() - 0.5),
        "a wider sidebar",
    );
}

#[gpui_kit::test]
fn a_framed_surface_has_ticks_on_its_corners(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    let card = h.bounds_of("palette".to_owned(), cx).unwrap();
    let tick = |h: &Harness, cx: &mut TestAppContext, name: &str| {
        h.bounds_of(format!("palette-tick-{name}"), cx)
            .unwrap_or_else(|| panic!("no tick {name}"))
    };
    // Just inside the border, on the corner.
    let (tl, tr, bl, br) = (
        tick(&h, cx, "tl"),
        tick(&h, cx, "tr"),
        tick(&h, cx, "bl"),
        tick(&h, cx, "br"),
    );
    let close = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| (a - b).abs() <= px(1.5);
    assert!(close(tl.left(), card.left()) && close(tl.top(), card.top()));
    assert!(close(tr.right(), card.right()) && close(tr.top(), card.top()));
    assert!(close(bl.left(), card.left()) && close(bl.bottom(), card.bottom()));
    assert!(close(br.right(), card.right()) && close(br.bottom(), card.bottom()));
    // The ticks are the theme's length.
    assert_eq!(tl.size.width, crate::theme::px(8.));
}

#[gpui_kit::test]
fn the_shortcuts_sheet_and_the_context_menu_have_ticks_too(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("?", cx);
    assert!(h.shows("shortcuts-sheet-tick-tl", cx));
    h.press("escape", cx);
    h.press("ctrl-l", cx);
    h.press("shift-f10", cx);
    assert!(h.shows("context-menu-tick-br", cx), "the menu's corners");
}

#[gpui_kit::test]
fn the_focused_terminal_pane_keeps_its_outline_and_wears_accent_ticks(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        terminal_of(h, cx, 1).is_some_and(|t| t.screen_text().contains("READY>"))
    });
    // One pane: nothing to tell apart, no marks.
    assert!(!h.shows("pane-1-tick-tl", cx));
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    let _ = script_of(&h, 2);
    assert!(
        h.shows("pane-2-tick-tl", cx),
        "the focused pane has its ticks"
    );
    assert!(!h.shows("pane-1-tick-tl", cx), "the other one stays quiet");
}

#[gpui_kit::test]
fn zavu_draws_only_its_one_crosshair_and_none_of_the_new_elements(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    cx.update(|cx| settings::set_theme_id(cx, ThemeId::Zavu));
    h.settle(cx);
    assert!(h.shows("crosshair-header", cx), "today's crosshair");
    assert!(!h.shows("crosshair-footer", cx));
    assert!(
        !h.shows("main-empty-frame", cx),
        "no motif in an empty state"
    );
    h.press("ctrl-shift-p", cx);
    assert!(h.shows("palette", cx));
    assert!(!h.shows("palette-tick-tl", cx), "no corner ticks");
    // Its footer is as it was: the status strip is its own height.
    h.press("escape", cx);
    let status = h.bounds_of("status-strip".to_owned(), cx).unwrap();
    assert_eq!(status.size.height, px(28.));
}

#[gpui_kit::test]
fn an_empty_state_is_framed_and_the_frame_holds_the_text_not_the_other_way_round(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let frame = h
        .bounds_of("main-empty-frame".to_owned(), cx)
        .expect("the frame");
    let hint = h.bounds_of("hint-GoTo".to_owned(), cx).unwrap();
    assert!(
        hint.left() >= frame.left()
            && hint.right() <= frame.right()
            && hint.top() >= frame.top()
            && hint.bottom() <= frame.bottom(),
        "the text sits inside the frame: {hint:?} in {frame:?}"
    );
    let tick = h.bounds_of("main-empty-tick-tl".to_owned(), cx).unwrap();
    assert!(
        tick.right() < hint.left() || tick.bottom() < hint.top(),
        "a tick is never over text"
    );
    assert!(h.shows("main-empty-dimension", cx), "the dimension line");
}

#[gpui_kit::test]
fn a_mark_takes_no_mouse_event_so_a_drag_started_on_it_reaches_the_edge_under_it(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    // The crosshair of the header sits on the sidebar's resize edge.
    let at = h
        .bounds_of("crosshair-header".to_owned(), cx)
        .unwrap()
        .center();
    let edge = h.bounds_of("sidebar-resize".to_owned(), cx).unwrap();
    assert!(edge.contains(&at), "the mark is over the edge");
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    for x in [350., 370., 400.] {
        visual.simulate_mouse_move(
            gpui_kit::point(px(x), at.y),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
    }
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    let width = h.bounds_of("sidebar".to_owned(), cx).unwrap().size.width;
    assert!(
        width > px(380.),
        "the drag started on the mark moved the edge: {width:?}"
    );
}
