//! Remembering what is open and restoring it at the next start.
//!
//! A restart is simulated by reading the state the first window wrote into
//! its store and handing it to a second window as the previous run's state.
//! The terminals are the scripted computer of the other UI tests (a real
//! `/bin/sh` with a fake `claude`), nothing touches the user's agents.

use super::live::{open_live, real_worktree, screen, script_of, terminal_of, wait_until};
use super::*;
use crate::ui::live::LiveId;
use crate::ui::panes::Layout;
use leon_core::{SavedState, SavedTerminal, Slot};

fn settings_with(dir: &tempfile::TempDir, json: &str) -> std::path::PathBuf {
    let file = dir.path().join("settings.json");
    std::fs::write(&file, json).unwrap();
    file
}

fn saved_of(h: &Harness, cx: &mut TestAppContext) -> SavedState {
    cx.run_until_parked();
    h.store
        .load_workspace(Slot::Current)
        .unwrap()
        .expect("the window wrote its state")
}

/// A second window whose store holds `state` as the previous run's.
fn restart(
    cx: &mut TestAppContext,
    settings: Option<std::path::PathBuf>,
    state: &SavedState,
) -> Harness {
    cx.executor().allow_parking();
    let state = state.clone();
    open_full(
        cx,
        ScriptedRunner::new(),
        settings,
        Picked::Cancelled,
        move |store| store.save_workspace(Slot::Current, &state).unwrap(),
    )
}

fn terminal(id: u64, cwd: &str, agent: Option<&str>, session: Option<&str>) -> SavedTerminal {
    SavedTerminal {
        id,
        machine: MachineId::local().as_str().to_owned(),
        cwd: cwd.to_owned(),
        agent: agent.map(str::to_owned),
        session: session.map(str::to_owned),
        confidence: session.map(|_| "resumed".to_owned()),
        history: None,
        name: None,
        title: None,
        started_at: 0,
        account: None,
    }
}

fn one_tab_each(cwd: &str, agent: &str, session: Option<&str>, ids: &[u64]) -> SavedState {
    use leon_core::{SavedLayout, SavedTab, SavedWorkspace};
    SavedState {
        saved_at: 1,
        clean_shutdown: true,
        selection: None,
        main: Some(ids[0]),
        workspaces: vec![SavedWorkspace {
            key: crate::ui::workspace::key_of(MachineId::local().as_str(), cwd),
            active: 0,
            tabs: ids
                .iter()
                .map(|id| SavedTab {
                    layout: SavedLayout::Leaf(*id),
                    focus: *id,
                    zoomed: false,
                })
                .collect(),
        }],
        terminals: ids
            .iter()
            .map(|id| terminal(*id, cwd, Some(agent), session))
            .collect(),
    }
}

fn click(h: &Harness, selector: String, cx: &mut TestAppContext) {
    h.mouse_on(selector, gpui_kit::MouseButton::Left, cx);
}

fn live_count(h: &Harness, cx: &mut TestAppContext) -> usize {
    h.shell(cx, |s| s.live.ids().len())
}

#[gpui_kit::test]
fn every_change_to_the_terminals_is_written_with_layout_ratio_and_focus(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| {
            if let Some(tab) = s.workspaces.tab_of_mut(LiveId(1)) {
                if let Layout::Split { ratio, .. } = &mut tab.layout {
                    *ratio = 0.3;
                }
            }
            cx.notify();
        })
    });
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals.len(), 2);
    assert!(state
        .terminals
        .iter()
        .all(|t| t.cwd == path && t.agent.is_none()));
    assert_eq!(state.main, Some(2), "the focused pane");
    let tab = &state.workspaces[0].tabs[0];
    assert_eq!(tab.focus, 2);
    match &tab.layout {
        leon_core::SavedLayout::Split { ratio, axis, .. } => {
            assert!((ratio - 0.3).abs() < 1e-6 && axis == "row");
        }
        other => panic!("{other:?}"),
    }
    assert!(!state.clean_shutdown, "a running window is not a clean end");
}

#[gpui_kit::test]
fn a_clean_quit_marks_the_run_and_a_start_clears_the_mark(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    saved_of(&h, cx);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.flush(cx)));
    assert!(
        h.store
            .load_workspace(Slot::Current)
            .unwrap()
            .unwrap()
            .clean_shutdown
    );

    let h2 = restart(
        cx,
        None,
        &h.store.load_workspace(Slot::Current).unwrap().unwrap(),
    );
    assert!(
        !h2.store
            .load_workspace(Slot::Current)
            .unwrap()
            .unwrap()
            .clean_shutdown,
        "the new run is not over"
    );
}

#[gpui_kit::test]
fn ask_offers_the_sessions_and_restore_all_reopens_the_same_layout_and_focus(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| {
            if let Some(Layout::Split { ratio, .. }) =
                s.workspaces.tab_of_mut(LiveId(1)).map(|t| &mut t.layout)
            {
                *ratio = 0.3;
            }
            cx.notify();
        })
    });
    let mut state = saved_of(&h, cx);
    state.clean_shutdown = false;

    let h2 = restart(cx, None, &state);
    assert_eq!(h2.shell(cx, |s| s.overlay), Overlay::Restore);
    assert!(
        h2.shell(cx, |s| s.restore.unclean),
        "Leon did not close normally"
    );
    assert_eq!(live_count(&h2, cx), 0, "nothing starts before the answer");
    assert!(h2.shows("restore-all", cx) && h2.shows("restore-choose", cx));
    assert!(h2.shows("restore-not-now", cx));

    click(&h2, "restore-all".to_owned(), cx);
    wait_until(&h2, cx, "both shells", |h, cx| live_count(h, cx) == 2);
    assert_eq!(h2.shell(cx, |s| s.overlay), Overlay::None);
    let (layout, focus) = h2.shell(cx, |s| {
        let tab = s.workspaces.tab_of(LiveId(1)).expect("restored workspace");
        (tab.layout.clone(), tab.focus)
    });
    match layout {
        Layout::Split { ratio, .. } => assert!((ratio - 0.3).abs() < 1e-6),
        other => panic!("{other:?}"),
    }
    assert_eq!(focus, LiveId(2), "the same pane has the keyboard");
    assert_eq!(h2.main_kind(cx), "live:2");
}

#[gpui_kit::test]
fn choose_restores_only_the_rows_picked(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut state = one_tab_each(&cwd, "claude", Some("s1"), &[1, 2]);
    state.terminals[0].agent = None;
    state.terminals[1].agent = None;
    let h = restart(cx, None, &state);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Restore);
    click(&h, "restore-choose".to_owned(), cx);
    click(&h, "restore-row-1".to_owned(), cx);
    click(&h, "restore-chosen".to_owned(), cx);
    wait_until(&h, cx, "one shell", |h, cx| live_count(h, cx) == 1);
    assert_eq!(h.shell(cx, |s| s.live.all()[0].cwd.clone()), cwd);
}

#[gpui_kit::test]
fn not_now_keeps_the_sessions_for_the_palette_command(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut state = one_tab_each(&cwd, "claude", None, &[1]);
    state.terminals[0].agent = None;
    let h = restart(cx, None, &state);
    click(&h, "restore-not-now".to_owned(), cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(live_count(&h, cx), 0);
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">restore last", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"Restore last sessions".to_owned()));
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Restore);
    click(&h, "restore-all".to_owned(), cx);
    wait_until(&h, cx, "the shell", |h, cx| live_count(h, cx) == 1);
}

#[gpui_kit::test]
fn always_reopens_without_asking_and_never_asks_nothing(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut state = one_tab_each(&cwd, "claude", None, &[1]);
    state.terminals[0].agent = None;

    let always = settings_with(&dir, r#"{"restore_sessions": "always"}"#);
    let h = restart(cx, Some(always), &state);
    wait_until(&h, cx, "the shell", |h, cx| live_count(h, cx) == 1);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);

    let never = settings_with(&dir, r#"{"restore_sessions": "never"}"#);
    let h = restart(cx, Some(never), &state);
    assert_eq!(live_count(&h, cx), 0);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    // The command still brings the sessions back.
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">restore last", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Restore);
}

#[gpui_kit::test]
fn agents_are_paused_and_resume_when_their_tab_is_shown(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = one_tab_each(&cwd, "claude", Some("sid-7"), &[1, 2]);
    let always = settings_with(&dir, r#"{"restore_sessions": "always"}"#);
    let h = restart(cx, Some(always), &state);
    wait_until(&h, cx, "both shells", |h, cx| live_count(h, cx) == 2);
    // The focused tab (1) resumed at once; the other one waits.
    wait_until(&h, cx, "the shown agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume sid-7")
    });
    assert!(h.shell(cx, |s| s.live.get(LiveId(2)).unwrap().is_paused()));
    assert!(h.shell(cx, |s| s
        .live
        .get(LiveId(2))
        .unwrap()
        .label()
        .contains("paused")));
    assert!(
        !screen(&h, cx, 2).contains("FAKE-CLAUDE"),
        "{}",
        screen(&h, cx, 2)
    );

    // Showing its tab wakes it.
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| {
            s.pending_open_for_test(LiveId(2), cx);
        })
    });
    wait_until(&h, cx, "the second agent", |h, cx| {
        screen(h, cx, 2).contains("FAKE-CLAUDE --resume sid-7")
    });
    assert!(!h.shell(cx, |s| s.live.get(LiveId(2)).unwrap().is_paused()));
    let _ = terminal_of;
}

#[gpui_kit::test]
fn enter_in_a_paused_terminal_resumes_it_instead_of_reaching_the_shell(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = one_tab_each(&cwd, "claude", Some("sid-9"), &[1, 2]);
    let always = settings_with(&dir, r#"{"restore_sessions": "always"}"#);
    let h = restart(cx, Some(always), &state);
    wait_until(&h, cx, "both shells", |h, cx| live_count(h, cx) == 2);
    // Put the keyboard on the paused one without showing its tab.
    cx.update(|cx| {
        h.shell.update(cx, |s, _| {
            s.main = Main::Live(LiveId(2));
        })
    });
    assert!(h.shell(cx, |s| s.live.get(LiveId(2)).unwrap().is_paused()));
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 2).contains("FAKE-CLAUDE --resume sid-9")
    });
}

#[gpui_kit::test]
fn what_cannot_be_restored_is_listed_with_its_reason(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let gone = dir.path().join("deleted").to_string_lossy().into_owned();
    let launch_only = leon_core::agent::builtin()
        .iter()
        .find(|spec| !spec.can_resume() && spec.history.is_none())
        .map(|spec| spec.id.as_str().to_owned())
        .expect("the catalogue has an agent that is launch only");
    let mut state = one_tab_each(&cwd, "claude", Some("ok"), &[1, 2, 3, 4, 5]);
    state.terminals[1].cwd = gone;
    state.terminals[2].session = None;
    state.terminals[3].machine = "no-such-machine".into();
    state.terminals[4].agent = Some(launch_only);
    state.terminals[4].session = None;
    let always = settings_with(&dir, r#"{"restore_sessions": "always"}"#);
    let h = restart(cx, Some(always), &state);
    wait_until(&h, cx, "the report", |h, cx| {
        h.shell(cx, |s| s.restore.report_open)
    });
    let failures = h.shell(cx, |s| s.restore.failures.clone());
    let reasons: Vec<&str> = failures.iter().map(|(_, why)| why.as_str()).collect();
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(
        reasons.iter().any(|r| r.contains("no longer exists")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r.contains("never learned")),
        "{reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("machine is no longer known")),
        "{reasons:?}"
    );
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Restore);
    // The launch-only agent became a plain shell with a note, not an agent.
    assert_eq!(live_count(&h, cx), 2);
    let notes: Vec<String> = h.shell(cx, |s| s.live.all().iter().map(|l| l.label()).collect());
    assert!(
        notes.iter().any(|label| label.contains("now a shell")),
        "{notes:?}"
    );
    assert!(h.shows("restore-ok", cx));
    click(&h, "restore-ok".to_owned(), cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn the_previous_run_is_not_erased_by_an_empty_window_before_it_was_answered(
    cx: &mut TestAppContext,
) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut state = one_tab_each(&cwd, "claude", None, &[1]);
    state.terminals[0].agent = None;
    let h = restart(cx, None, &state);
    cx.run_until_parked();
    // While the question is open the stored Current is still the old one.
    let kept = h.store.load_workspace(Slot::Current).unwrap().unwrap();
    assert_eq!(kept.terminals.len(), 1);
    // And it is also archived.
    assert_eq!(
        h.store
            .load_workspace(Slot::Previous)
            .unwrap()
            .unwrap()
            .terminals
            .len(),
        1
    );
}

#[gpui_kit::test]
fn a_paused_session_never_notifies(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = one_tab_each(&cwd, "claude", Some("sid-1"), &[1, 2]);
    let always = settings_with(&dir, r#"{"restore_sessions": "always"}"#);
    let h = restart(cx, Some(always), &state);
    wait_until(&h, cx, "both shells", |h, cx| live_count(h, cx) == 2);
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| {
            s.raise_for_test(crate::ui::notify::Event::Waiting, LiveId(2), cx);
        })
    });
    assert!(
        h.shell(cx, |s| s.banners.is_empty()),
        "the paused one is silent"
    );
    assert!(h.notes.borrow().is_empty());
}

// ----- learning the session id and importing promptly ---------------------------------------

use crate::launch::Launch;
use crate::ui::terminals::Place;

fn start_fresh(h: &Harness, cx: &mut TestAppContext, agent: leon_core::AgentId, cwd: &str) {
    start_fresh_as(h, cx, agent, cwd, None);
}

/// [`start_fresh`] as an account of the agent (an id from the settings).
fn start_fresh_as(
    h: &Harness,
    cx: &mut TestAppContext,
    agent: leon_core::AgentId,
    cwd: &str,
    account: Option<&str>,
) {
    let cwd = cwd.to_owned();
    let account = account.map(str::to_owned);
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |s, cx| {
            s.start_live(
                Launch::Agent {
                    kind: agent,
                    resume: None,
                    account,
                },
                &MachineId::local(),
                &cwd,
                Place::Session,
                None,
                window,
                cx,
            )
        })
    })
    .unwrap();
    cx.run_until_parked();
}

fn new_session(h: &Harness, agent: leon_core::AgentId, external: &str, cwd: &str, minute: u32) {
    new_session_titled(h, agent, external, cwd, "t", minute);
}

fn new_session_titled(
    h: &Harness,
    agent: leon_core::AgentId,
    external: &str,
    cwd: &str,
    title: &str,
    minute: u32,
) {
    use chrono::TimeZone;
    let at = chrono::Utc
        .with_ymd_and_hms(2026, 10, 4, 12, minute, 0)
        .unwrap();
    h.store
        .upsert_session(
            &leon_core::NewSession {
                agent,
                external_id: external.to_owned(),
                machine_id: MachineId::local(),
                cwd: cwd.to_owned(),
                title: title.to_owned(),
                model: None,
                started_at: at,
                updated_at: at,
            },
            &[leon_core::NewMessage {
                role: leon_core::Role::User,
                text: "hi".to_owned(),
                at,
            }],
        )
        .unwrap();
}

#[gpui_kit::test]
fn a_fresh_agent_learns_its_session_id_and_the_tree_shows_one_row(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CODEX, &path);
    assert_eq!(
        h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().learned.clone()),
        None
    );
    // The importer stores the session the agent just wrote.
    new_session(&h, leon_core::AgentId::CODEX, "codex-new", &path, 6);
    h.settle(cx);
    let (learned, history) = h.shell(cx, |s| {
        let live = s.live.get(LiveId(1)).unwrap();
        (live.learned.clone(), live.history.clone())
    });
    assert_eq!(
        learned,
        Some(("codex-new".to_owned(), "newest-in-folder".to_owned()))
    );
    let history = history.expect("linked to the history row");
    assert!(
        h.shell(cx, |s| s.placement.merged.contains(&history)),
        "one row: live now, history later"
    );
    // The remembered state carries the id so a restore can resume it.
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals[0].session.as_deref(), Some("codex-new"));
    assert_eq!(
        state.terminals[0].confidence.as_deref(),
        Some("newest-in-folder")
    );
}

#[gpui_kit::test]
fn a_session_of_an_account_is_saved_with_it_and_tags_the_history_row_it_is_linked_to(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    cx.update(|cx| {
        settings::add_account(
            cx,
            leon_core::AgentId::CLAUDE,
            "Work",
            "CLAUDE_CONFIG_DIR=/acct/work",
        )
    })
    .unwrap();
    start_fresh_as(
        &h,
        cx,
        leon_core::AgentId::CLAUDE,
        &path,
        Some("claude-work"),
    );
    // What the next start reads back: the account is part of the saved terminal.
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals[0].account.as_deref(), Some("claude-work"));

    // The session the agent wrote is the terminal's, and its row is tagged by
    // the engine so that resuming it later starts the same account.
    new_session(&h, leon_core::AgentId::CLAUDE, "claude-new", &path, 6);
    h.settle(cx);
    assert!(h
        .shell(cx, |s| s.live.get(LiveId(1)).unwrap().history.clone())
        .is_some());
    let row = h
        .store
        .session_by_external(
            &MachineId::local(),
            leon_core::AgentId::CLAUDE,
            "claude-new",
        )
        .unwrap()
        .expect("imported");
    assert_eq!(row.account.as_deref(), Some("claude-work"));
}

fn settings_with_work_account(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let account = leon_core::Account {
        id: "claude-work".to_owned(),
        agent: leon_core::AgentId::CLAUDE,
        name: "Work".to_owned(),
        env: vec![("CLAUDE_CONFIG_DIR".to_owned(), "/acct/work".to_owned())],
    };
    let json = serde_json::json!({
        "restore_sessions": "always",
        "agent_accounts": [serde_json::to_string(&account).unwrap()],
    });
    settings_with(dir, &json.to_string())
}

#[gpui_kit::test]
fn a_saved_session_of_an_account_comes_back_as_that_account(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut state = one_tab_each(&cwd, "claude", Some("sid-7"), &[1]);
    state.terminals[0].account = Some("claude-work".to_owned());
    let file = settings_with_work_account(&dir);
    let h = restart(cx, Some(file), &state);
    wait_until(&h, cx, "the shell", |h, cx| live_count(h, cx) == 1);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume sid-7")
    });
    assert_eq!(
        h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().account.clone())
            .as_deref(),
        Some("claude-work")
    );
    assert!(script_of(&h, 1)
        .spec()
        .env
        .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/acct/work".to_owned())));
}

#[gpui_kit::test]
fn a_saved_session_of_a_removed_account_is_not_restored_as_another_one(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut state = one_tab_each(&cwd, "claude", Some("sid-7"), &[1]);
    state.terminals[0].account = Some("claude-gone".to_owned());
    let always = settings_with(&dir, r#"{"restore_sessions": "always"}"#);
    let h = restart(cx, Some(always), &state);
    cx.run_until_parked();
    h.settle(cx);
    assert_eq!(
        live_count(&h, cx),
        0,
        "nothing runs as the account that is gone, nor as the agent's own setup"
    );
}

#[gpui_kit::test]
fn two_terminals_of_one_agent_in_one_folder_are_not_guessed_onto_one_session(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CODEX, &path);
    start_fresh(&h, cx, leon_core::AgentId::CODEX, &path);
    new_session(&h, leon_core::AgentId::CODEX, "only-one", &path, 6);
    h.settle(cx);
    let learned: Vec<_> = h.shell(cx, |s| {
        s.live.all().iter().map(|l| l.learned.clone()).collect()
    });
    assert_eq!(learned, [None, None], "either could own it: nobody does");
}

#[gpui_kit::test]
fn a_terminal_whose_title_names_another_session_is_relinked_to_it(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::OPENCODE, &path);
    new_session_titled(
        &h,
        leon_core::AgentId::OPENCODE,
        "ses-old",
        &path,
        "Problema al interrumpir comandos",
        6,
    );
    h.settle(cx);
    let old = h
        .shell(cx, |s| s.live.get(LiveId(1)).unwrap().history.clone())
        .expect("the first session was linked");
    // The agent moves to another session in the same terminal, as opencode
    // does, and the title it sets says so (truncated, behind its `OC | `).
    new_session_titled(
        &h,
        leon_core::AgentId::OPENCODE,
        "ses-new",
        &path,
        "Aumentar el espacio entre secciones para mejorar la lectura",
        7,
    );
    h.settle(cx);
    script_of(&h, 1).print("\u{1b}]0;OC | Aumentar el espacio entre secciones p\u{2026}\u{7}");
    wait_until(&h, cx, "the title", |h, cx| {
        h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().title.is_some())
    });
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| s.learn_session_ids(cx));
    });
    h.settle(cx);
    let new = h
        .shell(cx, |s| s.live.get(LiveId(1)).unwrap().history.clone())
        .expect("the link followed the title");
    assert_ne!(old, new);
    assert!(
        h.shell(cx, |s| s.placement.merged.contains(&new)),
        "one row: the terminal is the new session's row"
    );
    assert!(!h.shell(cx, |s| s.placement.merged.contains(&old)));
    // A restore resumes the session on screen, not the old one.
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals[0].session.as_deref(), Some("ses-new"));
    assert_eq!(state.terminals[0].confidence.as_deref(), Some("title"));
}

#[gpui_kit::test]
fn opening_a_session_a_terminal_already_shows_lands_on_it(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::OPENCODE, &path);
    new_session_titled(
        &h,
        leon_core::AgentId::OPENCODE,
        "ses-old",
        &path,
        "old one",
        6,
    );
    h.settle(cx);
    new_session_titled(
        &h,
        leon_core::AgentId::OPENCODE,
        "ses-new",
        &path,
        "the new one",
        7,
    );
    h.settle(cx);
    // The title says which session the terminal shows while the learned link
    // is still the old one: the import that would relink it has not run yet,
    // which is the race this guards.
    script_of(&h, 1).print("\u{1b}]0;OC | the new one\u{7}");
    wait_until(&h, cx, "the title", |h, cx| {
        h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().title.is_some())
    });
    let learned = h.shell(cx, |s| {
        s.live
            .get(LiveId(1))
            .unwrap()
            .learned
            .as_ref()
            .map(|(id, _)| id.clone())
    });
    assert_eq!(learned.as_deref(), Some("ses-old"), "the link is behind");
    let session = h
        .store
        .session_by_external(&MachineId::local(), leon_core::AgentId::OPENCODE, "ses-new")
        .unwrap()
        .expect("imported");
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |shell, cx| shell.resume_session(session, window, cx))
    })
    .unwrap();
    h.settle(cx);
    assert_eq!(live_count(&h, cx), 1, "no second agent on the session");
    assert_eq!(h.main_kind(cx), "live:1");
}

#[gpui_kit::test]
fn an_agent_started_in_leon_is_imported_without_a_refresh(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    // Claude's transcript for the folder, written after the terminal started.
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    std::fs::create_dir_all(projects.join("p")).unwrap();
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(projects.clone());
    h.engine.set_prefs(prefs);
    let line = serde_json::json!({"type": "user", "cwd": path,
        "timestamp": "2026-10-04T12:06:00Z",
        "message": {"role": "user", "content": "hello"}});
    std::fs::write(projects.join("p/claude-new.jsonl"), line.to_string()).unwrap();

    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    h.settle(cx);
    let sessions = h
        .store
        .recent_sessions(&leon_core::SessionFilter::default(), 50)
        .unwrap();
    assert!(
        sessions.iter().any(|s| s.external_id == "claude-new"),
        "starting the agent asked for an import: {:?}",
        sessions.iter().map(|s| &s.external_id).collect::<Vec<_>>()
    );
    // And a session written later shows up when an agent goes quiet or the
    // window asks again.
    let later = serde_json::json!({"type": "user", "cwd": path,
        "timestamp": "2026-10-04T12:07:00Z",
        "message": {"role": "user", "content": "again"}});
    std::fs::write(projects.join("p/claude-later.jsonl"), later.to_string()).unwrap();
    cx.update(|cx| h.shell.update(cx, |s, cx| s.request_import(cx)));
    h.settle(cx);
    assert!(h
        .store
        .recent_sessions(&leon_core::SessionFilter::default(), 50)
        .unwrap()
        .iter()
        .any(|s| s.external_id == "claude-later"));
}

// ----- quitting gently --------------------------------------------------------------------------

fn agent_in_front(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    let h = open_live(cx);
    let (dir, path) = real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    // The agent runs in front of the shell.
    super::live::script_of(&h, 1).set_foreground(false);
    (h, dir, path)
}

/// Asks to quit and lets the watcher it started register its timer: the
/// clock is only advanced after that, so no stage depends on whether some
/// other thread's wake-up happened to run the executor first.
fn quit(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| h.shell.update(cx, |s, cx| s.quit_now(cx)));
    cx.run_until_parked();
}

fn pass(cx: &mut TestAppContext, millis: u64) {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(millis));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn quitting_types_the_agents_exit_line_and_waits_for_it_to_leave(cx: &mut TestAppContext) {
    let (h, _dir, _path) = agent_in_front(cx);
    quit(&h, cx);
    assert!(
        super::live::script_of(&h, 1)
            .written_text()
            .contains("/exit\r"),
        "the agent was told to quit by its own command"
    );
    assert_eq!(h.quits.get(), 0, "not yet: it has not left");
    assert!(h.status().contains("Closing 1 session"), "{}", h.status());
    // The agent saves and leaves: the shell has the terminal back.
    super::live::script_of(&h, 1).set_foreground(true);
    pass(cx, 60);
    assert_eq!(
        h.quits.get(),
        1,
        "quit as soon as nothing is left to wait for"
    );
    assert!(!super::live::script_of(&h, 1).was_terminated());
}

#[gpui_kit::test]
fn an_agent_that_stays_gets_sigterm_and_then_the_hang_up_within_the_grace(cx: &mut TestAppContext) {
    let (h, _dir, _path) = agent_in_front(cx);
    quit(&h, cx);
    pass(cx, 150);
    assert!(!super::live::script_of(&h, 1).was_terminated(), "too early");
    pass(cx, 100);
    assert!(
        super::live::script_of(&h, 1).was_terminated(),
        "after the gesture's wait SIGTERM goes to the foreground"
    );
    assert_eq!(h.quits.get(), 0);
    // It ignores both: the grace ends and the terminal is hung up, quit is
    // never held longer.
    pass(cx, 400);
    assert_eq!(h.quits.get(), 1);
    assert!(h.shell(cx, |s| s.live.all().len()) == 1);
}

#[gpui_kit::test]
fn a_quit_with_no_agent_running_does_not_wait(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    quit(&h, cx);
    assert_eq!(h.quits.get(), 1, "a plain shell is hung up at once");
}

#[gpui_kit::test]
fn the_state_written_before_the_agents_leave_still_names_them(cx: &mut TestAppContext) {
    let (h, _dir, _path) = agent_in_front(cx);
    new_session(&h, leon_core::AgentId::CLAUDE, "claude-new", &_path, 6);
    h.settle(cx);
    quit(&h, cx);
    super::live::script_of(&h, 1).set_foreground(true);
    pass(cx, 60);
    let state = h.store.load_workspace(Slot::Current).unwrap().unwrap();
    assert!(state.clean_shutdown);
    assert_eq!(state.terminals[0].agent.as_deref(), Some("claude"));
    assert_eq!(state.terminals[0].session.as_deref(), Some("claude-new"));
}

/// How the agent leaves, and when, in laps of the watcher.
#[derive(Clone, Copy, Debug)]
enum Leave {
    Never,
    Foreground(u64),
    Exit(u64),
}

const LAP: u64 = 50;

/// One quit: the agent leaves as described; the quit must complete, once,
/// and never later than the grace, whatever the order of the events.
fn quit_with(cx: &mut TestAppContext, leave: Leave) {
    let (h, _dir, _path) = agent_in_front(cx);
    let script = super::live::script_of(&h, 1);
    quit(&h, cx);
    let grace = 500; // the harness's `quit_grace`
    let at = match leave {
        Leave::Never => None,
        Leave::Foreground(lap) | Leave::Exit(lap) => Some(lap * LAP),
    };
    let mut now = 0;
    while now <= grace + LAP {
        if at == Some(now) {
            match leave {
                Leave::Foreground(_) => script.set_foreground(true),
                Leave::Exit(_) => script.exit(0),
                Leave::Never => {}
            }
            cx.run_until_parked();
        }
        let done = h.quits.get();
        if let Some(left) = at.filter(|left| *left <= grace) {
            if now >= left {
                assert_eq!(
                    done, 1,
                    "{leave:?}: gone at {left} ms, still waiting at {now} ms"
                );
            } else if now < left {
                assert_eq!(
                    done, 0,
                    "{leave:?}: quit before the agent left, at {now} ms"
                );
            }
        }
        if now > grace {
            break;
        }
        pass(cx, LAP);
        now += LAP;
    }
    pass(cx, LAP);
    assert_eq!(
        h.quits.get(),
        1,
        "{leave:?}: the quit must complete once, within the grace"
    );
    // And it is never repeated.
    pass(cx, 2_000);
    assert_eq!(h.quits.get(), 1, "{leave:?}: the quit ran twice");
}

#[gpui_kit::test]
fn the_quit_always_completes_within_the_grace_whenever_the_agent_leaves_or_never(
    cx: &mut TestAppContext,
) {
    quit_with(cx, Leave::Never);
    for lap in 0..=11 {
        quit_with(cx, Leave::Foreground(lap));
        quit_with(cx, Leave::Exit(lap));
    }
}
