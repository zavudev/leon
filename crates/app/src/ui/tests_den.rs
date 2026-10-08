//! Tests of the Den in the window: how it opens and closes, which keys it
//! takes, that every live session is a lion, and that a session whose
//! transcript is on this computer shows what the transcript says. The
//! scripted terminals of `tests.rs` stand in for real ones; the transcript
//! is a file the test writes.

use super::live::{open_live, real_worktree, screen, wait_until};
use super::*;
use crate::schema::{self, Value};
use crate::ui::live::LiveId;
use leon_den::CubState;
use std::io::Write;
use std::time::Duration;

/// A long time after the view was made: everybody has arrived, every line
/// is typed.
const LATER: Duration = Duration::from_secs(3600);

fn toggle(h: &Harness, cx: &mut TestAppContext) {
    h.press_chord("cmd-alt-shift-l", "ctrl-alt-shift-l", cx);
}

fn shell_session(h: &Harness, cx: &mut TestAppContext) -> tempfile::TempDir {
    let (dir, _) = real_worktree(h, cx);
    h.press("ctrl-t", cx);
    wait_until(h, cx, "the shell", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(1)).is_some())
    });
    dir
}

/// Waits as [`wait_until`] does, with the clock of the window moving on:
/// the Den polls on a timer.
fn tick_until(
    h: &Harness,
    cx: &mut TestAppContext,
    what: &str,
    condition: impl Fn(&Harness, &mut TestAppContext) -> bool,
) {
    wait_until(h, cx, what, |h, cx| {
        cx.executor().advance_clock(Duration::from_millis(10));
        condition(h, cx)
    });
}

fn lions(h: &Harness, cx: &mut TestAppContext) -> Vec<(String, CubState, u32)> {
    h.shell(cx, |shell| {
        shell
            .den
            .cubs
            .iter()
            .map(|cub| (cub.name.clone(), cub.state, cub.level))
            .collect()
    })
}

fn said(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        let view = h
            .shell
            .read(cx)
            .den
            .view
            .clone()
            .expect("the Den was opened");
        view.read(cx).read(|den| den.frame(LATER).said.rows)
    })
}

/// What the feed holds, the oldest first: `said NAME: text` for what an
/// agent wrote, the narrator's line otherwise.
fn told(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        let view = h
            .shell
            .read(cx)
            .den
            .view
            .clone()
            .expect("the Den was opened");
        view.read(cx).read(|den| {
            den.feed()
                .entries()
                .map(|entry| {
                    let text = entry.item.text.replace('\n', " ");
                    match entry.item.kind {
                        leon_den::feed::Kind::Speech => format!("said: {text}"),
                        leon_den::feed::Kind::Narration { .. } => text,
                    }
                })
                .collect()
        })
    })
}

#[gpui_kit::test]
fn the_chord_opens_the_den_in_the_main_pane_and_again_puts_back_what_was_there(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    assert_eq!(h.main_kind(cx), "live:1");
    toggle(&h, cx);
    assert_eq!(h.main_kind(cx), "den");
    assert!(h.shows("den", cx), "the Den is drawn");
    assert!(h.shows("sidebar", cx), "the sidebar stays");
    assert!(h.shell(cx, |shell| shell.pane == Pane::Main));
    toggle(&h, cx);
    assert_eq!(h.main_kind(cx), "live:1", "back to the terminal");
    assert!(!h.shows("den", cx));
    assert!(
        h.shell(cx, |shell| shell.terminal_focused()),
        "and the terminal has the keyboard again"
    );
}

#[gpui_kit::test]
fn escape_closes_the_den_and_puts_back_a_project_too(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, _) = real_worktree(&h, cx);
    h.press("enter", cx);
    let before = h.main_kind(cx);
    assert!(before.starts_with("worktree:"), "{before}");
    toggle(&h, cx);
    assert_eq!(h.main_kind(cx), "den");
    h.press("escape", cx);
    assert_eq!(h.main_kind(cx), before);
}

#[gpui_kit::test]
fn the_den_is_in_the_palette_with_its_keys(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("open the den", cx);
    assert_eq!(h.palette_titles(cx)[1], "Open the Den");
    assert!(
        h.shows("palette-keys-1", cx),
        "its shortcut is printed beside it"
    );
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "den");
    // "lions" finds it too.
    h.press("escape", cx);
    h.press("ctrl-shift-p", cx);
    h.type_text("lions", cx);
    assert_eq!(h.palette_titles(cx)[1], "Open the Den");
}

#[gpui_kit::test]
fn an_empty_den_says_so_and_follows_nothing(cx: &mut TestAppContext) {
    let h = open_live(cx);
    toggle(&h, cx);
    assert!(lions(&h, cx).is_empty());
    assert_eq!(said(&h, cx), vec!["The den is quiet. Too quiet."]);
    assert!(
        h.shell(cx, |shell| shell.den.watch_is_idle()),
        "no session is live: nothing is polled"
    );
    let heading = h.shell(cx, |shell| shell.den_heading().2);
    assert_eq!(heading, "0 LIVE \u{b7} 0 WORKING");
}

#[gpui_kit::test]
fn every_live_session_is_a_lion_and_enter_opens_its_terminal(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    wait_until(&h, cx, "the second shell", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(2)).is_some())
    });
    toggle(&h, cx);
    let names: Vec<String> = lions(&h, cx).into_iter().map(|(name, _, _)| name).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    // A plain shell is no mystery: its terminal says all there is to say.
    assert!(h.shell(cx, |shell| shell.den.cubs.iter().all(|cub| !cub.mystery)));

    // Down selects the first, down again the second; Enter opens it.
    h.press("down", cx);
    h.press("down", cx);
    let selected = cx.update(|cx| {
        h.shell
            .read(cx)
            .den
            .view
            .clone()
            .unwrap()
            .read(cx)
            .selected()
    });
    assert_eq!(selected, Some(2));
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "live:2");
    // The Den was left for the terminal: its chord opens it again, and
    // closing it comes back to that terminal.
    toggle(&h, cx);
    h.press("escape", cx);
    assert_eq!(h.main_kind(cx), "live:2");
}

#[gpui_kit::test]
fn opening_something_from_the_sidebar_leaves_the_den(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    toggle(&h, cx);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.pane = Pane::Sidebar;
            cx.notify();
        })
    });
    // The sidebar's cursor is on the Den's own row: down from it is the
    // tree, where the shell's row is.
    h.press("down", cx);
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let live = shell
                .rows
                .iter()
                .position(|row| matches!(row.kind, crate::ui::tree::Kind::Live(_)))
                .expect("the shell has a row");
            shell.move_cursor_to(live);
            cx.notify();
        })
    });
    h.press("enter", cx);
    assert_ne!(h.main_kind(cx), "den");
    tick_until(&h, cx, "the loop to end", |h, cx| {
        h.shell(cx, |shell| shell.den.watch_is_idle())
    });
}

#[gpui_kit::test]
fn the_narrator_can_be_turned_off_for_a_plain_count(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    cx.update(|cx| {
        crate::settings::set_value(
            cx,
            schema::find("den_narrator").unwrap(),
            Value::Bool(false),
        )
    });
    toggle(&h, cx);
    tick_until(&h, cx, "the plain count", |h, cx| {
        said(h, cx)
            .first()
            .is_some_and(|row| row.starts_with("1 live session:"))
    });
}

fn line(file: &mut std::fs::File, json: &str) {
    writeln!(file, "{json}").unwrap();
    file.flush().unwrap();
}

#[gpui_kit::test]
fn a_session_whose_transcript_is_here_shows_what_the_transcript_says(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code
    wait_until(&h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });

    // Claude's files in a folder of the test's, and the session's own id as
    // if it had been learned from the process.
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let session = "0a1b2c3d-0000-4000-8000-00000000000a";
    let transcript = projects
        .join(leon_history::live::claude_project_dir_name(&path))
        .join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(&transcript).unwrap();
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":"fix the build"}}"#,
    );
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/work/shell.rs"}}]}}"#,
    );
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
    );
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/work/shell.rs","old_string":"a","new_string":"b"}}]}}"#,
    );
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(projects);
    h.engine.set_prefs(prefs);
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let live = shell.live.get_mut(LiveId(1)).unwrap();
            live.learned = Some((session.to_owned(), "state-file".to_owned()));
        })
    });

    toggle(&h, cx);
    tick_until(&h, cx, "the transcript to be read", |h, cx| {
        lions(h, cx)
            .first()
            .is_some_and(|(_, _, level)| *level == 2)
    });
    let (state, detail, mystery) = h.shell(cx, |shell| {
        let cub = &shell.den.cubs[0];
        (cub.state, cub.detail.clone(), cub.mystery)
    });
    assert!(!mystery);
    // The call has no result: editing while the terminal prints, a
    // permission prompt once it is quiet.
    assert!(
        matches!(state, CubState::Editing | CubState::NeedsPermission),
        "{state:?}"
    );
    assert!(detail.unwrap().contains("Edit shell.rs"));
    // What was read the first time is the past: it is summed up in one
    // line, not told tool by tool.
    let past = told(&h, cx);
    assert!(past.iter().all(|row| !row.contains("EDIT")), "{past:?}");
    assert_eq!(
        past.iter()
            .filter(|row| row.ends_with("used 2 tools."))
            .count(),
        1,
        "{past:?}"
    );

    // The edit ends and a command starts: told as news, with its real name.
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"ok"}]}}"#,
    );
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"cargo test -p leon-den"}}]}}"#,
    );
    tick_until(&h, cx, "the command to be seen", |h, cx| {
        lions(h, cx)
            .first()
            .is_some_and(|(_, _, level)| *level == 3)
    });
    tick_until(&h, cx, "the narrator's line", |h, cx| {
        told(h, cx).join(" ").contains("used CARGO TEST!")
    });

    // What the agent writes to the user is in the feed in its own words,
    // with the time the transcript gives it.
    line(
        &mut file,
        r#"{"type":"assistant","timestamp":"2026-10-04T12:04:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"The build is fixed.\n\nTwo files changed."}]}}"#,
    );
    tick_until(&h, cx, "the agent's words", |h, cx| {
        told(h, cx)
            .last()
            .is_some_and(|row| row == "said: The build is fixed.  Two files changed.")
    });
    let at = cx.update(|cx| {
        let view = h.shell.read(cx).den.view.clone().unwrap();
        let at = view
            .read(cx)
            .read(|den| den.feed().entries().last().unwrap().item.at);
        at
    });
    assert_eq!(
        at,
        Some(fixed_now().timestamp() - 60),
        "a minute before now"
    );

    // Closed, the Den reads nothing. What was written meanwhile is the
    // past when it opens again: summed up, with the agent's words kept.
    toggle(&h, cx);
    tick_until(&h, cx, "the loop to end", |h, cx| {
        h.shell(cx, |shell| shell.den.watch_is_idle())
    });
    let before = told(&h, cx).len();
    for n in 4..9 {
        line(
            &mut file,
            &format!(
                r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"t{n}","name":"Read","input":{{"file_path":"/work/f{n}.rs"}}}}]}}}}"#
            ),
        );
        line(
            &mut file,
            &format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t{n}","content":"ok"}}]}}}}"#
            ),
        );
    }
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"All read."}]}}"#,
    );
    toggle(&h, cx);
    tick_until(&h, cx, "the gap to be read", |h, cx| {
        told(h, cx)
            .last()
            .is_some_and(|row| row == "said: All read.")
    });
    let after = told(&h, cx);
    assert_eq!(
        after.len(),
        before + 2,
        "one line for the tools, one message: {after:?}"
    );
    assert!(after[before].ends_with("used 5 tools."), "{after:?}");

    // The session ends: its follower is dropped.
    h.press("escape", cx);
    tick_until(&h, cx, "the loop to end", |h, cx| {
        h.shell(cx, |shell| shell.den.watch_is_idle())
    });
}

/// What listing the processes of this computer answers: a script, so that
/// no `ps` runs. The test changes it between scans.
struct Listing(std::sync::Mutex<String>);

impl leon_remote::Runner for Listing {
    async fn run(
        &self,
        _: &leon_remote::CommandSpec,
    ) -> Result<leon_remote::Output, leon_remote::RunError> {
        Ok(leon_remote::Output::ok(self.0.lock().unwrap().clone()))
    }
}

impl Listing {
    fn show(&self, lines: &[String]) {
        *self.0.lock().unwrap() = format!("now=1000000\n{}\n", lines.join("\n"));
    }
}

/// A claude in a terminal of iTerm, in `cwd`, that Claude's own state file
/// ties to `session`.
fn claude_in_iterm(pid: u32, session: &str, cwd: &str) -> Vec<String> {
    vec![
        "T 2165 1 /Applications/iTerm.app/Contents/MacOS/iTerm2".to_owned(),
        format!("T {} 2165 -zsh", pid + 1),
        format!("T {pid} {} claude", pid + 1),
        format!("A {pid} {} ttys000 01:00 claude", pid + 1),
        format!("C {pid} {cwd}"),
        format!("S {{\"pid\":{pid},\"sessionId\":\"{session}\"}}"),
    ]
}

fn scan(h: &Harness, cx: &mut TestAppContext) {
    h.engine
        .submit(crate::engine::Op::Scan(leon_core::MachineId::local()));
    h.settle(cx);
}

#[gpui_kit::test]
fn a_session_that_runs_elsewhere_is_a_lion_told_by_its_transcript(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let listing = std::sync::Arc::new(Listing(std::sync::Mutex::new("now=1000000\n".into())));
    h.engine.set_process_scanner(listing.clone(), Some(100));
    // A session of the history, "alpha", whose own id is "alpha-3".
    let (_dir, path, stored) =
        super::live::local_session(&h, cx, leon_core::AgentId::CLAUDE, "alpha");
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let folder = projects.join(leon_history::live::claude_project_dir_name(&path));
    std::fs::create_dir_all(&folder).unwrap();
    let mut file = std::fs::File::create(folder.join("alpha-3.jsonl")).unwrap();
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":"fix the build"}}"#,
    );
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"On it."}]}}"#,
    );
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{"file_path":"/work/shell.rs","old_string":"a","new_string":"b"}}]}}"#,
    );
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(projects);
    h.engine.set_prefs(prefs);

    // Nothing runs elsewhere: the Den is empty and reads nothing.
    toggle(&h, cx);
    assert!(lions(&h, cx).is_empty());
    assert!(h.shell(cx, |shell| shell.den.watch_is_idle()));

    // It starts in a terminal of iTerm. The next look at the processes
    // finds it, and the Den follows its transcript with no session of this
    // window alive.
    listing.show(&claude_in_iterm(70645, "alpha-3", &path));
    scan(&h, cx);
    tick_until(&h, cx, "the lion of the session elsewhere", |h, cx| {
        lions(h, cx).len() == 1
    });
    let (name, state, level) = lions(&h, cx).remove(0);
    assert_eq!(
        (name.as_str(), level),
        ("alpha", 1),
        "named as the sidebar names it"
    );
    assert_eq!(
        state,
        CubState::Editing,
        "no terminal: no permission prompt is claimed"
    );
    let (id, detail, mystery) = h.shell(cx, |shell| {
        let cub = &shell.den.cubs[0];
        (cub.id, cub.detail.clone().unwrap(), cub.mystery)
    });
    assert!(super::super::den::is_away(id) && !mystery);
    assert!(detail.starts_with("Edit shell.rs"), "{detail}");
    assert!(
        detail.ends_with("runs in iTerm, outside Leon (pid 70645)"),
        "{detail}"
    );
    // Its past is in the feed: what it said, and its tool as one line.
    let past = told(&h, cx);
    assert!(past.contains(&"said: On it.".to_owned()), "{past:?}");
    assert!(past.contains(&"ALPHA used a tool.".to_owned()), "{past:?}");
    // What it does next is told as it happens.
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
    );
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Fixed."}]}}"#,
    );
    tick_until(&h, cx, "its turn to end", |h, cx| {
        lions(h, cx)
            .first()
            .is_some_and(|(_, state, _)| *state == CubState::WaitingForUser)
    });
    assert!(told(&h, cx).contains(&"said: Fixed.".to_owned()));

    // The same session twice in the listing is one lion; a process that
    // names no session (a guess by folder at best) is none.
    let mut both = claude_in_iterm(70645, "alpha-3", &path);
    both.extend([
        "T 300 2165 -zsh".to_owned(),
        "T 301 300 claude".to_owned(),
        "A 301 300 ttys001 00:10 claude --resume alpha-3".to_owned(),
        format!("C 301 {path}"),
        "T 400 2165 -zsh".to_owned(),
        "T 401 400 codex".to_owned(),
        "A 401 400 ttys002 00:10 codex".to_owned(),
        format!("C 401 {path}"),
    ]);
    listing.show(&both);
    scan(&h, cx);
    tick_until(&h, cx, "a poll", |h, cx| {
        cx.executor().advance_clock(Duration::from_millis(20));
        lions(h, cx).len() == 1
    });
    assert_eq!(cx.update(|cx| h.shell.read(cx).den_away(cx).len()), 1);

    // Enter opens what Leon has of it: its stored transcript, with the
    // notice of who holds it. Nothing is started and nothing is ended.
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "session:alpha");
    let held = h.shell(cx, |shell| match &shell.main {
        Main::Session(transcript) => transcript
            .notice
            .as_ref()
            .is_some_and(|notice| notice.elsewhere.is_some()),
        _ => false,
    });
    assert!(held, "the transcript says that it runs elsewhere");
    assert!(h.shell(cx, |shell| shell.live.all().is_empty()));
    let _ = stored;

    // The setting turns these lions off.
    toggle(&h, cx);
    assert_eq!(lions(&h, cx).len(), 1);
    cx.update(|cx| {
        crate::settings::set_value(
            cx,
            schema::find("den_elsewhere").unwrap(),
            Value::Bool(false),
        )
    });
    tick_until(&h, cx, "the lion to leave", |h, cx| lions(h, cx).is_empty());
    tick_until(&h, cx, "the loop to end", |h, cx| {
        h.shell(cx, |shell| shell.den.watch_is_idle())
    });
    cx.update(|cx| {
        crate::settings::set_value(
            cx,
            schema::find("den_elsewhere").unwrap(),
            Value::Bool(true),
        )
    });
    scan(&h, cx);
    tick_until(&h, cx, "the lion again", |h, cx| lions(h, cx).len() == 1);

    // Its process ends: it goes home. The exit code is not known, so it
    // does not faint.
    listing.show(&[]);
    scan(&h, cx);
    tick_until(&h, cx, "the lion to go home", |h, cx| {
        lions(h, cx).is_empty()
    });
    assert!(
        told(&h, cx).contains(&"ALPHA went home.".to_owned()),
        "{:?}",
        told(&h, cx)
    );
}

#[gpui_kit::test]
fn going_home_closes_no_terminal_and_the_home_counts_them_and_opens_the_one_that_waits(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    wait_until(&h, cx, "the second shell", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(2)).is_some())
    });
    assert_eq!(h.main_kind(cx), "live:2");
    let before = screen(&h, cx, 1);

    // Home, by its chord: both terminals are alive and in the sidebar.
    h.press_chord("cmd-alt-shift-h", "ctrl-alt-shift-h", cx);
    assert_eq!(h.main_kind(cx), "empty");
    assert_eq!(h.shell(cx, |shell| shell.live.all().len()), 2);
    assert_eq!(screen(&h, cx, 1), before, "its terminal is as it was");
    let heading = cx.update(|cx| h.shell.read(cx).main_heading(cx).2);
    assert!(heading.starts_with("2 sessions here"), "{heading}");
    assert!(!h.shows("home-waiting-0", cx), "nobody waits");
    assert!(!h.shows("sidebar-den-waits", cx));
    // The Den's row of the sidebar counts them, as a project's row counts
    // its sessions.
    assert!(h.shows("sidebar-den-count", cx));
    assert_eq!(cx.update(|cx| h.shell.read(cx).den_glance(cx)), (2, false));

    // One of them wants the user: the home lists it, and a click opens it.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.live.get_mut(LiveId(1)).unwrap().activity =
                crate::ui::activity::Activity::Waiting;
            cx.notify();
        })
    });
    let heading = cx.update(|cx| h.shell.read(cx).main_heading(cx).2);
    assert!(heading.contains("1 waiting for you"), "{heading}");
    assert!(h.shows("home-waiting-0", cx));
    assert!(!h.shows("home-waiting-1", cx));
    assert!(
        h.shows("sidebar-den-waits", cx),
        "and its row says that one waits"
    );
    assert_eq!(cx.update(|cx| h.shell.read(cx).den_glance(cx)), (2, true));
    h.mouse_on("home-waiting-0".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        h.main_kind(cx),
        "live:1",
        "the one that waits, not the other"
    );
    assert_eq!(h.shell(cx, |shell| shell.live.all().len()), 2);

    // And from the keyboard: the first stop after the actions.
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.live.get_mut(LiveId(2)).unwrap().activity =
                crate::ui::activity::Activity::Waiting;
            shell.live.get_mut(LiveId(1)).unwrap().activity = crate::ui::activity::Activity::Idle;
        })
    });
    h.press_chord("cmd-alt-shift-h", "ctrl-alt-shift-h", cx);
    for _ in 0..crate::ui::home::ACTIONS.len() {
        h.press("down", cx);
    }
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "live:2");
    // The Home row of the sidebar goes home from a terminal too.
    h.mouse_on("sidebar-home".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.main_kind(cx), "empty");
    assert_eq!(h.shell(cx, |shell| shell.live.all().len()), 2);
}

#[gpui_kit::test]
fn a_session_that_runs_elsewhere_is_listed_on_the_home_and_opens_as_from_the_sidebar(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let listing = std::sync::Arc::new(Listing(std::sync::Mutex::new("now=1000000\n".into())));
    h.engine.set_process_scanner(listing.clone(), Some(100));
    let (_dir, path, stored) =
        super::live::local_session(&h, cx, leon_core::AgentId::CLAUDE, "alpha");
    h.press_chord("cmd-alt-shift-h", "ctrl-alt-shift-h", cx);
    assert!(!h.shows("home-elsewhere-0", cx));
    // One the history knows, and one it does not.
    let mut lines = claude_in_iterm(70645, "alpha-3", &path);
    lines.extend([
        "T 300 2165 -zsh".to_owned(),
        "T 301 300 claude".to_owned(),
        "A 301 300 ttys001 00:10 claude --resume 0a1b2c3d-1111-4222-8333-444455556666".to_owned(),
        format!("C 301 {path}"),
    ]);
    listing.show(&lines);
    scan(&h, cx);
    let heading = cx.update(|cx| h.shell.read(cx).main_heading(cx).2);
    assert_eq!(heading, "2 running elsewhere");
    assert!(h.shows("home-elsewhere-0", cx) && h.shows("home-elsewhere-1", cx));

    // What its row of the sidebar does on Enter.
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&NodeId::Session(stored.clone()));
            shell.pane = Pane::Sidebar;
        })
    });
    h.press("enter", cx);
    h.settle(cx);
    let from_sidebar = (h.shell(cx, |shell| shell.overlay), h.main_kind(cx));
    h.press("escape", cx);

    h.press_chord("cmd-alt-shift-h", "ctrl-alt-shift-h", cx);
    h.mouse_on(
        "home-elsewhere-0".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    assert_eq!(
        (h.shell(cx, |shell| shell.overlay), h.main_kind(cx)),
        from_sidebar
    );
    assert!(
        h.shell(cx, |shell| shell.live.all().is_empty()),
        "nothing was started"
    );
    // The one the history does not know has nothing to open: a click on it
    // changes nothing.
    h.press("escape", cx);
    h.press_chord("cmd-alt-shift-h", "ctrl-alt-shift-h", cx);
    h.mouse_on(
        "home-elsewhere-1".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(h.main_kind(cx), "empty");
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
}
