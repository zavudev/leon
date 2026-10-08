//! Tests of the shelves and the undo: settling, snoozing and bringing back a
//! session, the banner that offers Undo, the early end of a snooze and the
//! setting that settles merged work. Time is the test clock of `tests.rs`
//! (`set_clock`) and the toolkit's executor clock.

use super::*;
use crate::keys::Command;
use crate::schema::{self, Value};
use crate::ui::shelf::ShelfKind;
use leon_core::{SessionId, SessionScope, Shelf};
use leon_remote::{CommandSpec, RunError, Runner};
use std::time::Duration;

/// A computer with no other terminals on it: every scan finds nothing, which
/// is still a scan, and what a scan says is what lets merged work be settled.
struct Quiet;

impl Runner for Quiet {
    async fn run(&self, _: &CommandSpec) -> Result<Output, RunError> {
        Ok(Output::ok("now=1000000\n".to_owned()))
    }
}

/// Lets the window look at the other terminals of this computer, and looks.
fn scan_quietly(h: &Harness, cx: &mut TestAppContext) {
    h.engine.set_process_scanner(Arc::new(Quiet), None);
    h.engine.submit(crate::engine::Op::Scan(MachineId::local()));
    h.settle(cx);
}

fn settle_merged_on(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| {
        settings::set_value(
            cx,
            schema::find("sidebar_settle_merged").unwrap(),
            Value::Bool(true),
        )
    });
    cx.update(|cx| h.shell.update(cx, |shell, cx| shell.sync_settings(cx)));
    h.settle(cx);
}

fn session_id(h: &Harness, title: &str) -> SessionId {
    h.store
        .recent_sessions(&leon_core::SessionFilter::default(), 50)
        .unwrap()
        .into_iter()
        .find(|session| session.title == title)
        .unwrap_or_else(|| panic!("no session called {title}"))
        .id
}

fn cursor_on(h: &Harness, cx: &mut TestAppContext, node: NodeId) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&node);
            shell.pane = Pane::Sidebar;
        })
    });
    h.settle(cx);
}

/// Runs a command as its chord, the palette or the menu would.
fn run(h: &Harness, cx: &mut TestAppContext, command: Command) {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.run_command(command, window, cx);
        })
    })
    .unwrap();
    h.settle(cx);
}

fn shelf_of(h: &Harness, id: &SessionId) -> Option<Shelf> {
    h.store.shelves().unwrap().get(id).copied()
}

fn banner(h: &Harness, cx: &mut TestAppContext) -> Option<String> {
    h.shell(cx, |shell| {
        shell
            .shelves
            .undo
            .as_ref()
            .map(|pending| pending.text.clone())
    })
}

fn shelf_row(h: &Harness, cx: &mut TestAppContext, kind: ShelfKind) -> Option<usize> {
    h.row_of(NodeId::Shelf(MachineId::local(), kind), cx)
}

fn open_shelf(h: &Harness, cx: &mut TestAppContext, kind: ShelfKind) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let key = NodeId::Shelf(MachineId::local(), kind).key();
            shell.set_open(&key, true);
            shell.rebuild_rows();
            cx.notify();
        })
    });
    h.settle(cx);
}

fn undo_chord(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-alt-z", "ctrl-shift-alt-z", cx);
}

#[gpui_kit::test]
fn settling_a_session_moves_it_to_a_folded_shelf_and_undo_puts_it_back(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    run(&h, cx, Command::SettleSession);

    assert_eq!(shelf_of(&h, &login), Some(Shelf::Settled));
    let shelf = shelf_row(&h, cx, ShelfKind::Settled).expect("the Settled shelf is in the tree");
    assert!(
        h.row_of(NodeId::Session(login.clone()), cx).is_none(),
        "folded: the session is behind its shelf"
    );
    let outline = h.outline(cx);
    assert_eq!(
        outline[shelf + 1],
        "machine:build box",
        "the shelf is at the bottom of its machine: {outline:?}"
    );
    assert!(h.shows("undo-banner", cx));
    assert_eq!(
        banner(&h, cx).as_deref(),
        Some("Settled fix the login bug.")
    );

    // Opened, the shelf holds the session.
    open_shelf(&h, cx, ShelfKind::Settled);
    assert!(h.row_of(NodeId::Session(login.clone()), cx).is_some());

    // Undo, on its chord, puts the session back in its list. It had no row, and
    // it gets the one of a session taken back by hand, so that the automatic
    // settling does not take it again.
    undo_chord(&h, cx);
    assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
    assert!(shelf_row(&h, cx, ShelfKind::Settled).is_none());
    assert!(h.row_of(NodeId::Session(login), cx).is_some());
    assert!(!h.shows("undo-banner", cx));
}

#[gpui_kit::test]
fn the_banner_goes_after_five_seconds_and_then_there_is_nothing_to_undo(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    run(&h, cx, Command::SettleSession);
    assert!(banner(&h, cx).is_some());
    cx.executor().advance_clock(Duration::from_millis(4_500));
    h.settle(cx);
    assert!(
        banner(&h, cx).is_some(),
        "still on offer at four and a half seconds"
    );
    cx.executor().advance_clock(Duration::from_secs(1));
    h.settle(cx);
    assert!(banner(&h, cx).is_none());
    assert!(!h.shows("undo-banner", cx));
    undo_chord(&h, cx);
    assert!(h.status().contains("nothing to undo"), "{}", h.status());
    assert_eq!(shelf_of(&h, &login), Some(Shelf::Settled));
}

#[gpui_kit::test]
fn the_banners_button_undoes_with_a_click(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    run(&h, cx, Command::SettleSession);
    h.mouse_on("undo-button".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
    assert!(shelf_row(&h, cx, ShelfKind::Settled).is_none());
}

#[gpui_kit::test]
fn snoozing_asks_until_when_and_the_row_says_when_it_comes_back(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    run(&h, cx, Command::SnoozeSession);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"In an hour".to_owned()), "{titles:?}");
    assert!(
        titles.contains(&"Tomorrow morning".to_owned()),
        "{titles:?}"
    );

    h.type_text("2h", cx);
    h.press("enter", cx);
    let until = fixed_now() + ChronoDuration::hours(2);
    assert_eq!(shelf_of(&h, &login), Some(Shelf::Snoozed(until)));
    assert!(shelf_row(&h, cx, ShelfKind::Snoozed).is_some());
    assert_eq!(
        banner(&h, cx).as_deref(),
        Some("Snoozed fix the login bug until 2026-10-04 14:05.")
    );

    open_shelf(&h, cx, ShelfKind::Snoozed);
    let at = h.row_of(NodeId::Session(login.clone()), cx).unwrap();
    assert!(
        h.shows_dynamic(format!("tree-age-{at}"), cx),
        "the row says when it is back"
    );

    // Undo returns it to its list, as one taken back by hand.
    undo_chord(&h, cx);
    assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
    assert!(shelf_row(&h, cx, ShelfKind::Snoozed).is_none());
}

#[gpui_kit::test]
fn a_time_that_cannot_be_read_is_refused_and_nothing_is_snoozed(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    run(&h, cx, Command::SnoozeSession);
    h.type_text("whenever", cx);
    h.press("enter", cx);
    assert_eq!(shelf_of(&h, &login), None);
    assert!(h.status().contains("Say when"), "{}", h.status());
}

#[gpui_kit::test]
fn a_snooze_ends_by_itself_when_its_time_comes(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    h.store
        .set_shelves(&[(
            login.clone(),
            Some(Shelf::Snoozed(fixed_now() + ChronoDuration::minutes(30))),
        )])
        .unwrap();
    h.settle(cx);
    assert!(h.row_of(NodeId::Session(login.clone()), cx).is_none());
    assert!(shelf_row(&h, cx, ShelfKind::Snoozed).is_some());

    // The clock passes the time and the timer, which looks at least once a
    // minute, places the sessions again.
    set_clock(fixed_now() + ChronoDuration::minutes(31));
    cx.executor().advance_clock(Duration::from_secs(61));
    h.settle(cx);
    assert!(h.row_of(NodeId::Session(login), cx).is_some());
    assert!(shelf_row(&h, cx, ShelfKind::Snoozed).is_none());
}

#[gpui_kit::test]
fn bringing_a_session_back_takes_it_off_its_shelf_and_keeps_the_automatic_settling_away(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    h.store
        .set_shelves(&[(login.clone(), Some(Shelf::Settled))])
        .unwrap();
    h.settle(cx);
    open_shelf(&h, cx, ShelfKind::Settled);
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    let at = h.row_of(NodeId::Session(login.clone()), cx).unwrap();
    let menu = h.shell(cx, |shell| shell.menu_for_test(at));
    assert!(menu.contains(&"Bring back".to_owned()), "{menu:?}");
    assert!(!menu.contains(&"Settle".to_owned()), "{menu:?}");
    run(&h, cx, Command::BringBackSession);
    assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
    assert!(shelf_row(&h, cx, ShelfKind::Settled).is_none());
    // The menu of a row off the shelves offers to settle and snooze it.
    let at = h.row_of(NodeId::Session(login), cx).unwrap();
    let menu = h.shell(cx, |shell| shell.menu_for_test(at));
    assert!(menu.contains(&"Settle".to_owned()), "{menu:?}");
    assert!(menu.contains(&"Snooze\u{2026}".to_owned()), "{menu:?}");
}

#[gpui_kit::test]
fn the_shelves_are_read_from_the_store_when_the_window_opens(cx: &mut TestAppContext) {
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        None,
        Picked::Cancelled,
        |store| {
            let sessions = store
                .recent_sessions(&leon_core::SessionFilter::default(), 50)
                .unwrap();
            let login = sessions
                .iter()
                .find(|s| s.title == "fix the login bug")
                .unwrap();
            let oauth = sessions.iter().find(|s| s.title == "add oauth").unwrap();
            store
                .set_shelves(&[
                    (login.id.clone(), Some(Shelf::Settled)),
                    (
                        oauth.id.clone(),
                        Some(Shelf::Snoozed(fixed_now() + ChronoDuration::days(1))),
                    ),
                ])
                .unwrap();
        },
    );
    let login = session_id(&h, "fix the login bug");
    let oauth = session_id(&h, "add oauth");
    assert!(shelf_row(&h, cx, ShelfKind::Settled).is_some());
    assert!(shelf_row(&h, cx, ShelfKind::Snoozed).is_some());
    assert!(h.row_of(NodeId::Session(login.clone()), cx).is_none());
    assert!(h.row_of(NodeId::Session(oauth.clone()), cx).is_none());
    open_shelf(&h, cx, ShelfKind::Settled);
    open_shelf(&h, cx, ShelfKind::Snoozed);
    assert!(h.row_of(NodeId::Session(login), cx).is_some());
    assert!(h.row_of(NodeId::Session(oauth), cx).is_some());
}

#[gpui_kit::test]
fn the_shelves_are_in_sight_with_only_the_active_sessions_listed(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    h.store
        .set_shelves(&[(login.clone(), Some(Shelf::Settled))])
        .unwrap();
    cx.update(|cx| {
        settings::set_value(
            cx,
            schema::find("sidebar_show_inactive").unwrap(),
            Value::Bool(false),
        );
        h.shell.update(cx, |shell, cx| shell.sync_settings(cx));
    });
    h.settle(cx);
    assert!(
        shelf_row(&h, cx, ShelfKind::Settled).is_some(),
        "the person put it there: it stays in sight"
    );
    let rotate = session_id(&h, "rotate the keys");
    assert!(
        h.row_of(NodeId::Session(rotate), cx).is_none(),
        "an inactive session that is on no shelf is hidden"
    );
    open_shelf(&h, cx, ShelfKind::Settled);
    assert!(h.row_of(NodeId::Session(login), cx).is_some());
}

#[gpui_kit::test]
fn unpinning_one_session_does_not_unpin_another_that_is_on_a_shelf(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    let oauth = session_id(&h, "add oauth");
    h.store
        .pin_sessions(
            &SessionScope::Machine(MachineId::local()),
            &[login.clone(), oauth.clone()],
        )
        .unwrap();
    h.store
        .set_shelves(&[(oauth.clone(), Some(Shelf::Settled))])
        .unwrap();
    h.settle(cx);
    cursor_on(&h, cx, NodeId::Session(login.clone()));
    run(&h, cx, Command::UnpinSession);
    assert!(h.store.session(&login).unwrap().sort_order.is_none());
    assert!(
        h.store.session(&oauth).unwrap().sort_order.is_some(),
        "the settled session keeps its pin"
    );
    // Undo puts the pin back, the shelved one still in place.
    undo_chord(&h, cx);
    assert!(h.store.session(&login).unwrap().sort_order.is_some());
    assert!(h.store.session(&oauth).unwrap().sort_order.is_some());
}

#[gpui_kit::test]
fn merged_work_is_settled_when_the_setting_is_on_and_only_then(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let oauth = session_id(&h, "add oauth");
    let worktree = worktree_id(&h, "feature/login");
    h.store.set_merged(&worktree, Some(true)).unwrap();
    h.settle(cx);
    assert_eq!(shelf_of(&h, &oauth), None, "off by default: nothing moves");
    assert!(shelf_row(&h, cx, ShelfKind::Settled).is_none());

    settle_merged_on(&h, cx);
    assert_eq!(
        shelf_of(&h, &oauth),
        None,
        "Leon has not looked for other terminals yet: nothing is claimed, nothing is settled"
    );
    scan_quietly(&h, cx);
    assert_eq!(shelf_of(&h, &oauth), Some(Shelf::Settled));
    let login = session_id(&h, "fix the login bug");
    assert_eq!(
        shelf_of(&h, &login),
        None,
        "the main worktree is not merged work"
    );

    // Taken back by hand, it stays where the person put it.
    open_shelf(&h, cx, ShelfKind::Settled);
    cursor_on(&h, cx, NodeId::Session(oauth.clone()));
    run(&h, cx, Command::BringBackSession);
    h.settle(cx);
    assert_eq!(shelf_of(&h, &oauth), Some(Shelf::Returned));
}

#[gpui_kit::test]
fn an_undone_settle_is_not_settled_again_by_the_automatic_settling(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let oauth = session_id(&h, "add oauth");
    let worktree = worktree_id(&h, "feature/login");
    h.store.set_merged(&worktree, Some(true)).unwrap();
    cursor_on(&h, cx, NodeId::Session(oauth.clone()));
    run(&h, cx, Command::SettleSession);
    assert_eq!(shelf_of(&h, &oauth), Some(Shelf::Settled));

    scan_quietly(&h, cx);
    settle_merged_on(&h, cx);
    undo_chord(&h, cx);
    h.settle(cx);
    assert_eq!(
        shelf_of(&h, &oauth),
        Some(Shelf::Returned),
        "back in its list, and left alone"
    );
    assert!(shelf_row(&h, cx, ShelfKind::Settled).is_none());
    assert!(h.row_of(NodeId::Session(oauth), cx).is_some());
}

#[gpui_kit::test]
fn merged_work_is_not_settled_on_a_machine_whose_scan_failed(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let oauth = session_id(&h, "add oauth");
    let worktree = worktree_id(&h, "feature/login");
    h.store.set_merged(&worktree, Some(true)).unwrap();
    // The listing of processes fails: a scan forgets what it found before and
    // claims nothing.
    struct Failing;
    impl Runner for Failing {
        async fn run(&self, _: &CommandSpec) -> Result<Output, RunError> {
            Ok(Output::failed(3, "ps: not found"))
        }
    }
    h.engine.set_process_scanner(Arc::new(Failing), None);
    settle_merged_on(&h, cx);
    h.engine.submit(crate::engine::Op::Scan(MachineId::local()));
    h.settle(cx);
    assert!(h.engine.elsewhere(&MachineId::local()).is_none());
    assert_eq!(shelf_of(&h, &oauth), None);
}

#[gpui_kit::test]
fn unpinning_a_session_that_is_not_pinned_offers_nothing_and_keeps_the_banner(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let login = session_id(&h, "fix the login bug");
    let oauth = session_id(&h, "add oauth");
    cursor_on(&h, cx, NodeId::Session(login));
    run(&h, cx, Command::SettleSession);
    cursor_on(&h, cx, NodeId::Session(oauth));
    run(&h, cx, Command::UnpinSession);
    assert_eq!(
        banner(&h, cx).as_deref(),
        Some("Settled fix the login bug.")
    );
}

#[gpui_kit::test]
fn undo_with_nothing_on_offer_says_so(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    run(&h, cx, Command::Undo);
    assert!(h.status().contains("nothing to undo"), "{}", h.status());
}

#[cfg(leon_posix_tests)]
mod live_sessions {
    use super::super::live::{open_live, real_worktree, script_of};
    use super::*;
    use crate::ui::live::LiveId;

    /// Starts a shell session in the real worktree, links it to the history
    /// session called `title` and puts the keyboard on its row.
    fn shell_linked_to(h: &Harness, cx: &mut TestAppContext, title: Option<&str>) -> LiveId {
        h.press_chord("cmd-t", "ctrl-shift-t", cx);
        let id = h.shell(cx, |shell| {
            *shell.live.ids().last().expect("a shell started")
        });
        if let Some(title) = title {
            let history = session_id(h, title);
            cx.update(|cx| {
                h.shell.update(cx, |shell, _| {
                    shell.live.get_mut(id).unwrap().history = Some(history);
                })
            });
        }
        id
    }

    #[gpui_kit::test]
    fn settling_is_refused_while_the_session_runs(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        shell_linked_to(&h, cx, Some("fix the login bug"));
        let login = session_id(&h, "fix the login bug");
        cursor_on(&h, cx, NodeId::Session(login.clone()));
        run(&h, cx, Command::SettleSession);
        assert_eq!(shelf_of(&h, &login), None);
        assert!(
            h.status().contains("running session is not settled"),
            "{}",
            h.status()
        );
        // The menu does not offer it either.
        let at = h.row_of(NodeId::Session(login), cx).unwrap();
        let menu = h.shell(cx, |shell| shell.menu_for_test(at));
        assert!(!menu.contains(&"Settle".to_owned()), "{menu:?}");
        assert!(menu.contains(&"Snooze\u{2026}".to_owned()), "{menu:?}");
    }

    #[gpui_kit::test]
    fn a_terminal_that_only_learned_its_session_counts_as_running_for_settling(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, None);
        let login = session_id(&h, "fix the login bug");
        let stored = h.store.session(&login).unwrap();
        // No link was made when it started: the agent's id was learned later.
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                let live = shell.live.get_mut(id).unwrap();
                assert!(live.history.is_none());
                live.agent = Some(stored.agent);
                live.learned = Some((stored.external_id.clone(), "state-file".to_owned()));
            })
        });
        h.settle(cx);
        cursor_on(&h, cx, NodeId::Session(login.clone()));
        run(&h, cx, Command::SettleSession);
        assert_eq!(shelf_of(&h, &login), None);
        assert!(
            h.status().contains("running session is not settled"),
            "{}",
            h.status()
        );
    }

    #[gpui_kit::test]
    fn quitting_inside_the_window_removes_a_closed_sessions_history(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, Some("fix the login bug"));
        let login = session_id(&h, "fix the login bug");
        cursor_on(&h, cx, NodeId::Live(id));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert!(h.store.session(&login).is_ok(), "waiting for the banner");
        // The application ends with the banner up and nothing left to run.
        cx.update(|cx| h.shell.update(cx, |shell, cx| shell.flush(cx)));
        assert!(
            h.store.session(&login).is_err(),
            "removed before flush returned"
        );
        assert!(h.shell(cx, |s| s.shelves.undo.is_none()));
    }

    #[gpui_kit::test]
    fn undoing_the_close_of_a_shell_adds_a_sleeping_row_after_the_others(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_real, path) = real_worktree(&h, cx);
        // Two sleeping rows, the first of which is gone: the numbers in use
        // do not start at zero.
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                let local = MachineId::local();
                shell.dormant.add(&local, "/srv/one", None, None, None);
                shell.dormant.add(&local, "/srv/two", None, None, None);
                let first = shell.dormant.all()[0].id();
                shell.dormant.remove(first);
            })
        });
        let id = shell_linked_to(&h, cx, None);
        cursor_on(&h, cx, NodeId::Live(id));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        undo_chord(&h, cx);
        h.shell(cx, |s| {
            let cwds: Vec<&str> = s.dormant.all().iter().map(|d| d.cwd.as_str()).collect();
            assert_eq!(cwds, ["/srv/two", path.as_str()]);
        });
    }

    #[gpui_kit::test]
    fn a_snoozed_session_that_fails_finishes_or_waits_is_back_even_with_notifications_off(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        cx.update(|cx| {
            settings::set_value(cx, schema::find("notify").unwrap(), Value::Bool(false))
        });
        let login = session_id(&h, "fix the login bug");
        let oauth = session_id(&h, "add oauth");
        let first = shell_linked_to(&h, cx, Some("fix the login bug"));
        let second = shell_linked_to(&h, cx, Some("add oauth"));
        let until = fixed_now() + ChronoDuration::hours(5);
        h.store
            .set_shelves(&[
                (login.clone(), Some(Shelf::Snoozed(until))),
                (oauth.clone(), Some(Shelf::Snoozed(until))),
            ])
            .unwrap();
        h.settle(cx);
        assert_eq!(shelf_of(&h, &login), Some(Shelf::Snoozed(until)));

        // A failure ends the snooze of the first; the second is still away.
        script_of(&h, first.0).exit(3);
        h.settle(cx);
        assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
        assert_eq!(shelf_of(&h, &oauth), Some(Shelf::Snoozed(until)));
        assert!(h.row_of(NodeId::Session(login), cx).is_some());

        // A clean end ends it for the second.
        script_of(&h, second.0).exit(0);
        h.settle(cx);
        assert_eq!(shelf_of(&h, &oauth), Some(Shelf::Returned));
    }

    #[gpui_kit::test]
    fn a_session_that_needs_you_wakes_its_snooze(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let login = session_id(&h, "fix the login bug");
        let id = shell_linked_to(&h, cx, Some("fix the login bug"));
        let until = fixed_now() + ChronoDuration::hours(5);
        h.store
            .set_shelves(&[(login.clone(), Some(Shelf::Snoozed(until)))])
            .unwrap();
        h.settle(cx);
        cx.update(|cx| {
            h.shell.update(cx, |shell, cx| {
                shell.raise(crate::ui::notify::Event::Waiting, id, cx)
            })
        });
        h.settle(cx);
        assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
    }

    #[gpui_kit::test]
    fn a_terminal_waiting_to_be_resumed_does_not_end_a_snooze(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let login = session_id(&h, "fix the login bug");
        let id = shell_linked_to(&h, cx, Some("fix the login bug"));
        let until = fixed_now() + ChronoDuration::hours(5);
        h.store
            .set_shelves(&[(login.clone(), Some(Shelf::Snoozed(until)))])
            .unwrap();
        h.settle(cx);
        // A restored session is paused until the person resumes it.
        let raise = |h: &Harness, cx: &mut TestAppContext, pending: Option<&str>| {
            cx.update(|cx| {
                h.shell.update(cx, |shell, cx| {
                    shell.live.get_mut(id).unwrap().pending = pending.map(str::to_owned);
                    shell.raise(crate::ui::notify::Event::Waiting, id, cx)
                })
            });
            h.settle(cx);
        };
        raise(&h, cx, Some("claude --resume x"));
        assert_eq!(shelf_of(&h, &login), Some(Shelf::Snoozed(until)));
        raise(&h, cx, None);
        assert_eq!(shelf_of(&h, &login), Some(Shelf::Returned));
    }

    #[gpui_kit::test]
    fn closing_a_session_with_history_waits_for_the_banner_and_undo_keeps_it_asleep(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, Some("fix the login bug"));
        let login = session_id(&h, "fix the login bug");
        cursor_on(&h, cx, NodeId::Live(id));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 0);
        // The row is gone from the sidebar, the store still has the session,
        // and the banner says the program cannot come back.
        assert!(
            h.store.session(&login).is_ok(),
            "the history waits for the banner"
        );
        let outline = h.outline(cx);
        assert!(
            !outline
                .iter()
                .any(|line| line.contains("fix the login bug")),
            "{outline:?}"
        );
        let said = banner(&h, cx).expect("the banner offers undo");
        assert!(said.contains("cannot be brought back"), "{said}");
        assert!(said.contains("asleep"), "{said}");

        // Undo restores the row as a sleeping session.
        undo_chord(&h, cx);
        assert!(h.store.session(&login).is_ok());
        let outline = h.outline(cx);
        assert!(
            outline
                .iter()
                .any(|line| line.contains("session:fix the login bug")),
            "{outline:?}"
        );
        assert!(h.shell(cx, |s| s.slept.contains(&login)));
        assert_eq!(
            h.shell(cx, |s| s.live.ids().len()),
            0,
            "no program came back"
        );
    }

    #[gpui_kit::test]
    fn a_closed_sessions_history_is_removed_when_the_banner_goes(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, Some("fix the login bug"));
        let login = session_id(&h, "fix the login bug");
        cursor_on(&h, cx, NodeId::Live(id));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert!(h.store.session(&login).is_ok());
        cx.executor().advance_clock(Duration::from_secs(6));
        h.settle(cx);
        assert!(
            h.store.session(&login).is_err(),
            "closing is final once undo is over"
        );
    }

    #[gpui_kit::test]
    fn a_newer_step_makes_the_one_before_final(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, Some("fix the login bug"));
        let login = session_id(&h, "fix the login bug");
        let oauth = session_id(&h, "add oauth");
        cursor_on(&h, cx, NodeId::Live(id));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        cursor_on(&h, cx, NodeId::Session(oauth.clone()));
        run(&h, cx, Command::SettleSession);
        assert!(h.store.session(&login).is_err(), "the close is final now");
        assert_eq!(shelf_of(&h, &oauth), Some(Shelf::Settled));
    }

    #[gpui_kit::test]
    fn closing_a_plain_shell_is_undone_as_a_sleeping_row_and_sleeping_is_undone_by_waking(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_real, path) = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, None);
        cursor_on(&h, cx, NodeId::Live(id));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 0);
        assert!(
            h.shell(cx, |s| s.dormant.all().is_empty()),
            "closed for good"
        );
        assert!(banner(&h, cx).unwrap().contains("cannot be brought back"));
        undo_chord(&h, cx);
        h.shell(cx, |s| {
            assert_eq!(s.dormant.all().len(), 1);
            assert_eq!(s.dormant.all()[0].cwd, path);
        });

        // Wake it, then put it to sleep: Undo wakes it again.
        let asleep = h.shell(cx, |s| s.dormant.all()[0].id());
        cursor_on(&h, cx, NodeId::Live(asleep));
        h.press("enter", cx);
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 1);
        let woken = h.shell(cx, |s| s.live.ids()[0]);
        cursor_on(&h, cx, NodeId::Live(woken));
        run(&h, cx, Command::SleepSession);
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 0);
        assert_eq!(h.shell(cx, |s| s.dormant.all().len()), 1);
        assert!(banner(&h, cx).unwrap().contains("new program"));
        undo_chord(&h, cx);
        assert_eq!(
            h.shell(cx, |s| s.live.ids().len()),
            1,
            "woken with a new program"
        );
        assert!(h.shell(cx, |s| s.dormant.all().is_empty()));
    }

    #[gpui_kit::test]
    fn closing_a_sleeping_row_is_undone_with_the_same_place(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_real, _path) = real_worktree(&h, cx);
        let id = shell_linked_to(&h, cx, None);
        cursor_on(&h, cx, NodeId::Live(id));
        run(&h, cx, Command::SleepSession);
        let record = h.shell(cx, |s| s.dormant.all()[0].clone());
        cursor_on(&h, cx, NodeId::Live(record.id()));
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert!(h.shell(cx, |s| s.dormant.all().is_empty()));
        undo_chord(&h, cx);
        h.shell(cx, |s| {
            assert_eq!(s.dormant.all(), std::slice::from_ref(&record))
        });
    }
}
