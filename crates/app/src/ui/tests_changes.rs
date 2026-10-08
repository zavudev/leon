//! Tests of the changes tab: the list of changed files, the diff, the keys,
//! and the commit, push and pull request that take the changes away. The
//! computer is the scripted runner of `tests.rs`: every git and gh answer is
//! queued by the test, in the order the window asks for them, and the calls
//! the window made are read back from the runner.

use super::*;
use crate::ui::live::LiveId;

const STATUS: &str = "## topic...origin/topic\0 M src/a.rs\0A  b.rs\0?? c.txt\0";

const DIFF_A: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n";

const DIFF_B: &str = "diff --git a/b.rs b/b.rs\nnew file mode 100644\n--- /dev/null\n+++ b/b.rs\n@@ -0,0 +1 @@\n+fresh\n";

/// What reading the start of a pull request asks: the branch, the base the
/// remote names, and the commits between.
fn queue_start(h: &Harness, log: &str) {
    h.runner.queue(Output::ok("## topic...origin/topic\0"));
    h.runner.queue(Output::ok("origin/main\n"));
    h.runner.queue(Output::ok(log.to_owned()));
}

/// A window over a project whose only worktree is a real folder, with the
/// keyboard on it.
fn with_worktree(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
    cx.executor().allow_parking();
    let h = open(cx, ScriptedRunner::new());
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let project = h
        .store
        .add_project(&MachineId::local(), "real", &path)
        .unwrap();
    h.store
        .replace_worktrees(
            &project.id,
            vec![NewWorktree {
                path: path.clone(),
                branch: Some("topic".into()),
                head: None,
                is_main: true,
            }],
        )
        .unwrap();
    h.settle(cx);
    let worktree = h.store.worktrees(&project.id).unwrap().remove(0).id;
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&NodeId::Worktree(worktree));
            shell.pane = Pane::Sidebar;
        })
    });
    (h, dir, path)
}

/// The tab, opened by the chord a person uses, with the git answers it asks
/// for in the order it asks: the status, the diff of the first file and the
/// start of the pull request.
fn open_tab(h: &Harness, cx: &mut TestAppContext) -> LiveId {
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(h, "Fix the tabs\u{1f}They leaked.\u{1e}");
    h.press_chord("cmd-alt-g", "ctrl-shift-alt-g", cx);
    // The status, then the diff, then the pull request's start: each answer
    // wakes the next question.
    for _ in 0..3 {
        h.settle(cx);
    }
    h.shell(cx, |shell| shell.focused_changes().expect("a changes tab"))
}

fn view<R>(
    h: &Harness,
    cx: &mut TestAppContext,
    read: impl FnOnce(&super::super::changes::ChangesView) -> R,
) -> R {
    h.shell(cx, |shell| {
        read(shell.changes.values().next().expect("a changes tab"))
    })
}

fn set_text(
    h: &Harness,
    cx: &mut TestAppContext,
    pick: impl FnOnce(&super::super::changes::ChangesView) -> TextField,
    text: &str,
) {
    let field = h.shell(cx, |shell| pick(shell.changes.values().next().unwrap()));
    cx.update_window(h.window.into(), |_, window, cx| match field {
        TextField::Area(state) => state.update(cx, |field, cx| {
            field.set_value(text.to_owned(), window, cx);
        }),
        TextField::Line(state) => state.update(cx, |field, cx| {
            field.set_value(text.to_owned(), window, cx);
        }),
    })
    .unwrap();
    h.settle(cx);
}

enum TextField {
    Area(Entity<gpui_kit::component::input::TextareaState>),
    Line(Entity<gpui_kit::component::input::InputState>),
}

fn message(h: &Harness, cx: &mut TestAppContext) -> String {
    view(h, cx, |view| view.message.clone());
    let state = view(h, cx, |view| view.message.clone());
    cx.update(|cx| state.read(cx).value().to_string())
}

fn field_text(
    cx: &mut TestAppContext,
    state: &Entity<gpui_kit::component::input::InputState>,
) -> String {
    cx.update(|cx| state.read(cx).value().to_string())
}

fn click(h: &Harness, cx: &mut TestAppContext, selector: &str) {
    h.mouse_on(selector.to_owned(), gpui_kit::MouseButton::Left, cx);
}

fn outcome(h: &Harness, cx: &mut TestAppContext) -> Option<(bool, String, String, Option<String>)> {
    view(h, cx, |view| {
        view.outcome
            .as_ref()
            .map(|o| (o.ok, o.title.clone(), o.text.clone(), o.url.clone()))
    })
}

fn calls(h: &Harness) -> Vec<(String, Vec<String>)> {
    h.runner
        .calls()
        .iter()
        .map(|call| (call.program.clone(), call.args.clone()))
        .collect()
}

#[gpui_kit::test]
fn the_chord_opens_the_changes_of_the_worktree_in_a_tab_with_its_files_and_the_first_diff(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = with_worktree(cx);
    let id = open_tab(&h, cx);
    assert_eq!(h.main_kind(cx), format!("live:{id}"));
    assert!(h.shows("changes-list", cx));
    assert!(h.shows("changes-file-0", cx) && h.shows("changes-file-2", cx));
    assert!(!h.shows("changes-file-3", cx), "three files changed");
    let paths: Vec<String> = view(&h, cx, |v| {
        v.files.iter().flatten().map(|f| f.path.clone()).collect()
    });
    assert_eq!(paths, ["src/a.rs", "b.rs", "c.txt"]);
    assert_eq!(
        view(&h, cx, |v| v.selected.clone()).as_deref(),
        Some("src/a.rs")
    );
    // The first file's diff is drawn: a hunk header, a context line, a
    // removed line and an added one.
    assert!(h.shows("changes-line-0", cx) && h.shows("changes-line-3", cx));
    assert!(!h.shows("changes-line-4", cx));
    // It went through the runner, in the worktree.
    let ran = h.runner.calls();
    assert_eq!(ran[0].program, "git");
    assert_eq!(ran[0].cwd.as_deref(), Some(path.as_str()));
    assert!(ran[1].args.contains(&"HEAD".to_owned()));
    assert_eq!(ran[1].args.last().map(String::as_str), Some("src/a.rs"));
    // The pull request's fields start from the branch and its commits.
    let base = view(&h, cx, |v| v.base.clone());
    assert_eq!(field_text(cx, &base), "main");
    let title = view(&h, cx, |v| v.title.clone());
    assert_eq!(field_text(cx, &title), "Fix the tabs");
    // The tab is named by what changed, and the branch is said.
    assert!(h.shows("terminal-tabs", cx));
    assert_eq!(
        h.shell(cx, |s| s.changes_heading(&s.changes[&id]).1),
        "topic"
    );
    assert_eq!(
        h.shell(cx, |s| Shell::changes_label(&s.changes[&id])),
        "Changes 3"
    );
}

#[gpui_kit::test]
fn asking_again_shows_the_tab_it_has_instead_of_adding_one(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    let id = open_tab(&h, cx);
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    h.press_chord("cmd-alt-g", "ctrl-shift-alt-g", cx);
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1);
    assert_eq!(h.main_kind(cx), format!("live:{id}"));
}

#[gpui_kit::test]
fn the_arrows_choose_the_file_and_each_choice_reads_its_diff(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    h.runner.queue(Output::ok(DIFF_B));
    h.press("down", cx);
    assert_eq!(
        view(&h, cx, |v| v.selected.clone()).as_deref(),
        Some("b.rs")
    );
    assert!(h.shows("changes-line-2", cx) && !h.shows("changes-line-3", cx));
    assert_eq!(
        h.runner
            .calls()
            .last()
            .unwrap()
            .args
            .last()
            .map(String::as_str),
        Some("b.rs")
    );
    // `k` and `j` move too, as in the other lists.
    h.runner.queue(Output::ok(DIFF_A));
    h.press("k", cx);
    assert_eq!(
        view(&h, cx, |v| v.selected.clone()).as_deref(),
        Some("src/a.rs")
    );
    h.runner.queue(Output::ok(DIFF_B));
    h.press("j", cx);
    assert_eq!(
        view(&h, cx, |v| v.selected.clone()).as_deref(),
        Some("b.rs")
    );
    // The last file stops the movement.
    h.runner.queue(Output {
        status: Some(1),
        stdout: "diff --git 1/c.txt 2/c.txt\nnew file mode 100644\n--- /dev/null\n+++ 2/c.txt\n@@ -0,0 +1 @@\n+x\n".into(),
        stderr: String::new(),
    });
    h.press("down", cx);
    h.press("down", cx);
    assert_eq!(
        view(&h, cx, |v| v.selected.clone()).as_deref(),
        Some("c.txt")
    );
    let last = h.runner.calls().last().unwrap().args.clone();
    assert!(
        last.contains(&"--no-index".to_owned()),
        "a new file is diffed against nothing"
    );
}

#[gpui_kit::test]
fn enter_opens_the_selected_file_in_the_editor(cx: &mut TestAppContext) {
    let (h, dir, path) = with_worktree(cx);
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "keep\nnew\n").unwrap();
    let id = open_tab(&h, cx);
    h.press("enter", cx);
    crate::ui::tests::live::wait_until(&h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |shell| shell.focused_file().is_some())
    });
    let opened = h.shell(cx, |s| {
        s.files
            .values()
            .map(|doc| doc.path.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(opened, [format!("{path}/src/a.rs")]);
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1, "the changes tab stays");
    assert_ne!(h.main_kind(cx), format!("live:{id}"));
}

#[gpui_kit::test]
fn space_takes_a_file_out_of_the_commit_and_back_in(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    h.press("space", cx);
    assert!(view(&h, cx, |v| v.unchosen.contains("src/a.rs")));
    assert_eq!(view(&h, cx, |v| v.chosen().len()), 2);
    h.press("space", cx);
    assert!(view(&h, cx, |v| v.unchosen.is_empty()));
}

#[gpui_kit::test]
fn a_binary_file_and_a_huge_diff_are_said_instead_of_drawn(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    h.runner.queue(Output::ok(
        "diff --git a/b.rs b/b.rs\nnew file mode 100644\nBinary files /dev/null and b/b.rs differ\n",
    ));
    h.press("down", cx);
    assert!(h.shows("changes-binary", cx));
    assert!(!h.shows("changes-line-0", cx));
    h.runner.queue(Output::ok(format!(
        "@@ -1 +1 @@\n+{}\n",
        "x".repeat(1_100_000)
    )));
    h.press("down", cx);
    assert!(h.shows("changes-too-large", cx));
}

#[gpui_kit::test]
fn a_diff_git_cannot_make_is_shown_as_git_said_it(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    h.runner
        .queue(Output::failed(128, "fatal: unable to read the object"));
    h.runner
        .queue(Output::failed(128, "fatal: unable to read the object"));
    h.press("down", cx);
    assert!(h.shows("changes-diff-failed", cx));
}

fn allowed(h: &Harness, cx: &mut TestAppContext) -> super::super::changes::Allowed {
    let id = h.shell(cx, |s| *s.changes.keys().next().unwrap());
    cx.update(|cx| h.shell.read(cx).changes_allowed(id, cx))
}

#[gpui_kit::test]
fn a_clean_tree_says_so_and_the_commit_is_not_allowed(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    h.runner.queue(Output::ok("## topic...origin/topic\0"));
    queue_start(&h, "");
    h.press_chord("cmd-alt-g", "ctrl-shift-alt-g", cx);
    assert!(h.shows("changes-clean", cx));
    assert!(!allowed(&h, cx).commit);
}

#[gpui_kit::test]
fn a_status_git_cannot_give_is_shown_as_git_said_it(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    h.runner.queue(Output::failed(
        128,
        "fatal: not a git repository (or any of the parent directories)",
    ));
    queue_start(&h, "");
    h.press_chord("cmd-alt-g", "ctrl-shift-alt-g", cx);
    assert!(h.shows("changes-failed", cx));
    assert!(view(&h, cx, |v| v.failed.clone().unwrap()).contains("not a git repository"));
}

#[gpui_kit::test]
fn committing_adds_and_commits_what_is_chosen_and_empties_the_message(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    assert!(!allowed(&h, cx).commit, "no message yet");
    set_text(
        &h,
        cx,
        |v| TextField::Area(v.message.clone()),
        "Fix the tabs\n\nThey leaked.",
    );
    assert!(allowed(&h, cx).commit);
    // Take one file out: the commit names the others.
    h.press("down", cx); // reads the diff of b.rs
    h.press("space", cx);
    let before = h.runner.calls().len();
    h.runner.queue(Output::ok("")); // git add
    h.runner.queue(Output::ok(
        "[topic 1a2b3c4] Fix the tabs\n 2 files changed\n",
    )); // git commit
    h.runner
        .queue(Output::ok("## topic...origin/topic\0 M src/a.rs\0")); // the store's status
    h.runner
        .queue(Output::ok("## topic...origin/topic [ahead 1]\0")); // the window reads again
    queue_start(&h, "Fix the tabs\u{1f}They leaked.\u{1e}");
    click(&h, cx, "changes-commit");
    let ran = calls(&h);
    let add = &ran[before];
    assert_eq!(add.0, "git");
    assert_eq!(add.1[1], "add");
    assert!(
        add.1
            .ends_with(&["--".into(), "src/a.rs".into(), "c.txt".into()]),
        "{add:?}"
    );
    let commit = h.runner.calls()[before + 1].clone();
    assert_eq!(commit.args[1], "commit");
    assert_eq!(
        commit.stdin.as_deref(),
        Some(b"Fix the tabs\n\nThey leaked.\n".as_slice())
    );
    let (ok, title, text, url) = outcome(&h, cx).expect("an outcome");
    assert!(ok && url.is_none());
    assert_eq!(title, "Committed");
    assert!(text.contains("[topic 1a2b3c4] Fix the tabs"));
    assert_eq!(message(&h, cx), "", "the message was used");
    assert!(h.status().starts_with("Committed"));
}

fn run(h: &Harness, cx: &mut TestAppContext, command: Command) {
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |shell, cx| shell.run_command(command, window, cx))
    })
    .unwrap();
    h.settle(cx);
}

#[gpui_kit::test]
fn a_commit_without_a_message_asks_for_one_and_runs_nothing(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    let before = h.runner.calls().len();
    // Showing the tab reads it again; nothing is committed or pushed.
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    run(&h, cx, Command::ShipChanges);
    assert_eq!(h.runner.calls().len(), before + 2);
    assert!(calls(&h)[before..]
        .iter()
        .all(|(_, args)| !args.contains(&"commit".to_owned())));
    let (ok, title, _, _) = outcome(&h, cx).expect("said why");
    assert!(!ok);
    assert_eq!(title, "Write a commit message first.");
}

#[gpui_kit::test]
fn the_chain_on_a_fresh_tab_waits_for_the_reading_and_then_pushes_a_branch_with_nothing_to_commit(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _path) = with_worktree(cx);
    // The tab is not open: its files are read first (clean, one commit
    // ahead), then the start of the pull request from the commits.
    h.runner
        .queue(Output::ok("## topic...origin/topic [ahead 1]\0"));
    queue_start(&h, "Add the form\u{1f}Wire it.\u{1e}");
    // Then the chain itself: the branch, the push and `gh pr create`.
    h.runner
        .queue(Output::ok("## topic...origin/topic [ahead 1]\0"));
    h.runner.queue(Output::ok(""));
    h.runner
        .queue(Output::ok("https://github.com/zavudev/leon/pull/43\n"));
    run(&h, cx, Command::ShipChanges);
    for _ in 0..6 {
        h.settle(cx);
    }
    let ran = calls(&h);
    assert!(
        ran.iter()
            .all(|(_, args)| !args.contains(&"commit".to_owned())
                && !args.contains(&"add".to_owned())),
        "nothing to commit, nothing committed: {ran:?}"
    );
    let push = ran
        .iter()
        .position(|(_, args)| args.first().map(String::as_str) == Some("push"))
        .unwrap_or_else(|| panic!("no push in {ran:?}"));
    let create = ran
        .iter()
        .position(|(program, args)| {
            program == "gh" && args.get(1).map(String::as_str) == Some("create")
        })
        .unwrap_or_else(|| panic!("no pull request in {ran:?}"));
    assert!(push < create, "the push comes first: {ran:?}");
    assert_eq!(
        ran[create].1[3], "Add the form",
        "the title is the one the commits gave"
    );
    let (ok, title, _, url) = outcome(&h, cx).unwrap();
    assert!(ok, "{title}");
    assert_eq!(title, "Pushed and opened a pull request");
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/zavudev/leon/pull/43")
    );
}

#[gpui_kit::test]
fn the_chain_on_a_fresh_tab_whose_files_cannot_be_read_says_so_and_runs_nothing(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _path) = with_worktree(cx);
    h.runner.queue(Output::failed(
        128,
        "fatal: not a git repository (or any of the parent directories)",
    ));
    queue_start(&h, "");
    run(&h, cx, Command::ShipChanges);
    for _ in 0..4 {
        h.settle(cx);
    }
    let (ok, title, _, _) = outcome(&h, cx).unwrap();
    assert!(!ok);
    assert_eq!(title, "The changes could not be read, so nothing was run.");
    assert!(calls(&h)
        .iter()
        .all(|(_, args)| args.first().map(String::as_str) != Some("push")));
}

#[gpui_kit::test]
fn a_hook_that_refuses_the_commit_is_shown_whole_and_the_message_stays(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    set_text(&h, cx, |v| TextField::Area(v.message.clone()), "Fix");
    h.runner.queue(Output::ok(""));
    h.runner.queue(Output {
        status: Some(1),
        stdout: "running the checks\n".into(),
        stderr: "check failed: 2 errors\n".into(),
    });
    click(&h, cx, "changes-commit");
    let (ok, title, text, _) = outcome(&h, cx).expect("an outcome");
    assert!(!ok);
    assert_eq!(title, "The commit failed");
    assert_eq!(text, "running the checks\ncheck failed: 2 errors");
    assert_eq!(message(&h, cx), "Fix", "nothing is lost");
    assert!(h.shows("changes-outcome-text", cx));
}

#[gpui_kit::test]
fn pushing_sets_the_upstream_when_there_is_none_and_never_forces(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    h.runner.queue(Output::ok("## topic\0 M a.rs\0"));
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "");
    h.press_chord("cmd-alt-g", "ctrl-shift-alt-g", cx);
    for _ in 0..3 {
        h.settle(cx);
    }
    assert!(!view(&h, cx, |v| v.has_upstream));
    let before = h.runner.calls().len();
    h.runner.queue(Output::ok("## topic\0 M a.rs\0")); // the branch, read before pushing
    h.runner.queue(Output {
        status: Some(0),
        stdout: String::new(),
        stderr: "To github.com:zavudev/leon.git\n * [new branch]      topic -> topic\n".into(),
    });
    h.runner
        .queue(Output::ok("## topic...origin/topic\0 M a.rs\0")); // the store
    h.runner
        .queue(Output::ok("## topic...origin/topic\0 M a.rs\0")); // the window
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "");
    click(&h, cx, "changes-push");
    let ran = calls(&h);
    assert_eq!(
        ran[before + 1].1,
        ["push", "--set-upstream", "origin", "topic"]
    );
    assert!(ran
        .iter()
        .all(|(_, args)| !args.iter().any(|a| a.contains("force"))));
    let (ok, title, text, _) = outcome(&h, cx).unwrap();
    assert!(ok);
    assert_eq!(title, "Pushed");
    assert!(text.contains("[new branch]"));
    assert!(
        view(&h, cx, |v| v.has_upstream),
        "the window read the branch again"
    );
}

#[gpui_kit::test]
fn the_pull_request_form_opens_prefilled_and_opens_the_pull_request_with_a_link(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = with_worktree(cx);
    open_tab(&h, cx);
    assert!(!h.shows("changes-form", cx));
    // Opening the form asks the commits again.
    queue_start(&h, "Fix the tabs\u{1f}They leaked.\u{1e}");
    click(&h, cx, "changes-pull-request");
    assert!(h.shows("changes-form", cx));
    click(&h, cx, "changes-draft");
    assert!(view(&h, cx, |v| v.draft));
    let before = h.runner.calls().len();
    h.runner
        .queue(Output::ok("https://github.com/zavudev/leon/pull/41\n"));
    h.runner
        .queue(Output::ok("## topic...origin/topic\0 M src/a.rs\0")); // the store
    h.runner
        .queue(Output::ok("git@github.com:zavudev/leon.git\n")); // is it on GitHub
    h.runner.queue(Output::ok(
        r#"[{"number":41,"url":"https://github.com/zavudev/leon/pull/41","headRefName":"topic","isDraft":true,"reviewDecision":"","statusCheckRollup":[]}]"#,
    ));
    h.runner
        .queue(Output::ok("## topic...origin/topic\0 M src/a.rs\0")); // the window
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "Fix the tabs\u{1f}They leaked.\u{1e}");
    click(&h, cx, "changes-open-pull-request");
    let create = h.runner.calls()[before].clone();
    assert_eq!(create.program, "gh");
    assert_eq!(
        create.args,
        [
            "pr",
            "create",
            "--title",
            "Fix the tabs",
            "--body-file",
            "-",
            "--base",
            "main",
            "--draft"
        ]
    );
    assert_eq!(create.stdin.as_deref(), Some(b"They leaked.\n".as_slice()));
    let (ok, title, _, url) = outcome(&h, cx).unwrap();
    assert!(ok);
    assert_eq!(title, "Opened a pull request");
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/zavudev/leon/pull/41")
    );
    // The worktree's own status has the pull request at once.
    let worktree = h
        .store
        .all_worktrees()
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.path == path)
        .expect("the worktree of the test");
    let status = h
        .store
        .worktree_statuses()
        .unwrap()
        .remove(&worktree.id)
        .expect("its status was written");
    assert_eq!(
        status.pull_request.map(|pr| (pr.number, pr.draft)),
        Some((41, true))
    );
    // The link opens in the browser.
    assert!(h.shows("changes-url", cx));
    click(&h, cx, "changes-url");
    assert_eq!(
        *h.urls.borrow(),
        ["https://github.com/zavudev/leon/pull/41"]
    );
}

#[gpui_kit::test]
fn one_command_commits_pushes_and_opens_the_pull_request_and_each_failure_stops_it(
    cx: &mut TestAppContext,
) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    set_text(
        &h,
        cx,
        |v| TextField::Area(v.message.clone()),
        "Fix the tabs",
    );
    set_text(&h, cx, |v| TextField::Line(v.title.clone()), "");
    let before = h.runner.calls().len();
    h.runner.queue(Output::ok("")); // add
    h.runner.queue(Output::ok("[topic 1a2b3c4] Fix the tabs\n")); // commit
    h.runner.queue(Output::ok("## topic...origin/topic\0")); // the branch
    h.runner.queue(Output::failed(
        1,
        "! [rejected] topic -> topic (non-fast-forward)\nhint: pull first\n",
    ));
    h.runner
        .queue(Output::ok("## topic...origin/topic [behind 1]\0")); // the store
    h.runner
        .queue(Output::ok("## topic...origin/topic [behind 1]\0")); // the window
    h.runner.queue(Output::ok("diff --git a/x b/x\n"));
    queue_start(&h, "");
    click(&h, cx, "changes-ship");
    let ran = calls(&h);
    let programs: Vec<&str> = ran[before..before + 4]
        .iter()
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(programs, ["git", "git", "git", "git"]);
    assert!(
        !ran.iter().any(|(program, _)| program == "gh"),
        "the pull request waits for the push: {ran:?}"
    );
    let (ok, title, text, _) = outcome(&h, cx).unwrap();
    assert!(!ok);
    assert_eq!(title, "The push failed");
    assert!(text.contains("non-fast-forward") && text.contains("hint: pull first"));
    assert!(
        text.contains("[topic 1a2b3c4]"),
        "the commit that was made is told too: {text:?}"
    );
}

#[gpui_kit::test]
fn the_whole_chain_ends_with_the_address_of_the_pull_request(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    set_text(
        &h,
        cx,
        |v| TextField::Area(v.message.clone()),
        "Fix the tabs\n\nThey leaked.",
    );
    set_text(&h, cx, |v| TextField::Line(v.title.clone()), "");
    let before = h.runner.calls().len();
    h.runner.queue(Output::ok(""));
    h.runner.queue(Output::ok("[topic 1a2b3c4] Fix the tabs\n"));
    h.runner.queue(Output::ok("## topic...origin/topic\0"));
    h.runner.queue(Output::ok(""));
    h.runner
        .queue(Output::ok("https://github.com/zavudev/leon/pull/42\n"));
    h.runner.queue(Output::ok("## topic...origin/topic\0"));
    h.runner
        .queue(Output::ok("git@github.com:zavudev/leon.git\n"));
    h.runner.queue(Output::ok("[]"));
    h.runner.queue(Output::ok("## topic...origin/topic\0"));
    queue_start(&h, "");
    click(&h, cx, "changes-ship");
    let ran = calls(&h);
    let steps: Vec<String> = ran[before..before + 5]
        .iter()
        .map(|(program, args)| format!("{program} {}", args.get(1).or(args.first()).unwrap()))
        .collect();
    assert_eq!(
        steps,
        [
            "git add",
            "git commit",
            "git status",
            "git push",
            "gh create"
        ]
    );
    // The pull request starts from the message when nothing else was written.
    let create = h.runner.calls()[before + 4].clone();
    assert_eq!(create.args[3], "Fix the tabs");
    assert_eq!(create.stdin.as_deref(), Some(b"They leaked.\n".as_slice()));
    let (ok, title, _, url) = outcome(&h, cx).unwrap();
    assert!(ok);
    assert_eq!(title, "Committed, pushed and opened a pull request");
    assert_eq!(
        url.as_deref(),
        Some("https://github.com/zavudev/leon/pull/42")
    );
}

#[gpui_kit::test]
fn an_agent_with_a_known_headless_form_words_the_message_and_a_failure_is_plain(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = with_worktree(cx);
    open_tab(&h, cx);
    assert!(
        h.shows("changes-suggest", cx),
        "claude can be asked headless"
    );
    h.runner.queue(Output::ok(DIFF_A)); // the diff it reads
    h.runner.queue(Output::ok("```\nFix the tabs\n```\n")); // its answer
    click(&h, cx, "changes-suggest");
    assert_eq!(message(&h, cx), "Fix the tabs");
    let ask = h.runner.calls().last().unwrap().clone();
    assert_eq!(ask.program, "claude");
    assert_eq!(ask.cwd.as_deref(), Some(path.as_str()));
    assert!(ask
        .stdin
        .as_deref()
        .is_some_and(|stdin| stdin.starts_with(b"diff --git")));
    // A failure is a message, and committing is still allowed.
    set_text(&h, cx, |v| TextField::Area(v.message.clone()), "");
    h.runner.queue(Output::ok(DIFF_A));
    h.runner.queue(Output::failed(1, "Not logged in"));
    click(&h, cx, "changes-suggest");
    let (ok, title, text, _) = outcome(&h, cx).unwrap();
    assert!(!ok);
    assert!(title.contains("could not word it"));
    assert_eq!(text, "Not logged in");
    set_text(&h, cx, |v| TextField::Area(v.message.clone()), "By hand");
    assert!(allowed(&h, cx).commit, "a failed suggestion blocks nothing");
}

#[gpui_kit::test]
fn typing_in_the_message_is_text_not_shortcuts(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    let field = view(&h, cx, |v| v.message.clone());
    cx.update_window(h.window.into(), |_, window, cx| {
        use gpui_kit::Focusable as _;
        window.focus(&field.focus_handle(cx), cx);
    })
    .unwrap();
    h.type_text("a/b j k", cx);
    h.press("down", cx);
    assert_eq!(message(&h, cx), "a/b j k");
    assert_eq!(
        view(&h, cx, |v| v.selected.clone()).as_deref(),
        Some("src/a.rs"),
        "an arrow in the field moves the cursor of the text, not the list"
    );
}

#[gpui_kit::test]
fn the_palette_lists_the_three_commands_and_asks_which_worktree_when_none_is_in_view(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let h = open(cx, ScriptedRunner::new());
    h.press_chord("cmd-shift-p", "ctrl-shift-p", cx);
    h.set_palette_text(">changes", cx);
    let titles = h.palette_titles(cx);
    assert!(
        titles
            .iter()
            .any(|t| t == "Show the changes of the worktree"),
        "{titles:?}"
    );
    h.set_palette_text(">push", cx);
    assert!(h.palette_titles(cx).iter().any(|t| t == "Push the branch"));
    h.set_palette_text(">pull request", cx);
    assert!(h
        .palette_titles(cx)
        .iter()
        .any(|t| t == "Commit, push and open a pull request"));
    h.press("escape", cx);
    // Nothing is in view: the palette asks which worktree.
    run(&h, cx, Command::OpenChanges);
    assert_eq!(overlay(&h, cx), Overlay::Palette);
    assert_eq!(asked(&h, cx), "Worktree");
    // The first one is chosen and its changes open in a tab.
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "");
    h.press("enter", cx);
    for _ in 0..3 {
        h.settle(cx);
    }
    assert_eq!(overlay(&h, cx), Overlay::None);
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1);
    assert!(h.shows("changes-list", cx));
}

fn overlay(h: &Harness, cx: &mut TestAppContext) -> Overlay {
    h.shell(cx, |shell| shell.overlay)
}

fn asked(h: &Harness, cx: &mut TestAppContext) -> String {
    h.shell(cx, |shell| {
        shell
            .palette
            .flow
            .as_ref()
            .map(|flow| flow.step.prompt.to_owned())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn the_worktree_screen_has_a_changes_button(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    h.press("enter", cx);
    assert!(h.main_kind(cx).starts_with("worktree:"));
    assert!(h.shows("worktree-changes", cx));
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "");
    click(&h, cx, "worktree-changes");
    for _ in 0..3 {
        h.settle(cx);
    }
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1);
    assert!(h.main_kind(cx).starts_with("live:"));
    assert!(h.shows("changes-file-0", cx));
}

#[gpui_kit::test]
fn the_cross_of_the_tab_closes_it_and_the_worktree_comes_back(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    assert!(h.shows("terminal-tab-close-0", cx));
    click(&h, cx, "terminal-tab-close-0");
    assert_eq!(h.shell(cx, |s| s.changes.len()), 0);
    assert!(
        h.main_kind(cx).starts_with("worktree:"),
        "{}",
        h.main_kind(cx)
    );
    assert_eq!(h.shell(cx, |s| s.workspaces.len()), 0);
}

#[gpui_kit::test]
fn the_file_tree_has_a_chip_for_the_changes_of_its_folder(cx: &mut TestAppContext) {
    let (h, _dir, path) = with_worktree(cx);
    let worktree = h
        .store
        .all_worktrees()
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.path == path)
        .unwrap();
    h.store
        .update_worktree_status(&worktree.id, |status| status.changed = Some(3))
        .unwrap();
    h.settle(cx);
    h.press_chord("cmd-shift-e", "ctrl-shift-alt-e", cx);
    for _ in 0..3 {
        h.settle(cx);
    }
    assert!(h.shows("files-changes", cx));
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "");
    click(&h, cx, "files-changes");
    for _ in 0..3 {
        h.settle(cx);
    }
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1);
    assert!(h.shows("changes-file-0", cx));
}

#[gpui_kit::test]
fn the_palette_command_pushes_the_branch_of_a_worktree_whose_tab_is_not_open_yet(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = with_worktree(cx);
    // The tab opens (its status, diff and pull request start are read) while
    // the push asks git for the branch: the commands run side by side, so any
    // answer will do for all of them but the one this test looks for.
    for _ in 0..12 {
        h.runner.queue(Output::ok(STATUS));
    }
    run(&h, cx, Command::PushBranch);
    for _ in 0..3 {
        h.settle(cx);
    }
    let ran = h.runner.calls();
    let push = ran
        .iter()
        .find(|call| call.args.first().map(String::as_str) == Some("push"))
        .unwrap_or_else(|| panic!("no push in {:?}", calls(&h)));
    assert_eq!(push.args[0], "push", "never forced: {:?}", push.args);
    assert!(push.args.iter().all(|arg| !arg.contains("force")));
    assert_eq!(push.cwd.as_deref(), Some(path.as_str()));
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1);
}

#[gpui_kit::test]
fn the_close_chord_closes_the_changes_tab_and_leaves_the_rest(cx: &mut TestAppContext) {
    let (h, _dir, _path) = with_worktree(cx);
    open_tab(&h, cx);
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert_eq!(h.shell(cx, |s| s.changes.len()), 0);
    assert!(
        h.main_kind(cx).starts_with("worktree:"),
        "{}",
        h.main_kind(cx)
    );
}

#[gpui_kit::test]
fn removing_merged_worktrees_closes_their_changes_tabs_and_a_second_batch_cannot_cut_the_first(
    cx: &mut TestAppContext,
) {
    let (h, _dir, path) = with_worktree(cx);
    let project = h
        .store
        .projects(Some(&MachineId::local()))
        .unwrap()
        .into_iter()
        .find(|project| project.root == path)
        .unwrap()
        .id;
    let (first, second) = (format!("{path}-first"), format!("{path}-second"));
    let linked = |folder: &str, branch: &str| NewWorktree {
        path: folder.to_owned(),
        branch: Some(branch.to_owned()),
        head: None,
        is_main: false,
    };
    h.store
        .replace_worktrees(
            &project,
            vec![
                NewWorktree {
                    path: path.clone(),
                    branch: Some("topic".into()),
                    head: None,
                    is_main: true,
                },
                linked(&first, "first"),
                linked(&second, "second"),
            ],
        )
        .unwrap();
    h.settle(cx);
    let ids: Vec<leon_core::WorktreeId> = h
        .store
        .worktrees(&project)
        .unwrap()
        .into_iter()
        .filter(|worktree| !worktree.is_main)
        .map(|worktree| worktree.id)
        .collect();
    // The changes of the first linked worktree are open in a tab.
    h.runner.queue(Output::ok(STATUS));
    h.runner.queue(Output::ok(DIFF_A));
    queue_start(&h, "");
    let target = super::super::changes::Target {
        machine: MachineId::local(),
        project: project.clone(),
        worktree: Some(ids[0].clone()),
        path: first.clone(),
    };
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.open_changes(target, window, cx);
        })
    })
    .unwrap();
    for _ in 0..3 {
        h.settle(cx);
    }
    assert_eq!(h.shell(cx, |s| s.changes.len()), 1);
    // git removes the first, lists the project and (the first sync) reads the
    // remote, which is not GitHub; then the second one and its list.
    let listing = |extra: Option<&str>| {
        let mut text = format!("worktree {path}\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/topic\n");
        if let Some(extra) = extra {
            text.push_str(&format!(
                "\nworktree {extra}\nHEAD 3333333333333333333333333333333333333333\nbranch refs/heads/second\n"
            ));
        }
        text
    };
    h.runner.queue(Output::ok(""));
    h.runner.queue(Output::ok(listing(Some(&second))));
    h.runner
        .queue(Output::ok("git@gitlab.com:zavudev/leon.git\n"));
    h.runner.queue(Output::ok(""));
    h.runner.queue(Output::ok(listing(None)));
    // Both batches are asked for before the executor runs anything.
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.remove_worktrees(
                ids.iter()
                    .map(|worktree| (project.clone(), worktree.clone()))
                    .collect(),
                window,
                cx,
            );
            shell.remove_worktrees(vec![(project.clone(), ids[1].clone())], window, cx);
        })
    })
    .unwrap();
    assert_eq!(h.status(), crate::ui::steps::REMOVAL_RUNNING);
    h.settle(cx);
    h.settle(cx);
    let removed: Vec<String> = calls(&h)
        .into_iter()
        .filter(|(_, args)| args.starts_with(&["worktree".to_owned(), "remove".to_owned()]))
        .map(|(_, args)| args[2].clone())
        .collect();
    assert_eq!(
        removed,
        [first, second],
        "the refused second batch did not cut the first"
    );
    assert_eq!(h.status(), "Removed 2 worktrees.");
    assert_eq!(
        h.shell(cx, |s| s.changes.len()),
        0,
        "the tab of a removed worktree closes with it"
    );
    assert_eq!(h.store.worktrees(&project).unwrap().len(), 1);
}
