//! The cleaner in the window: the machine's menu opens it, the keys move and tick, and
//! a worktree that would lose work cannot be ticked.

use super::*;
use crate::ui::sweep::Tab;

fn open_sweep(h: &Harness, cx: &mut TestAppContext) {
    h.pick_in_machine_menu(crate::keys::Command::CleanUp, cx);
    h.settle(cx);
}

#[gpui_kit::test]
fn the_machine_menu_opens_the_cleaner_and_escape_closes_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_sweep(&h, cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Cleanup);
    assert!(h.shows("sweep", cx));
    // The one linked worktree of the seed; the main ones are never listed.
    assert!(h.shows("sweep-worktree-0", cx));
    assert!(!h.shows_dynamic("sweep-worktree-1".to_owned(), cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn the_palette_offers_the_cleaner(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">clean up", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Cleanup);
}

#[gpui_kit::test]
fn a_worktree_nobody_read_cannot_be_ticked_and_a_clean_one_can(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_sweep(&h, cx);
    h.press("enter", cx);
    assert!(h.shell(cx, |s| s.sweep.ticks.is_empty()), "not read: kept");

    let linked = worktree_id(&h, "feature/login");
    h.store
        .update_worktree_status(&linked, |status| {
            status.changed = Some(0);
            status.divergence = Some(leon_core::Divergence::default());
        })
        .unwrap();
    h.settle(cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.sweep.ticks.len()), 1);
    h.press("enter", cx);
    assert!(h.shell(cx, |s| s.sweep.ticks.is_empty()), "Enter unticks");
}

#[gpui_kit::test]
fn tab_switches_to_the_old_sessions(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_sweep(&h, cx);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.sweep.tab), Tab::Sessions);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.sweep.tab), Tab::Worktrees);
}

#[gpui_kit::test]
fn what_is_typed_filters_the_list(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_sweep(&h, cx);
    h.type_text("login", cx);
    assert_eq!(h.shell(cx, |s| s.sweep.query.clone()), "login");
    assert!(h.shows("sweep-worktree-0", cx));
    h.type_text("zz", cx);
    assert!(!h.shows("sweep-worktree-0", cx));
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Cleanup);
}
