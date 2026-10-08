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
struct Processes(
    Mutex<Option<String>>,
    std::sync::atomic::AtomicUsize,
    /// When set, the listing is empty once this list of pids has an entry:
    /// the process ended when it was asked to.
    Mutex<Option<Arc<Mutex<Vec<u32>>>>>,
);

impl Processes {
    fn new() -> Arc<Self> {
        Arc::new(Self(
            Mutex::new(Some("now=1000000\n".to_owned())),
            std::sync::atomic::AtomicUsize::new(0),
            Mutex::new(None),
        ))
    }

    /// Everything listed ends as soon as a pid is asked to end.
    fn ends_when_asked(&self, asked: Arc<Mutex<Vec<u32>>>) {
        *self.2.lock().unwrap() = Some(asked);
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
        let ended = self
            .2
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|asked| !asked.lock().unwrap().is_empty());
        if ended {
            return Ok(Output::ok("now=1000000\n".to_owned()));
        }
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
    let (dir, path, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
    put_cursor_on(&h, cx, NodeId::Session(id.clone()));
    h.settle(cx);
    (h, processes, dir, path, id)
}

/// [`rig`] with the setting's own default: inactive sessions hidden.
fn rig_filter(
    cx: &mut TestAppContext,
) -> (
    Harness,
    Arc<Processes>,
    tempfile::TempDir,
    String,
    leon_core::SessionId,
) {
    let rigged = rig(cx);
    cx.update(|cx| {
        settings::set_value(
            cx,
            crate::schema::find("sidebar_show_inactive").unwrap(),
            crate::schema::Value::Bool(false),
        );
        rigged
            .0
            .shell
            .update(cx, |shell, cx| shell.sync_settings(cx));
    });
    rigged.0.settle(cx);
    rigged
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
    h.press("enter", cx); // Resume
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
    h.press("enter", cx); // Resume
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
    h.press("enter", cx); // Resume
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
    h.press("enter", cx); // Resume
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
    h.press("enter", cx); // Resume
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
    h.press("down", cx);
    h.press("enter", cx); // Resume anyway
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
    h.press("enter", cx); // Resume
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
    h.press("enter", cx); // Resume
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

#[gpui_kit::test]
fn a_session_running_in_another_terminal_is_not_resumed_a_second_time_by_a_restore(
    cx: &mut TestAppContext,
) {
    use leon_core::{SavedLayout, SavedState, SavedTab, SavedTerminal, SavedWorkspace, Slot};
    let (h, processes, _dir, path, _id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    let state = SavedState {
        saved_at: 1,
        clean_shutdown: true,
        selection: None,
        main: Some(1),
        workspaces: vec![SavedWorkspace {
            key: crate::ui::workspace::key_of(MachineId::local().as_str(), &path),
            active: 0,
            tabs: vec![SavedTab {
                layout: SavedLayout::Leaf(1),
                focus: 1,
                zoomed: false,
            }],
        }],
        terminals: vec![SavedTerminal {
            id: 1,
            machine: MachineId::local().as_str().to_owned(),
            cwd: path,
            agent: Some("claude".into()),
            session: Some("alpha-3".into()),
            confidence: Some("resumed".into()),
            history: None,
            name: None,
            title: None,
            started_at: 0,
        }],
    };
    h.store.save_workspace(Slot::Previous, &state).unwrap();
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">restore last", cx);
    h.press("enter", cx);
    h.mouse_on("restore-all".to_owned(), gpui_kit::MouseButton::Left, cx);
    wait_until(&h, cx, "the report", |h, cx| {
        h.shell(cx, |s| s.restore.report_open)
    });
    let failures = h.shell(cx, |s| s.restore.failures.clone());
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0]
            .1
            .contains("already running in another terminal"),
        "{failures:?}"
    );
    assert_eq!(live_count(&h, cx), 0, "no second agent was started");
}

fn badge_shown(h: &Harness, id: &leon_core::SessionId, cx: &mut TestAppContext) -> bool {
    h.shows_dynamic(format!("tree-held-{id}"), cx)
}

#[gpui_kit::test]
fn a_session_held_elsewhere_wears_a_badge_that_says_where(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, id) = rig(cx);
    assert!(!badge_shown(&h, &id, cx));
    // In a plain terminal.
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    assert!(badge_shown(&h, &id, cx));
    h.shell(cx, |s| {
        let session = s.snapshot.sessions.iter().find(|x| x.id == id).unwrap();
        let found = s.elsewhere_of(session).unwrap();
        assert_eq!(found.holder, crate::elsewhere::Holder::Terminal);
    });
    // Below another Leon.
    processes.show(&[
        "T 300 1 leon",
        "T 301 300 claude",
        "A 301 300 ttys3 00:30 claude",
        "S {\"pid\":301,\"sessionId\":\"alpha-3\"}",
    ]);
    scan(&h, cx);
    assert!(badge_shown(&h, &id, cx));
    h.shell(cx, |s| {
        let session = s.snapshot.sessions.iter().find(|x| x.id == id).unwrap();
        let found = s.elsewhere_of(session).unwrap();
        assert_eq!(found.holder, crate::elsewhere::Holder::OtherLeon(300));
    });
    // Nothing runs any more: the badge goes.
    processes.show(&[]);
    scan(&h, cx);
    assert!(!badge_shown(&h, &id, cx));
}

#[gpui_kit::test]
fn the_menu_of_a_session_held_elsewhere_starts_with_the_transcript_and_does_not_offer_resume(
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
    assert_eq!(labels[0], "Open transcript", "{labels:?}");
    if cfg!(unix) {
        assert_eq!(labels[1], "Take over…", "{labels:?}");
        assert_eq!(labels[2], "Resume here anyway…", "{labels:?}");
    } else {
        assert_eq!(labels[1], "Resume here anyway…", "{labels:?}");
    }
    assert!(!labels.contains(&"Resume".to_owned()), "{labels:?}");
    assert!(
        labels.contains(&"Remove from history".to_owned()),
        "{labels:?}"
    );
    assert!(!labels.contains(&"Sleep".to_owned()), "{labels:?}");
}

#[gpui_kit::test]
fn opening_a_session_held_elsewhere_asks_and_the_default_reads_the_transcript(
    cx: &mut TestAppContext,
) {
    let (h, processes, _dir, _path, _id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    h.press("enter", cx); // the question
    h.press("enter", cx); // the default: Open transcript
    h.settle(cx);
    let notice = notice(&h, cx).expect("a notice");
    assert!(
        notice.text.contains("running in another terminal"),
        "{}",
        notice.text
    );
    assert_eq!(live_count(&h, cx), 0, "nothing was started");
}

#[gpui_kit::test]
fn the_transcript_notice_and_the_menu_offer_to_take_over(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, _id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    h.press("enter", cx);
    h.press("enter", cx); // Resume
    h.settle(cx);
    let note = notice(&h, cx).and_then(|n| n.elsewhere).expect("held");
    assert_eq!(note.can_take_over, cfg!(unix));
    assert_eq!(h.shows("transcript-take-over", cx), cfg!(unix));
}

#[cfg(unix)]
#[gpui_kit::test]
fn taking_over_ends_the_other_process_and_then_resumes_here(cx: &mut TestAppContext) {
    let (h, processes, _dir, path, _id) = rig(cx);
    processes.ends_when_asked(h.terminated.clone());
    show(&processes, &strange_claude("alpha-3"));
    h.press("enter", cx);
    h.press("enter", cx); // Resume
    h.settle(cx);
    h.mouse_on(
        "transcript-take-over".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    // A question first: nothing is signalled and nothing starts.
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(h.terminated.lock().unwrap().is_empty());
    h.press("down", cx);
    h.press("enter", cx);
    h.settle(cx);
    for _ in 0..4 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        h.settle(cx);
    }
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    assert_eq!(*h.terminated.lock().unwrap(), vec![70645]);
    assert_eq!(live_count(&h, cx), 1);
    let spec = super::live::script_of(&h, 1).spec().clone();
    assert_eq!(spec.cwd.as_deref(), Some(path.as_str()));
}

#[cfg(unix)]
#[gpui_kit::test]
fn a_process_that_does_not_end_leaves_the_session_alone(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, id) = rig(cx);
    show(&processes, &strange_claude("alpha-3"));
    h.press("enter", cx);
    h.press("enter", cx); // Resume
    h.settle(cx);
    h.mouse_on(
        "transcript-take-over".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
    h.press("down", cx);
    h.press("enter", cx);
    h.settle(cx);
    for _ in 0..8 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        h.settle(cx);
    }
    assert_eq!(*h.terminated.lock().unwrap(), vec![70645], "asked once");
    assert_eq!(live_count(&h, cx), 0, "nothing was resumed");
    assert!(h.status().contains("did not end"), "{}", h.status());
    assert!(marked(&h, &id, cx), "it still runs elsewhere");
}

fn kinds(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
    h.shell(cx, |s| {
        s.rows
            .iter()
            .map(|row| match &row.kind {
                Kind::Project { project, .. } => format!("project:{}", project.name),
                Kind::Session(session) => format!("session:{}", session.title),
                Kind::Live(_) => "live".to_owned(),
                Kind::NoActive => "no-active".to_owned(),
                Kind::Machine(_) => "machine".to_owned(),
                Kind::Worktree { .. } => "worktree".to_owned(),
                _ => "other".to_owned(),
            })
            .collect()
    })
}

fn toggle_show_inactive(h: &Harness, cx: &mut TestAppContext) {
    h.mouse_on(
        if h.shows("filter-show-inactive-on", cx) {
            "filter-show-inactive-on"
        } else {
            "filter-show-inactive"
        }
        .to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    h.settle(cx);
}

#[gpui_kit::test]
fn inactive_sessions_are_hidden_by_default_and_the_toggle_shows_them_and_is_remembered(
    cx: &mut TestAppContext,
) {
    let (h, _processes, _dir, _path, _id) = rig_filter(cx);
    // Off from the start: nothing is active, so the tree says so.
    assert!(!cx.update(|cx| crate::settings::flag(cx, "sidebar_show_inactive")));
    assert_eq!(kinds(&h, cx), ["no-active"]);
    assert!(h.shows("tree-no-active", cx));
    assert!(h.shows("filter-show-inactive", cx));
    // On: the whole tree is back.
    toggle_show_inactive(&h, cx);
    assert!(kinds(&h, cx).iter().any(|k| k.starts_with("session:")));
    assert!(h.shows("filter-show-inactive-on", cx));
    assert!(cx.update(|cx| crate::settings::flag(cx, "sidebar_show_inactive")));
    // Off again: only the active ones.
    toggle_show_inactive(&h, cx);
    assert_eq!(kinds(&h, cx), ["no-active"]);
    assert!(!cx.update(|cx| crate::settings::flag(cx, "sidebar_show_inactive")));
}

#[gpui_kit::test]
fn the_old_active_only_setting_is_ignored(cx: &mut TestAppContext) {
    // A file written by the version that had "Show only active sessions" off
    // (the default then): it does not turn inactive sessions on.
    let (h, _processes, _dir, _path, _id) = rig_filter(cx);
    let def = crate::schema::find("sidebar_show_inactive").unwrap();
    assert_eq!(def.default, crate::schema::Initial::Bool(false));
    assert!(crate::schema::find("sidebar_active_only").is_none());
    assert_eq!(kinds(&h, cx), ["no-active"]);
}

#[gpui_kit::test]
fn hiding_inactive_keeps_a_session_running_elsewhere_and_what_holds_it(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, _id) = rig_filter(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    toggle_show_inactive(&h, cx);
    let before = kinds(&h, cx);
    toggle_show_inactive(&h, cx);
    let shown = kinds(&h, cx);
    assert!(shown.contains(&"session:alpha".to_owned()), "{shown:?}");
    assert!(shown.len() < before.len(), "{shown:?} < {before:?}");
    assert!(!shown.contains(&"no-active".to_owned()));
    // The other projects, and every session that is only history, are gone.
    assert_eq!(
        shown.iter().filter(|k| k.starts_with("session:")).count(),
        1,
        "{shown:?}"
    );
    // When the other process ends, nothing is active.
    processes.show(&[]);
    scan(&h, cx);
    assert_eq!(kinds(&h, cx), ["no-active"]);
}

#[gpui_kit::test]
fn hiding_inactive_keeps_a_live_terminal_and_the_nodes_above_it(cx: &mut TestAppContext) {
    let (h, _processes, _dir, _path, id) = rig_filter(cx);
    assert_eq!(kinds(&h, cx), ["no-active"]);
    toggle_show_inactive(&h, cx);
    put_cursor_on(&h, cx, NodeId::Session(id));
    h.press("enter", cx);
    h.press("enter", cx); // Resume
    wait_until(&h, cx, "the resumed agent", |h, cx| {
        screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
    });
    toggle_show_inactive(&h, cx);
    let shown = kinds(&h, cx);
    assert!(!shown.contains(&"no-active".to_owned()), "{shown:?}");
    // The terminal's own row and the nodes above it, nothing else: no other
    // session, no "show more", no hint.
    assert_eq!(
        shown.iter().filter(|k| k.starts_with("session:")).count(),
        1,
        "{shown:?}"
    );
    assert_eq!(shown.last().map(String::as_str), Some("session:alpha"));
    assert!(shown.len() < 6, "{shown:?}");
}

#[gpui_kit::test]
fn hiding_inactive_lets_the_person_fold_and_open_nodes(cx: &mut TestAppContext) {
    let (h, processes, _dir, _path, _id) = rig_filter(cx);
    show(&processes, &strange_claude("alpha-3"));
    scan(&h, cx);
    let open = kinds(&h, cx);
    assert!(open.contains(&"session:alpha".to_owned()), "{open:?}");
    // Folding the node above it hides what is under it, and its row stays.
    let project = h.shell(cx, |s| {
        s.rows
            .iter()
            .find(|row| row.open == Some(true) && matches!(row.kind, Kind::Folder { .. }))
            .map(super::tree::Row::key)
            .unwrap()
    });
    cx.update(|cx| h.shell.update(cx, |s, _| s.set_open(&project, false)));
    let folded = kinds(&h, cx);
    assert!(!folded.contains(&"session:alpha".to_owned()), "{folded:?}");
    assert!(!folded.contains(&"no-active".to_owned()), "{folded:?}");
    assert_eq!(
        h.shell(cx, |s| s
            .rows
            .iter()
            .find(|row| row.key() == project)
            .and_then(|row| row.open)),
        Some(false)
    );
    // Opening it brings the session back.
    cx.update(|cx| h.shell.update(cx, |s, _| s.set_open(&project, true)));
    assert_eq!(kinds(&h, cx), open);
}
