//! The project's `leon.toml` in the window: running its scripts, the keys of
//! the scripts, the setup of a new worktree and the question that stands
//! before any of its commands. The terminals are the scripted computer of
//! `tests.rs`, whose shell understands `sh -c '<setup>' && <rest>` the way a
//! shell does (the rest runs only if the setup did not fail), and the project
//! files are written in temporary folders.

use super::live::{
    open_live, put_cursor_on, queue_new_worktree, real_worktree_in, screen, terminal_of, wait_until,
};
use super::*;
use crate::trust::Verdict;
use crate::ui::live::LiveId;
use leon_core::ProjectId;
use std::time::Duration;

const HELLO: &str =
    "[[script]]\nname = \"Hello\"\ncommand = \"echo SCRIPT-RAN\"\nicon = \"terminal\"\n";

/// The window over a project in `leon.toml`'s folder, with the file read.
fn open_with_file(cx: &mut TestAppContext, text: &str) -> (Harness, tempfile::TempDir, String) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leon.toml"), text).unwrap();
    let (dir, path) = real_worktree_in(&h, cx, dir);
    redraw(&h, cx);
    let id = real_project(&h, cx);
    assert!(
        h.engine.project_state(&id).is_some(),
        "the project in view was read"
    );
    (h, dir, path)
}

/// Paints the window again, as every change to what is in view does.
fn redraw(h: &Harness, cx: &mut TestAppContext) {
    cx.update(|cx| h.shell.update(cx, |_, cx| cx.notify()));
    h.settle(cx);
}

fn real_project(h: &Harness, cx: &mut TestAppContext) -> ProjectId {
    h.shell(cx, |shell| shell.snapshot.projects.clone())
        .into_iter()
        .find(|entry| entry.project.name == "real")
        .expect("the project of the worktree")
        .project
        .id
}

fn run_a_script(h: &Harness, cx: &mut TestAppContext) {
    h.press("ctrl-shift-p", cx);
    h.type_text("run a script", cx);
    h.press("enter", cx);
    h.settle(cx);
}

fn overlay(h: &Harness, cx: &mut TestAppContext) -> Overlay {
    h.shell(cx, |shell| shell.overlay)
}

fn asked(h: &Harness, cx: &mut TestAppContext) -> Option<String> {
    h.shell(cx, |shell| {
        shell.scripts.ask.as_ref().map(|ask| ask.command.clone())
    })
}

/// Trusts a command as the person would have by answering the question.
fn trust(h: &Harness, cx: &mut TestAppContext, project: &ProjectId, command: &str) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.scripts.trusted.allow(project, command);
        })
    });
}

fn tabs_of(h: &Harness, cx: &mut TestAppContext, id: u64) -> usize {
    h.shell(cx, |shell| {
        shell
            .workspaces
            .workspace_of(LiveId(id))
            .map_or(0, |workspace| workspace.tabs.len())
    })
}

#[gpui_kit::test]
fn a_script_asks_first_runs_in_a_new_tab_and_is_remembered(cx: &mut TestAppContext) {
    let (h, _dir, path) = open_with_file(cx, HELLO);
    run_a_script(&h, cx);
    assert_eq!(h.palette_titles(cx), ["Hello"], "the project's scripts");
    h.press("enter", cx);

    // Nothing has run: the exact command is on the card.
    assert_eq!(overlay(&h, cx), Overlay::Trust);
    assert_eq!(asked(&h, cx).as_deref(), Some("echo SCRIPT-RAN"));
    assert!(h.shows("trust-command", cx));
    assert!(terminal_of(&h, cx, 1).is_none(), "no terminal yet");

    h.press("ctrl-shift-enter", cx); // run it and remember it
    assert_eq!(overlay(&h, cx), Overlay::None);
    wait_until(&h, cx, "the script's output", |h, cx| {
        screen(h, cx, 1).contains("SCRIPT-RAN")
    });
    let project = real_project(&h, cx);
    assert!(h.shell(cx, |s| s
        .scripts
        .trusted
        .allows(&project, "echo SCRIPT-RAN")));
    assert!(
        screen(&h, cx, 1).contains(&format!("FAKE-SHELL in {path}")),
        "in the worktree"
    );

    // Again: no question, and a tab of the terminal in view.
    run_a_script(&h, cx);
    h.press("enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::None);
    wait_until(&h, cx, "the second run", |h, cx| {
        screen(h, cx, 2).contains("SCRIPT-RAN")
    });
    assert_eq!(tabs_of(&h, cx, 1), 2, "a new tab of the same session");
}

#[gpui_kit::test]
fn a_script_whose_command_changed_asks_again(cx: &mut TestAppContext) {
    let (h, dir, _path) = open_with_file(cx, HELLO);
    let project = real_project(&h, cx);
    trust(&h, cx, &project, "echo SCRIPT-RAN");
    run_a_script(&h, cx);
    h.press("enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::None, "trusted as it is written");
    wait_until(&h, cx, "the first run", |h, cx| {
        screen(h, cx, 1).contains("SCRIPT-RAN")
    });

    std::fs::write(
        dir.path().join("leon.toml"),
        "[[script]]\nname = \"Hello\"\ncommand = \"echo SCRIPT-RAN && echo MORE\"\n",
    )
    .unwrap();
    run_a_script(&h, cx); // reads the file again
    h.press("enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::Trust, "the text changed");
    assert_eq!(
        asked(&h, cx).as_deref(),
        Some("echo SCRIPT-RAN && echo MORE")
    );
}

#[gpui_kit::test]
fn running_once_does_not_remember_and_not_running_says_so(cx: &mut TestAppContext) {
    let (h, _dir, _path) = open_with_file(cx, HELLO);
    run_a_script(&h, cx);
    h.press("enter", cx);
    h.press("ctrl-enter", cx); // run it once
    wait_until(&h, cx, "the script", |h, cx| {
        screen(h, cx, 1).contains("SCRIPT-RAN")
    });
    let project = real_project(&h, cx);
    assert!(!h.shell(cx, |s| s
        .scripts
        .trusted
        .allows(&project, "echo SCRIPT-RAN")));

    run_a_script(&h, cx);
    h.press("enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::Trust, "asked again");
    h.press("escape", cx);
    assert_eq!(overlay(&h, cx), Overlay::None);
    assert!(terminal_of(&h, cx, 2).is_none(), "nothing was started");
    assert!(
        h.status().contains("Did not run \"Hello\""),
        "{}",
        h.status()
    );
}

#[gpui_kit::test]
fn a_trusted_command_is_remembered_in_a_file_that_holds_no_command(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let folder = tempfile::tempdir().unwrap();
    let h = open_with(
        cx,
        ScriptedRunner::new(),
        Some(folder.path().join("settings.json")),
    );
    show_inactive(cx);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leon.toml"), HELLO).unwrap();
    let (_dir, _path) = real_worktree_in(&h, cx, dir);
    redraw(&h, cx);
    run_a_script(&h, cx);
    h.press("enter", cx);
    h.press("ctrl-shift-enter", cx);
    let text = std::fs::read_to_string(folder.path().join(crate::trust::FILE_NAME)).unwrap();
    assert!(!text.contains("SCRIPT-RAN"), "hashes only: {text}");
    let project = real_project(&h, cx);
    assert!(
        crate::trust::Trusted::load(&folder.path().join(crate::trust::FILE_NAME))
            .allows(&project, "echo SCRIPT-RAN")
    );
}

#[gpui_kit::test]
fn a_scripts_key_runs_it_even_while_a_terminal_has_the_keyboard(cx: &mut TestAppContext) {
    let (h, _dir, _path) = open_with_file(
        cx,
        "[[script]]\nname = \"Hello\"\ncommand = \"echo SCRIPT-RAN\"\nkey = \"mod+shift+alt+f9\"\n",
    );
    let project = real_project(&h, cx);
    trust(&h, cx, &project, "echo SCRIPT-RAN");
    h.press("ctrl-shift-alt-f9", cx);
    wait_until(&h, cx, "the script", |h, cx| {
        screen(h, cx, 1).contains("SCRIPT-RAN")
    });
    // A terminal has the keyboard now, and the key still reaches Leon.
    assert!(h.shell(cx, |s| s.terminal_focused()));
    h.press("ctrl-shift-alt-f9", cx);
    wait_until(&h, cx, "the second run", |h, cx| {
        screen(h, cx, 2).contains("SCRIPT-RAN")
    });
}

#[gpui_kit::test]
fn a_key_that_leon_has_is_reported_and_never_shadowed(cx: &mut TestAppContext) {
    // mod+shift+t is "Open a shell here".
    let (h, _dir, _path) = open_with_file(
        cx,
        "[[script]]\nname = \"Hello\"\ncommand = \"echo SCRIPT-RAN\"\nkey = \"mod+shift+t\"\n",
    );
    let project = real_project(&h, cx);
    trust(&h, cx, &project, "echo SCRIPT-RAN");
    assert!(
        h.status().contains("Open a shell here"),
        "reported: {}",
        h.status()
    );
    h.press("ctrl-shift-t", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        screen(h, cx, 1).contains("FAKE-SHELL")
    });
    assert!(
        !screen(&h, cx, 1).contains("SCRIPT-RAN"),
        "the command was Leon's"
    );
    // The script is still in the palette, with the reason.
    h.press("escape", cx);
    run_a_script(&h, cx);
    let detail = h.shell(cx, |shell| {
        match shell.palette.flow.as_ref().map(|f| &f.step.kind) {
            Some(crate::ui::steps::StepKind::Choices { choices, .. }) => choices[0].detail.clone(),
            _ => String::new(),
        }
    });
    assert!(detail.contains("is Leon's"), "{detail}");
}

#[gpui_kit::test]
fn a_wrong_file_is_told_with_its_line_and_lists_no_scripts(cx: &mut TestAppContext) {
    let (h, _dir, _path) = open_with_file(
        cx,
        "[[script]]\nname = \"Hello\"\ncommand = \"echo\"\nrun = \"x\"\n",
    );
    assert!(h.status().contains("leon.toml line 4"), "{}", h.status());
    run_a_script(&h, cx);
    assert!(h.status().contains("leon.toml line 4"), "{}", h.status());
    assert_eq!(overlay(&h, cx), Overlay::None, "there is nothing to choose");
}

#[gpui_kit::test]
fn a_project_without_the_file_says_how_to_have_scripts(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    let (_dir, _path) = real_worktree_in(&h, cx, dir);
    redraw(&h, cx);
    run_a_script(&h, cx);
    assert!(h.status().contains("has no leon.toml"), "{}", h.status());
}

// ----- the setup of a new worktree ---------------------------------------------------------------

/// Creates the worktree `branch` in the dialog, with Claude Code when `agent`.
fn create_in_the_dialog(h: &Harness, cx: &mut TestAppContext, branch: &str, agent: bool) {
    h.press("ctrl-shift-n", cx);
    h.type_text(branch, cx);
    if agent {
        h.press("tab", cx);
        h.press("tab", cx);
        h.press("down", cx); // None -> Claude Code
        h.press("enter", cx);
        h.press("tab", cx);
        h.press("tab", cx);
    } else {
        for _ in 0..4 {
            h.press("tab", cx);
        }
    }
    h.press("enter", cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    h.settle(cx);
}

fn setup_file(command: &str) -> String {
    format!("[worktree]\nsetup = \"{command}\"\n")
}

fn project_with_setup(
    cx: &mut TestAppContext,
    command: &str,
    branch: &str,
) -> (Harness, tempfile::TempDir, String, ProjectId) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("leon.toml"), setup_file(command)).unwrap();
    let (dir, path) = real_worktree_in(&h, cx, dir);
    let (project, added) = queue_new_worktree(&h, cx, &path, branch);
    put_cursor_on(&h, cx, NodeId::Project(project.clone()));
    (h, dir, added, project)
}

#[gpui_kit::test]
fn the_setup_runs_in_the_new_worktree_before_the_agent_is_typed(cx: &mut TestAppContext) {
    let (h, _dir, added, project) = project_with_setup(cx, "echo set-up", "feature/setup");
    create_in_the_dialog(&h, cx, "feature/setup", true);
    wait_until(&h, cx, "the question", |h, cx| {
        overlay(h, cx) == Overlay::Trust
    });
    assert_eq!(asked(&h, cx).as_deref(), Some("echo set-up"));
    assert!(
        terminal_of(&h, cx, 1).is_none(),
        "nothing runs, and no agent starts, before the answer"
    );
    // The card came on its own: what is typed meanwhile answers nothing.
    for stray in ["enter", "o", "y", "shift-enter", "space"] {
        h.press(stray, cx);
    }
    assert_eq!(overlay(&h, cx), Overlay::Trust, "no plain key answers");
    assert!(
        !h.shell(cx, |s| s.scripts.trusted.allows(&project, "echo set-up")),
        "nothing was trusted by a stray key"
    );
    assert!(terminal_of(&h, cx, 1).is_none(), "and nothing ran");
    h.press("ctrl-shift-enter", cx);
    wait_until(&h, cx, "the agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    let shown = screen(&h, cx, 1);
    assert!(shown.contains(&format!("FAKE-SHELL in {added}")), "{shown}");
    let setup = shown.find("SETUP-RAN echo set-up").expect(&shown);
    assert!(setup < shown.find("FAKE-CLAUDE").unwrap(), "{shown}");
    assert!(h.shell(cx, |s| s.scripts.trusted.allows(&project, "echo set-up")));
}

#[gpui_kit::test]
fn a_trusted_setup_runs_without_a_question(cx: &mut TestAppContext) {
    let (h, _dir, _added, project) = project_with_setup(cx, "echo set-up", "feature/quiet");
    trust(&h, cx, &project, "echo set-up");
    create_in_the_dialog(&h, cx, "feature/quiet", true);
    wait_until(&h, cx, "the agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    assert_eq!(overlay(&h, cx), Overlay::None);
    assert!(screen(&h, cx, 1).contains("SETUP-RAN echo set-up"));
}

#[gpui_kit::test]
fn a_setup_that_fails_leaves_the_terminal_open_and_starts_no_agent(cx: &mut TestAppContext) {
    let (h, _dir, _added, project) = project_with_setup(cx, "fail now", "feature/broken");
    trust(&h, cx, &project, "fail now");
    create_in_the_dialog(&h, cx, "feature/broken", true);
    wait_until(&h, cx, "the failure", |h, cx| {
        screen(h, cx, 1).contains("SETUP-FAILED")
    });
    h.settle(cx);
    let terminal = terminal_of(&h, cx, 1).expect("the terminal stays open");
    assert!(terminal.exit_info().is_none(), "it is still running");
    assert!(!screen(&h, cx, 1).contains("FAKE-CLAUDE"), "no agent");
    assert!(screen(&h, cx, 1).contains("READY>"), "at its prompt");
}

#[gpui_kit::test]
fn a_setup_that_was_not_trusted_is_skipped_and_the_session_still_starts(cx: &mut TestAppContext) {
    let (h, _dir, added, project) = project_with_setup(cx, "echo set-up", "feature/skipped");
    create_in_the_dialog(&h, cx, "feature/skipped", false);
    wait_until(&h, cx, "the question", |h, cx| {
        overlay(h, cx) == Overlay::Trust
    });
    h.press("escape", cx);
    wait_until(&h, cx, "the shell", |h, cx| {
        screen(h, cx, 1).contains(&format!("FAKE-SHELL in {added}"))
    });
    assert!(!screen(&h, cx, 1).contains("SETUP-RAN"));
    assert!(
        h.status().contains("Did not run the setup"),
        "{}",
        h.status()
    );
    assert!(!h.shell(cx, |s| s.scripts.trusted.allows(&project, "echo set-up")));
}

#[gpui_kit::test]
fn a_setup_whose_text_changed_is_asked_about_again(cx: &mut TestAppContext) {
    let (h, dir, _added, project) = project_with_setup(cx, "echo set-up", "feature/edited");
    trust(&h, cx, &project, "echo set-up");
    // The repository changed the command after it was trusted.
    std::fs::write(
        dir.path().join("leon.toml"),
        setup_file("echo set-up && echo more"),
    )
    .unwrap();
    create_in_the_dialog(&h, cx, "feature/edited", false);
    wait_until(&h, cx, "the question", |h, cx| {
        overlay(h, cx) == Overlay::Trust
    });
    assert_eq!(asked(&h, cx).as_deref(), Some("echo set-up && echo more"));
    assert_eq!(
        crate::trust::verdict(
            &h.shell(cx, |s| s.scripts.trusted.clone()),
            &project,
            "echo set-up && echo more"
        ),
        Verdict::Ask
    );
}

#[gpui_kit::test]
fn a_project_without_a_setup_starts_its_session_as_before(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    let (_dir, path) = real_worktree_in(&h, cx, dir);
    let (project, added) = queue_new_worktree(&h, cx, &path, "feature/plain");
    put_cursor_on(&h, cx, NodeId::Project(project));
    create_in_the_dialog(&h, cx, "feature/plain", false);
    wait_until(&h, cx, "the shell", |h, cx| {
        screen(h, cx, 1).contains(&format!("FAKE-SHELL in {added}"))
    });
    assert_eq!(overlay(&h, cx), Overlay::None);
    assert!(!screen(&h, cx, 1).contains("SETUP-RAN"));
}

#[gpui_kit::test]
fn the_new_worktree_goes_where_the_setting_says(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    let (_dir, path) = real_worktree_in(&h, cx, dir);
    cx.update(|cx| {
        crate::settings::set_value(
            cx,
            crate::schema::find("worktree_location").unwrap(),
            crate::schema::Value::Text("{root}/.wt/{branch}".into()),
        )
    });
    h.settle(cx);
    let (project, _) = queue_new_worktree(&h, cx, &path, "feature/there");
    put_cursor_on(&h, cx, NodeId::Project(project));
    create_in_the_dialog(&h, cx, "feature/there", false);
    let add = h
        .runner
        .calls()
        .into_iter()
        .find(|call| {
            call.args.first().map(String::as_str) == Some("worktree")
                && call.args.get(1).map(String::as_str) == Some("add")
        })
        .expect("git was asked to add the worktree");
    assert!(
        add.args.contains(&format!("{path}/.wt/feature-there")),
        "{:?}",
        add.args
    );
}

// ----- the card cannot be answered by accident, and cards wait their turn ----------------------

/// Starts the sessions of `paths` as a worktree just made would, with the
/// project's setup as the window last read it.
fn start_sessions(h: &Harness, cx: &mut TestAppContext, project: &ProjectId, paths: &[&str]) {
    let state = h.engine.project_state(project);
    let starts: Vec<(String, crate::launch::Launch)> = paths
        .iter()
        .map(|path| ((*path).to_owned(), crate::launch::Launch::Shell))
        .collect();
    let project = project.clone();
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.start_worktree_sessions(&project, &MachineId::local(), starts, state, window, cx)
        })
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn a_stray_key_neither_runs_nor_trusts_a_command_nobody_read(cx: &mut TestAppContext) {
    let (h, _dir, _path) = open_with_file(cx, HELLO);
    run_a_script(&h, cx);
    h.press("enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::Trust);
    for stray in ["enter", "o", "t", "y", "space", "shift-enter", "tab"] {
        h.press(stray, cx);
        assert_eq!(overlay(&h, cx), Overlay::Trust, "{stray} answered the card");
    }
    let project = real_project(&h, cx);
    assert!(!h.shell(cx, |s| s
        .scripts
        .trusted
        .allows(&project, "echo SCRIPT-RAN")));
    assert!(terminal_of(&h, cx, 1).is_none(), "nothing ran");
    assert!(h.shows("trust-keys", cx), "the card tells how to answer");
    h.press("ctrl-enter", cx);
    wait_until(&h, cx, "the script", |h, cx| {
        screen(h, cx, 1).contains("SCRIPT-RAN")
    });
    assert!(
        !h.shell(cx, |s| s
            .scripts
            .trusted
            .allows(&project, "echo SCRIPT-RAN")),
        "once is not remembered"
    );
}

#[gpui_kit::test]
fn a_card_that_comes_on_its_own_waits_for_a_palette_and_never_replaces_it(cx: &mut TestAppContext) {
    let (h, _dir, path) = open_with_file(cx, &setup_file("echo set-up"));
    let project = real_project(&h, cx);
    h.press("ctrl-shift-p", cx);
    assert_eq!(overlay(&h, cx), Overlay::Palette);
    start_sessions(&h, cx, &project, &[&path]);
    assert_eq!(overlay(&h, cx), Overlay::Palette, "the palette stays");
    assert_eq!(h.shell(cx, |s| s.scripts.queued.len()), 1);
    h.press("escape", cx);
    h.settle(cx);
    assert_eq!(overlay(&h, cx), Overlay::Trust, "its turn came");
    assert_eq!(asked(&h, cx).as_deref(), Some("echo set-up"));
}

#[gpui_kit::test]
fn two_cards_in_a_row_are_asked_one_at_a_time_and_none_is_lost(cx: &mut TestAppContext) {
    let (h, _dir, path) = open_with_file(cx, &setup_file("echo set-up"));
    let project = real_project(&h, cx);
    start_sessions(&h, cx, &project, &[&path]);
    start_sessions(&h, cx, &project, &[&path]);
    assert_eq!(overlay(&h, cx), Overlay::Trust);
    assert_eq!(h.shell(cx, |s| s.scripts.queued.len()), 1, "one waits");
    assert!(terminal_of(&h, cx, 1).is_none());

    // Remembering the first answers the second: the same command.
    h.press("ctrl-shift-enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::None);
    assert_eq!(h.shell(cx, |s| s.scripts.queued.len()), 0);
    wait_until(&h, cx, "both sessions", |h, cx| {
        screen(h, cx, 1).contains("SETUP-RAN echo set-up")
            && screen(h, cx, 2).contains("SETUP-RAN echo set-up")
    });
}

#[gpui_kit::test]
fn a_command_run_once_is_asked_again_for_the_next_card(cx: &mut TestAppContext) {
    let (h, _dir, path) = open_with_file(cx, &setup_file("echo set-up"));
    let project = real_project(&h, cx);
    start_sessions(&h, cx, &project, &[&path]);
    start_sessions(&h, cx, &project, &[&path]);
    h.press("ctrl-enter", cx);
    assert_eq!(overlay(&h, cx), Overlay::Trust, "the second is asked");
    h.press("escape", cx);
    assert_eq!(overlay(&h, cx), Overlay::None);
    wait_until(&h, cx, "the shell of each", |h, cx| {
        terminal_of(h, cx, 1).is_some() && terminal_of(h, cx, 2).is_some()
    });
    assert!(!screen(&h, cx, 2).contains("SETUP-RAN"), "declined");
}

#[gpui_kit::test]
fn a_setup_with_no_agent_is_handed_to_sh_like_one_with_an_agent(cx: &mut TestAppContext) {
    let (h, _dir, path) = open_with_file(cx, &setup_file("echo set-up"));
    let project = real_project(&h, cx);
    trust(&h, cx, &project, "echo set-up");
    start_sessions(&h, cx, &project, &[&path]);
    wait_until(&h, cx, "the setup", |h, cx| {
        screen(h, cx, 1).contains("SETUP-RAN echo set-up")
    });
    assert!(screen(&h, cx, 1).contains("sh -c 'echo set-up'"));
}
