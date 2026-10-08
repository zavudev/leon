//! Durable local sessions, in the window.
//!
//! The keeper is a scripted one (`Fake`): it lists what a test says it holds,
//! hands out remote terminals whose link records what the window does to them,
//! and says who is in front when told. Nothing is started, no socket is opened
//! and nothing touches the user's real runtime or data directory. What these
//! tests pin down is the window's side: which terminals the keeper gets, what
//! a restart attaches to and what it falls back to, what quitting and closing
//! do to them, and what the window keeps knowing about a terminal it no longer
//! holds.

use super::live::wait_until;
use super::tests_restore::{
    live_count, new_session, one_tab_each, pass, quit, restart, saved_of, settings_with,
    start_fresh,
};
use super::*;
use crate::durable::{Durable, Held, KeeperSlot};
use crate::ui::live::{LiveId, LiveSession};
use gpui_kit::App;
use leon_core::{KeeperRef, SavedState, Slot};
use leon_term::{
    HeldForeground, RemoteFeed, RemoteLink, SpawnError, Terminal, TerminalTheme, Wake,
};
use leon_wire::Grid;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// The link of a terminal the scripted keeper holds: records what the window
/// does to it, and says who is in front when told.
#[derive(Default)]
struct Link {
    events: Mutex<Vec<String>>,
    front: Mutex<Option<HeldForeground>>,
}

impl Link {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    /// The keeper says the shell is (not) in front, with `pid` for the shell.
    fn say(&self, shell_in_front: bool, command: Option<&str>) {
        *self.front.lock().unwrap() = Some(HeldForeground {
            pid: Some(4242),
            shell_in_front: Some(shell_in_front),
            command: command.map(str::to_owned),
        });
    }
}

impl RemoteLink for Link {
    fn write(&self, bytes: Vec<u8>) {
        self.events
            .lock()
            .unwrap()
            .push(format!("write {}", String::from_utf8_lossy(&bytes)));
    }
    fn resize(&self, size: leon_term::GridSize) {
        self.events
            .lock()
            .unwrap()
            .push(format!("resize {}x{}", size.cols, size.rows));
    }
    fn close(&self) {
        self.events.lock().unwrap().push("close".into());
    }
    fn detach(&self) {
        self.events.lock().unwrap().push("detach".into());
    }
    fn is_local(&self) -> bool {
        true
    }
    fn foreground(&self) -> Option<HeldForeground> {
        self.front.lock().unwrap().clone()
    }
    fn terminate_foreground(&self) -> bool {
        self.events.lock().unwrap().push("terminate".into());
        true
    }
}

/// A keeper a test drives.
#[derive(Default)]
struct Fake {
    held: Mutex<Vec<Held>>,
    /// Nothing is running: listing it is an error, as with no socket.
    down: AtomicBool,
    /// Opening a terminal fails.
    refuses: AtomicBool,
    /// Opening a terminal returns at once, the keeper's reference to follow.
    slow: AtomicBool,
    pending: Mutex<Vec<KeeperSlot>>,
    listed: AtomicUsize,
    opened: Mutex<Vec<(leon_term::SpawnSpec, String)>>,
    attached: Mutex<Vec<u64>>,
    ended_all: AtomicUsize,
    settled: AtomicUsize,
    /// The keeper does not confirm what it was told.
    unconfirmed: AtomicBool,
    links: Mutex<Vec<Arc<Link>>>,
    feeds: Mutex<Vec<RemoteFeed>>,
    /// What an attach replays into the screen.
    replay: Mutex<String>,
    next: AtomicU64,
}

impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(100),
            ..Self::default()
        })
    }

    fn link(&self, at: usize) -> Arc<Link> {
        self.links.lock().unwrap()[at].clone()
    }
}

impl Durable for Fake {
    fn held(&self) -> Result<Vec<Held>, String> {
        self.listed.fetch_add(1, Ordering::SeqCst);
        if self.down.load(Ordering::SeqCst) {
            return Err("no keeper is running".into());
        }
        Ok(self.held.lock().unwrap().clone())
    }

    fn open(
        &self,
        spec: &leon_term::SpawnSpec,
        size: leon_term::GridSize,
        theme: TerminalTheme,
        wake: Wake,
        token: &str,
    ) -> Result<(Terminal, KeeperSlot), SpawnError> {
        if self.refuses.load(Ordering::SeqCst) {
            return Err(SpawnError("the socket is not there".into()));
        }
        let link = Arc::new(Link::default());
        let (terminal, feed) = Terminal::remote(spec, size, theme, wake, link.clone());
        self.links.lock().unwrap().push(link);
        self.feeds.lock().unwrap().push(feed);
        self.opened
            .lock()
            .unwrap()
            .push((spec.clone(), token.to_owned()));
        let pty = self.next.fetch_add(1, Ordering::SeqCst);
        if self.slow.load(Ordering::SeqCst) {
            let slot = KeeperSlot::default();
            self.pending.lock().unwrap().push(slot.clone());
            return Ok((terminal, slot));
        }
        Ok((
            terminal,
            KeeperSlot::known(KeeperRef {
                pty,
                token: token.to_owned(),
            }),
        ))
    }

    fn attach(
        &self,
        held: &Held,
        theme: TerminalTheme,
        wake: Wake,
    ) -> Result<Terminal, SpawnError> {
        self.attached.lock().unwrap().push(held.pty);
        let link = Arc::new(Link::default());
        let size = leon_term::GridSize::new(held.size.cols, held.size.rows);
        let (terminal, feed) = Terminal::remote(
            &leon_term::SpawnSpec::default(),
            size,
            theme,
            wake,
            link.clone(),
        );
        feed.replay(self.replay.lock().unwrap().as_bytes());
        self.links.lock().unwrap().push(link);
        self.feeds.lock().unwrap().push(feed);
        Ok(terminal)
    }

    fn end_all(&self) -> bool {
        self.ended_all.fetch_add(1, Ordering::SeqCst);
        !self.unconfirmed.load(Ordering::SeqCst)
    }

    fn settle(&self) -> bool {
        self.settled.fetch_add(1, Ordering::SeqCst);
        !self.unconfirmed.load(Ordering::SeqCst)
    }

    fn keeper_pid(&self) -> Option<u32> {
        None
    }

    fn token(&self) -> String {
        format!("tok-{}", self.next.fetch_add(1, Ordering::SeqCst))
    }
}

const ON: &str = r#"{"durable_sessions": true, "sidebar_show_inactive": true}"#;
const OFF: &str = r#"{"durable_sessions": false, "sidebar_show_inactive": true}"#;

/// Whether this system has durable sessions at all: the setting does not exist
/// on Windows, whatever the file says.
fn possible() -> bool {
    crate::schema::Platform::Unix.here()
}

fn grid() -> Grid {
    Grid {
        cols: 100,
        rows: 30,
        cell_width: 0,
        cell_height: 0,
    }
}

fn held(pty: u64, token: &str, cwd: &str) -> Held {
    Held {
        pty,
        token: token.to_owned(),
        cwd: Some(cwd.to_owned()),
        size: grid(),
        started_unix: 1_700_000_000,
        exit: None,
        attached: 0,
    }
}

/// A window with `keeper` and these settings; `state` is the previous run's.
fn open_keeper(
    cx: &mut TestAppContext,
    dir: &tempfile::TempDir,
    json: &str,
    keeper: &Arc<Fake>,
    state: Option<&SavedState>,
) -> Harness {
    cx.executor().allow_parking();
    let settings = settings_with(dir, json);
    let durable: Arc<dyn Durable> = keeper.clone();
    super::DURABLE.with(|slot| *slot.borrow_mut() = Some(durable));
    let state = state.cloned();
    open_full(
        cx,
        ScriptedRunner::new(),
        Some(settings),
        Picked::Cancelled,
        move |store| {
            if let Some(state) = &state {
                store.save_workspace(Slot::Current, state).unwrap();
            }
        },
    )
}

fn session_of<R>(
    h: &Harness,
    cx: &mut TestAppContext,
    id: u64,
    read: impl FnOnce(&LiveSession, &App) -> R,
) -> R {
    cx.update(|cx| {
        let shell = h.shell.read(cx);
        read(shell.live.get(LiveId(id)).expect("the session"), cx)
    })
}

fn keeper_of(session: &LiveSession) -> Option<KeeperRef> {
    session.keeper.as_ref().and_then(KeeperSlot::get)
}

fn held_terminal(h: &Harness, cx: &mut TestAppContext, id: u64) -> bool {
    terminal_of_id(h, cx, id).is_some_and(|t| t.is_held())
}

fn terminal_of_id(h: &Harness, cx: &mut TestAppContext, id: u64) -> Option<Arc<Terminal>> {
    super::live::terminal_of(h, cx, id)
}

/// A saved agent terminal that the keeper held.
fn saved_held(cwd: &str, pty: u64, token: &str) -> SavedState {
    let mut state = one_tab_each(cwd, "claude", Some("sid-7"), &[1]);
    state.terminals[0].keeper = Some(KeeperRef {
        pty,
        token: token.to_owned(),
    });
    state
}

fn real_cwd(dir: &tempfile::TempDir) -> String {
    dir.path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

// ----- starting ------------------------------------------------------------------------------------

#[gpui_kit::test]
fn with_the_setting_on_a_new_shell_is_held_by_the_keeper_and_remembered(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        // Windows: the setting does not exist and nothing changes.
        assert!(keeper.opened.lock().unwrap().is_empty());
        assert!(!held_terminal(&h, cx, 1));
        return;
    }
    let opened = keeper.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1, "the keeper was asked for the terminal");
    // Started with the plan the window would have run in-process: the same
    // folder, the same shell.
    assert_eq!(opened[0].0.cwd.as_deref(), Some(path.as_str()));
    assert!(!opened[0].0.program.is_empty());
    assert!(
        h.computer.script(0).is_none(),
        "nothing was started in the window"
    );
    assert!(held_terminal(&h, cx, 1));
    let terminal = terminal_of_id(&h, cx, 1).unwrap();
    assert!(
        !terminal.is_remote(),
        "it runs here: it is not a terminal of another computer"
    );
    // What the layout will need at the next start is remembered.
    let reference = session_of(&h, cx, 1, |s, _| keeper_of(s)).expect("the keeper's number");
    assert_eq!(reference.token, opened[0].1);
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals[0].keeper, Some(reference));
}

#[gpui_kit::test]
fn with_the_setting_off_a_new_shell_lives_in_the_window_as_ever(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, OFF, &keeper, None);
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    assert!(keeper.opened.lock().unwrap().is_empty());
    assert!(h.computer.script(0).is_some(), "started in the window");
    assert!(!held_terminal(&h, cx, 1));
    assert_eq!(session_of(&h, cx, 1, |s, _| keeper_of(s)), None);
    assert_eq!(saved_of(&h, cx).terminals[0].keeper, None);
}

#[gpui_kit::test]
fn a_keeper_that_cannot_be_reached_leaves_the_session_in_the_window_with_a_plain_message(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    keeper.refuses.store(true, Ordering::SeqCst);
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    assert!(h.computer.script(0).is_some(), "the session still starts");
    assert!(!held_terminal(&h, cx, 1));
    assert_eq!(session_of(&h, cx, 1, |s, _| keeper_of(s)), None);
    if possible() {
        let status = h.status();
        assert!(
            status.contains("keeper is not available") && status.contains("lives in the window"),
            "{status}"
        );
    }
}

// ----- the next start ------------------------------------------------------------------------------

#[gpui_kit::test]
fn with_the_setting_on_a_restart_attaches_instead_of_typing_the_resume_line(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    keeper.held.lock().unwrap().push(held(5, "t1", &cwd));
    *keeper.replay.lock().unwrap() = "while you were away\r\n".to_owned();
    let state = saved_held(&cwd, 5, "t1");
    let h = restart_with(cx, &settings, ON, &keeper, &state);
    if !possible() {
        assert!(keeper.attached.lock().unwrap().is_empty());
        return;
    }
    // Running programs are not a question, whatever `restore_sessions` says.
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(live_count(&h, cx), 1);
    assert_eq!(*keeper.attached.lock().unwrap(), [5]);
    assert!(
        h.computer.script(0).is_none(),
        "no terminal was started in the window"
    );
    assert!(held_terminal(&h, cx, 1));
    // Its identity is what it was: the agent and its session, not paused.
    session_of(&h, cx, 1, |s, _| {
        assert_eq!(s.agent, Some(leon_core::AgentId::CLAUDE));
        assert_eq!(s.learned, Some(("sid-7".to_owned(), "resumed".to_owned())));
        assert!(!s.is_paused(), "nothing is waiting to be resumed");
        assert_eq!(s.pending, None);
        assert_eq!(
            keeper_of(s),
            Some(KeeperRef {
                pty: 5,
                token: "t1".into()
            })
        );
    });
    // The screen is rebuilt from what the keeper kept, and nothing was typed.
    assert!(
        super::live::screen(&h, cx, 1).contains("while you were away"),
        "{}",
        super::live::screen(&h, cx, 1)
    );
    assert!(
        keeper
            .link(0)
            .events()
            .iter()
            .all(|e| !e.starts_with("write")),
        "{:?}",
        keeper.link(0).events()
    );
    assert_eq!(h.main_kind(cx), "live:1");
    // And it is saved again with the same reference.
    assert_eq!(
        saved_of(&h, cx).terminals[0].keeper,
        Some(KeeperRef {
            pty: 5,
            token: "t1".into()
        })
    );
}

/// A restart: `state` is the previous run's, `keeper` is what is running.
fn restart_with(
    cx: &mut TestAppContext,
    dir: &tempfile::TempDir,
    json: &str,
    keeper: &Arc<Fake>,
    state: &SavedState,
) -> Harness {
    let settings = settings_with(dir, json);
    let durable: Arc<dyn Durable> = keeper.clone();
    super::DURABLE.with(|slot| *slot.borrow_mut() = Some(durable));
    restart(cx, Some(settings), state)
}

#[gpui_kit::test]
fn with_the_setting_off_and_nothing_kept_a_restart_asks_as_it_always_did(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    // Nothing in the layout was ever held by a keeper.
    let state = one_tab_each(&cwd, "claude", Some("sid-7"), &[1]);
    let h = restart_with(cx, &settings, OFF, &keeper, &state);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Restore, "it asks");
    assert_eq!(live_count(&h, cx), 0, "nothing starts before the answer");
    assert!(keeper.attached.lock().unwrap().is_empty());
    assert!(keeper.opened.lock().unwrap().is_empty());
    // Answered, the agent is a paused session of the window, resumed by its
    // line as before.
    let always = r#"{"durable_sessions": false, "restore_sessions": "always", "sidebar_show_inactive": true}"#;
    let h = restart_with(cx, &settings, always, &keeper, &state);
    wait_until(&h, cx, "the agent", |h, cx| live_count(h, cx) == 1);
    // Its resume line is typed into a terminal of the window.
    wait_until(&h, cx, "the resume line", |h, cx| {
        super::live::screen(h, cx, 1).contains("FAKE-CLAUDE --resume sid-7")
    });
    assert!(!held_terminal(&h, cx, 1));
    assert!(keeper.attached.lock().unwrap().is_empty());
    assert!(keeper.opened.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn turning_the_setting_off_does_not_start_a_second_agent_beside_a_running_one(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    // The layout remembers a session the keeper holds, and the keeper still
    // holds it, but the setting was turned off since.
    let keeper = Fake::new();
    keeper.held.lock().unwrap().push(held(5, "t1", &cwd));
    let state = saved_held(&cwd, 5, "t1");
    let json = r#"{"durable_sessions": false, "restore_sessions": "always", "sidebar_show_inactive": true}"#;
    let h = restart_with(cx, &settings, json, &keeper, &state);
    if !possible() {
        return;
    }
    wait_until(&h, cx, "the session", |h, cx| live_count(h, cx) == 1);
    assert_eq!(
        *keeper.attached.lock().unwrap(),
        [5],
        "attached to what is running, not resumed beside it"
    );
    assert!(held_terminal(&h, cx, 1));
    assert!(keeper.opened.lock().unwrap().is_empty());
    assert!(h.computer.script(0).is_none());
    // New sessions are the window's again.
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    assert!(keeper.opened.lock().unwrap().is_empty());
    assert!(h.computer.script(0).is_some());
}

#[gpui_kit::test]
fn a_saved_session_whose_terminal_is_gone_falls_back_to_the_paused_resume_line(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    // After a restart of the computer there is no keeper at all.
    let keeper = Fake::new();
    keeper.down.store(true, Ordering::SeqCst);
    let state = saved_held(&cwd, 5, "t1");
    let json = r#"{"durable_sessions": true, "restore_sessions": "always", "sidebar_show_inactive": true}"#;
    let h = restart_with(cx, &settings, json, &keeper, &state);
    wait_until(&h, cx, "the agent", |h, cx| live_count(h, cx) == 1);
    assert!(keeper.attached.lock().unwrap().is_empty());
    if !possible() {
        return;
    }
    // Nothing was there to attach to, so the session is resumed the way every
    // saved session is: a new terminal (under a keeper that is started for
    // it) gets the agent's resume line.
    assert_eq!(keeper.opened.lock().unwrap().len(), 1);
    wait_until(&h, cx, "the resume line", |_, _| {
        keeper
            .link(0)
            .events()
            .iter()
            .any(|e| e.starts_with("write") && e.contains("--resume sid-7"))
    });
    // The new terminal is the one remembered, not the lost one.
    let reference = session_of(&h, cx, 1, |s, _| keeper_of(s)).expect("a new terminal");
    assert_ne!(reference.token, "t1");
    // And the window says what happened to the old one, in plain words.
    let status = h.status();
    assert!(
        status.contains("Restored 1") && status.contains("no longer held 1 of the sessions"),
        "{status}"
    );
}

#[gpui_kit::test]
fn a_keeper_that_lost_the_terminal_or_reused_its_number_does_not_attach_to_a_stranger(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    // A newer keeper numbered another terminal 5.
    let keeper = Fake::new();
    keeper
        .held
        .lock()
        .unwrap()
        .push(held(5, "someone-else", &cwd));
    let state = saved_held(&cwd, 5, "t1");
    let json = r#"{"durable_sessions": true, "restore_sessions": "always", "sidebar_show_inactive": true}"#;
    let h = restart_with(cx, &settings, json, &keeper, &state);
    if !possible() {
        return;
    }
    wait_until(&h, cx, "the sessions", |h, cx| live_count(h, cx) == 2);
    // The stranger is running, so it appears as a session of its own (that is
    // the only terminal attached); the saved session got a new terminal and
    // its resume line. Nothing was attached under the saved identity.
    assert_eq!(*keeper.attached.lock().unwrap(), [5]);
    assert_eq!(keeper.opened.lock().unwrap().len(), 1);
    let sessions: Vec<(Option<leon_core::AgentId>, KeeperRef)> = h
        .shell(cx, |s| s.live.ids())
        .iter()
        .map(|id| session_of(&h, cx, id.0, |s, _| (s.agent, keeper_of(s).expect("held"))))
        .collect();
    let stranger = sessions.iter().find(|(agent, _)| agent.is_none()).unwrap();
    assert_eq!(stranger.1.token, "someone-else");
    let saved = sessions
        .iter()
        .find(|(agent, _)| *agent == Some(leon_core::AgentId::CLAUDE))
        .unwrap();
    assert_ne!(saved.1.token, "t1");
    assert_ne!(saved.1.token, "someone-else");
}

#[gpui_kit::test]
fn a_running_terminal_the_layout_does_not_know_appears_instead_of_being_orphaned(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    keeper.held.lock().unwrap().push(held(9, "late", &cwd));
    // Nothing saved at all: Leon crashed before it wrote its first layout.
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    if !possible() {
        assert_eq!(live_count(&h, cx), 0);
        return;
    }
    assert_eq!(live_count(&h, cx), 1);
    assert_eq!(*keeper.attached.lock().unwrap(), [9]);
    assert!(held_terminal(&h, cx, 1));
    session_of(&h, cx, 1, |s, _| {
        assert_eq!(s.cwd, cwd);
        assert_eq!(keeper_of(s).map(|k| k.pty), Some(9));
    });
    assert!(
        h.shell(cx, |s| s.workspaces.locate(LiveId(1)).is_some()),
        "it has a tab to be seen in"
    );
    assert!(h.status().contains("did not know"), "{}", h.status());
}

// ----- what the window keeps knowing without the pseudo-terminal -----------------------------------

#[gpui_kit::test]
fn the_transcript_status_still_applies_to_a_durable_local_session(cx: &mut TestAppContext) {
    use crate::ui::activity::{transcript_applies, Transcript};
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    if !possible() {
        return;
    }
    assert!(held_terminal(&h, cx, 1));
    // Before the keeper has answered nothing is claimed: no transcript is
    // followed on a guess.
    let signals = session_of(&h, cx, 1, |s, cx| s.signals(cx));
    assert_eq!(signals.shell_in_front, None);
    assert!(!transcript_applies(&signals));
    // The keeper says the agent is in front: the transcript applies and the
    // session reads what the agent's own file says.
    keeper.link(0).say(false, Some("claude"));
    let signals = session_of(&h, cx, 1, |s, cx| s.signals(cx));
    assert_eq!(signals.shell_in_front, Some(false));
    assert!(transcript_applies(&signals));
    let activity = session_of(&h, cx, 1, |s, cx| {
        crate::ui::activity::session_activity(
            crate::ui::activity::Activity::Idle,
            &s.signals(cx),
            Some(Transcript::Turn),
        )
    });
    assert_eq!(activity, crate::ui::activity::Activity::Working);
    // The shell got the terminal back: the transcript is history again.
    keeper.link(0).say(true, None);
    let signals = session_of(&h, cx, 1, |s, cx| s.signals(cx));
    assert!(!transcript_applies(&signals));
}

#[gpui_kit::test]
fn the_transcript_status_applies_to_a_re_attached_session_too(cx: &mut TestAppContext) {
    use crate::ui::activity::transcript_applies;
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    keeper.held.lock().unwrap().push(held(5, "t1", &cwd));
    let h = restart_with(cx, &settings, ON, &keeper, &saved_held(&cwd, 5, "t1"));
    if !possible() {
        return;
    }
    keeper.link(0).say(false, Some("claude"));
    let signals = session_of(&h, cx, 1, |s, cx| s.signals(cx));
    assert_eq!(signals.shell_in_front, Some(false));
    assert!(signals.agent, "it is an agent, and not paused");
    assert!(transcript_applies(&signals));
    // The shell's pid, which the learning of the agent's own session id by
    // process goes by, comes from the keeper.
    let terminal = terminal_of_id(&h, cx, 1).unwrap();
    assert_eq!(terminal.process_id(), Some(4242));
    // A re-attached agent that already returned to its shell reads as one.
    keeper.link(0).say(true, None);
    keeper.feeds.lock().unwrap()[0].changed();
    cx.run_until_parked();
    assert_eq!(
        session_of(&h, cx, 1, |s, _| s.phase),
        crate::ui::live::AgentPhase::Returned
    );
}

#[gpui_kit::test]
fn a_durable_terminal_is_local_for_links_and_pastes(cx: &mut TestAppContext) {
    use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat};
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (dir, _path) = super::live::real_worktree(&h, cx);
    std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    // A path printed in the terminal is a link to a file of this computer.
    keeper.feeds.lock().unwrap()[0].data(b"see notes.txt now\r\n");
    let terminal = terminal_of_id(&h, cx, 1).unwrap();
    assert!(
        terminal
            .link_at(6, 0)
            .is_some_and(|l| l.contains("notes.txt")),
        "{:?}",
        terminal.link_at(6, 0)
    );
    // An image on the clipboard is pasted by Ctrl+V, as the program here can
    // read it.
    let item = ClipboardItem {
        entries: vec![ClipboardEntry::Image(Image::from_bytes(
            ImageFormat::Png,
            vec![1, 2, 3],
        ))],
    };
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.read_clipboard = Rc::new(move |_| Some(item.clone()));
        })
    });
    h.press_chord("cmd-v", "ctrl-v", cx);
    assert!(
        keeper.link(0).events().iter().any(|e| e == "write \u{16}"),
        "{:?}",
        keeper.link(0).events()
    );
    assert!(!h
        .status()
        .contains("needs the agent to run on this computer"));
    // Files dropped on the pane paste their quoted paths.
    cx.update(|cx| {
        h.shell.update(cx, |s, cx| {
            s.paste_dropped(LiveId(1), &[dir.path().join("a b.txt")], cx);
        })
    });
    assert!(
        keeper
            .link(0)
            .events()
            .iter()
            .any(|e| e.contains("a b.txt") && e.starts_with("write")),
        "{:?}",
        keeper.link(0).events()
    );
}

// ----- closing, sleeping and quitting --------------------------------------------------------------

#[gpui_kit::test]
fn closing_a_durable_session_ends_its_terminal(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    assert!(held_terminal(&h, cx, 1));
    // The keeper has said that the shell is in front: nothing runs in it.
    keeper.link(0).say(true, None);
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    assert!(h.shell(cx, |s| s.live.ids().is_empty()), "closed");
    // Hung up, not just let go of (the resize is the view's own, and the
    // detach is the terminal being dropped after it was ended).
    let events: Vec<String> = keeper
        .link(0)
        .events()
        .into_iter()
        .filter(|e| !e.starts_with("resize") && e != "detach")
        .collect();
    assert_eq!(events, ["close"], "ended under the keeper");
}

#[gpui_kit::test]
fn putting_a_durable_session_to_sleep_ends_its_terminal(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    if !possible() {
        return;
    }
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |s, cx| s.sleep_live(LiveId(1), window, cx))
    })
    .unwrap();
    cx.run_until_parked();
    assert!(h.shell(cx, |s| s.live.ids().is_empty()));
    assert!(keeper.link(0).events().contains(&"close".to_owned()));
}

#[gpui_kit::test]
fn quitting_with_the_setting_on_detaches_and_signals_nothing(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    if !possible() {
        return;
    }
    keeper.link(0).say(false, Some("claude"));
    let typed = keeper.link(0).events().len();
    quit(&h, cx);
    assert_eq!(h.quits.get(), 1, "no agent is waited for");
    let events = keeper.link(0).events();
    assert_eq!(
        &events[typed..],
        ["detach"],
        "no exit line, no signal, no hang-up: {events:?}"
    );
    assert_eq!(keeper.ended_all.load(Ordering::SeqCst), 0);
    // The layout written for the next start names the terminal.
    let state = h.store.load_workspace(Slot::Current).unwrap().unwrap();
    assert!(state.clean_shutdown);
    assert!(state.terminals[0].keeper.is_some());
}

#[gpui_kit::test]
fn quit_and_end_every_session_is_the_old_behaviour_for_the_held_terminals_too(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    if !possible() {
        return;
    }
    keeper.link(0).say(false, Some("claude"));
    cx.update_window(h.window.into(), |_, window, cx| {
        h.shell
            .update(cx, |s, cx| s.end_sessions_and_quit(None, window, cx))
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        keeper.link(0).events().iter().any(|e| e == "write /exit\r"),
        "the agent is given its own exit line: {:?}",
        keeper.link(0).events()
    );
    assert_eq!(h.quits.get(), 0, "it waits for the agent to leave");
    // The agent leaves.
    keeper.link(0).say(true, None);
    pass(cx, 60);
    assert_eq!(h.quits.get(), 1);
    let events = keeper.link(0).events();
    assert!(events.contains(&"close".to_owned()), "{events:?}");
    assert!(!events.contains(&"detach".to_owned()), "{events:?}");
    assert_eq!(
        keeper.ended_all.load(Ordering::SeqCst),
        1,
        "the keeper was told to hang up whatever it holds, and answered"
    );
    // Nothing is left to attach to at the next start.
    let state = h.store.load_workspace(Slot::Current).unwrap().unwrap();
    assert!(state.terminals.iter().all(|t| t.keeper.is_none()));
}

#[gpui_kit::test]
fn the_quit_question_says_that_held_sessions_keep_running(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let json = r#"{"durable_sessions": true, "quit_confirmation": "always", "sidebar_show_inactive": true}"#;
    let h = open_keeper(cx, &settings, json, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    if !possible() {
        return;
    }
    keeper.link(0).say(false, Some("claude"));
    h.press_chord("cmd-q", "ctrl-shift-q", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette, "it asks");
    let rows = h.palette_titles(cx);
    assert!(
        rows.iter().any(|row| row.contains("keeps running")),
        "the question says what happens to the session: {rows:?}"
    );
}

// ----- a keeper that dies, and the quit with files to save -------------------------------------------

#[gpui_kit::test]
fn a_keeper_that_died_is_remembered_so_the_next_start_can_say_so(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    let reference = session_of(&h, cx, 1, |s, _| keeper_of(s)).unwrap();
    // The pump found the keeper gone: the terminal ends, marked.
    keeper.feeds.lock().unwrap()[0].exit(1, Some(crate::durable::KEEPER_LOST.to_owned()));
    cx.run_until_parked();
    assert!(terminal_of_id(&h, cx, 1).unwrap().exit_info().is_some());
    // What is remembered still names it, with the keeper's reference.
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals.len(), 1);
    assert_eq!(state.terminals[0].keeper, Some(reference.clone()));
    // A terminal whose program simply ended is not remembered.
    start_fresh(&h, cx, leon_core::AgentId::CLAUDE, &path);
    keeper.feeds.lock().unwrap()[1].exit(0, None);
    cx.run_until_parked();
    let state = saved_of(&h, cx);
    assert_eq!(state.terminals.len(), 1, "{:?}", state.terminals);
    // The next start finds no keeper: it says so and resumes the usual way.
    let down = Fake::new();
    down.down.store(true, Ordering::SeqCst);
    let json = r#"{"durable_sessions": true, "restore_sessions": "always", "sidebar_show_inactive": true}"#;
    let h2 = restart_with(cx, &settings, json, &down, &state);
    wait_until(&h2, cx, "the session", |h, cx| live_count(h, cx) == 1);
    assert!(
        h2.status().contains("no longer held 1 of the sessions"),
        "{}",
        h2.status()
    );
}

fn open_file(h: &Harness, cx: &mut TestAppContext, typed: &str) {
    h.press_chord("cmd-shift-o", "ctrl-shift-alt-o", cx);
    h.type_text(typed, cx);
    h.press("enter", cx);
    wait_until(h, cx, "the file's tab", |h, cx| {
        h.shell(cx, |shell| shell.focused_file().is_some())
    });
}

#[gpui_kit::test]
fn saving_the_files_first_does_not_turn_quit_and_end_into_a_plain_quit(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    let a = std::path::Path::new(&path).join("a.txt");
    std::fs::write(&a, "a\n").unwrap();
    open_file(&h, cx, "a.txt");
    h.type_text("1", cx);
    keeper.link(0).say(true, None);
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">end every", cx);
    assert!(h
        .palette_titles(cx)
        .contains(&"Quit and end every session".to_owned()));
    h.press("enter", cx);
    // The first answer saves the files, and the quit follows the saves.
    h.press("enter", cx);
    wait_until(&h, cx, "the quit", |h, _| h.quits.get() == 1);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "1a\n");
    let events = keeper.link(0).events();
    assert!(events.contains(&"close".to_owned()), "{events:?}");
    assert!(!events.contains(&"detach".to_owned()), "{events:?}");
    assert_eq!(keeper.ended_all.load(Ordering::SeqCst), 1);
}

#[gpui_kit::test]
fn discarding_the_files_and_ending_every_session_hangs_the_held_terminals_up(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    let a = std::path::Path::new(&path).join("a.txt");
    std::fs::write(&a, "a\n").unwrap();
    open_file(&h, cx, "a.txt");
    h.type_text("1", cx);
    keeper.link(0).say(true, None);
    h.press("ctrl-shift-p", cx);
    h.set_palette_text(">end every", cx);
    h.press("enter", cx);
    h.press("down", cx);
    h.press("enter", cx);
    wait_until(&h, cx, "the quit", |h, _| h.quits.get() == 1);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a\n");
    assert!(keeper.link(0).events().contains(&"close".to_owned()));
    assert_eq!(keeper.ended_all.load(Ordering::SeqCst), 1);
}

// ----- the layout in one pass, other windows, and a keeper nobody asked for ---------------------------

#[gpui_kit::test]
fn a_tab_with_a_survivor_and_a_gone_terminal_is_restored_in_one_piece(cx: &mut TestAppContext) {
    use crate::ui::panes::Layout;
    use leon_core::{SavedLayout, SavedTab};
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    keeper.held.lock().unwrap().push(held(5, "t1", &cwd));
    // One tab, two panes side by side: 1 is still running under the keeper, 2
    // is gone (it never was held); the second pane has the keyboard.
    let mut state = one_tab_each(&cwd, "claude", Some("sid-7"), &[1, 2]);
    state.terminals[0].keeper = Some(KeeperRef {
        pty: 5,
        token: "t1".into(),
    });
    state.terminals[1].session = Some("sid-8".into());
    state.main = Some(2);
    state.workspaces[0].tabs = vec![SavedTab {
        layout: SavedLayout::Split {
            axis: "row".into(),
            ratio: 0.3,
            a: Box::new(SavedLayout::Leaf(1)),
            b: Box::new(SavedLayout::Leaf(2)),
        },
        focus: 2,
        zoomed: false,
    }];
    let h = restart_with(cx, &settings, ON, &keeper, &state);
    if !possible() {
        return;
    }
    wait_until(&h, cx, "both panes", |h, cx| live_count(h, cx) == 2);
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::None,
        "nothing is asked"
    );
    assert_eq!(*keeper.attached.lock().unwrap(), [5]);
    let (workspaces, layout, focus) = h.shell(cx, |s| {
        let tab = s.workspaces.tab_of(LiveId(1)).expect("a tab");
        (s.workspaces.all().len(), tab.layout.clone(), tab.focus)
    });
    assert_eq!(workspaces, 1, "one workspace, not one per pass");
    match layout {
        Layout::Split { ratio, .. } => assert!((ratio - 0.3).abs() < 1e-6),
        other => panic!("the split was lost: {other:?}"),
    }
    assert_eq!(layout_leaves(&h, cx), [LiveId(1), LiveId(2)]);
    assert_eq!(focus, LiveId(2), "the keyboard is where it was");
    // The survivor attached; the gone one was resumed in its own pane.
    assert_eq!(keeper_of_id(&h, cx, 1).map(|k| k.pty), Some(5));
    assert_eq!(
        session_of(&h, cx, 2, |s, _| s.learned.clone()).map(|l| l.0),
        Some("sid-8".to_owned())
    );
}

fn layout_leaves(h: &Harness, cx: &mut TestAppContext) -> Vec<LiveId> {
    h.shell(cx, |s| {
        s.workspaces.tab_of(LiveId(1)).unwrap().layout.leaves()
    })
}

fn keeper_of_id(h: &Harness, cx: &mut TestAppContext, id: u64) -> Option<KeeperRef> {
    session_of(h, cx, id, |s, _| keeper_of(s))
}

#[gpui_kit::test]
fn a_session_another_window_is_attached_to_is_left_to_it(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    let mut busy = held(5, "t1", &cwd);
    busy.attached = 1;
    keeper.held.lock().unwrap().push(busy);
    let state = saved_held(&cwd, 5, "t1");
    let json = r#"{"durable_sessions": true, "restore_sessions": "always", "sidebar_show_inactive": true}"#;
    let h = restart_with(cx, &settings, json, &keeper, &state);
    if !possible() {
        return;
    }
    cx.run_until_parked();
    assert_eq!(
        live_count(&h, cx),
        0,
        "not attached, and not resumed beside it"
    );
    assert!(keeper.attached.lock().unwrap().is_empty());
    assert!(keeper.opened.lock().unwrap().is_empty());
    assert!(h.status().contains("another Leon window"), "{}", h.status());
}

#[gpui_kit::test]
fn a_keeper_found_with_the_setting_off_is_not_left_stranded(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let cwd = real_cwd(&settings);
    let keeper = Fake::new();
    keeper.held.lock().unwrap().push(held(9, "late", &cwd));
    let h = open_keeper(cx, &settings, OFF, &keeper, None);
    if !possible() {
        return;
    }
    assert_eq!(live_count(&h, cx), 1, "its running terminal is added");
    assert_eq!(*keeper.attached.lock().unwrap(), [9]);
    assert!(
        h.status().contains("is off") && h.status().contains("did not know"),
        "{}",
        h.status()
    );
}

#[gpui_kit::test]
fn a_new_terminal_is_there_at_once_and_remembered_when_the_keeper_has_opened_it(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    keeper.slow.store(true, Ordering::SeqCst);
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    assert_eq!(live_count(&h, cx), 1, "the terminal is on screen already");
    assert!(held_terminal(&h, cx, 1));
    // The keeper has not answered: it is held, and not yet remembered.
    assert_eq!(keeper_of_id(&h, cx, 1), None);
    assert_eq!(saved_of(&h, cx).terminals[0].keeper, None);
    // It answers.
    let slot = keeper.pending.lock().unwrap()[0].clone();
    slot.set(KeeperRef {
        pty: 77,
        token: "tok".into(),
    });
    keeper.feeds.lock().unwrap()[0].changed();
    cx.run_until_parked();
    assert_eq!(
        saved_of(&h, cx).terminals[0].keeper,
        Some(KeeperRef {
            pty: 77,
            token: "tok".into()
        })
    );
}

#[gpui_kit::test]
fn a_quit_the_keeper_does_not_confirm_says_so(cx: &mut TestAppContext) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    if !possible() {
        return;
    }
    keeper.link(0).say(true, None);
    keeper.unconfirmed.store(true, Ordering::SeqCst);
    // Closing a session and quitting at once: the hang-up must have reached
    // the keeper, and when that cannot be shown the window does not pretend.
    h.press_chord("cmd-w", "ctrl-shift-w", cx);
    quit(&h, cx);
    assert_eq!(h.quits.get(), 1, "the quit is not held hostage");
    assert!(keeper.settled.load(Ordering::SeqCst) >= 1);
    assert!(h.status().contains("did not confirm"), "{}", h.status());
}

#[gpui_kit::test]
fn a_keeper_folder_nobody_can_vouch_for_leaves_the_session_in_the_window(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let settings = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let base = tempfile::tempdir().unwrap();
    let real = base.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    // The place the socket would be is a link to a folder: whoever made it may
    // be listening at the other end.
    let link = base.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let paths = leon_host::keeper::KeeperPaths {
        socket: link.join("k.sock"),
        lock: link.join("k.lock"),
        dir: link,
    };
    let local: Arc<dyn Durable> = Arc::new(crate::durable::LocalKeeper::new(
        paths,
        base.path().join("no-such-leon"),
        base.path().join("data"),
        runtime.handle().clone(),
    ));
    cx.executor().allow_parking();
    let file = settings_with(&settings, ON);
    super::DURABLE.with(|slot| *slot.borrow_mut() = Some(local));
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        Some(file),
        Picked::Cancelled,
        |_| {},
    );
    let (_dir, _) = super::live::real_worktree(&h, cx);
    h.press("ctrl-t", cx);
    assert!(h.computer.script(0).is_some(), "a terminal of the window");
    assert!(!held_terminal(&h, cx, 1));
    if possible() {
        let status = h.status();
        assert!(status.contains("cannot be trusted"), "{status}");
    }
}

#[gpui_kit::test]
fn a_held_agent_is_not_given_the_less_sure_match_while_its_pid_is_still_unknown(
    cx: &mut TestAppContext,
) {
    let settings = tempfile::tempdir().unwrap();
    let keeper = Fake::new();
    let h = open_keeper(cx, &settings, ON, &keeper, None);
    let (_dir, path) = super::live::real_worktree(&h, cx);
    start_fresh(&h, cx, leon_core::AgentId::CODEX, &path);
    if !possible() {
        return;
    }
    // The agent wrote its session before the keeper answered who is in front
    // (no pid yet): the session is looked at again, not guessed from its
    // folder now.
    new_session(&h, leon_core::AgentId::CODEX, "codex-new", &path, 6);
    h.settle(cx);
    assert_eq!(session_of(&h, cx, 1, |s, _| s.learned.clone()), None);
    // The first answer arrives; the next pass learns it.
    keeper.link(0).say(false, Some("codex"));
    cx.update(|cx| h.shell.update(cx, |s, cx| s.learn_session_ids(cx)));
    cx.run_until_parked();
    assert_eq!(
        session_of(&h, cx, 1, |s, _| s.learned.clone()).map(|l| l.0),
        Some("codex-new".to_owned())
    );
}
