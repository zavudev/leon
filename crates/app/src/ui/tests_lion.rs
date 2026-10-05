//! Tests of the lion in the window: it is the mark wherever the mark shows,
//! in the theme's logo colour, its mood follows the sessions and the focus of
//! the window, and the settings still it. The scripted terminals of
//! `tests.rs` stand in for real ones.

use super::live::{
    open_live, real_worktree, screen, script_of, set_waiting_after, wait_until, HOUR,
};
use super::*;
use crate::schema::{self, Value};
use leon_mark::{probe, Mood, Pose, Variant};
use std::time::Duration;

fn lion(id: &str) -> probe::Painted {
    probe::painted(id).unwrap_or_else(|| panic!("{id} was not painted"))
}

/// A window the lion treats as the one in front: test windows start without
/// the focus.
fn in_front(h: &Harness, cx: &mut TestAppContext) {
    VisualTestContext::from_window(h.window.into(), cx)
        .update(|window, _| window.activate_window());
    h.settle(cx);
}

fn lit(cx: &mut TestAppContext) -> Harness {
    probe::clear();
    let h = open_live(cx);
    in_front(&h, cx);
    h
}

fn set(h: &Harness, cx: &mut TestAppContext, key: &str, value: Value) {
    cx.update(|cx| crate::settings::set_value(cx, schema::find(key).unwrap(), value));
    cx.update(|cx| h.shell.update(cx, |_, cx| cx.notify()));
    h.settle(cx);
}

fn start_agent(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code: prints, and takes the terminal
    wait_until(h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
}

#[gpui_kit::test]
fn the_header_has_the_lion_in_the_logo_colour_of_the_theme(cx: &mut TestAppContext) {
    let h = lit(cx);
    assert!(
        h.shows("header-mark", cx),
        "the sidebar header carries the lion"
    );
    let painted = lion("header-mark");
    assert!(painted.paints >= 1);
    let expected = cx.update(|cx| crate::theme::palette(cx).logo);
    assert_eq!(painted.color, expected);
}

#[gpui_kit::test]
fn the_lion_takes_the_logo_colour_of_every_theme_and_appearance(cx: &mut TestAppContext) {
    let h = lit(cx);
    for theme in ["leon", "zavu"] {
        for appearance in ["dark", "light"] {
            set(&h, cx, "theme_id", Value::Text(theme.to_owned()));
            set(&h, cx, "theme", Value::Text(appearance.to_owned()));
            let expected = cx.update(|cx| crate::theme::palette(cx).logo);
            assert_eq!(lion("header-mark").color, expected, "{theme} {appearance}");
        }
    }
}

#[gpui_kit::test]
fn the_lion_is_the_fitted_drawing_at_twenty_pixels_and_the_full_one_in_the_about_card(
    cx: &mut TestAppContext,
) {
    let h = lit(cx);
    assert_eq!(lion("header-mark").variant, Variant::Small);
    h.press("ctrl-shift-p", cx);
    h.type_text("about", cx);
    h.press("enter", cx);
    assert!(h.shows("about-mark", cx), "the About card carries the lion");
    assert_eq!(lion("about-mark").variant, Variant::Full);
    assert_eq!(lion("about-mark").mood, Mood::Idle);
}

#[gpui_kit::test]
fn the_main_header_keeps_the_lion_when_the_sidebar_is_away(cx: &mut TestAppContext) {
    let h = lit(cx);
    assert!(!h.shows("main-header-mark", cx), "the sidebar has it");
    h.press("ctrl-b", cx);
    assert!(
        h.shows("main-header-mark", cx),
        "the main pane header has it now"
    );
    assert!(
        lion("main-header-mark").hit_testing,
        "and it answers the pointer"
    );
}

#[gpui_kit::test]
fn the_header_lion_narrows_its_eyes_when_the_pointer_arrives(cx: &mut TestAppContext) {
    let h = lit(cx);
    assert!(
        lion("header-mark").hit_testing,
        "it listens for the pointer"
    );
    let at = h
        .bounds_of("header-mark".to_owned(), cx)
        .expect("drawn")
        .center();
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    visual.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
    visual.run_until_parked();
    assert_eq!(lion("header-mark").pokes, 1);
}

#[gpui_kit::test]
fn the_lion_follows_the_sessions_working_waiting_and_failing_and_then_rests(
    cx: &mut TestAppContext,
) {
    let h = lit(cx);
    let _real = real_worktree(&h, cx);
    set_waiting_after(&h, cx, HOUR);
    h.press("ctrl-n", cx);
    assert_eq!(lion("header-mark").mood, Mood::Idle, "nothing is going on");

    // An agent that prints: working.
    h.press("enter", cx);
    wait_until(&h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    assert_eq!(lion("header-mark").mood, Mood::Working);

    // Quiet beyond the threshold: it wants the user.
    set_waiting_after(&h, cx, Duration::ZERO);
    assert_eq!(lion("header-mark").mood, Mood::Waiting);

    // A plain shell in the same worktree that prints beats waiting.
    set_waiting_after(&h, cx, HOUR);
    script_of(&h, 1).print("more output\n");
    h.settle(cx);
    assert_eq!(lion("header-mark").mood, Mood::Working);

    // The agent fails: a moment of error, then it settles.
    script_of(&h, 1).exit(3);
    h.settle(cx);
    assert_eq!(lion("header-mark").mood, Mood::Error);
    cx.executor().advance_clock(Duration::from_secs(5));
    h.settle(cx);
    assert_ne!(lion("header-mark").mood, Mood::Error, "the error was brief");
}

#[gpui_kit::test]
fn a_clean_exit_is_not_an_error_for_the_lion(cx: &mut TestAppContext) {
    let h = lit(cx);
    let _real = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        screen(h, cx, 1).contains("READY>")
    });
    script_of(&h, 1).exit(0);
    h.settle(cx);
    assert_ne!(lion("header-mark").mood, Mood::Error);
}

#[gpui_kit::test]
fn losing_the_focus_puts_the_lion_to_sleep_and_it_wakes_with_the_window(cx: &mut TestAppContext) {
    let h = lit(cx);
    let _real = real_worktree(&h, cx);
    start_agent(&h, cx);
    assert_eq!(lion("header-mark").mood, Mood::Working);

    let before = lion("header-mark");
    VisualTestContext::from_window(h.window.into(), cx).deactivate_window();
    h.settle(cx);
    let asleep = lion("header-mark");
    assert_eq!(asleep.mood, Mood::Asleep, "whatever the sessions do");
    assert!(asleep.pose.eye_open <= 0.15, "{:?}", asleep.pose);
    assert!(!asleep.live, "and it asks the window for nothing");
    assert_eq!(asleep.frames_requested, before.frames_requested);
    assert_eq!(asleep.timers_set, before.timers_set);

    in_front(&h, cx);
    assert_eq!(lion("header-mark").mood, Mood::Working, "awake again");
}

#[gpui_kit::test]
fn the_entrance_plays_once_when_the_window_first_has_the_focus(cx: &mut TestAppContext) {
    probe::clear();
    let h = open_live(cx);
    assert!(
        h.shell(cx, |shell| shell.intro_played.get()),
        "it was asked for"
    );
    // The window opened without the focus: the entrance waits for it.
    assert_eq!(lion("header-mark").frames_requested, 0);
    in_front(&h, cx);
    assert!(
        lion("header-mark").frames_requested >= 1,
        "the entrance plays"
    );

    // Hiding and showing the sidebar makes a new mark, which does not play it.
    h.press("ctrl-b", cx);
    h.press("ctrl-b", cx);
    let painted = lion("header-mark");
    assert_eq!(
        (painted.pose.plate, painted.pose.glow),
        (1., 0.),
        "{:?}",
        painted.pose
    );
}

#[gpui_kit::test]
fn turning_animate_the_lion_off_keeps_it_at_rest_and_asks_for_nothing(cx: &mut TestAppContext) {
    let h = lit(cx);
    let _real = real_worktree(&h, cx);
    set(&h, cx, "animate_lion", Value::Bool(false));
    start_agent(&h, cx);
    let painted = lion("header-mark");
    assert!(!painted.live);
    assert_eq!(painted.pose, Pose::REST, "the owner's drawing, exactly");
    let (frames, timers) = (painted.frames_requested, painted.timers_set);
    script_of(&h, 1).print("more output\n");
    h.settle(cx);
    assert_eq!(lion("header-mark").pose, Pose::REST);
    assert_eq!(lion("header-mark").frames_requested, frames);
    assert_eq!(lion("header-mark").timers_set, timers);

    set(&h, cx, "animate_lion", Value::Bool(true));
    assert!(
        lion("header-mark").live,
        "and it moves again when turned on"
    );
}

#[gpui_kit::test]
fn reduce_motion_on_stills_the_lion_and_off_lets_it_move_against_the_system(
    cx: &mut TestAppContext,
) {
    let h = lit(cx);
    set(&h, cx, "reduce_motion", Value::Text("on".to_owned()));
    assert!(!lion("header-mark").live);
    assert_eq!(lion("header-mark").pose, Pose::REST);

    // The system asks for reduced motion; "off" is the user saying no.
    cx.update(|cx| cx.set_reduce_motion(true));
    set(&h, cx, "reduce_motion", Value::Text("off".to_owned()));
    assert!(lion("header-mark").live);
    // And "follow the system" follows it.
    set(&h, cx, "reduce_motion", Value::Text("system".to_owned()));
    assert!(!lion("header-mark").live);
    cx.update(|cx| cx.set_reduce_motion(false));
    set(&h, cx, "reduce_motion", Value::Text("system".to_owned()));
    assert!(lion("header-mark").live);
}

#[gpui_kit::test]
fn both_settings_are_in_the_schema_with_the_defaults_the_brief_asks_for(cx: &mut TestAppContext) {
    let animate = schema::find("animate_lion").expect("the lion can be turned off");
    assert_eq!(animate.label, "Animate the lion");
    assert_eq!(animate.default, schema::Initial::Bool(true));
    let reduce = schema::find("reduce_motion").expect("motion can be reduced");
    assert_eq!(reduce.label, "Reduce motion");
    assert_eq!(reduce.default, schema::Initial::Text("system"));
    let schema::Kind::Choice(schema::Choices::Fixed(choices)) = reduce.kind else {
        panic!("a fixed choice")
    };
    let values: Vec<&str> = choices.iter().map(|(value, _)| *value).collect();
    assert_eq!(values, ["system", "on", "off"]);
    let _ = cx;
}
