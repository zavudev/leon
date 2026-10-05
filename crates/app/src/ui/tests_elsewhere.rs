//! Sessions that run in another terminal: marked in the tree, never started a
//! second time by `Enter`, never counted as Leon's own.
//!
//! The processes of "this computer" are a script (`Processes`) behind the
//! engine's scanner, so no `ps` runs; the terminals are the scripted
//! computer of the other UI tests.

use super::live::{local_session, open_live, put_cursor_on, screen, wait_until};
use super::*;
use crate::engine::Op;
use crate::keys::Command;
use crate::ui::steps::{self, Action, Outcome};
use leon_remote::{CommandSpec, RunError, Runner};
use std::sync::Mutex;

/// The pid of "this Leon" in the scripted process tree.
const LEON: u32 = 100;

/// What listing the processes answers; the test changes it between scans.
struct Processes(Mutex<Option<String>>, std::sync::atomic::AtomicUsize);

impl Processes {
    fn new() -> Arc<Self> {
        Arc::new(Self(
            Mutex::new(Some("now=1000000\n".to_owned())),
            std::sync::atomic::AtomicUsize::new(0),
        ))
    }

    /// The listing now says exactly this.
    fn show(&self, lines: &[&str]) {
        *self.0.lock().unwrap() = Some(format!("now=1000000\n{}\n", lines.join("\n")));
    }

    /// The listing fails.
    fn fail(&self) {
        *self.0.lock().unwrap() = None;
    }
}

impl Runner for Processes {
    async fn run(&self, _: &CommandSpec) -> Result<Output, RunError> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(match self.0.lock().unwrap().clone() {
            Some(text) => Output::ok(text),
            None => Output::failed(3, "ps: not found"),
        })
    }
}

/// A claude in another terminal that Claude's own state file ties to
/// `session`.
fn strange_claude(session: &str) -> Vec<String> {
    vec![
        "T 2165 1 /Applications/iTerm.app/Contents/MacOS/iTerm2".to_owned(),
        "T 200 2165 -zsh".to_owned(),
        "T 70645 200 claude".to_owned(),
        "A 70645 200 ttys000 01:00 claude".to_owned(),
        format!("S {{\"pid\":70645,\"sessionId\":\"{session}\"}}"),
    ]
}

fn show(processes: &Processes, lines: &[String]) {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    processes.show(&lines);
}

/// The window with a scanner, a session "alpha" in a real folder, and the
/// cursor on it.
fn rig(
    cx: &mut TestAppContext,
) -> (
    Harness,
    Arc<Processes>,
    tempfile::TempDir,
    String,
    leon_core::SessionId,
) {
    let h = open_live(cx);
    let processes = Processes::new();
    h.engine.set_process_scanner(processes.clone(), Some(LEON));
    let (dir, path, id) = local_session(&h, cx, AgentKind::Claude, "alpha");
    put_cursor_on(&h, cx, NodeId::Session(id.clone()));
    h.settle(cx);
    (h, processes, dir, path, id)
}

fn scan(h: &Harness, cx: &mut TestAppContext) {
    h.engine.submit(Op::Scan(MachineId::local()));
    h.settle(cx);
}

fn marked(h: &Harness, id: &leon_core::SessionId, cx: &mut TestAppContext) -> bool {
    h.shows_dynamic(format!("tree-elsewhere-{id}"), cx)
}

fn notice(h: &Harness, cx: &mut TestAppContext) -> Option<super::super::shell::Notice> {
    h.shell(cx, |s| match &s.main {
        Main::Session(transcript) => transcript.notice.clone(),
        _ => None,
    })
}

fn header(h: &Harness, cx: &mut TestAppContext) -> String {
    cx.update(|cx| h.shell.read(cx).main_heading_for_test(cx))
}

fn live_count(h: &Harness, cx: &mut TestAppContext) -> usize {
    h.shell(cx, |s| s.live.ids().len())
}

#[gpui_kit::test]
fn a_session_running_in_another_terminal_is_marked_and_not_counted_as_live(
    cx: &mut TestAppContext,
) {
    let (h, processes, _dir, _path, id) = rig(cx);
    assert!(!marked(&h, &id, cx), "nothing runs elsewhere yet");
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    assert!(marked(&h, &id, cx), "the session wears the mark");
    assert_eq!(live_count(&h, cx), 0, "it is not a live session of Leon");
    // The mark is its own: no live light on the row, no activity anywhere.
    assert!(!h.shows_dynamic(format!("tree-live-led-{}", 1), cx));
    h.shell(cx, |s| {
        assert!(s.live.all().is_empty());
        let session = s.snapshot.sessions.iter().find(|x| x.id == id).unwrap();
        let found = s
            .elsewhere_of(session)
            .expect("the session is held elsewhere");
        assert_eq!(found.pid, 70645);
        assert!(found.is_certain());
    });
}

#[gpui_kit::test]
fn opening_a_session_running_elsewhere_shows_its_transcript_and_does_not_start_a_terminal(
    cx: &mut TestAppContext,
) {
    let (h, processes, _dir, _path, id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    h.press("enter", cx);
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "session:alpha");
    assert_eq!(live_count(&h, cx), 0);
    assert!(h.computer.script(0).is_none(), "no terminal was started");
    let notice = notice(&h, cx).expect("the transcript says why");
    assert!(
        notice
            .text
            .starts_with("This session is running in another terminal (pid 70645, started "),
        "{}",
        notice.text
    );
    assert!(notice.text.contains("ttys000"), "{}", notice.text);
    assert!(notice.text.contains("in iTerm"), "{}", notice.text);
    let note = notice.elsewhere.expect("what holds it");
    assert!(!note.likely && note.can_reveal);
    for selector in [
        "transcript-notice",
        "transcript-open",
        "transcript-resume-anyway",
        "transcript-reveal",
    ] {
        assert!(h.shows(selector, cx), "{selector} is drawn");
    }
    // The header says where it runs, and the mark is on the row.
    let header = header(&h, cx);
    assert!(
        header.contains("RUNNING IN ANOTHER TERMINAL (PID 70645)"),
        "{header}"
    );
    assert!(marked(&h, &id, cx));
}

#[gpui_kit::test]
fn resume_here_anyway_asks_for_confirmation_then_resumes(cx: &mut TestAppContext) {
    let (h, processes, _dir, path, _id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    h.press("enter", cx);
    h.settle(cx);
    h.mouse_on(
        "transcript-resume-anyway".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    // A question first, and nothing is started by it.
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert_eq!(live_count(&h, cx), 0);
    let titles = h.palette_titles(cx);
    assert_eq!(titles[0], "Cancel", "{titles:?}");
    assert!(titles[1].contains("here anyway"), "{titles:?}");
    // The first answer, which Enter takes, leaves things alone.
    h.press("enter", cx);
    h.settle(cx);
    assert_eq!(live_count(&h, cx), 0);
    // Asked again, the second answer resumes it here.
    h.mouse_on(
        "transcript-resume-anyway".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    assert_eq!(h.main_kind(cx), "live:1");
    assert_eq!(live_count(&h, cx), 1);
    assert!(h.status().contains("also holds it"), "{}", h.status());
    let spec = super::live::script_of(&h, 1).spec().clone();
    assert_eq!(spec.cwd.as_deref(), Some(path.as_str()));
}

#[gpui_kit::test]
fn a_session_resumed_inside_leon_is_never_reported_as_elsewhere(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, id) = rig(cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    // The agent Leon started runs below this Leon: shell, then claude.
    processes.show(&[
        "T 100 1 leon",
        "T 101 100 /bin/sh",
        "T 102 101 claude",
        "A 102 101 ttys9 00:05 claude --resume 11111111-2222-3333-4444-555555555555",
        "S {\"pid\":102,\"sessionId\":\"alpha-3\"}",
    ]);
    scan(&h, cx);
    assert!(!marked(&h, &id, cx), "its own child is not elsewhere");
    assert_eq!(live_count(&h, cx), 1);
    h.shell(cx, |s| {
        let session = s.snapshot.sessions.iter().find(|x| x.id == id).unwrap();
        assert!(s.elsewhere_of(session).is_none());
    });
    // A second Leon's agent is elsewhere, and the warning says so.
    processes.show(&[
        "T 100 1 leon",
        "T 101 100 /bin/sh",
        "T 102 101 claude",
        "T 300 1 leon",
        "T 301 300 claude",
        "A 102 101 ttys9 00:05 claude",
        "A 301 300 ttys8 00:05 claude",
        "S {\"pid\":102,\"sessionId\":\"alpha-3\"}",
        "S {\"pid\":301,\"sessionId\":\"alpha-3\"}",
    ]);
    scan(&h, cx);
    assert!(marked(&h, &id, cx), "another Leon's session is elsewhere");
    let header = header(&h, cx);
    assert!(
        header.contains("PID 301 HOLDS THIS SESSION TOO"),
        "{header}"
    );
}

#[gpui_kit::test]
fn quitting_does_not_count_sessions_running_elsewhere(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    assert!(marked(&h, &id, cx));
    let world = cx.update(|cx| h.shell.update(cx, |s, cx| s.world(cx)));
    assert_eq!(
        steps::advance(Command::Quit, &[], &world),
        Outcome::Run(Action::Quit),
        "nothing of Leon's runs, so quitting does not ask"
    );
}

#[gpui_kit::test]
fn the_mark_clears_when_the_other_process_exits(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    assert!(marked(&h, &id, cx));
    processes.show(&[]);
    scan(&h, cx);
    assert!(!marked(&h, &id, cx));
    // And the session opens and resumes normally again.
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    assert_eq!(h.main_kind(cx), "live:1");
}

#[gpui_kit::test]
fn a_likely_match_is_worded_as_likely(cx: &mut TestAppContext) {
    let (h, processes, _dir, path, id) = rig(cx);
    // `claude --continue` in the session's folder: nothing names the session.
    processes.show(&[
        "T 200 1 -zsh",
        "T 70646 200 claude",
        "A 70646 200 ttys001 00:10 claude --continue",
        &format!("C 70646 {path}"),
    ]);
    scan(&h, cx);
    assert!(marked(&h, &id, cx));
    h.press("enter", cx);
    h.settle(cx);
    let notice = notice(&h, cx).expect("a notice");
    assert!(
        notice.text.contains("probably running in another terminal"),
        "{}",
        notice.text
    );
    assert!(
        notice.text.contains("Enter resumes it here"),
        "{}",
        notice.text
    );
    assert!(notice.elsewhere.as_ref().unwrap().likely);
    assert_eq!(live_count(&h, cx), 0);
    // Enter on the notice resumes, without the question.
    cx.update(|cx| h.shell.update(cx, |s, _| s.pane = Pane::Main));
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(h.status().contains("probably"), "{}", h.status());
}

#[gpui_kit::test]
fn a_check_that_cannot_be_made_opens_the_session_as_ever(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    assert!(marked(&h, &id, cx));
    processes.fail();
    h.press("enter", cx);
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    assert!(
        !marked(&h, &id, cx),
        "what could not be checked is not claimed"
    );
}

#[gpui_kit::test]
fn reveal_brings_the_terminal_application_forward_when_the_tree_names_it(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, _id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    h.press("enter", cx);
    h.settle(cx);
    h.mouse_on(
        "transcript-reveal".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert_eq!(
        *h.revealed_apps.borrow(),
        vec!["/Applications/iTerm.app".to_owned()]
    );
}

#[gpui_kit::test]
fn without_a_known_application_reveal_says_so_and_offers_no_button(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, _id) = rig(cx);
    processes.show(&[
        "T 200 1 -zsh",
        "T 70645 200 claude",
        "A 70645 200 ttys000 01:00 claude",
        "S {\"pid\":70645,\"sessionId\":\"alpha-3\"}",
    ]);
    h.press("enter", cx);
    h.settle(cx);
    assert!(h.shows("transcript-resume-anyway", cx));
    assert!(!h.shows("transcript-reveal", cx));
    cx.update(|cx| h.shell.update(cx, |s, cx| s.reveal_terminal_here(cx)));
    assert!(h.revealed_apps.borrow().is_empty());
    assert!(h.status().contains("cannot tell"), "{}", h.status());
}

#[gpui_kit::test]
fn the_timer_and_the_focus_look_at_this_computer_and_only_at_machines_that_answer(
    cx: &mut TestAppContext,
) {
    let (h, processes, _dir, _path, _id) = rig(cx);
    let calls = || processes.1.load(std::sync::atomic::Ordering::SeqCst);
    // The seeded SSH machine has never answered: only this computer is read.
    cx.update(|cx| h.shell.update(cx, |s, cx| s.scan_elsewhere_now(false, cx)));
    h.settle(cx);
    assert_eq!(calls(), 1);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.scan_elsewhere_now(true, cx)));
    h.settle(cx);
    assert_eq!(calls(), 2, "an unprobed machine is not asked");
}

#[gpui_kit::test]
fn the_tree_menu_of_a_session_held_elsewhere_offers_the_same_choices_as_the_palette(
    cx: &mut TestAppContext,
) {
    let (h, processes, _dir, _path, id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    let labels = h.shell(cx, |s| {
        let at = s
            .rows
            .iter()
            .position(|r| r.id == NodeId::Session(id.clone()))
            .unwrap();
        s.menu_for_test(at)
    });
    assert!(
        labels.contains(&"Resume here anyway…".to_owned()),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Reveal the terminal".to_owned()),
        "{labels:?}"
    );
    processes.show(&[]);
    scan(&h, cx);
    let labels = h.shell(cx, |s| {
        let at = s
            .rows
            .iter()
            .position(|r| r.id == NodeId::Session(id.clone()))
            .unwrap();
        s.menu_for_test(at)
    });
    assert!(
        !labels.contains(&"Resume here anyway…".to_owned()),
        "{labels:?}"
    );
}

#[gpui_kit::test]
fn turning_detection_off_stops_every_scan_and_the_tree_forgets_nothing_it_never_asked(
    cx: &mut TestAppContext,
) {
    use crate::schema::{self, Value};
    let (h, processes, _dir, _path, _id) = rig(cx);
    let calls = || processes.1.load(std::sync::atomic::Ordering::SeqCst);
    cx.update(|cx| h.shell.update(cx, |s, cx| s.scan_elsewhere_now(false, cx)));
    h.settle(cx);
    assert_eq!(calls(), 1);
    let def = schema::find("detect_elsewhere").unwrap();
    cx.update(|cx| settings::set_value(cx, def, Value::Bool(false)));
    // The timer, the focus and a refresh all go through the same gate.
    cx.update(|cx| h.shell.update(cx, |s, cx| s.scan_elsewhere_now(true, cx)));
    h.settle(cx);
    assert_eq!(calls(), 1, "no scan while it is off");
    cx.update(|cx| settings::set_value(cx, def, Value::Bool(true)));
    cx.update(|cx| h.shell.update(cx, |s, cx| s.scan_elsewhere_now(false, cx)));
    h.settle(cx);
    assert_eq!(calls(), 2, "and it resumes when it is on again");
}
