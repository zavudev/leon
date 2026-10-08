//! One prompt, several agents in the window: the palette flow ends in one
//! worktree and one session per agent, each started with the prompt on its
//! launch line, a worktree that git refuses is told by agent while the others
//! go on, and the project's `setup` is asked about once for the whole batch.
//! The terminals are the scripted computer of `tests.rs`; git is a scripted
//! runner; no agent, shell or network is involved.

use super::live::{open_live, real_worktree_in, screen, terminal_of};
use super::*;
use crate::address::worktree_path;
use leon_core::ProjectId;
use std::time::Duration;

/// What `git worktree list --porcelain` says with these branches made.
fn listing(path: &str, made: &[&str]) -> String {
    let mut text = format!(
        "worktree {path}\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/trunk\n"
    );
    for branch in made {
        text.push_str(&format!(
            "worktree {}\nHEAD 2222222222222222222222222222222222222222\nbranch refs/heads/{branch}\n",
            worktree_path(path, branch)
        ));
    }
    text
}

/// The palette's flow up to the base: project, prompt, then each agent ticked
/// by its name and `Start`.
fn ask_for(h: &Harness, cx: &mut TestAppContext, prompt: &str, agents: &[&str]) {
    h.press("ctrl-shift-p", cx);
    h.type_text("one prompt", cx);
    h.press("enter", cx);
    h.press("enter", cx); // the project the keyboard is on
    answer(h, prompt, cx);
    for agent in agents {
        answer(h, agent, cx);
    }
    answer(h, "start", cx);
    h.press("enter", cx); // HEAD
    cx.executor().advance_clock(Duration::from_secs(1));
    h.settle(cx);
}

/// Lets the threads beside the window and the UI catch up until `condition`
/// holds. The window looks for the worktrees it made on a clock, which the
/// test moves along.
fn wait_until(
    h: &Harness,
    cx: &mut TestAppContext,
    what: &str,
    condition: impl Fn(&Harness, &mut TestAppContext) -> bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        cx.executor().advance_clock(Duration::from_millis(200));
        h.settle(cx);
        if condition(h, cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("timed out waiting for {what}: {}", h.status());
}

/// The answer to the first thing a fan-out asks git: the branches it has.
fn queue_branches(h: &Harness, branches: &[&str]) {
    h.runner.queue(Output::ok(branches.join("\n")));
}

fn made_calls(h: &Harness) -> Vec<String> {
    h.runner
        .calls()
        .iter()
        .filter(|call| {
            call.args.iter().any(|arg| arg == "worktree") && call.args.contains(&"add".to_owned())
        })
        .map(|call| call.args.join(" "))
        .collect()
}

#[gpui_kit::test]
fn two_agents_each_get_a_worktree_and_start_with_the_prompt(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree_in(&h, cx, tempfile::tempdir().unwrap());
    let claude = "fix-the-login-bug-claude";
    let codex = "fix-the-login-bug-codex";
    for branch in [claude, codex] {
        std::fs::create_dir_all(worktree_path(&path, branch)).unwrap();
    }
    queue_branches(&h, &["trunk"]);
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..2 {
        h.runner.queue(Output::ok(listing(&path, &[claude])));
    }
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..4 {
        h.runner.queue(Output::ok(listing(&path, &[claude, codex])));
    }
    ask_for(&h, cx, "Fix the login bug", &["claude", "codex"]);

    wait_until(&h, cx, "the first agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    wait_until(&h, cx, "the second agent", |h, cx| {
        screen(h, cx, 2).contains("FAKE-CODEX")
    });
    let shown = screen(&h, cx, 1);
    // The settings of the test add a flag to Claude Code: the prompt follows
    // it, typed as one quoted word.
    assert!(
        shown.contains("claude --dangerously-skip-permissions 'Fix the login bug'"),
        "the line typed: {shown}"
    );
    assert!(
        shown.contains("FAKE-CLAUDE --dangerously-skip-permissions Fix the login bug"),
        "what the agent received: {shown}"
    );
    let adds = made_calls(&h);
    assert_eq!(adds.len(), 2, "{adds:?}");
    assert!(adds[0].contains(&format!("-b {claude}")), "{adds:?}");
    assert!(adds[1].contains(&format!("-b {codex}")), "{adds:?}");
    assert!(adds[0].ends_with("HEAD"), "from the base chosen: {adds:?}");
    let rows = h.shell(cx, |shell| {
        shell
            .snapshot
            .project(&real_project_of(shell))
            .map(|entry| {
                entry
                    .worktrees
                    .iter()
                    .filter_map(|worktree| worktree.branch.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    assert!(
        rows.contains(&claude.to_owned()) && rows.contains(&codex.to_owned()),
        "{rows:?}"
    );
    assert!(
        h.status().starts_with("Started 2 of 2 agents."),
        "{}",
        h.status()
    );
}

fn real_project_of(shell: &Shell) -> ProjectId {
    shell
        .snapshot
        .projects
        .iter()
        .find(|entry| entry.project.name == "real")
        .expect("the project")
        .project
        .id
        .clone()
}

#[gpui_kit::test]
fn a_worktree_git_refuses_is_told_by_agent_and_the_other_still_starts(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree_in(&h, cx, tempfile::tempdir().unwrap());
    let codex = "run-it-codex";
    std::fs::create_dir_all(worktree_path(&path, codex)).unwrap();
    // The first branch is refused; the second is made.
    queue_branches(&h, &["trunk"]);
    h.runner.queue(Output::failed(
        128,
        "fatal: cannot lock ref 'refs/heads/run-it-claude'",
    ));
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..4 {
        h.runner.queue(Output::ok(listing(&path, &[codex])));
    }
    ask_for(&h, cx, "Run it", &["claude", "codex"]);

    wait_until(&h, cx, "the agent that could be made", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CODEX")
    });
    let status = h.status();
    assert!(status.starts_with("Started 1 of 2 agents:"), "{status}");
    assert!(status.contains("Claude Code:"), "{status}");
    assert!(status.contains("cannot lock ref"), "{status}");
    assert!(
        terminal_of(&h, cx, 2).is_none(),
        "no session for the worktree that was not made"
    );
}

#[gpui_kit::test]
fn the_setup_is_asked_about_once_and_runs_before_each_agent(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("leon.toml"),
        "[worktree]\nsetup = \"echo set-up\"\n",
    )
    .unwrap();
    let (_dir, path) = real_worktree_in(&h, cx, dir);
    let (claude, codex) = ("do-it-claude", "do-it-codex");
    for branch in [claude, codex] {
        std::fs::create_dir_all(worktree_path(&path, branch)).unwrap();
    }
    queue_branches(&h, &["trunk"]);
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..2 {
        h.runner.queue(Output::ok(listing(&path, &[claude])));
    }
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..4 {
        h.runner.queue(Output::ok(listing(&path, &[claude, codex])));
    }
    ask_for(&h, cx, "Do it", &["claude", "codex"]);

    wait_until(&h, cx, "the question", |h, cx| {
        h.shell(cx, |shell| shell.overlay) == Overlay::Trust
    });
    assert!(
        terminal_of(&h, cx, 1).is_none(),
        "nothing starts before the answer"
    );
    let what = h.shell(cx, |shell| {
        shell.scripts.ask.as_ref().map(|ask| ask.what.clone())
    });
    assert_eq!(what.as_deref(), Some("Setup of 2 new worktrees"));
    h.press("enter", cx); // a stray key answers nothing
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Trust);
    h.press("ctrl-enter", cx); // once
    wait_until(&h, cx, "both agents", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE") && screen(h, cx, 2).contains("FAKE-CODEX")
    });
    for id in [1, 2] {
        assert!(screen(&h, cx, id).contains("SETUP-RAN echo set-up"));
    }
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
}

#[gpui_kit::test]
fn a_prompt_given_again_is_numbered_past_the_branches_git_still_has(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree_in(&h, cx, tempfile::tempdir().unwrap());
    // The first run's worktrees were removed; their branches stayed.
    let (claude, codex) = ("again-it-2-claude", "again-it-2-codex");
    for branch in [claude, codex] {
        std::fs::create_dir_all(worktree_path(&path, branch)).unwrap();
    }
    queue_branches(&h, &["trunk", "again-it-claude", "again-it-codex"]);
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..2 {
        h.runner.queue(Output::ok(listing(&path, &[claude])));
    }
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..4 {
        h.runner.queue(Output::ok(listing(&path, &[claude, codex])));
    }
    ask_for(&h, cx, "Again it", &["claude", "codex"]);

    wait_until(&h, cx, "both agents", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE") && screen(h, cx, 2).contains("FAKE-CODEX")
    });
    let adds = made_calls(&h);
    assert_eq!(adds.len(), 2, "{adds:?}");
    assert!(adds[0].contains(&format!("-b {claude}")), "{adds:?}");
    assert!(adds[1].contains(&format!("-b {codex}")), "{adds:?}");
    assert!(
        h.status().starts_with("Started 2 of 2 agents."),
        "{}",
        h.status()
    );
}

#[gpui_kit::test]
fn a_wrong_project_file_is_told_in_the_line_that_counts_the_agents(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("leon.toml"),
        "[worktree]\nsetup = \"make\"\nrun = \"x\"\n",
    )
    .unwrap();
    let (_dir, path) = real_worktree_in(&h, cx, dir);
    let (claude, codex) = ("check-it-claude", "check-it-codex");
    for branch in [claude, codex] {
        std::fs::create_dir_all(worktree_path(&path, branch)).unwrap();
    }
    queue_branches(&h, &["trunk"]);
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..2 {
        h.runner.queue(Output::ok(listing(&path, &[claude])));
    }
    h.runner.queue(Output::ok(String::new()));
    for _ in 0..4 {
        h.runner.queue(Output::ok(listing(&path, &[claude, codex])));
    }
    ask_for(&h, cx, "Check it", &["claude", "codex"]);

    wait_until(&h, cx, "both agents", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE") && screen(h, cx, 2).contains("FAKE-CODEX")
    });
    let status = h.status();
    assert!(status.starts_with("Started 2 of 2 agents."), "{status}");
    assert!(
        status.contains("No setup was run: leon.toml line"),
        "the count did not replace the mistake: {status}"
    );
    assert!(!screen(&h, cx, 1).contains("SETUP-RAN"));
}
