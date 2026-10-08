//! Tests of the sessions' lights read from the agents' transcripts: that an
//! agent which finished its turn, one that asks and one that thinks silently
//! read differently, that it is said in words that tell them apart, and that
//! the Den does not have to be open. The scripted terminals of `tests.rs`
//! stand in for real ones; the transcript is a file the test writes.

use super::live::{
    open_live, real_worktree, screen, script_of, set_waiting_after, show_worktree_detail,
    wait_until, HOUR,
};
use super::*;
use crate::ui::activity::{Activity, Transcript};
use crate::ui::live::LiveId;
use crate::ui::notify::{Kind, Note};
use std::io::Write;
use std::time::Duration;

const SESSION: &str = "0a1b2c3d-0000-4000-8000-0000000000b1";

const PROMPT: &str = r#"{"type":"user","message":{"role":"user","content":"fix the build"}}"#;
const THINKING: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"}]}}"#;
const ENDED: &str = r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Done."}]}}"#;
const BASH: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
const ASK: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"AskUserQuestion","input":{}}]}}"#;
const RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;

/// A window the shell treats as the one in front: test windows start without
/// the focus.
fn in_front(h: &Harness, cx: &mut TestAppContext) {
    VisualTestContext::from_window(h.window.into(), cx)
        .update(|window, _| window.activate_window());
    h.settle(cx);
}

/// Waits as [`wait_until`] does, with the clock of the window moving on: the
/// transcripts are read on a timer.
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

fn line(file: &mut std::fs::File, json: &str) {
    writeln!(file, "{json}").unwrap();
    file.flush().unwrap();
}

fn activity(h: &Harness, cx: &mut TestAppContext) -> Activity {
    h.shell(cx, |shell| shell.live.get(LiveId(1)).unwrap().activity)
}

/// What the dot of the worktree row of the tree shows.
fn dot(h: &Harness, cx: &mut TestAppContext) -> Activity {
    h.shell(cx, |shell| {
        let row = shell
            .rows
            .iter()
            .find(|row| {
                matches!(&row.kind, tree::Kind::Worktree { worktree, .. }
                    if tree::worktree_label(worktree) == "trunk"
                        && shell
                            .snapshot
                            .project(&worktree.project_id)
                            .is_some_and(|entry| entry.project.name == "real"))
            })
            .expect("the worktree is in the tree");
        shell.row_activity(row).expect("it has a dot")
    })
}

fn banners(h: &Harness, cx: &mut TestAppContext) -> Vec<Note> {
    h.shell(cx, |shell| {
        shell
            .banners
            .iter()
            .map(|banner| banner.note.clone())
            .collect()
    })
}

/// A Claude Code session on screen in a real worktree, shown out of sight,
/// and its transcript: a file of the test's, with the session's own id
/// learned the way the agent's state file tells it.
struct Fixture {
    _dir: tempfile::TempDir,
    _home: tempfile::TempDir,
    transcript: std::path::PathBuf,
    file: std::fs::File,
}

fn agent(h: &Harness, cx: &mut TestAppContext, how: &str) -> Fixture {
    let (dir, path) = real_worktree(h, cx);
    in_front(h, cx);
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude Code: prints, and takes the terminal
    wait_until(h, cx, "the agent's first line", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE")
    });
    show_worktree_detail(h, cx, "real", "trunk");

    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("projects");
    let transcript = projects
        .join(leon_history::live::claude_project_dir_name(&path))
        .join(format!("{SESSION}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&transcript).unwrap();
    let mut prefs = h.engine.prefs();
    prefs.roots.claude_projects = Some(projects);
    h.engine.set_prefs(prefs);
    let how = how.to_owned();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            let live = shell.live.get_mut(LiveId(1)).unwrap();
            live.learned = Some((SESSION.to_owned(), how));
        })
    });
    Fixture {
        _dir: dir,
        _home: home,
        transcript,
        file,
    }
}

/// Lets the coarse timer of the window pass: it is what starts the reading
/// of a transcript when nothing else did.
fn let_the_timer_pass(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_secs(3));
}

#[gpui_kit::test]
fn an_agent_that_finished_its_turn_reads_ready_and_is_said_so_with_the_den_closed(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let mut f = agent(&h, cx, "state-file");
    line(&mut f.file, PROMPT);
    line(&mut f.file, ENDED);
    set_waiting_after(&h, cx, HOUR);
    let_the_timer_pass(cx);
    tick_until(&h, cx, "the transcript to be read", |h, cx| {
        activity(h, cx) == Activity::TurnOver
    });
    assert!(
        h.shell(cx, |shell| shell.den.watch_is_idle()),
        "the Den was never open"
    );
    assert_eq!(
        h.shell(cx, |shell| shell.live.get(LiveId(1)).unwrap().transcript),
        Some(Transcript::TurnOver)
    );

    // It is the user's move, and the banner says so in those words.
    let said = banners(&h, cx);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].kind, Kind::TurnOver);
    assert!(
        said[0].body.contains("finished its turn"),
        "{}",
        said[0].body
    );
    assert!(!said[0].body.contains("needs an answer"));
    // The worktree's dot and the home count say it too.
    let counts = h.shell(cx, |shell| shell.home_model().pride);
    assert_eq!((counts.finished, counts.asking, counts.waiting), (1, 0, 0));
    assert_eq!(dot(&h, cx), Activity::TurnOver);
}

#[gpui_kit::test]
fn a_silent_thinking_agent_works_and_a_call_without_a_result_asks_once_the_terminal_is_quiet(
    cx: &mut TestAppContext,
) {
    let h = open_live(cx);
    let mut f = agent(&h, cx, "state-file");
    set_waiting_after(&h, cx, HOUR);
    let_the_timer_pass(cx);
    line(&mut f.file, PROMPT);
    line(&mut f.file, THINKING);
    tick_until(&h, cx, "the turn to be seen", |h, cx| {
        h.shell(cx, |shell| {
            shell.live.get(LiveId(1)).unwrap().transcript == Some(Transcript::Turn)
        })
    });
    // The terminal has been quiet for longer than the threshold: the
    // heuristic alone reads waiting, the transcript says it thinks.
    set_waiting_after(&h, cx, Duration::ZERO);
    let heuristic = cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let session = shell.live.get(LiveId(1)).unwrap();
            crate::ui::activity::terminal_activity(&session.signals(cx), &shell.options.activity)
        })
    });
    assert_eq!(heuristic, Activity::Waiting);
    assert_eq!(activity(&h, cx), Activity::Working, "thinking is work");
    assert!(banners(&h, cx).is_empty(), "nobody is told of work");

    // A call with no result while the terminal prints: a tool that runs.
    set_waiting_after(&h, cx, HOUR);
    line(&mut f.file, BASH);
    tick_until(&h, cx, "the call to be seen", |h, cx| {
        h.shell(cx, |shell| {
            shell.live.get(LiveId(1)).unwrap().transcript == Some(Transcript::Call { asks: false })
        })
    });
    assert_eq!(activity(&h, cx), Activity::Working);
    assert!(banners(&h, cx).is_empty());

    // The same call in a terminal gone quiet: inferred to be a permission
    // prompt, and said as a question and not as a finished turn.
    set_waiting_after(&h, cx, Duration::ZERO);
    assert_eq!(activity(&h, cx), Activity::NeedsYou);
    let said = banners(&h, cx);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].kind, Kind::NeedsAnswer);
    assert!(said[0].body.contains("needs an answer"), "{}", said[0].body);
    assert!(said[0].body.contains("probably"), "it is inferred");
    assert!(!said[0].body.contains("finished"));

    // Answered: a result, the model decides, then the turn ends.
    line(&mut f.file, RESULT);
    tick_until(&h, cx, "the result to be seen", |h, cx| {
        activity(h, cx) == Activity::Working
    });
    line(&mut f.file, ENDED);
    tick_until(&h, cx, "the end of the turn", |h, cx| {
        activity(h, cx) == Activity::TurnOver
    });
    let said = banners(&h, cx);
    assert_eq!(said.len(), 1, "the session's banner is refreshed");
    assert_eq!(said[0].kind, Kind::TurnOver);
}

#[gpui_kit::test]
fn a_question_is_a_fact_whatever_the_terminal_does(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let mut f = agent(&h, cx, "state-file");
    let_the_timer_pass(cx);
    line(&mut f.file, PROMPT);
    line(&mut f.file, ASK);
    // The terminal printed a moment ago.
    set_waiting_after(&h, cx, HOUR);
    tick_until(&h, cx, "the question to be seen", |h, cx| {
        activity(h, cx) == Activity::NeedsYou
    });
    assert_eq!(banners(&h, cx)[0].kind, Kind::NeedsAnswer);
}

#[gpui_kit::test]
fn what_the_transcript_does_not_say_is_left_to_the_terminal(cx: &mut TestAppContext) {
    let h = open_live(cx);
    // The id is only a guess by the newest file of the folder: not followed,
    // whatever the file says.
    let mut f = agent(&h, cx, "newest-in-folder");
    line(&mut f.file, PROMPT);
    line(&mut f.file, ENDED);
    let_the_timer_pass(cx);
    set_waiting_after(&h, cx, Duration::ZERO);
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(10));
        h.settle(cx);
    }
    assert_eq!(activity(&h, cx), Activity::Waiting, "the heuristic");
    assert!(h.shell(cx, |shell| shell.status.is_idle()));
    assert_eq!(
        h.shell(cx, |shell| shell.live.get(LiveId(1)).unwrap().transcript),
        None
    );
    let said = banners(&h, cx);
    assert_eq!(said[0].kind, Kind::Waiting);
    assert!(
        !said[0].body.contains("finished"),
        "nothing tells whether it finished: {}",
        said[0].body
    );
}

#[gpui_kit::test]
fn a_followed_transcript_that_is_empty_or_missing_leaves_the_heuristic(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let f = agent(&h, cx, "state-file");
    // Nothing written yet.
    let_the_timer_pass(cx);
    set_waiting_after(&h, cx, Duration::ZERO);
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(10));
        h.settle(cx);
    }
    assert_eq!(activity(&h, cx), Activity::Waiting);
    // The file goes away: the same.
    std::fs::remove_file(&f.transcript).unwrap();
    for _ in 0..20 {
        cx.executor().advance_clock(Duration::from_millis(10));
        h.settle(cx);
    }
    assert_eq!(activity(&h, cx), Activity::Waiting);
    assert_eq!(
        h.shell(cx, |shell| shell.live.get(LiveId(1)).unwrap().transcript),
        None
    );
}

#[gpui_kit::test]
fn an_agent_that_returned_to_the_shell_is_idle_and_the_reading_stops(cx: &mut TestAppContext) {
    let h = open_live(cx);
    let mut f = agent(&h, cx, "state-file");
    line(&mut f.file, PROMPT);
    line(&mut f.file, THINKING);
    let_the_timer_pass(cx);
    tick_until(&h, cx, "the turn to be seen", |h, cx| {
        activity(h, cx) == Activity::Working
            && h.shell(cx, |shell| {
                shell.live.get(LiveId(1)).unwrap().transcript.is_some()
            })
    });
    // The shell has the terminal back: the transcript is history.
    script_of(&h, 1).set_foreground(true);
    h.settle(cx);
    assert_eq!(activity(&h, cx), Activity::Idle);
    tick_until(&h, cx, "the reading to stop", |h, cx| {
        h.shell(cx, |shell| shell.status.is_idle())
    });
    assert_eq!(
        h.shell(cx, |shell| shell.live.get(LiveId(1)).unwrap().transcript),
        None
    );
}
