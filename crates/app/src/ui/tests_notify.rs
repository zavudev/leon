//! Tests of the notifications: the geek banner in the window and the desktop
//! note when a session wants the user or ends. The scripted terminals of
//! `tests.rs` stand in for real ones.

use super::live::{
    open_live, real_worktree, screen, script_of, set_waiting_after, show_worktree_detail,
    wait_until, HOUR,
};
use super::*;
use crate::schema::{self, Value};
use crate::ui::live::LiveId;
use crate::ui::notify::{Kind, Note};
use std::time::Duration;

/// A window the shell treats as the one in front: test windows start without
/// the focus.
fn in_front(h: &Harness, cx: &mut TestAppContext) {
    VisualTestContext::from_window(h.window.into(), cx)
        .update(|window, _| window.activate_window());
    h.settle(cx);
}

/// The notes the shell showed in the window, oldest first.
fn banners(h: &Harness, cx: &mut TestAppContext) -> Vec<Note> {
    h.shell(cx, |shell| {
        shell
            .banners
            .iter()
            .map(|banner| banner.note.clone())
            .collect()
    })
}

/// The notes the shell handed to the desktop.
fn notes(h: &Harness) -> Vec<Note> {
    h.notes.borrow().clone()
}

fn set(h: &Harness, cx: &mut TestAppContext, key: &str, value: Value) {
    cx.update(|cx| crate::settings::set_value(cx, schema::find(key).unwrap(), value));
    cx.update(|cx| h.shell.update(cx, |_, cx| cx.notify()));
    h.settle(cx);
}

/// Starts an agent in the worktree and waits for it to print.
fn start_agent(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code: prints, and takes the terminal
    wait_until(h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
}

/// Starts a plain shell and waits for its prompt. The chord holds Shift so
/// that it is a command and not input while a terminal has the keyboard.
fn start_shell(h: &Harness, cx: &mut TestAppContext, id: u64) {
    h.press_chord("cmd-t", "ctrl-shift-t", cx);
    wait_until(h, cx, "the shell's prompt", |h, cx| {
        screen(h, cx, id).contains("READY>")
    });
}

#[gpui_kit::test]
fn a_session_that_goes_waiting_out_of_sight_is_said_in_the_window_and_on_the_desktop(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_real, path) = real_worktree(&h, cx);
    let folder = std::path::Path::new(&path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    in_front(&h, cx);
    set_waiting_after(&h, cx, HOUR);
    start_agent(&h, cx);

    // On screen and in front: the dot and the lion say it; nothing else.
    set_waiting_after(&h, cx, Duration::ZERO);
    assert!(
        banners(&h, cx).is_empty(),
        "the session on screen is its own notification"
    );
    assert!(notes(&h).is_empty(), "and the desktop is not told");

    // Out of sight the geek banner says it; the desktop waits for the focus.
    show_worktree_detail(&h, cx, "real", "trunk");
    set_waiting_after(&h, cx, HOUR);
    script_of(&h, 1).print("more output\n");
    h.settle(cx);
    set_waiting_after(&h, cx, Duration::ZERO);
    let said = banners(&h, cx);
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].kind, Kind::Waiting);
    assert_eq!(said[0].session, Some(LiveId(1)));
    assert_eq!(said[0].title, format!("Claude Code · {folder}"));
    assert!(said[0].body.contains("waiting for you"));
    assert!(
        h.shows_dynamic("notify-0".into(), cx),
        "the banner is drawn"
    );
    assert!(notes(&h).is_empty(), "in front: the desktop waits");

    // Away, a new turn that ends is told to the desktop as well.
    cx.update(|cx| h.shell.update(cx, |shell, _| shell.window_active = false));
    set_waiting_after(&h, cx, HOUR);
    script_of(&h, 1).print("more output\n");
    h.settle(cx);
    set_waiting_after(&h, cx, Duration::ZERO);
    let told = notes(&h);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].kind, Kind::Waiting);
    assert_eq!(told[0].session, Some(LiveId(1)));
    assert_eq!(
        banners(&h, cx).len(),
        1,
        "the same session's banner is refreshed, not stacked"
    );
}

#[gpui_kit::test]
fn the_end_of_a_session_is_said_with_its_code_and_a_failure_as_a_failure(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_real, path) = real_worktree(&h, cx);
    let folder = std::path::Path::new(&path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    start_shell(&h, cx, 1);
    start_shell(&h, cx, 2);
    show_worktree_detail(&h, cx, "real", "trunk");

    // A clean end.
    script_of(&h, 1).exit(0);
    h.settle(cx);
    let said = notes(&h);
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].kind, Kind::Finished);
    assert!(said[0].body.contains("exited cleanly"));
    assert!(said[0].body.contains("code 0"));
    let shown = banners(&h, cx);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].kind, Kind::Finished);
    assert_eq!(shown[0].title, format!("Shell · {folder}"));

    // A failure is marked as one.
    script_of(&h, 2).exit(2);
    h.settle(cx);
    let said = notes(&h);
    assert_eq!(said.len(), 2);
    assert_eq!(said[1].kind, Kind::Failed);
    assert!(said[1].body.contains("failed · code 2"));
    assert_eq!(banners(&h, cx).len(), 2);
}

#[gpui_kit::test]
fn the_settings_can_turn_the_events_and_the_ways_off(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _real = real_worktree(&h, cx);
    start_shell(&h, cx, 1);
    show_worktree_detail(&h, cx, "real", "trunk");

    // The master switch says nothing at all.
    set(&h, cx, "notify", Value::Bool(false));
    script_of(&h, 1).exit(0);
    h.settle(cx);
    assert!(notes(&h).is_empty());
    assert!(banners(&h, cx).is_empty());
    set(&h, cx, "notify", Value::Bool(true));

    // Desktop only: the banner is not drawn.
    set(&h, cx, "notify_how", Value::Text("system".to_owned()));
    start_shell(&h, cx, 2);
    show_worktree_detail(&h, cx, "real", "trunk");
    script_of(&h, 2).exit(2);
    h.settle(cx);
    assert_eq!(notes(&h).len(), 1, "the failure reached the desktop");
    assert!(banners(&h, cx).is_empty(), "no banner was asked for");

    // Banner only: the desktop is not asked.
    set(&h, cx, "notify_how", Value::Text("banner".to_owned()));
    start_shell(&h, cx, 3);
    show_worktree_detail(&h, cx, "real", "trunk");
    script_of(&h, 3).exit(0);
    h.settle(cx);
    assert_eq!(notes(&h).len(), 1, "the desktop was not asked again");
    assert_eq!(banners(&h, cx).len(), 1, "the banner was");

    // A clean end can be silenced without silencing a failure.
    set(&h, cx, "notify_finished", Value::Bool(false));
    start_shell(&h, cx, 4);
    show_worktree_detail(&h, cx, "real", "trunk");
    script_of(&h, 4).exit(0);
    h.settle(cx);
    assert_eq!(notes(&h).len(), 1, "a clean end was not asked for");
    assert_eq!(banners(&h, cx).len(), 1);
}

#[gpui_kit::test]
fn a_banner_goes_away_on_its_own_or_when_it_is_dismissed(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _real = real_worktree(&h, cx);
    start_shell(&h, cx, 1);
    show_worktree_detail(&h, cx, "real", "trunk");
    script_of(&h, 1).exit(0);
    h.settle(cx);
    assert_eq!(banners(&h, cx).len(), 1);
    assert!(h.shows_dynamic("notify-0".into(), cx));

    // The cross takes it away.
    h.mouse_on("notify-dismiss-0".into(), gpui_kit::MouseButton::Left, cx);
    assert!(banners(&h, cx).is_empty());
    assert!(!h.shows_dynamic("notify-0".into(), cx));

    // Another one goes away on its own.
    start_shell(&h, cx, 2);
    show_worktree_detail(&h, cx, "real", "trunk");
    script_of(&h, 2).exit(0);
    h.settle(cx);
    assert_eq!(banners(&h, cx).len(), 1);
    cx.executor().advance_clock(Duration::from_secs(31));
    h.settle(cx);
    assert!(banners(&h, cx).is_empty(), "it expired");
    assert!(!h.shows_dynamic("notify-1".into(), cx));
}
