//! Tests of the Den in the window: how it opens and closes, which keys it
//! takes, that every live agent is a lion and a plain shell is none, and that a session whose
//! transcript is on this computer shows what the transcript says. The
//! scripted terminals of `tests.rs` stand in for real ones; the transcript
//! is a file the test writes.

use super::live::{open_live, real_worktree, screen, wait_until};
use super::*;
use crate::schema::{self, Value};
use crate::ui::live::LiveId;
use leon_den::CubState;
use leon_term::Script;
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

/// Makes a live session an agent's, as if it had been started as one where
/// what runs in front of its shell cannot be told: the scripted shell stands
/// in for the agent. Only an agent has a lion.
fn agent_in(h: &Harness, cx: &mut TestAppContext, id: u64) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let session = shell.live.get_mut(LiveId(id)).expect("a live session");
            session.agent = Some(leon_core::AgentId::CLAUDE);
            session.phase = crate::ui::live::AgentPhase::Launched;
            session.can_detect = false;
            cx.notify();
        })
    });
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
fn a_click_in_the_den_gives_it_the_keyboard_back_from_the_sidebar(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    toggle(&h, cx);
    assert_eq!(h.main_kind(cx), "den");
    // A click in the tree leaves the keyboard with the sidebar.
    cx.update(|cx| h.shell.update(cx, |shell, _| shell.pane = Pane::Sidebar));
    h.mouse_on("den".to_owned(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.shell(cx, |shell| shell.pane), Pane::Main);
    h.press("a", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
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
fn the_den_is_shown_at_the_start_when_asked_and_a_development_run_fills_it(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    // What `--den` does: the Den is open, and asking again leaves it open.
    for _ in 0..2 {
        h.window
            .update(cx, |_, window, cx| {
                h.shell.update(cx, |shell, cx| shell.show_den(window, cx));
            })
            .unwrap();
        assert!(h.shell(cx, |shell| shell.den_open()));
    }
    assert!(lions(&h, cx).is_empty(), "nobody is made up unless asked");
    // What `LEON_DEN_CAST=5` does in a development build: five lions that
    // are nobody's session, and that an open of theirs does nothing with.
    h.window
        .update(cx, |_, _, cx| {
            h.shell.update(cx, |shell, cx| {
                shell.options.den_cast = 5;
                shell.den_refresh(false, cx);
            });
        })
        .unwrap();
    assert_eq!(lions(&h, cx).len(), 5);
    let heading = h.shell(cx, |shell| shell.den_heading().2);
    assert!(heading.starts_with("5 LIVE"), "{heading}");
    h.press("tab", cx);
    h.press("enter", cx);
    assert!(h.shell(cx, |shell| shell.den_open()), "nothing to open");
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
fn a_plain_shell_is_no_lion_and_becomes_one_when_an_agent_is_in_it(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    wait_until(&h, cx, "the second shell", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(2)).is_some())
    });
    toggle(&h, cx);
    // Two terminals, no agent: nobody is in the den, and its row of the
    // sidebar counts nobody.
    assert!(lions(&h, cx).is_empty(), "{:?}", lions(&h, cx));
    assert_eq!(cx.update(|cx| h.shell.read(cx).den_glance(cx)), (0, false));
    assert!(!h.shows("sidebar-den-count", cx));
    // A shell that is quiet or that rang its bell is nobody waiting.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.live.get_mut(LiveId(1)).unwrap().activity =
                crate::ui::activity::Activity::Waiting;
            cx.notify();
        })
    });
    assert_eq!(cx.update(|cx| h.shell.read(cx).den_glance(cx)), (0, false));

    // An agent in one of them: that one is a lion, the other still is not.
    agent_in(&h, cx, 2);
    tick_until(&h, cx, "the agent's lion", |h, cx| lions(h, cx).len() == 1);
    assert_eq!(cx.update(|cx| h.shell.read(cx).den_glance(cx)).0, 1);
    let only = h.shell(cx, |shell| shell.den.cubs[0].id);
    assert_eq!(only, 2);

    // The agent is quit and its terminal is back at the prompt: it goes
    // home, and the terminal stays.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.live.get_mut(LiveId(2)).unwrap().phase = crate::ui::live::AgentPhase::Returned;
            cx.notify();
        })
    });
    tick_until(&h, cx, "the lion to go home", |h, cx| {
        lions(h, cx).is_empty()
    });
    assert_eq!(h.shell(cx, |shell| shell.live.all().len()), 2);
}

#[gpui_kit::test]
fn every_live_agent_is_a_lion_and_enter_opens_its_terminal(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    wait_until(&h, cx, "the second shell", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(2)).is_some())
    });
    agent_in(&h, cx, 1);
    agent_in(&h, cx, 2);
    toggle(&h, cx);
    let names: Vec<String> = lions(&h, cx).into_iter().map(|(name, _, _)| name).collect();
    assert_eq!(names.len(), 2, "{names:?}");

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
    agent_in(&h, cx, 1);
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
    agent_in(&h, cx, 1);
    agent_in(&h, cx, 2);
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

// ----- what is done to a lion ---------------------------------------------------------

fn menu_labels(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.shell(cx, |shell| {
        shell
            .menu
            .as_ref()
            .map(|menu| menu.items.iter().map(|item| item.label.clone()).collect())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn the_selected_lion_is_what_the_commands_of_a_session_act_on(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    // A second pane of the same session: a lion is the session, not a pane.
    h.press_chord("cmd-d", "ctrl-shift-d", cx);
    wait_until(&h, cx, "the second pane", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(2)).is_some())
    });
    agent_in(&h, cx, 1);
    toggle(&h, cx);
    assert_eq!(lions(&h, cx).len(), 1);
    // Nobody is selected: the commands are not the Den's, and the bar
    // offers nothing for a lion.
    assert_eq!(h.shell(cx, |shell| shell.den_lion()), None);
    assert!(!h.shows("den-lion-message", cx));

    h.press("down", cx);
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(1)));
    assert_eq!(h.shell(cx, |shell| shell.here_live()), Some(LiveId(1)));
    for button in [
        "den-lion-message",
        "den-lion-rename",
        "den-lion-home",
        "den-lion-menu",
    ] {
        assert!(h.shows(button, cx), "{button}");
    }

    // Its menu, by the key. The session is not in the history: no pin.
    h.press("m", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Menu);
    assert_eq!(
        menu_labels(&h, cx),
        [
            "Open",
            "Message\u{2026}",
            "Interrupt",
            "Rename\u{2026}",
            "Send home",
            "Close"
        ]
    );
    // Rename, from the menu: the lion's session, not the tree's cursor.
    h.type_text("ren", cx);
    h.press("enter", cx);
    h.type_text("build", cx);
    h.press("enter", cx);
    assert_eq!(
        h.shell(cx, |shell| shell.live.get(LiveId(1)).unwrap().label()),
        "build"
    );
    assert!(h.shell(cx, |shell| shell.den_open()), "the Den stays");
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(1)));

    // Send home always asks, and says what it does.
    h.press("m", cx);
    h.type_text("send", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    assert_eq!(
        h.palette_titles(cx),
        ["Send BUILD home", "Cancel"].map(str::to_owned)
    );
    h.press("down", cx);
    h.press("enter", cx); // Cancel
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    assert_eq!(h.shell(cx, |shell| shell.live.ids().len()), 2, "kept");
    h.press("m", cx);
    h.type_text("send", cx);
    h.press("enter", cx);
    h.press("enter", cx);
    // The whole session went, both panes, and it sleeps in the sidebar.
    assert!(h.shell(cx, |shell| shell.live.ids().is_empty()));
    assert_eq!(h.shell(cx, |shell| shell.dormant.all().len()), 1);
    assert!(h.status().contains("Sent BUILD home"), "{}", h.status());
    assert!(h.shell(cx, |shell| shell.den_open()));
    assert!(lions(&h, cx).is_empty());
    // It is told as going home, not as a faint.
    let told = told(&h, cx);
    assert!(
        told.iter().any(|row| row.contains("BUILD went home")),
        "{told:?}"
    );
    assert!(told.iter().all(|row| !row.contains("fainted")), "{told:?}");
}

#[gpui_kit::test]
fn a_right_click_on_a_lion_opens_its_menu_and_open_shows_its_terminal(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    agent_in(&h, cx, 1);
    toggle(&h, cx);
    assert_eq!(lions(&h, cx).len(), 1);
    // The view asks for the menu as the right button does.
    let view = h.shell(cx, |shell| shell.den.view.clone().unwrap());
    cx.update(|cx| {
        view.update(cx, |den, cx| {
            den.select(Some(1), cx);
            assert!(den.menu_selection(cx));
        })
    });
    h.settle(cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Menu);
    assert_eq!(
        h.shell(cx, |shell| shell.menu.as_ref().unwrap().lion),
        Some(1)
    );
    h.press("enter", cx); // Open
    assert_eq!(h.main_kind(cx), "live:1");
}

#[gpui_kit::test]
fn a_message_is_typed_at_the_prompt_waits_while_the_agent_works_and_never_answers_a_question(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dir, path) = real_worktree(&h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code
    wait_until(&h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let session = "0a1b2c3d-0000-4000-8000-00000000000b";
    let transcript = projects
        .join(leon_history::live::claude_project_dir_name(&path))
        .join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(&transcript).unwrap();
    let prompt = r#"{"type":"user","message":{"role":"user","content":"fix the build"}}"#;
    let done = r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}}"#;
    line(&mut file, prompt);
    line(&mut file, done);
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(projects);
    h.engine.set_prefs(prefs);
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let live = shell.live.get_mut(LiveId(1)).unwrap();
            live.learned = Some((session.to_owned(), "state-file".to_owned()));
        })
    });
    // A quiet terminal with a program in front waits, at once.
    super::live::set_waiting_after(&h, cx, Duration::ZERO);
    let script = super::live::script_of(&h, 1);
    let typed = |script: &Script| String::from_utf8_lossy(&script.written()).into_owned();

    toggle(&h, cx);
    tick_until(&h, cx, "the transcript to be read", |h, cx| {
        lions(h, cx)
            .first()
            .is_some_and(|(_, state, _)| *state == CubState::WaitingForUser)
    });
    h.press("down", cx);
    let notes = |h: &Harness, cx: &mut TestAppContext| -> Vec<String> {
        cx.update(|cx| {
            let view = h.shell.read(cx).den.view.clone().unwrap();
            let notes = view
                .read(cx)
                .read(|den| den.truth(1).map(|card| card.notes).unwrap_or_default());
            notes.into_iter().map(|note| note.text).collect()
        })
    };
    // The summary of the session is on its card.
    let card = notes(&h, cx);
    assert!(
        card.iter().any(|row| row.starts_with("Running for ")),
        "{card:?}"
    );
    assert!(card.contains(&"Last said: Done.".to_owned()), "{card:?}");
    // What the user wrote is there too, and the agent that runs.
    assert!(
        card.contains(&"You said: fix the build".to_owned()),
        "{card:?}"
    );
    assert!(card.contains(&"CONVERSATION".to_owned()), "{card:?}");
    assert!(card.contains(&"SESSION".to_owned()), "{card:?}");

    // At its prompt: the message is pasted, and Enter follows.
    h.press("i", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    h.type_text("run the tests", cx);
    h.press("enter", cx);
    assert!(h.status().starts_with("Sent to "), "{}", h.status());
    tick_until(&h, cx, "the message and its Enter", |_, _| {
        typed(&script).ends_with("run the tests\r")
    });

    // The agent took it and works: the next one waits.
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":"run the tests"}}"#,
    );
    tick_until(&h, cx, "the turn to be seen", |h, cx| {
        lions(h, cx)
            .first()
            .is_some_and(|(_, state, _)| *state == CubState::Thinking)
    });
    h.press("i", cx);
    h.type_text("then commit", cx);
    h.press("enter", cx);
    assert!(
        h.status().contains("is busy: not sent yet"),
        "{}",
        h.status()
    );
    assert!(!typed(&script).contains("then commit"));
    tick_until(&h, cx, "the card to count it", |h, cx| {
        notes(h, cx).contains(&"1 message queued for its prompt".to_owned())
    });

    // A call without a result on a quiet terminal is a question on screen:
    // nothing is typed into it, however long it stays.
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
    );
    tick_until(&h, cx, "the permission prompt", |h, cx| {
        lions(h, cx)
            .first()
            .is_some_and(|(_, state, _)| *state == CubState::NeedsPermission)
    });
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(10));
        h.settle(cx);
    }
    assert!(!typed(&script).contains("then commit"));
    // The Den is closed: what waits is still typed when the turn ends.
    toggle(&h, cx);
    assert!(!h.shell(cx, |shell| shell.den_open()));
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
    );
    line(&mut file, done);
    tick_until(&h, cx, "the queued message", |_, _| {
        typed(&script).ends_with("then commit\r")
    });
    assert!(
        h.status().contains("the queued message was typed"),
        "{}",
        h.status()
    );
    // Seen busy again, nothing is left to do and the transcripts rest.
    line(
        &mut file,
        r#"{"type":"user","message":{"role":"user","content":"then commit"}}"#,
    );
    tick_until(&h, cx, "the loop to end", |h, cx| {
        h.shell(cx, |shell| shell.den.watch_is_idle())
    });
}

#[gpui_kit::test]
fn a_session_without_a_transcript_is_not_messaged_and_one_that_ends_loses_what_waited(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let _dir = shell_session(&h, cx);
    agent_in(&h, cx, 1);
    toggle(&h, cx);
    h.press("down", cx);
    // No transcript is followed: a quiet terminal may be a question.
    h.press("i", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    assert!(
        h.status()
            .contains("cannot tell its prompt from a question"),
        "{}",
        h.status()
    );
    // What waited for a session that ended is dropped, and said.
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell
                .den
                .post
                .post(1, "later", crate::ui::den_post::Ready::Later, 0);
            shell.den_watch(cx);
        })
    });
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            shell.live.get_mut(LiveId(1)).unwrap().phase = crate::ui::live::AgentPhase::Returned;
            cx.notify();
        })
    });
    tick_until(&h, cx, "the line to be dropped", |h, cx| {
        h.shell(cx, |shell| !shell.den.post.busy())
    });
    assert!(
        h.status().contains("1 message queued for it was dropped"),
        "{}",
        h.status()
    );
}

/// A Claude Code session whose transcript the Den follows: the folders to
/// keep, the transcript to write lines to, and the scripted terminal.
fn followed_agent(
    h: &Harness,
    cx: &mut TestAppContext,
) -> (
    (tempfile::TempDir, tempfile::TempDir),
    std::fs::File,
    Script,
) {
    let (dir, path) = real_worktree(h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code
    wait_until(h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let session = "0a1b2c3d-0000-4000-8000-00000000000c";
    let transcript = projects
        .join(leon_history::live::claude_project_dir_name(&path))
        .join(format!("{session}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&transcript).unwrap();
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(projects);
    h.engine.set_prefs(prefs);
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let live = shell.live.get_mut(LiveId(1)).unwrap();
            live.learned = Some((session.to_owned(), "state-file".to_owned()));
        })
    });
    super::live::set_waiting_after(h, cx, Duration::ZERO);
    let script = super::live::script_of(h, 1);
    ((dir, home), file, script)
}

const PROMPT: &str = r#"{"type":"user","message":{"role":"user","content":"fix the build"}}"#;
const DONE: &str = r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}}"#;

fn state_is(h: &Harness, cx: &mut TestAppContext, wanted: CubState) -> bool {
    lions(h, cx)
        .first()
        .is_some_and(|(_, state, _)| *state == wanted)
}

fn card_of(h: &Harness, cx: &mut TestAppContext, id: u64) -> Vec<(String, leon_den::NoteTone)> {
    cx.update(|cx| {
        let view = h.shell.read(cx).den.view.clone().unwrap();
        let notes = view
            .read(cx)
            .read(|den| den.truth(id).map(|card| card.notes).unwrap_or_default());
        notes
            .into_iter()
            .map(|note| (note.text, note.tone))
            .collect()
    })
}

#[gpui_kit::test]
fn the_lion_that_needs_the_user_is_one_key_away_and_its_card_says_what_it_asks(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dirs, mut file, script) = followed_agent(&h, cx);
    // What was typed since the agent was started.
    let launched = script.written().len();
    let typed =
        |script: &Script| String::from_utf8_lossy(&script.written()[launched..]).into_owned();
    line(&mut file, PROMPT);
    toggle(&h, cx);
    tick_until(&h, cx, "the turn to be seen", |h, cx| {
        state_is(h, cx, CubState::Thinking)
    });
    // It works: nobody needs the user, and the key says so.
    assert!(!h.shows("den-needy", cx));
    h.press("n", cx);
    assert!(
        h.status().starts_with("No lion needs you"),
        "{}",
        h.status()
    );
    assert_eq!(h.shell(cx, |shell| shell.den_lion()), None);

    // A call without a result on a quiet terminal: it asks for a permission.
    line(
        &mut file,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"rm -rf target && cargo build --release --locked --workspace --all-targets"}}]}}"#,
    );
    tick_until(&h, cx, "the permission prompt", |h, cx| {
        state_is(h, cx, CubState::NeedsPermission)
    });
    assert!(
        h.shows("den-needy", cx),
        "the bar counts who needs the user"
    );
    h.press("n", cx);
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(1)));
    // Its card says the whole command, where a permission is asked, and
    // that it is answered in its terminal. No answer is offered.
    tick_until(&h, cx, "the card", |h, cx| !card_of(h, cx, 1).is_empty());
    let card = card_of(&h, cx, 1);
    assert_eq!(
        card[0],
        (
            "Wants to run: rm -rf target && cargo build --release --locked --workspace --all-targets"
                .to_owned(),
            leon_den::NoteTone::Urgent
        ),
        "{card:?}"
    );
    assert_eq!(card[1].0, "Open it to allow or refuse it in its terminal.");
    assert!(card
        .iter()
        .any(|(text, _)| text == "You said: fix the build"));
    assert_eq!(
        menu_of_lion(&h, cx),
        [
            "Open",
            "Message\u{2026}",
            "Interrupt",
            "Rename\u{2026}",
            "Send home",
            "Close"
        ]
    );
    // Nothing was typed into the question by any of this.
    assert_eq!(typed(&script), "");

    // The keys, over the room: any key takes them away and does nothing else.
    h.press("?", cx);
    let shown = |h: &Harness, cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = h.shell.read(cx).den.view.clone().unwrap();
            let shown = view.read(cx).keys_shown();
            shown
        })
    };
    assert!(shown(&h, cx));
    h.press("escape", cx);
    assert!(!shown(&h, cx));
    assert!(
        h.shell(cx, |shell| shell.den_open()),
        "the Den is still open"
    );
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(1)));

    // Enter goes straight to its terminal: the question is answered there.
    h.press("enter", cx);
    assert!(h.shell(cx, |shell| matches!(shell.main, Main::Live(LiveId(1)))));
}

/// The labels of the selected lion's menu, which is closed again.
fn menu_of_lion(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.press("m", cx);
    let labels = menu_labels(h, cx);
    h.press("escape", cx);
    labels
}

#[gpui_kit::test]
fn an_agent_is_interrupted_only_in_the_middle_of_a_turn_and_queued_messages_are_taken_back(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dirs, mut file, script) = followed_agent(&h, cx);
    // What was typed since the agent was started.
    let launched = script.written().len();
    let typed =
        |script: &Script| String::from_utf8_lossy(&script.written()[launched..]).into_owned();
    line(&mut file, PROMPT);
    line(&mut file, DONE);
    toggle(&h, cx);
    tick_until(&h, cx, "the turn to be over", |h, cx| {
        state_is(h, cx, CubState::WaitingForUser)
    });
    // Nobody is selected: the key says so and nothing is sent anywhere.
    h.press("x", cx);
    assert_eq!(h.status(), "Select a lion in the Den first.");
    h.press("n", cx);
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(1)));
    // At its prompt there is nothing to interrupt: no key is sent.
    h.press("x", cx);
    assert!(
        h.status().contains("is not in the middle of a turn"),
        "{}",
        h.status()
    );
    assert_eq!(typed(&script), "");

    // It works again: two messages wait for it.
    line(&mut file, PROMPT);
    tick_until(&h, cx, "the turn to be seen", |h, cx| {
        state_is(h, cx, CubState::Thinking)
    });
    for text in ["run the tests", "then commit"] {
        h.press("i", cx);
        h.type_text(text, cx);
        h.press("enter", cx);
    }
    assert!(h.status().contains("(2 messages queued)"), "{}", h.status());
    assert_eq!(
        menu_of_lion(&h, cx)[2],
        "Queued messages (2)\u{2026}",
        "the menu counts them"
    );
    // The first is taken back; the other still waits.
    h.press("q", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    h.press("enter", cx);
    assert!(
        h.status().starts_with("Took back a message queued for"),
        "{}",
        h.status()
    );
    assert_eq!(h.shell(cx, |shell| shell.den.post.list(1)), ["then commit"]);
    // Then all that is left.
    h.press("q", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.den.post.waiting(1)), 0);
    h.press("q", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    assert!(
        h.status().starts_with("No message is queued for"),
        "{}",
        h.status()
    );

    // In the middle of its turn, the key is Escape and nothing else.
    h.press("x", cx);
    assert!(h.status().starts_with("Sent Escape to "), "{}", h.status());
    assert_eq!(typed(&script), "\u{1b}");
    // None of the messages that were taken back was typed.
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(10));
        h.settle(cx);
    }
    assert_eq!(typed(&script), "\u{1b}");
}

#[gpui_kit::test]
fn the_pride_is_messaged_after_saying_who_and_a_lion_is_gone_to_by_what_is_typed(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dirs, mut file, script) = followed_agent(&h, cx);
    // What was typed since the agent was started.
    let launched = script.written().len();
    let typed =
        |script: &Script| String::from_utf8_lossy(&script.written()[launched..]).into_owned();
    line(&mut file, PROMPT);
    line(&mut file, DONE);
    // With the Den closed there is no pride to message.
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell.update(cx, |shell, cx| {
            shell.run_command(crate::keys::Command::MessagePride, window, cx);
        })
    })
    .unwrap();
    assert!(
        h.status().starts_with("Open the Den first"),
        "{}",
        h.status()
    );
    toggle(&h, cx);
    tick_until(&h, cx, "the turn to be over", |h, cx| {
        state_is(h, cx, CubState::WaitingForUser)
    });
    // Go to it by its state: nobody was selected.
    h.press("/", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    h.type_text("waiting", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(1)));

    // One message to those that wait: who, what, then a confirmation.
    h.press("p", cx);
    h.press("enter", cx); // Those that wait at their prompt (1)
    h.type_text("pull and rebuild", cx);
    h.press("enter", cx);
    assert_eq!(
        typed(&script),
        "",
        "nothing is typed before the confirmation"
    );
    h.press("enter", cx); // Send to 1 lion
    assert!(h.status().starts_with("Sent to "), "{}", h.status());
    tick_until(&h, cx, "the message and its Enter", |_, _| {
        typed(&script).ends_with("pull and rebuild\r")
    });
}

#[gpui_kit::test]
fn a_lion_that_was_sent_home_is_listed_at_home_and_woken_from_the_den_and_a_lion_is_hatched_there(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let (_dirs, mut file, _script) = followed_agent(&h, cx);
    line(&mut file, PROMPT);
    line(&mut file, DONE);
    toggle(&h, cx);
    tick_until(&h, cx, "the lion", |h, cx| lions(h, cx).len() == 1);
    let at_home = |h: &Harness, cx: &mut TestAppContext| -> Vec<String> {
        cx.update(|cx| {
            let view = h.shell.read(cx).den.view.clone().unwrap();
            let home = view.read(cx).read(|den| den.home().to_vec());
            home.into_iter().map(|entry| entry.name).collect()
        })
    };
    assert!(at_home(&h, cx).is_empty());
    // Nobody is at home yet: waking says so.
    h.press("w", cx);
    assert_eq!(h.status(), "Nobody is at home: no session was sent home.");

    // Hatching asks where and which agent, and stays in the Den with the
    // new lion selected.
    h.press("a", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    h.type_text("real", cx); // the worktree that is really there
    h.press("enter", cx);
    h.press("enter", cx); // Claude Code
    wait_until(&h, cx, "the second session", |h, cx| {
        h.shell(cx, |shell| shell.live.get(LiveId(2)).is_some())
    });
    assert!(h.shell(cx, |shell| shell.den_open()), "still in the Den");
    assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), Some(LiveId(2)));
    tick_until(&h, cx, "two lions", |h, cx| lions(h, cx).len() == 2);

    // Sent home, it is listed under the roster.
    h.press("h", cx);
    h.press("enter", cx);
    assert!(h.status().starts_with("Sent "), "{}", h.status());
    tick_until(&h, cx, "it to be at home", |h, cx| {
        at_home(h, cx).len() == 1
    });
    assert!(h.shell(cx, |shell| shell.live.get(LiveId(2)).is_none()));
    // And woken from there: its agent starts again, and the Den stays.
    h.press("w", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    h.press("enter", cx);
    assert!(h.status().starts_with("Woke "), "{}", h.status());
    wait_until(&h, cx, "the woken session", |h, cx| {
        h.shell(cx, |shell| shell.live.all().len() == 2)
    });
    assert!(h.shell(cx, |shell| shell.den_open()), "back in the Den");
    tick_until(&h, cx, "nobody at home", |h, cx| at_home(h, cx).is_empty());
}

#[gpui_kit::test]
fn a_message_may_have_several_lines_and_is_typed_as_one_prompt(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dirs, mut file, script) = followed_agent(&h, cx);
    let launched = script.written().len();
    let typed =
        |script: &Script| String::from_utf8_lossy(&script.written()[launched..]).into_owned();
    line(&mut file, PROMPT);
    line(&mut file, DONE);
    toggle(&h, cx);
    tick_until(&h, cx, "the turn to be over", |h, cx| {
        state_is(h, cx, CubState::WaitingForUser)
    });
    h.press("n", cx);
    h.press("i", cx);
    // Shift+Enter ends a line and goes on; nothing is sent by it.
    h.type_text("first line", cx);
    h.press("shift-enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    assert_eq!(
        h.shell(cx, |shell| shell.palette.draft.clone()),
        ["first line"]
    );
    assert_eq!(typed(&script), "");
    // The lines so far are shown as they will be typed.
    h.type_text("second", cx);
    let rows = h.palette_titles(cx);
    assert_eq!(rows[..2], ["line:first line", "line:second"], "{rows:?}");
    // Backspace in an empty line takes the one before back.
    h.press("shift-enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.palette.draft.len()), 2);
    h.press("backspace", cx);
    assert_eq!(
        h.shell(cx, |shell| shell.palette.draft.clone()),
        ["first line"]
    );
    // Enter sends all of it as one prompt: one Enter at its end, and no
    // line of it submitted by itself.
    h.press("enter", cx);
    assert!(h.status().starts_with("Sent to "), "{}", h.status());
    tick_until(&h, cx, "the message and its Enter", |_, _| {
        typed(&script).ends_with('\r')
    });
    let sent = typed(&script);
    assert_eq!(sent.matches('\r').count(), 1, "{sent:?}");
    assert!(
        sent == "first line second\r" || sent.contains("first line\nsecond"),
        "{sent:?}"
    );
    // An empty message is no message.
    h.press("i", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::Palette);
    assert_eq!(
        h.shell(cx, |shell| shell.palette.error.clone()).as_deref(),
        Some("Type a message.")
    );
}

fn editing(h: &Harness, cx: &mut TestAppContext) -> bool {
    cx.update(|cx| h.shell.read(cx).den_editing(cx))
}

/// Every key the Den takes for itself that is also something one types.
const DEN_LETTERS: &str = "nixqprhwagme?/0123456789";

#[gpui_kit::test]
fn what_is_typed_into_a_question_never_fires_a_key_of_the_den(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dirs, mut file, script) = followed_agent(&h, cx);
    let launched = script.written().len();
    let typed =
        |script: &Script| String::from_utf8_lossy(&script.written()[launched..]).into_owned();
    // In the middle of a turn: here `x` would interrupt the agent.
    line(&mut file, PROMPT);
    toggle(&h, cx);
    tick_until(&h, cx, "the turn to be seen", |h, cx| {
        state_is(h, cx, CubState::Thinking)
    });
    h.press("tab", cx);
    let chosen = h.shell(cx, |shell| shell.den_lion_live());
    assert_eq!(chosen, Some(LiveId(1)));

    // The message, the pride's message, the new name, the lion to go to and
    // the new session all ask in the palette: while it asks, each of the
    // Den's letters is a letter.
    for start in ["i", "p", "r", "g", "a"] {
        h.press(start, cx);
        assert_eq!(
            h.shell(cx, |shell| shell.overlay),
            Overlay::Palette,
            "{start} asks in the palette"
        );
        let before = h.status();
        for key in DEN_LETTERS.chars() {
            h.press(&key.to_string(), cx);
            assert_eq!(
                h.shell(cx, |shell| shell.overlay),
                Overlay::Palette,
                "{key} after {start}"
            );
        }
        assert_eq!(typed(&script), "", "nothing reaches the agent from {start}");
        assert_eq!(
            h.status(),
            before,
            "no key of the Den answered after {start}"
        );
        assert_eq!(h.shell(cx, |shell| shell.den_lion_live()), chosen);
        assert!(!editing(&h, cx), "e did not open the editor after {start}");
        for _ in 0..6 {
            if h.shell(cx, |shell| shell.overlay) == Overlay::None {
                break;
            }
            h.press("escape", cx);
        }
        assert_eq!(h.shell(cx, |shell| shell.overlay), Overlay::None);
    }

    // And what is typed arrives whole: the message is queued as written.
    h.press("i", cx);
    h.type_text("fix the nix expr, wrap it? a/b 42", cx);
    h.press("enter", cx);
    assert_eq!(
        h.shell(cx, |shell| shell.den.post.list(1)),
        ["fix the nix expr, wrap it? a/b 42"]
    );
    assert_eq!(typed(&script), "", "queued, and no Escape was sent");

    // While a room is edited the lion keys do nothing either: the editor
    // has its own, and none of them reaches an agent.
    h.press("e", cx);
    assert!(editing(&h, cx));
    for key in DEN_LETTERS.chars().filter(|key| *key != 'e') {
        h.press(&key.to_string(), cx);
    }
    assert_eq!(typed(&script), "");
    assert_eq!(h.shell(cx, |shell| shell.den.post.waiting(1)), 1);
}

#[gpui_kit::test]
fn a_message_that_was_never_seen_to_start_a_turn_holds_the_ones_behind_it(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let (_dirs, mut file, script) = followed_agent(&h, cx);
    let launched = script.written().len();
    let typed =
        |script: &Script| String::from_utf8_lossy(&script.written()[launched..]).into_owned();
    line(&mut file, PROMPT);
    line(&mut file, DONE);
    toggle(&h, cx);
    tick_until(&h, cx, "the turn to be over", |h, cx| {
        state_is(h, cx, CubState::WaitingForUser)
    });
    h.press("n", cx);
    for text in ["first", "second"] {
        h.press("i", cx);
        h.type_text(text, cx);
        h.press("enter", cx);
    }
    tick_until(&h, cx, "the first message and its Enter", |_, _| {
        typed(&script).ends_with('\r')
    });
    // The transcript never shows the prompt and the agent is never busy:
    // the Enter may have been lost. The second is not typed after the
    // first, and the status line says why.
    tick_until(&h, cx, "the line to be held", |h, _| {
        h.status().contains("did not start on the message")
    });
    assert!(
        h.status().contains("1 message queued is held"),
        "{}",
        h.status()
    );
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(10));
        h.settle(cx);
    }
    let sent = typed(&script);
    assert!(
        sent.contains("first") && !sent.contains("second"),
        "{sent:?}"
    );
    assert_eq!(sent.matches('\r').count(), 1, "{sent:?}");
    assert_eq!(h.shell(cx, |shell| shell.den.post.list(1)), ["second"]);
    // A third does not go over it either, and says so.
    h.press("i", cx);
    h.type_text("third", cx);
    h.press("enter", cx);
    assert!(
        h.status().contains("may still be in its input"),
        "{}",
        h.status()
    );
    assert!(!typed(&script).contains("third"));
    // The user sends it in the terminal: the turn runs, and the line moves.
    line(&mut file, PROMPT);
    line(&mut file, DONE);
    tick_until(&h, cx, "the second message", |_, _| {
        typed(&script).contains("second")
    });
    assert!(!typed(&script).contains("third"));
}
