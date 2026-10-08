//! Tests of several accounts of one agent in the window: the question when a
//! session starts, the setting that skips it, the variables a terminal gets,
//! what a session remembers, what is shown, and the palette commands that keep
//! the accounts. The terminals are the scripted computer of `tests.rs`; the
//! accounts live in the in-memory settings; nothing waits on the clock.

use super::live::{
    open_live, put_cursor_on, real_worktree, script_of, stored_session, terminal_of, wait_until,
};
use super::*;
use crate::schema::Value;
use crate::ui::live::LiveId;
use leon_core::{AgentId, MachineId};

fn add(cx: &mut TestAppContext, agent: AgentId, name: &str, variables: &str) {
    cx.update(|cx| settings::add_account(cx, agent, name, variables))
        .unwrap();
}

fn two_claude_accounts(cx: &mut TestAppContext) {
    add(cx, AgentId::CLAUDE, "Work", "CLAUDE_CONFIG_DIR=/acct/work");
    add(cx, AgentId::CLAUDE, "Home", "CLAUDE_CONFIG_DIR=/acct/home");
}

fn account_of(h: &Harness, cx: &mut TestAppContext, id: u64) -> Option<String> {
    h.shell(cx, |s| {
        s.live.get(LiveId(id)).and_then(|l| l.account.clone())
    })
}

fn start_claude_here(h: &Harness, cx: &mut TestAppContext) {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.new_session_with(AgentId::CLAUDE, window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn an_agent_with_accounts_asks_which_and_starts_the_terminal_with_that_accounts_variables(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    two_claude_accounts(cx);

    start_claude_here(&h, cx);

    // Nothing started: the palette asks which account, the agent's own first.
    assert!(terminal_of(&h, cx, 1).is_none());
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    let titles = h.palette_titles(cx);
    assert_eq!(
        titles,
        [
            "Claude Code (default)",
            "Claude Code (Work)",
            "Claude Code (Home)"
        ]
    );

    h.set_palette_text("work", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });

    let spec = script_of(&h, 1).spec().clone();
    assert!(
        spec.env
            .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/acct/work".to_owned())),
        "{:?}",
        spec.env
    );
    assert_eq!(account_of(&h, cx, 1).as_deref(), Some("claude-work"));
    // The line typed is the agent's own: the variables are the terminal's.
    wait_until(&h, cx, "the agent's line", |h, cx| {
        super::live::screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    assert!(!super::live::screen(&h, cx, 1).contains("/acct/work"));
}

#[gpui_kit::test]
fn choosing_the_agents_own_setup_adds_nothing_to_the_environment(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    start_claude_here(&h, cx);
    h.press("enter", cx); // the first choice is the agent's own setup
    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    let spec = script_of(&h, 1).spec().clone();
    assert!(spec.env.iter().all(|(name, _)| name != "CLAUDE_CONFIG_DIR"));
    assert_eq!(account_of(&h, cx, 1), None);
}

#[gpui_kit::test]
fn the_default_account_setting_starts_the_session_without_asking(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    let def = crate::schema::find("default_accounts").unwrap();
    cx.update(|cx| settings::set_value(cx, def, Value::List(vec!["claude=home".into()])));
    h.settle(cx);

    start_claude_here(&h, cx);

    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None, "no question");
    assert!(script_of(&h, 1)
        .spec()
        .env
        .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/acct/home".to_owned())));
    assert_eq!(account_of(&h, cx, 1).as_deref(), Some("claude-home"));
}

#[gpui_kit::test]
fn an_agent_without_accounts_starts_at_once_as_it_always_did(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    add(cx, AgentId::CODEX, "Lab", "CODEX_HOME=/acct/lab");
    start_claude_here(&h, cx);
    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(account_of(&h, cx, 1), None);
}

#[gpui_kit::test]
fn escaping_the_account_question_starts_nothing_and_forgets_the_session(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    start_claude_here(&h, cx);
    h.press("escape", cx);
    h.press("escape", cx);
    assert!(terminal_of(&h, cx, 1).is_none());
    // The next "New agent session" is its own question, not the waiting one.
    h.press_chord("cmd-n", "ctrl-n", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(h.shell(cx, |s| s.pending_start.is_none()));
    let titles = h.palette_titles(cx);
    assert!(titles.iter().any(|t| t == "Codex"), "{titles:?}");
}

#[gpui_kit::test]
fn a_history_session_resumes_with_the_account_it_ran_with_and_never_asks(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    let id = stored_session(&h, cx, AgentId::CLAUDE, &MachineId::local(), &path, "alpha");
    h.store
        .set_session_account(&id, Some("claude-work"))
        .unwrap();
    h.settle(cx);
    put_cursor_on(&h, cx, NodeId::Session(id));
    h.press("enter", cx);
    h.press("enter", cx); // Resume
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    assert!(script_of(&h, 1)
        .spec()
        .env
        .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/acct/work".to_owned())));
    assert_eq!(account_of(&h, cx, 1).as_deref(), Some("claude-work"));
    wait_until(&h, cx, "the resume line", |h, cx| {
        super::live::screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
}

#[gpui_kit::test]
fn a_session_of_a_removed_account_is_not_resumed_as_another_one(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    let id = stored_session(&h, cx, AgentId::CLAUDE, &MachineId::local(), &path, "alpha");
    h.store
        .set_session_account(&id, Some("claude-gone"))
        .unwrap();
    h.settle(cx);
    put_cursor_on(&h, cx, NodeId::Session(id));
    h.press("enter", cx);
    h.press("enter", cx); // Resume
    assert!(terminal_of(&h, cx, 1).is_none(), "nothing was started");
    assert!(
        h.status().contains("claude-gone") && h.status().contains("not in your settings"),
        "{}",
        h.status()
    );
}

#[gpui_kit::test]
fn the_account_survives_a_restart_in_the_saved_terminals_and_in_the_header(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, _path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    start_claude_here(&h, cx);
    h.set_palette_text("work", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    let saved = h.shell(cx, |s| {
        s.live
            .all()
            .iter()
            .map(|session| session.account.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(saved, [Some("claude-work".to_owned())]);
    // The header says which account it is, in capitals like the rest of it.
    let line = cx.update(|cx| h.shell.read(cx).main_heading(cx).2);
    assert!(line.contains("CLAUDE CODE · WORK"), "{line}");
}

#[gpui_kit::test]
fn the_palette_adds_renames_and_removes_an_account(cx: &mut TestAppContext) {
    let h = open_live(cx);
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">add an account", cx);
    h.press("enter", cx);
    h.set_palette_text("claude code", cx);
    h.press("enter", cx);
    h.type_text("Side", cx);
    h.press("enter", cx);
    h.type_text("CLAUDE_CONFIG_DIR=/acct/side", cx);
    h.press("enter", cx);
    let accounts = cx.update(|cx| settings::accounts(cx));
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, "claude-side");
    assert_eq!(accounts[0].config_dir(), Some("/acct/side"));
    assert!(
        h.status().contains("limits get a line of their own"),
        "{}",
        h.status()
    );

    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">rename an account", cx);
    h.press("enter", cx);
    h.press("enter", cx); // the only account
    h.type_text("Job", cx);
    h.press("enter", cx);
    let accounts = cx.update(|cx| settings::accounts(cx));
    assert_eq!(
        (accounts[0].id.as_str(), accounts[0].name.as_str()),
        ("claude-side", "Job")
    );

    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">remove an account", cx);
    h.press("enter", cx);
    h.press("enter", cx); // the only account
    h.press("enter", cx); // Remove
    assert!(cx.update(|cx| settings::accounts(cx)).is_empty());
}

#[gpui_kit::test]
fn renaming_an_account_keeps_its_id_and_the_default_line_that_named_it(cx: &mut TestAppContext) {
    let h = open_live(cx);
    two_claude_accounts(cx);
    let def = crate::schema::find("default_accounts").unwrap();
    cx.update(|cx| settings::set_value(cx, def, Value::List(vec!["claude=work".into()])));
    cx.update(|cx| settings::rename_account(cx, "claude-work", "Job"))
        .unwrap();
    let (accounts, defaults) =
        cx.update(|cx| (settings::accounts(cx), settings::default_accounts(cx)));
    assert_eq!(accounts[0].id, "claude-work");
    assert_eq!(accounts[0].name, "Job");
    assert_eq!(defaults, ["claude=Job"]);
    // A name another account has is refused and changes nothing.
    let refused = cx.update(|cx| settings::rename_account(cx, "claude-work", "home"));
    assert!(refused.is_err());
    assert_eq!(cx.update(|cx| settings::accounts(cx))[0].name, "Job");
    drop(h);
}

#[gpui_kit::test]
fn removing_an_account_takes_its_usage_line_and_history_with_it_through_the_engine(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    two_claude_accounts(cx);
    let local = MachineId::local();
    let series = |account: &str| {
        leon_usage::series_key_of(
            local.as_str(),
            AgentId::CLAUDE,
            Some("max"),
            Some(account).filter(|account| !account.is_empty()),
        )
    };
    for account in ["", "claude-work", "claude-home"] {
        let usage = leon_usage::AgentUsage {
            plan: Some("max".to_owned()),
            account: Some(account.to_owned()).filter(|account| !account.is_empty()),
            ..leon_usage::AgentUsage::unknown(
                AgentId::CLAUDE,
                local.as_str(),
                leon_usage::Reason::SourceDisabled,
            )
        };
        h.store
            .put_usage_reading_for(
                &local,
                AgentId::CLAUDE,
                account,
                &serde_json::to_string(&usage).unwrap(),
                1,
            )
            .unwrap();
        h.store
            .record_usage_points(
                &local,
                AgentId::CLAUDE,
                &series(account),
                &[(
                    "five_hour".to_owned(),
                    leon_core::UsagePoint {
                        at: 5,
                        used_percent: 1.0,
                    },
                )],
                0,
            )
            .unwrap();
    }
    cx.update_window(h.window.into(), |_, _, cx| {
        h.shell
            .update(cx, |shell, cx| shell.remove_account("claude-work", cx))
    })
    .unwrap();
    // The window only asks: the engine drops the line and its history.
    h.settle(cx);
    let left: Vec<String> = h
        .store
        .usage_readings()
        .unwrap()
        .into_iter()
        .map(|row| row.account)
        .collect();
    assert_eq!(left, ["", "claude-home"]);
    let points = |account: &str| {
        h.store
            .usage_history(&local, AgentId::CLAUDE, &series(account), "five_hour", 0)
            .unwrap()
            .len()
    };
    assert_eq!(points("claude-work"), 0, "its history went with it");
    assert_eq!(points("claude-home"), 1);
    assert_eq!(points(""), 1, "the agent's own history stays");
}

fn sleep_as(h: &Harness, cx: &mut TestAppContext, path: &str, account: Option<&str>) -> LiveId {
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| {
            s.dormant.add(
                &MachineId::local(),
                path,
                Some(AgentId::CLAUDE),
                Some("fix".to_owned()),
                account.map(str::to_owned),
            );
            s.refresh_live();
            cx.notify();
            s.dormant.all()[0].id()
        })
    })
}

fn wake(h: &Harness, cx: &mut TestAppContext, id: LiveId) {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |shell, cx| shell.wake_dormant(id, window, cx))
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn waking_a_sleeping_session_starts_the_account_it_ran_with(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    let asleep = sleep_as(&h, cx, &path, Some("claude-work"));
    wake(&h, cx, asleep);
    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    assert!(script_of(&h, 1)
        .spec()
        .env
        .contains(&("CLAUDE_CONFIG_DIR".to_owned(), "/acct/work".to_owned())));
    assert_eq!(account_of(&h, cx, 1).as_deref(), Some("claude-work"));
    assert!(h.shell(cx, |s| s.dormant.all().is_empty()), "it is awake");
}

#[gpui_kit::test]
fn a_sleeping_session_of_a_removed_account_stays_asleep_until_the_account_is_back(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    let asleep = sleep_as(&h, cx, &path, Some("claude-gone"));

    wake(&h, cx, asleep);
    assert!(terminal_of(&h, cx, 1).is_none(), "nothing was started");
    assert_eq!(
        h.shell(cx, |s| s.dormant.all().len()),
        1,
        "the sleeping session is kept"
    );
    assert!(
        h.status().contains("claude-gone") && h.status().contains("not in your settings"),
        "{}",
        h.status()
    );

    // Adding the account again with the same name is what the message asks.
    add(cx, AgentId::CLAUDE, "Gone", "CLAUDE_CONFIG_DIR=/acct/gone");
    wake(&h, cx, asleep);
    wait_until(&h, cx, "the terminal", |h, cx| {
        terminal_of(h, cx, 1).is_some()
    });
    assert_eq!(account_of(&h, cx, 1).as_deref(), Some("claude-gone"));
    assert!(h.shell(cx, |s| s.dormant.all().is_empty()));
}

#[gpui_kit::test]
fn the_sidebar_row_of_a_session_of_an_account_names_the_account(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    two_claude_accounts(cx);
    let id = stored_session(&h, cx, AgentId::CLAUDE, &MachineId::local(), &path, "alpha");
    h.store
        .set_session_account(&id, Some("claude-work"))
        .unwrap();
    h.settle(cx);
    let row = h.shell(cx, |s| {
        s.rows
            .iter()
            .position(|row| matches!(&row.kind, tree::Kind::Session(session) if session.id == id))
    });
    let row = row.expect("the session has a row");
    assert!(h.shows_dynamic(format!("tree-account-{row}"), cx));
}
