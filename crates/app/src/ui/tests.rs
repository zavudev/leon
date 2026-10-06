//! UI tests: the real shell in a headless window, driven by key presses, over
//! an in-memory store and a scripted runner. Nothing here touches the home
//! directory, the network or `ssh`.
//!
//! Terminals are the scripted backend of `leon-term` (no process) in every
//! test but the handful in `live::real_pty`, which run a real shell in a real
//! PTY with a fixed environment.

use super::activity::Thresholds;
use super::palette::{Item, Look, Scope};
use super::shell::{Main, Options, Overlay, Pane, Picked, Shell};
use super::steps::StepKind;
use super::terminals::Readiness;
use super::tree::{self, Kind, NodeId};
use crate::engine::{Engine, MachineState, StatusKind};
use crate::keys::Command;
use crate::launch::System;
use crate::settings::{self, AppearanceChoice};
use crate::theme::Appearance;
use chrono::{Duration as ChronoDuration, TimeZone, Utc};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    px, size, AppContext as _, Bounds, Entity, Point, Task, TestAppContext, VisualTestContext,
    WindowBounds, WindowHandle, WindowOptions,
};
use leon_core::{
    AgentId, Machine, MachineId, MachineKind, NewMessage, NewSession, NewWorktree, Project, Role,
    Store,
};
use leon_history::HistoryRoots;
use leon_remote::{Output, ScriptedRunner, SshOptions};
use leon_term::{Script, Scripted};
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;

const API_ROOT: &str = "/srv/api";

const API_LISTING: &str = "\
worktree /srv/api
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /srv/api-worktrees/feature-login
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature/login
";

const MAIN_ONLY: &str =
    "worktree /srv/api\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n";

const PROBE_OUTPUT: &str = "os=Linux\narch=x86_64\nhome=/home/dev\ntool=git=/usr/bin/git\ntool=claude=/home/dev/.local/bin/claude\n";

type Window = WindowHandle<gpui_kit::base::Root>;

/// What `main` does before the first window: the component library and the
/// settings, kept in memory unless a test gives them a file.
fn prepare(cx: &mut gpui_kit::App, settings_file: Option<std::path::PathBuf>) {
    gpui_kit::init(cx);
    settings::init(
        settings_file,
        settings::Overrides {
            appearance: Some(AppearanceChoice::Dark),
            theme: None,
        },
        cx,
    );
}

/// A key as this platform has it. The tests write the secondary key as
/// `ctrl`, which is what it is on Linux and Windows; on macOS it is Cmd.
fn platform_key(key: &str) -> String {
    if crate::platform::is_mac() {
        key.replacen("ctrl-", "cmd-", 1)
    } else {
        key.to_owned()
    }
}

/// This computer, as the tests see it. Its login shell is a real `sh`, only
/// used by the few tests that run a real PTY (the `real_pty` module); every
/// other test gets terminals from the scripted [`computer`] and never starts
/// one.
///
/// The shell is started interactively with a startup file of its own (the
/// fixture below, standing in for `~/.profile`), a known prompt, no history
/// file and a fixed `PATH`, so nothing depends on the developer's shell, rc
/// files, home directory or `PATH`. The agents are shell functions of that
/// file that run a small `sh -c` program: no executable is written to disk
/// (a freshly written program costs a third of a second to start on macOS,
/// which scans it) and the agent still runs as a process of its own in front
/// of the shell, as in life.
struct FakeSystem {
    dir: tempfile::TempDir,
    /// Programs this computer does not have.
    missing: Vec<&'static str>,
}

/// The shell's startup file: a prompt, a banner, and the three agents.
const FAKE_SHELL_RC: &str = r#"PS1='READY> '
printf 'FAKE-SHELL in %s\n' "$(pwd -P)"
# Raw input, shown with `cat -vt`: ^C, ^[ and ^M are what arrived.
claude() { /bin/sh -c 'stty -echo -icanon -isig -icrnl; printf "FAKE-CLAUDE %s\n" "$*"; exec cat -vt' claude "$@"; }
opencode() { printf 'OPENCODE-RAN\n'; }
# Reads one line in the normal way: Enter ends it.
codex() { /bin/sh -c 'printf "FAKE-CODEX\n"; read answer'; }
"#;

impl FakeSystem {
    fn new() -> Rc<Self> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rc"), FAKE_SHELL_RC).unwrap();
        Rc::new(Self {
            dir,
            missing: Vec::new(),
        })
    }

    /// The same computer without these programs.
    fn without(missing: &[&'static str]) -> Rc<Self> {
        let mut computer = Rc::try_unwrap(Self::new()).ok().expect("just made");
        computer.missing = missing.to_vec();
        Rc::new(computer)
    }
}

impl System for FakeSystem {
    fn find_program(&self, name: &str) -> Option<std::path::PathBuf> {
        (matches!(name, "claude" | "codex" | "opencode") && !self.missing.contains(&name))
            .then(|| self.dir.path().join(name))
    }
    fn login_shell(&self) -> (String, Vec<String>) {
        let rc = self.dir.path().join("rc");
        (
            "/bin/sh".to_owned(),
            vec![
                "-c".to_owned(),
                format!(
                    "ENV='{}' HISTFILE=/dev/null PATH=/usr/bin:/bin exec /bin/sh -i",
                    rc.display()
                ),
            ],
        )
    }
    fn dir_exists(&self, path: &str) -> bool {
        std::path::Path::new(path).is_dir()
    }
}

/// A computer that lives in memory: the shell and the agents of
/// [`FakeSystem`] as a script of what they print and do, behind the scripted
/// terminal backend, so a session starts no process and waits on no clock.
///
/// The shell prints a banner and a `READY> ` prompt and echoes what it is
/// typed; a line it is given runs one of: `claude` (raw mode: shows each key
/// as `cat -vt` would), `codex` (reads one line, Enter ends it), `opencode`
/// (prints a line and returns), `pwd -P`, `exit N`.
fn computer() -> Scripted {
    use std::sync::Mutex;

    enum Mode {
        Shell(String),
        Claude,
        Codex,
    }
    const PROMPT: &str = "READY> ";

    /// The words of a line, with single quotes taken off as a shell does.
    fn words(line: &str) -> Vec<String> {
        let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
        for c in line.chars() {
            match c {
                '\'' => {
                    quoted = !quoted;
                    any = true;
                }
                c if c.is_whitespace() && !quoted => {
                    if any {
                        words.push(std::mem::take(&mut word));
                        any = false;
                    }
                }
                c => {
                    word.push(c);
                    any = true;
                }
            }
        }
        if any {
            words.push(word);
        }
        words
    }

    fn run(script: &Script, line: &str, cwd: &str) -> Mode {
        let words = words(line);
        match words.first().map(String::as_str) {
            Some("claude") => {
                script.print(format!("FAKE-CLAUDE {}\n", words[1..].join(" ")));
                script.set_foreground(false);
                return Mode::Claude;
            }
            Some("codex") => {
                script.print("FAKE-CODEX\n");
                script.set_foreground(false);
                return Mode::Codex;
            }
            Some("opencode") => script.print(format!("OPENCODE-RAN\n{PROMPT}")),
            Some("pwd") => script.print(format!("{cwd}\n{PROMPT}")),
            Some("exit") => {
                script.exit(words.get(1).and_then(|n| n.parse().ok()).unwrap_or(0));
            }
            Some(other) => script.print(format!("sh: {other}: not found\n{PROMPT}")),
            None => script.print(PROMPT),
        }
        Mode::Shell(String::new())
    }

    Scripted::with_setup(|script| {
        let cwd = script.spec().cwd.clone().unwrap_or_default();
        script.print(format!("FAKE-SHELL in {cwd}\n{PROMPT}"));
        let mode = Mutex::new(Mode::Shell(String::new()));
        script.on_input(move |script, bytes| {
            let mut mode = mode.lock().unwrap();
            for c in String::from_utf8_lossy(bytes).chars() {
                let enter = c == '\r' || c == '\n';
                match &mut *mode {
                    Mode::Shell(line) if enter => {
                        let line = std::mem::take(line);
                        script.print("\n");
                        *mode = run(script, &line, &cwd);
                    }
                    Mode::Shell(line) => {
                        line.push(c);
                        script.print(c.to_string());
                    }
                    Mode::Claude if c.is_control() => {
                        let shown = match c {
                            '\x7f' => "^?".to_owned(),
                            c => format!("^{}", char::from(c as u8 + 64)),
                        };
                        script.print(shown);
                    }
                    Mode::Claude => script.print(c.to_string()),
                    Mode::Codex if enter => {
                        script.set_foreground(true);
                        script.print(format!("\n{PROMPT}"));
                        *mode = Mode::Shell(String::new());
                    }
                    Mode::Codex => {}
                }
            }
        });
    })
}

/// When a new shell is ready for an agent's command line in the tests: at its
/// first output, looked for every millisecond.
const READY_NOW: Readiness = Readiness {
    quiet: std::time::Duration::ZERO,
    timeout: std::time::Duration::from_secs(5),
    poll: std::time::Duration::from_millis(1),
};

// Some fields are read only by the Unix-only test modules.
#[cfg_attr(not(leon_posix_tests), allow(dead_code))]
struct Harness {
    // Dropped last: the engine's background calls run here.
    runtime: Runtime,
    engine: Engine,
    runner: Arc<ScriptedRunner>,
    store: Arc<Store>,
    window: Window,
    shell: Entity<Shell>,
    /// The folders the file manager was asked to show.
    revealed: Rc<std::cell::RefCell<Vec<std::path::PathBuf>>>,
    /// The application bundles brought forward.
    revealed_apps: Rc<std::cell::RefCell<Vec<String>>>,
    /// The files the system's editor was asked to open.
    opened: Rc<std::cell::RefCell<Vec<std::path::PathBuf>>>,
    /// The desktop notifications the shell asked for.
    notes: Rc<std::cell::RefCell<Vec<crate::ui::notify::Note>>>,
    /// How many times the system's folder dialog was asked for.
    folder_dialogs: Rc<std::cell::Cell<usize>>,
    /// The terminals the shell started, and what they were sent.
    computer: Rc<Scripted>,
    /// The user's `~/.ssh`, in memory.
    ssh: Arc<tests_connect::FakeSsh>,
    /// What the key file dialog answers.
    key_answer: Rc<std::cell::RefCell<Picked>>,
    /// The addresses the browser was asked to open.
    urls: Rc<std::cell::RefCell<Vec<String>>>,
    /// How many times the application was asked to end.
    quits: Rc<std::cell::Cell<usize>>,
}

/// Opens the window over an in-memory store, with nothing waiting on the
/// clock.
fn open(cx: &mut TestAppContext, runner: ScriptedRunner) -> Harness {
    open_with(cx, runner, None)
}

fn open_with(
    cx: &mut TestAppContext,
    runner: ScriptedRunner,
    settings_file: Option<std::path::PathBuf>,
) -> Harness {
    open_full(cx, runner, settings_file, Picked::Cancelled, |_| {})
}

/// The clock of every test: five minutes after the newest seeded session
/// would be, on the day the seeds are dated.
fn fixed_now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 4, 12, 5, 0).unwrap()
}

/// Opens the window; `picked` is what the folder picker answers and `before`
/// changes the store before the window reads it.
fn open_full(
    cx: &mut TestAppContext,
    runner: ScriptedRunner,
    settings_file: Option<std::path::PathBuf>,
    picked: Picked,
    before: impl FnOnce(&Store),
) -> Harness {
    open_core(cx, runner, settings_file, picked, before, |_| None)
}

/// [`open_full`] with an updater: `updates` is given the harness's runtime.
fn open_core(
    cx: &mut TestAppContext,
    runner: ScriptedRunner,
    settings_file: Option<std::path::PathBuf>,
    picked: Picked,
    before: impl FnOnce(&Store),
    updates: impl FnOnce(&tokio::runtime::Handle) -> Option<Arc<crate::updates::Service>>,
) -> Harness {
    cx.update(|cx| prepare(cx, settings_file));
    // A current-thread runtime only makes progress inside `block_on`, on this
    // thread. The test therefore decides when background work runs, which
    // GPUI's deterministic test scheduler requires.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = Store::open_in_memory().unwrap();
    seed(&store);
    before(&store);
    let runner = Arc::new(runner);
    let engine = Engine::new(
        store.clone(),
        runner.clone(),
        SshOptions::without_multiplexing(),
        HistoryRoots::default(),
        runtime.handle().clone(),
    );
    let revealed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let revealed_in = revealed.clone();
    let revealed_apps = Rc::new(std::cell::RefCell::new(Vec::new()));
    let revealed_apps_in = revealed_apps.clone();
    let dialogs = Rc::new(std::cell::Cell::new(0usize));
    let dialogs_in = dialogs.clone();
    let computer = Rc::new(computer());
    let opened = Rc::new(std::cell::RefCell::new(Vec::new()));
    let opened_in = opened.clone();
    let notes = Rc::new(std::cell::RefCell::new(Vec::new()));
    let notes_in = notes.clone();
    let ssh = Arc::new(tests_connect::FakeSsh::default());
    let key_answer = Rc::new(std::cell::RefCell::new(Picked::Cancelled));
    let key_answer_in = key_answer.clone();
    let urls = Rc::new(std::cell::RefCell::new(Vec::new()));
    let urls_in = urls.clone();
    let quits = Rc::new(std::cell::Cell::new(0usize));
    let quits_in = quits.clone();
    let updates = updates(runtime.handle());
    let options = Options {
        backend: computer.clone(),
        ready: READY_NOW,
        reload_throttle: std::time::Duration::ZERO,
        search_debounce: std::time::Duration::ZERO,
        now: fixed_now,
        pick_folder: Rc::new(move |_| {
            dialogs_in.set(dialogs_in.get() + 1);
            Task::ready(picked.clone())
        }),
        system: FakeSystem::new(),
        reveal: Rc::new(move |_, path| revealed_in.borrow_mut().push(path.to_path_buf())),
        activity: Thresholds::default(),
        error_flash: std::time::Duration::from_secs(4),
        notify: Rc::new(move |note, _| notes_in.borrow_mut().push(note.clone())),
        banner_duration: std::time::Duration::from_secs(30),
        save_debounce: std::time::Duration::ZERO,
        import_debounce: std::time::Duration::ZERO,
        import_interval: std::time::Duration::ZERO,
        quit_gesture_wait: std::time::Duration::from_millis(200),
        quit_grace: std::time::Duration::from_millis(500),
        pick_image: Rc::new(|_| Task::ready(Picked::Cancelled)),
        save_file: Rc::new(|_, _| Task::ready(Picked::Cancelled)),
        read_clipboard: Rc::new(|_| None),
        quit: Rc::new(move |_| quits_in.set(quits_in.get() + 1)),
        theme_poll: None,
        elsewhere_poll: None,
        usage_timer: false,
        reveal_app: Rc::new(move |_, bundle| revealed_apps_in.borrow_mut().push(bundle.to_owned())),
        open_file: Rc::new(move |_, path| opened_in.borrow_mut().push(path.to_path_buf())),
        pick_key: Rc::new(move |_| Task::ready(key_answer_in.borrow().clone())),
        ssh_dir: ssh.clone(),
        remote: None,
        updates,
        update_timer: false,
        open_url: Rc::new(move |_, url| urls_in.borrow_mut().push(url.to_owned())),
    };
    let (window, shell) = cx.update(|cx| {
        let engine = engine.clone();
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1240.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Shell::new(engine, options, window, cx)),
        )
        .unwrap()
    });
    cx.run_until_parked();
    Harness {
        runtime,
        engine,
        runner,
        store,
        window: window.downcast().expect("the window has a base root"),
        shell,
        revealed,
        revealed_apps,
        opened,
        notes,
        folder_dialogs: dialogs,
        computer,
        ssh,
        key_answer,
        urls,
        quits,
    }
}

impl Harness {
    /// Lets the engine's background tasks run to completion, then lets the UI
    /// react to what they stored.
    fn settle(&self, cx: &mut TestAppContext) {
        self.drain();
        cx.run_until_parked();
        // The UI may have asked for more work while reacting.
        self.drain();
        cx.run_until_parked();
    }

    /// Runs the engine's tasks until they have nothing left to do. The jobs
    /// on the blocking pool (the history import, reading a chosen image) run
    /// on threads of their own, so yielding alone is a race with them that a
    /// slow machine loses: wait for them, up to a generous limit.
    fn drain(&self) {
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(30);
        self.runtime.block_on(async {
            loop {
                for _ in 0..64 {
                    tokio::task::yield_now().await;
                }
                if std::time::Instant::now() > limit {
                    break;
                }
                if self.engine.blocking_jobs() > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    continue;
                }
                // A job that just ended hands its result to a task that runs
                // now, and that task may start the next job: look once more.
                for _ in 0..64 {
                    tokio::task::yield_now().await;
                }
                if self.engine.blocking_jobs() == 0 {
                    break;
                }
            }
        });
    }

    fn press(&self, key: &str, cx: &mut TestAppContext) {
        let key = platform_key(key);
        cx.update_window(self.window.into(), |_, window, cx| window.press(&key, cx))
            .unwrap();
        self.settle(cx);
    }

    /// A chord that is not the same on every platform: `mac` on macOS, `other`
    /// elsewhere, written as GPUI writes keystrokes.
    fn press_chord(&self, mac: &str, other: &str, cx: &mut TestAppContext) {
        let key = if crate::platform::is_mac() {
            mac
        } else {
            other
        };
        cx.update_window(self.window.into(), |_, window, cx| window.press(key, cx))
            .unwrap();
        self.settle(cx);
    }

    /// Presses a mouse button on the middle of a drawn element and lets go.
    fn mouse_on(&self, selector: String, button: gpui_kit::MouseButton, cx: &mut TestAppContext) {
        let bounds = self
            .bounds_of(selector.clone(), cx)
            .unwrap_or_else(|| panic!("{selector} is not drawn"));
        let mut visual = VisualTestContext::from_window(self.window.into(), cx);
        let at = bounds.center();
        visual.simulate_mouse_down(at, button, gpui_kit::Modifiers::none());
        visual.simulate_mouse_up(at, button, gpui_kit::Modifiers::none());
        visual.run_until_parked();
        self.settle(cx);
    }

    /// Replaces the whole text of the palette's field in one step, as a paste
    /// does: one change of the field instead of one per key. Typing costs a
    /// few milliseconds a character (the input lays its text out and the
    /// window is read again for each), which a test that goes through every
    /// entry of a list cannot afford; the tests of typing itself use
    /// [`Harness::type_text`].
    fn set_palette_text(&self, text: &str, cx: &mut TestAppContext) {
        let input = self.shell(cx, |shell| shell.palette.input.clone());
        cx.update_window(self.window.into(), |_, window, cx| {
            input.update(cx, |field, cx| field.set_value(text.to_owned(), window, cx))
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| self.shell.update(cx, |shell, cx| shell.palette_changed(cx)));
        self.settle(cx);
    }

    fn type_text(&self, text: &str, cx: &mut TestAppContext) {
        cx.update_window(self.window.into(), |_, window, cx| window.input(text, cx))
            .unwrap();
        self.settle(cx);
    }

    fn shows(&self, selector: &'static str, cx: &mut TestAppContext) -> bool {
        VisualTestContext::from_window(self.window.into(), cx)
            .debug_bounds(selector)
            .is_some()
    }

    fn shows_dynamic(&self, selector: String, cx: &mut TestAppContext) -> bool {
        self.bounds_of(selector, cx).is_some()
    }

    fn shell<R>(&self, cx: &mut TestAppContext, read: impl FnOnce(&Shell) -> R) -> R {
        cx.update(|cx| read(self.shell.read(cx)))
    }

    fn status(&self) -> String {
        self.engine
            .status()
            .map(|line| line.text)
            .unwrap_or_default()
    }

    /// The titles of the palette's rows, headings written as `[Name]`.
    fn palette_titles(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.shell(cx, |shell| {
            shell
                .palette
                .items
                .iter()
                .map(|item| match item {
                    Item::Section(title) => format!("[{title}]"),
                    Item::Machine(machine) => machine.name.clone(),
                    Item::Project(project) => project.name.clone(),
                    Item::Worktree(worktree, project) => format!(
                        "{} / {}",
                        project.name,
                        worktree.branch.clone().unwrap_or_default()
                    ),
                    Item::Session(session) => session.title.clone(),
                    Item::Hit(hit) => format!("hit:{}", hit.session.title),
                    Item::Command(command) => crate::keys::label(*command).to_owned(),
                    Item::Look(look) => look.label(),
                    Item::Setting(def) => format!("setting:{}", def.key),
                    Item::Choice(place) => {
                        match shell.palette.flow.as_ref().map(|f| &f.step.kind) {
                            Some(StepKind::Choices { choices, .. }) => {
                                choices[*place].label.clone()
                            }
                            _ => String::new(),
                        }
                    }
                    Item::Custom(text) => format!("custom:{text}"),
                    Item::Mode(place) => super::palette::MODES[*place].0.to_owned(),
                    Item::Line(name, _) => format!("line:{name}"),
                })
                .collect()
        })
    }

    /// The row of the tree the cursor is on, as a short description.
    fn cursor_row(&self, cx: &mut TestAppContext) -> String {
        self.shell(cx, |shell| {
            shell
                .cursor
                .and_then(|index| shell.rows.get(index))
                .map_or_else(|| "none".to_owned(), describe)
        })
    }

    /// Every row of the tree, indented by depth.
    fn outline(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.shell(cx, |shell| {
            shell
                .rows
                .iter()
                .map(|row| format!("{}{}", "  ".repeat(usize::from(row.depth)), describe(row)))
                .collect()
        })
    }

    /// The name of the machine the keyboard is on.
    fn machine_name(&self, cx: &mut TestAppContext) -> String {
        self.shell(cx, |shell| {
            let id = shell.current_machine();
            shell
                .snapshot
                .machine(&id)
                .map(|machine| machine.name.clone())
                .unwrap_or_default()
        })
    }

    /// Where the row of this node is in the tree, if it is shown.
    fn row_of(&self, node: NodeId, cx: &mut TestAppContext) -> Option<usize> {
        self.shell(cx, |shell| shell.rows.iter().position(|row| row.id == node))
    }

    /// The row at `index` is drawn: its bounds.
    fn bounds_of(
        &self,
        selector: String,
        cx: &mut TestAppContext,
    ) -> Option<Bounds<gpui_kit::Pixels>> {
        VisualTestContext::from_window(self.window.into(), cx)
            .debug_bounds(Box::leak(selector.into_boxed_str()))
    }

    fn main_kind(&self, cx: &mut TestAppContext) -> String {
        self.shell(cx, |shell| match &shell.main {
            Main::Empty => "empty".to_owned(),
            Main::Project(id) => format!(
                "project:{}",
                shell
                    .snapshot
                    .project(id)
                    .map_or("?", |e| e.project.name.as_str())
            ),
            Main::Worktree(project, worktree) => {
                let branch = shell
                    .snapshot
                    .project(project)
                    .and_then(|e| e.worktrees.iter().find(|w| &w.id == worktree))
                    .and_then(|w| w.branch.clone())
                    .unwrap_or_default();
                format!("worktree:{branch}")
            }
            Main::Session(transcript) => format!("session:{}", transcript.session.title),
            Main::Live(id) => format!("live:{id}"),
        })
    }
}

/// A row as a short description.
fn describe(row: &tree::Row) -> String {
    match &row.kind {
        Kind::Machine(machine) => format!("machine:{}", machine.name),
        Kind::Project { project, .. } => format!("project:{}", project.name),
        Kind::Worktree { worktree, .. } => format!("worktree:{}", tree::worktree_label(worktree)),
        Kind::Session(session) => format!("session:{}", session.title),
        Kind::Live(entry) => format!("live:{}", entry.id),
        Kind::Unsorted { .. } => "unsorted".to_owned(),
        Kind::Folder { cwd, .. } => format!("folder:{cwd}"),
        Kind::More { hidden } => format!("more:{hidden}"),
        Kind::Open => "open".to_owned(),
        Kind::NoMatch => "no-match".to_owned(),
    }
}

/// The id of the worktree of a local project with this label.
fn worktree_id(h: &Harness, label: &str) -> leon_core::WorktreeId {
    h.store
        .all_worktrees()
        .unwrap()
        .into_iter()
        .find(|worktree| tree::worktree_label(worktree) == label)
        .expect("a worktree with that name")
        .id
}

/// Opens the session with this title, as a click on its row would.
fn open_session_titled(h: &Harness, title: &str, cx: &mut TestAppContext) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, cx| {
            let session = shell
                .snapshot
                .sessions
                .iter()
                .find(|session| session.title == title)
                .cloned()
                .expect("the session is in the list");
            shell.open_session(session, None, cx);
        })
    });
    h.settle(cx);
}

fn session_at(
    store: &Store,
    machine: &MachineId,
    cwd: &str,
    title: &str,
    minutes_ago: i64,
    messages: &[(Role, &str)],
) {
    let now = Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap();
    let at = now - ChronoDuration::minutes(minutes_ago);
    let messages: Vec<NewMessage> = messages
        .iter()
        .enumerate()
        .map(|(index, (role, text))| NewMessage {
            role: *role,
            text: (*text).to_owned(),
            at: at + ChronoDuration::seconds(index as i64),
        })
        .collect();
    store
        .upsert_session(
            &NewSession {
                agent: AgentId::CLAUDE,
                external_id: format!("{title}-{minutes_ago}"),
                machine_id: machine.clone(),
                cwd: cwd.to_owned(),
                title: title.to_owned(),
                model: None,
                started_at: at,
                updated_at: at,
            },
            &messages,
        )
        .unwrap();
}

fn ssh_kind() -> MachineKind {
    MachineKind::Ssh {
        host: "build.example".into(),
        user: Some("dev".into()),
        port: None,
        identity_file: None,
    }
}

/// The world every test starts from: a local project with a linked worktree,
/// a second local project, a remote machine with a project of its own, and
/// three sessions.
fn seed(store: &Store) -> (Project, Project, Machine) {
    let api = store
        .add_project(&MachineId::local(), "api", API_ROOT)
        .unwrap();
    store
        .replace_worktrees(
            &api.id,
            vec![
                NewWorktree {
                    path: API_ROOT.into(),
                    branch: Some("main".into()),
                    head: Some("1111111111111111".into()),
                    is_main: true,
                },
                NewWorktree {
                    path: "/srv/api-worktrees/feature-login".into(),
                    branch: Some("feature/login".into()),
                    head: Some("2222222222222222".into()),
                    is_main: false,
                },
            ],
        )
        .unwrap();
    let web = store
        .add_project(&MachineId::local(), "web", "/srv/web")
        .unwrap();
    store
        .replace_worktrees(
            &web.id,
            vec![NewWorktree {
                path: "/srv/web".into(),
                branch: Some("main".into()),
                head: None,
                is_main: true,
            }],
        )
        .unwrap();
    let remote = store.add_machine("build box", ssh_kind()).unwrap();
    let remote_project = store
        .add_project(&remote.id, "infra", "/opt/infra")
        .unwrap();
    store
        .replace_worktrees(
            &remote_project.id,
            vec![NewWorktree {
                path: "/opt/infra".into(),
                branch: Some("main".into()),
                head: None,
                is_main: true,
            }],
        )
        .unwrap();
    session_at(
        store,
        &MachineId::local(),
        API_ROOT,
        "fix the login bug",
        5,
        &[
            (Role::User, "the login form rejects valid passwords"),
            (Role::Assistant, "I will look at the validation"),
        ],
    );
    session_at(
        store,
        &MachineId::local(),
        "/srv/api-worktrees/feature-login",
        "add oauth",
        60,
        &[(Role::User, "add oauth support")],
    );
    session_at(
        store,
        &remote.id,
        "/opt/infra",
        "rotate the keys",
        120,
        &[(Role::User, "rotate the deploy keys")],
    );
    (api, web, remote)
}

fn many_messages(store: &Store, count: usize, hit_at: usize) {
    let mut texts: Vec<(Role, String)> = (0..count)
        .map(|n| {
            (
                if n % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                format!("message number {n} about nothing in particular"),
            )
        })
        .collect();
    texts[hit_at].1 = "we should migrate the users table before friday".to_owned();
    let borrowed: Vec<(Role, &str)> = texts.iter().map(|(r, t)| (*r, t.as_str())).collect();
    session_at(
        store,
        &MachineId::local(),
        API_ROOT,
        "database planning",
        1,
        &borrowed,
    );
}

// ----- the window --------------------------------------------------------------

#[gpui_kit::test]
fn the_window_opens_on_one_tree_of_machines_projects_worktrees_and_sessions(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    assert_eq!(
        h.outline(cx),
        [
            "machine:This machine",
            "  project:api",
            "    worktree:main",
            "      session:fix the login bug",
            "    worktree:feature/login",
            "      session:add oauth",
            "  project:web",
            "    worktree:main",
            "machine:build box",
            "  project:infra",
            "    worktree:main",
            "      session:rotate the keys",
        ]
    );
    h.shell(cx, |shell| {
        assert!(shell.snapshot.machines[0].id.is_local(), "Local is first");
        assert_eq!(shell.pane, Pane::Sidebar);
    });
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    assert!(h.shows("sidebar-header", cx) && h.shows("sidebar-title", cx));
    assert!(h.shows("tree-row-0", cx) && h.shows("tree-row-11", cx));
    assert!(h.shows("main-empty", cx), "nothing is open yet");
    assert!(
        !h.shows("terminal-placeholder", cx),
        "the placeholder strip is gone: terminals fill the pane"
    );
}

#[gpui_kit::test]
fn the_sidebar_has_the_three_tools_at_its_foot_and_no_rail_is_left(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    for selector in [
        "tool-theme",
        "tool-shortcuts",
        "tool-settings",
        "sidebar-tools",
    ] {
        assert!(h.shows(selector, cx), "{selector}");
    }
    assert!(!h.shows("rail-machine-0", cx));
    // The local machine is online, the remote one has not been probed.
    assert_eq!(
        h.engine.machine_state(&MachineId::local()),
        MachineState::Online(None)
    );
}

#[gpui_kit::test]
fn the_sidebar_header_lines_up_with_the_main_header_and_rows_have_one_height(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let side = h.bounds_of("sidebar-header".into(), cx).unwrap();
    let main = h.bounds_of("main-header".into(), cx).unwrap();
    assert_eq!(side.origin.y, main.origin.y);
    assert_eq!(
        side.size.height, main.size.height,
        "one rule across the window"
    );
    let heights: Vec<_> = (0..12)
        .map(|n| {
            h.bounds_of(format!("tree-row-{n}"), cx)
                .unwrap()
                .size
                .height
        })
        .collect();
    assert!(
        heights.iter().all(|height| *height == heights[0]),
        "{heights:?}"
    );
}

#[gpui_kit::test]
fn a_very_long_title_is_cut_inside_its_row_and_keeps_the_age_in_place(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let long = "x".repeat(300);
    session_at(
        &h.store,
        &MachineId::local(),
        API_ROOT,
        &long,
        1,
        &[(Role::User, "hi")],
    );
    h.settle(cx);
    let index = h
        .shell(cx, |s| {
            s.rows.iter().position(
                |row| matches!(&row.kind, Kind::Session(session) if session.title == long),
            )
        })
        .expect("the session is in the tree");
    let row = h.bounds_of(format!("tree-row-{index}"), cx).unwrap();
    let side = h.bounds_of("sidebar-header".into(), cx).unwrap();
    assert!(
        row.size.width <= side.size.width,
        "the row stays inside the sidebar"
    );
}

// ----- the list -----------------------------------------------------------------

#[gpui_kit::test]
fn j_and_k_and_the_arrows_move_the_cursor_down_and_up_the_tree(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    h.press("j", cx);
    assert_eq!(h.cursor_row(cx), "project:api");
    h.press("down", cx);
    assert_eq!(h.cursor_row(cx), "worktree:main");
    h.press("down", cx);
    assert_eq!(h.cursor_row(cx), "session:fix the login bug");
    h.press("k", cx);
    assert_eq!(h.cursor_row(cx), "worktree:main");
    h.press("up", cx);
    h.press("up", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    h.press("up", cx);
    assert_eq!(
        h.cursor_row(cx),
        "machine:This machine",
        "it stops at the top"
    );
}

#[gpui_kit::test]
fn home_and_end_go_to_the_first_and_last_rows_and_page_keys_move_further(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("end", cx);
    assert_eq!(h.cursor_row(cx), "session:rotate the keys");
    h.press("home", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    h.press("pagedown", cx);
    assert_eq!(
        h.cursor_row(cx),
        "machine:build box",
        "a page is eight rows"
    );
    h.press("pagedown", cx);
    assert_eq!(h.cursor_row(cx), "session:rotate the keys");
    h.press("pageup", cx);
    h.press("pageup", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
}

#[gpui_kit::test]
fn enter_toggles_a_project_and_opens_a_worktree_or_a_session_in_the_main_pane(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    h.press("j", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "empty", "a project only folds");
    assert_eq!(
        h.outline(cx)[2],
        "  project:web",
        "its worktrees are folded away"
    );
    h.press("enter", cx);
    assert_eq!(h.outline(cx)[2], "    worktree:main", "and back");
    h.press("j", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "worktree:main");
    assert!(h.shows("worktree-detail", cx));
    h.press("j", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "session:fix the login bug");
    h.settle(cx);
    assert!(h.shows("transcript", cx));
    assert!(h.shows("message-0", cx));
}

#[gpui_kit::test]
fn a_worktree_shows_the_sessions_that_ran_inside_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let linked = h
        .row_of(NodeId::Worktree(worktree_id(&h, "feature/login")), cx)
        .unwrap();
    let main = h
        .row_of(NodeId::Worktree(worktree_id(&h, "main")), cx)
        .unwrap();
    for (index, label) in [(linked, "worktree:feature/login"), (main, "worktree:main")] {
        h.shell.update(cx, |shell, _| shell.move_cursor_to(index));
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), label);
        assert!(
            !h.shows("worktree-no-sessions", cx),
            "{label} has a session"
        );
    }
    // The web project's only worktree has had no session.
    let web = h
        .outline(cx)
        .iter()
        .position(|line| line == "  project:web")
        .unwrap()
        + 1;
    h.shell.update(cx, |shell, _| shell.move_cursor_to(web));
    h.press("enter", cx);
    assert!(h.shows("worktree-no-sessions", cx));
}

#[gpui_kit::test]
fn a_worktree_screen_offers_its_actions_and_opens_a_session_from_its_list(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let linked = h
        .row_of(NodeId::Worktree(worktree_id(&h, "feature/login")), cx)
        .unwrap();
    h.shell.update(cx, |shell, _| shell.move_cursor_to(linked));
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "worktree:feature/login");
    for selector in [
        "worktree-hero",
        "worktree-actions",
        "worktree-new-session",
        "worktree-new-claude",
        "worktree-new-codex",
        "worktree-new-opencode",
        "worktree-shell",
        "worktree-copy-path",
        "worktree-copy-branch",
        "worktree-reveal",
        "worktree-new-worktree",
        "worktree-remove",
        "worktree-sessions",
        "worktree-session-0",
    ] {
        assert!(h.shows(selector, cx), "{selector} is drawn");
    }
    // Copying the path works on the worktree on screen, not on the cursor.
    h.mouse_on("worktree-copy-path".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("/srv/api-worktrees/feature-login")
    );
    // A session of the list opens its stored transcript.
    h.mouse_on("worktree-session-0".into(), gpui_kit::MouseButton::Left, cx);
    assert_eq!(h.main_kind(cx), "session:add oauth");
    assert!(h.shows("transcript", cx));
}

#[gpui_kit::test]
fn the_cursor_stays_on_its_node_when_rows_appear_above_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("end", cx);
    assert_eq!(h.cursor_row(cx), "session:rotate the keys");
    // A new, more recent session appears near the top of the tree.
    session_at(
        &h.store,
        &MachineId::local(),
        API_ROOT,
        "brand new",
        0,
        &[(Role::User, "hello")],
    );
    h.settle(cx);
    assert_eq!(h.cursor_row(cx), "session:rotate the keys");
    assert!(h
        .outline(cx)
        .contains(&"      session:brand new".to_owned()));
}

#[gpui_kit::test]
fn a_session_row_opens_a_transcript_read_from_the_store(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    for _ in 0..3 {
        h.press("j", cx);
    }
    h.press("enter", cx);
    h.settle(cx);
    h.shell(cx, |shell| match &shell.main {
        Main::Session(transcript) => {
            let messages = transcript
                .messages
                .as_ref()
                .expect("the transcript was read");
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].role, Role::User);
        }
        _ => panic!("a session should be open"),
    });
}

#[gpui_kit::test]
fn a_long_transcript_draws_only_the_rows_that_fit(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    many_messages(&h.store, 3000, 10);
    h.settle(cx);
    open_session_titled(&h, "database planning", cx);
    assert_eq!(h.main_kind(cx), "session:database planning");
    let drawn = (0..3000)
        .filter(|n| {
            VisualTestContext::from_window(h.window.into(), cx)
                .debug_bounds(Box::leak(format!("message-{n}").into_boxed_str()))
                .is_some()
        })
        .count();
    assert!(drawn > 0, "some rows are drawn");
    assert!(drawn < 100, "{drawn} rows drawn out of 3000");
}

// ----- panes and machines ----------------------------------------------------------

#[gpui_kit::test]
fn tab_and_shift_tab_move_between_the_sidebar_and_the_main_pane(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    h.press("shift-tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    assert!(
        h.shows("pane-focus", cx),
        "the pane with the keyboard has its rule"
    );
}

#[gpui_kit::test]
fn secondary_l_focuses_the_sidebar_from_the_main_pane(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("tab", cx);
    h.press("ctrl-l", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    h.press("tab", cx);
    h.press("ctrl-l", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    assert!(
        h.shows("tree-cursor", cx),
        "the row the keyboard is on carries the bar"
    );
}

#[gpui_kit::test]
fn secondary_digits_jump_to_a_machines_row_from_anywhere_even_while_typing(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    h.press("tab", cx);
    h.press("ctrl-2", cx);
    assert_eq!(h.cursor_row(cx), "machine:build box");
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    h.press("ctrl-p", cx);
    h.type_text("zzz", cx);
    h.press("ctrl-1", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::None,
        "the palette closed"
    );
    // A number with no machine does nothing.
    h.press("ctrl-9", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
}

#[gpui_kit::test]
fn escape_in_the_main_pane_returns_the_keyboard_to_the_sidebar(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("tab", cx);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
}

#[gpui_kit::test]
fn the_main_pane_scrolls_a_transcript_with_the_keyboard(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    many_messages(&h.store, 400, 10);
    h.settle(cx);
    open_session_titled(&h, "database planning", cx);
    h.press("tab", cx);
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    let top = |h: &Harness, cx: &mut TestAppContext| {
        h.shell(cx, |shell| match &shell.main {
            Main::Session(t) => t.list.logical_scroll_top().item_ix,
            _ => panic!("a session is open"),
        })
    };
    assert_eq!(top(&h, cx), 0);
    for _ in 0..3 {
        h.press("pagedown", cx);
    }
    let after_pages = top(&h, cx);
    assert!(after_pages > 0, "the transcript scrolled");
    h.press("end", cx);
    assert!(
        top(&h, cx) > after_pages,
        "End goes further than three pages"
    );
    h.press("home", cx);
    assert_eq!(top(&h, cx), 0);
}

// ----- the palette ---------------------------------------------------------------------

#[gpui_kit::test]
fn pressing_secondary_p_and_typing_a_project_name_then_enter_selects_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(h.shows("palette", cx));
    h.type_text("web", cx);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"[Projects]".to_owned()), "{titles:?}");
    assert_eq!(titles[1], "web", "{titles:?}");
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(h.main_kind(cx), "project:web");
    assert_eq!(h.cursor_row(cx), "project:web", "the tree follows");
    assert!(!h.shows("palette", cx));
}

#[gpui_kit::test]
fn secondary_k_opens_the_same_palette(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-k", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
}

#[gpui_kit::test]
fn the_palette_finds_worktrees_and_machines_across_machines(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("login", cx);
    let titles = h.palette_titles(cx);
    assert!(
        titles.contains(&"api / feature/login".to_owned()),
        "{titles:?}"
    );
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "worktree:feature/login");

    h.press("ctrl-p", cx);
    h.type_text("infra", cx);
    h.press("enter", cx);
    assert_eq!(
        h.machine_name(cx),
        "build box",
        "the tree goes to that machine"
    );
    assert_eq!(h.cursor_row(cx), "project:infra");
    assert_eq!(h.main_kind(cx), "project:infra");
}

#[gpui_kit::test]
fn the_machine_prefix_lists_only_machines_and_enter_shows_one(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("@", cx);
    let names = h.shell(cx, |s| {
        s.snapshot
            .machines
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>()
    });
    let expected: Vec<String> = std::iter::once("[Machines]".to_owned())
        .chain(names)
        .collect();
    assert_eq!(h.palette_titles(cx), expected);
    h.type_text("build", cx);
    h.press("enter", cx);
    assert_eq!(h.cursor_row(cx), "machine:build box");
}

#[gpui_kit::test]
fn the_project_prefix_lists_projects_and_worktrees(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("#api", cx);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"[Projects]".to_owned()));
    assert!(titles.contains(&"[Worktrees]".to_owned()));
    assert!(titles.contains(&"api".to_owned()));
}

#[gpui_kit::test]
fn the_question_mark_prefix_lists_the_prefixes(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("?", cx);
    let titles = h.palette_titles(cx);
    assert_eq!(titles[0], "[What to type]");
    assert_eq!(&titles[1..], ["", ">", "@", "#", "/", "?"]);
    // Choosing a prefix types it.
    h.press("down", cx);
    h.press("enter", cx);
    assert_eq!(
        h.shell(cx, |s| s.palette.input.clone())
            .read_with(cx, |input, _| input.value().to_string()),
        ">"
    );
}

#[gpui_kit::test]
fn secondary_shift_p_opens_the_commands_and_enter_runs_the_chosen_one(cx: &mut TestAppContext) {
    let h = open(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(MAIN_ONLY))
            .reply(Output::ok(MAIN_ONLY)),
    );
    h.press("ctrl-shift-p", cx);
    let titles = h.palette_titles(cx);
    assert_eq!(titles[0], "[Commands]");
    assert!(titles.contains(&"Connect a machine…".to_owned()));
    assert!(titles.contains(&"Open project…".to_owned()));
    assert!(
        !titles.contains(&"Down".to_owned()),
        "movement keys are not commands"
    );
    h.type_text("import", cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(h.status().contains("Imported"), "{}", h.status());
}

#[gpui_kit::test]
fn commands_are_matched_by_their_initials(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("nw", cx);
    assert_eq!(h.palette_titles(cx)[1], "New worktree");
}

#[gpui_kit::test]
fn the_commands_used_most_float_up_and_are_remembered_next_to_the_settings(
    cx: &mut TestAppContext,
) {
    let directory = tempfile::tempdir().unwrap();
    let settings_file = directory.path().join(settings::FILE_NAME);
    let h = open_with(cx, ScriptedRunner::new(), Some(settings_file.clone()));
    // Without any use, the shorter name comes first.
    h.press("ctrl-shift-p", cx);
    h.type_text("interface", cx);
    assert_eq!(h.palette_titles(cx)[1], "Larger interface");
    // Run the other one.
    cx.update_window(h.window.into(), |_, window, cx| {
        for _ in 0.."interface".len() {
            window.press("backspace", cx);
        }
    })
    .unwrap();
    h.type_text("smaller", cx);
    h.press("enter", cx);
    // Now it is first for the same query.
    h.press("ctrl-shift-p", cx);
    h.type_text("interface", cx);
    assert_eq!(h.palette_titles(cx)[1], "Smaller interface");
    let remembered =
        std::fs::read_to_string(directory.path().join(crate::usage::FILE_NAME)).unwrap();
    assert!(remembered.contains("Smaller"), "{remembered}");
    cx.update(|cx| assert_eq!(settings::get(cx).interface_scale, 90));
    crate::theme::set_scale(100);
}

#[gpui_kit::test]
fn the_places_gone_to_are_listed_first_when_the_palette_opens(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("web", cx);
    h.press("enter", cx);
    h.press("ctrl-p", cx);
    let titles = h.palette_titles(cx);
    assert_eq!(titles[0], "[Recent]");
    assert_eq!(titles[1], "web");
}

#[gpui_kit::test]
fn escape_closes_the_palette_and_a_second_press_does_nothing_harmful(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(!h.shows("overlay", cx));
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn typing_in_the_palette_does_not_trigger_the_list_shortcuts(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("jjkk?", cx);
    assert_eq!(
        h.cursor_row(cx),
        "machine:This machine",
        "the tree did not move"
    );
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::Palette,
        "? is a prefix here"
    );
    let typed = h
        .shell(cx, |s| s.palette.input.clone())
        .read_with(cx, |input, _| input.value().to_string());
    assert_eq!(typed, "jjkk?");
}

#[gpui_kit::test]
fn the_arrows_walk_the_palette_rows_and_wrap_around(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("a", cx);
    let first = h.shell(cx, |s| s.palette.cursor);
    h.press("down", cx);
    assert!(h.shell(cx, |s| s.palette.cursor) > first);
    h.press("up", cx);
    assert_eq!(h.shell(cx, |s| s.palette.cursor), first);
    h.press("up", cx);
    assert!(
        h.shell(cx, |s| s.palette.cursor) > first,
        "it wraps to the last row"
    );
    assert!(h.shows("palette-cursor", cx));
}

#[gpui_kit::test]
fn the_palette_shows_what_matched_by_its_prefix_scope(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("zzzz", cx);
    assert!(h.shows("palette-empty", cx), "nothing matches");
    assert_eq!(super::palette::parse("zzzz").0, Scope::All);
}

// ----- searching history ----------------------------------------------------------------

#[gpui_kit::test]
fn searching_history_from_the_palette_opens_the_matching_session(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    many_messages(&h.store, 80, 57);
    h.settle(cx);
    h.press_chord("cmd-shift-f", "ctrl-shift-i", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    h.type_text("migrate users", cx);
    h.settle(cx);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"[History]".to_owned()), "{titles:?}");
    assert!(
        titles.contains(&"hit:database planning".to_owned()),
        "{titles:?}"
    );
    h.press("enter", cx);
    h.settle(cx);

    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(h.main_kind(cx), "session:database planning");
    h.shell(cx, |shell| match &shell.main {
        Main::Session(transcript) => {
            assert_eq!(transcript.hit, Some(57));
            assert_eq!(transcript.hit_index(), Some(57));
            assert_eq!(
                transcript.list.logical_scroll_top().item_ix,
                57,
                "the transcript is scrolled to the hit"
            );
        }
        _ => panic!("a session should be open"),
    });
    assert!(h.shows("message-hit", cx), "the hit message is marked");
}

#[gpui_kit::test]
fn the_slash_prefix_in_the_go_to_palette_searches_the_history_too(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("/oauth", cx);
    h.settle(cx);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"hit:add oauth".to_owned()), "{titles:?}");
}

#[gpui_kit::test]
fn a_hit_on_another_machine_switches_to_that_machine(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press_chord("cmd-shift-f", "ctrl-shift-i", cx);
    h.type_text("deploy keys", cx);
    h.settle(cx);
    h.press("enter", cx);
    h.settle(cx);
    assert_eq!(h.machine_name(cx), "build box");
    assert_eq!(h.cursor_row(cx), "session:rotate the keys");
    assert_eq!(h.main_kind(cx), "session:rotate the keys");
}

#[gpui_kit::test]
fn an_empty_history_search_says_what_to_type_and_a_miss_finds_nothing(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press_chord("cmd-shift-f", "ctrl-shift-i", cx);
    let titles = h.palette_titles(cx);
    assert!(titles[0].starts_with("line:Type to search"), "{titles:?}");
    h.type_text("qqqqqqqq", cx);
    h.settle(cx);
    assert!(h.shows("palette-empty", cx));
}

#[gpui_kit::test]
fn a_session_found_by_its_title_opens_from_the_go_to_palette(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("oauth", cx);
    h.settle(cx);
    let titles = h.palette_titles(cx);
    assert!(titles.contains(&"[Sessions]".to_owned()), "{titles:?}");
    let place = titles.iter().position(|t| t == "add oauth").unwrap();
    // The cursor starts on the first row; walk to the session.
    let rows_before = titles[..place]
        .iter()
        .filter(|t| !t.starts_with('['))
        .count();
    for _ in 0..rows_before {
        h.press("down", cx);
    }
    h.press("enter", cx);
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "session:add oauth");
}

// ----- flows -----------------------------------------------------------------------------

fn answer(h: &Harness, text: &str, cx: &mut TestAppContext) {
    h.type_text(text, cx);
    h.press("enter", cx);
}

#[gpui_kit::test]
fn adding_a_machine_from_the_connect_screen_saves_it_to_the_store(cx: &mut TestAppContext) {
    let h = open(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(PROBE_OUTPUT)),
    );
    h.press("ctrl-shift-m", cx);
    h.mouse_on(
        "pair-method-ssh".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    assert!(h.shows("connect", cx), "the screen is on screen");
    answer_field(&h, "staging", cx);
    h.type_text("dev@staging.example:2222", cx);
    h.press("ctrl-s", cx);
    h.settle(cx);
    let machines = h.store.machines().unwrap();
    let added = machines
        .iter()
        .find(|machine| machine.name == "staging")
        .expect("the machine was saved");
    assert_eq!(
        added.kind,
        MachineKind::Ssh {
            host: "staging.example".into(),
            user: Some("dev".into()),
            port: Some(2222),
            identity_file: None
        }
    );
    let call = &h.runner.calls()[0];
    assert_eq!(call.program, "ssh", "the new machine was tested");
    assert!(call.args.contains(&"2222".to_owned()));
    assert!(matches!(
        h.engine.machine_state(&added.id),
        MachineState::Online(Some(_))
    ));
    assert_eq!(h.shell(cx, |s| s.snapshot.machines.len()), 3);
    assert!(
        h.outline(cx).contains(&"machine:staging".to_owned()),
        "the tree shows it, even though it is empty"
    );
}

/// Types a name into the field that has the keyboard and moves to the next.
fn answer_field(h: &Harness, text: &str, cx: &mut TestAppContext) {
    h.type_text(text, cx);
    h.press("tab", cx);
}

#[gpui_kit::test]
fn a_bad_destination_is_refused_in_place_and_nothing_is_saved(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-m", cx);
    h.mouse_on(
        "pair-method-ssh".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    answer_field(&h, "staging", cx);
    h.type_text("dev@host:notaport", cx);
    h.press("ctrl-s", cx);
    h.settle(cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Connect, "still asking");
    assert!(h.shows("connect-problem-host", cx));
    assert_eq!(h.store.machines().unwrap().len(), 2);
    assert!(h.runner.calls().is_empty());
}

#[gpui_kit::test]
fn a_machine_that_does_not_answer_is_saved_and_its_light_turns_red(cx: &mut TestAppContext) {
    let h = open(
        cx,
        ScriptedRunner::new()
            // The test before saving, then the probe after it.
            .reply(Output::failed(255, "ssh: connection refused"))
            .reply(Output::failed(255, "ssh: connection refused")),
    );
    h.press("ctrl-shift-m", cx);
    h.mouse_on(
        "pair-method-ssh".to_owned(),
        gpui_kit::MouseButton::Left,
        cx,
    );
    answer_field(&h, "down", cx);
    h.type_text("down.example", cx);
    h.press("ctrl-s", cx);
    h.press("ctrl-s", cx);
    h.settle(cx);
    let id = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "down")
        .unwrap()
        .id;
    assert!(matches!(
        h.engine.machine_state(&id),
        MachineState::Offline(_)
    ));
    assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
    assert!(h.status().contains("connection refused"));
}

#[gpui_kit::test]
fn adding_a_project_by_path_asks_machine_then_folder_then_name_and_syncs_its_worktrees(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new().reply(Output::ok(MAIN_ONLY)));
    h.press("ctrl-shift-p", cx);
    h.type_text("add a remote project by", cx);
    h.press("enter", cx);
    let titles = h.palette_titles(cx);
    assert_eq!(
        titles,
        ["build box".to_owned()],
        "only machines without a folder dialog are on offer"
    );
    h.press("enter", cx); // build box
                          // A relative path is refused where it is typed.
    answer(&h, "relative/dir", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(h.palette_titles(cx).iter().any(|t| t.contains("absolute")));
    h.type_text("", cx);
    // Replace the text with an absolute path.
    cx.update_window(h.window.into(), |_, window, cx| {
        for _ in 0.."relative/dir".len() {
            window.press("backspace", cx);
        }
    })
    .unwrap();
    answer(&h, "/srv/leon", cx);
    h.press("enter", cx); // the default name
    h.settle(cx);
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|machine| machine.name == "build box")
        .unwrap();
    let projects = h.store.projects(Some(&remote.id)).unwrap();
    let added = projects
        .iter()
        .find(|p| p.root == "/srv/leon")
        .expect("saved");
    assert_eq!(added.name, "leon");
    assert_eq!(h.store.worktrees(&added.id).unwrap().len(), 1);
    assert!(h.status().contains("Added project leon"), "{}", h.status());
}

#[gpui_kit::test]
fn creating_a_worktree_from_the_palette_runs_git_and_shows_it_in_the_list(cx: &mut TestAppContext) {
    let listing = format!(
        "{API_LISTING}\nworktree /srv/api-worktrees/fix-cache\nHEAD 3333333333333333333333333333333333333333\nbranch refs/heads/fix-cache\n"
    );
    let h = open(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(listing)),
    );
    // The keyboard is on the api project, so it is the first choice.
    h.press("ctrl-shift-n", cx);
    assert_eq!(
        h.palette_titles(cx),
        ["api", "web", "infra"].map(str::to_owned)
    );
    h.press("enter", cx);
    answer(&h, "fix-cache", cx);
    let bases = h.palette_titles(cx);
    assert_eq!(bases[0], "HEAD", "{bases:?}");
    assert!(bases.contains(&"main".to_owned()));
    h.press("enter", cx); // HEAD
    h.settle(cx);

    let calls = h.runner.calls();
    assert_eq!(
        calls[0].args,
        [
            "worktree",
            "add",
            "-b",
            "fix-cache",
            "/srv/api-worktrees/fix-cache",
            "HEAD"
        ]
    );
    assert_eq!(calls[0].cwd.as_deref(), Some(API_ROOT));
    assert_eq!(calls[1].args, ["worktree", "list", "--porcelain"]);
    assert!(h.outline(cx).contains(&"    worktree:fix-cache".to_owned()));
    assert!(h.status().contains("fix-cache"), "{}", h.status());
}

#[gpui_kit::test]
fn a_branch_name_git_would_misread_is_refused_before_anything_runs(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-n", cx);
    h.press("enter", cx);
    answer(&h, "bad name", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert!(h.palette_titles(cx).iter().any(|t| t.contains("no spaces")));
    assert!(h.runner.calls().is_empty());
}

#[gpui_kit::test]
fn a_base_can_be_typed_as_well_as_chosen(cx: &mut TestAppContext) {
    let h = open(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(API_LISTING)),
    );
    h.press("ctrl-shift-n", cx);
    h.press("enter", cx);
    answer(&h, "hotfix", cx);
    h.type_text("release/1.2", cx);
    let titles = h.palette_titles(cx);
    assert!(
        titles.contains(&"custom:release/1.2".to_owned()),
        "{titles:?}"
    );
    h.press("enter", cx);
    h.settle(cx);
    assert_eq!(h.runner.calls()[0].args.last().unwrap(), "release/1.2");
}

#[gpui_kit::test]
fn removing_a_worktree_asks_to_confirm_and_cancelling_keeps_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("remove a work", cx);
    h.press("enter", cx);
    assert_eq!(h.palette_titles(cx), ["feature/login".to_owned()]);
    h.press("enter", cx);
    assert_eq!(
        h.palette_titles(cx),
        ["Remove feature/login", "Cancel"].map(str::to_owned)
    );
    h.press("down", cx);
    h.press("enter", cx); // Cancel
    h.settle(cx);
    assert!(h.runner.calls().is_empty(), "nothing ran");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    let api = h.store.projects(Some(&MachineId::local())).unwrap()[0]
        .id
        .clone();
    assert_eq!(h.store.worktrees(&api).unwrap().len(), 2);
}

#[gpui_kit::test]
fn confirming_removes_the_worktree_through_git_and_the_list_updates(cx: &mut TestAppContext) {
    let h = open(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(""))
            .reply(Output::ok(MAIN_ONLY)),
    );
    h.press("ctrl-shift-p", cx);
    h.type_text("remove a work", cx);
    h.press("enter", cx);
    h.press("enter", cx); // the linked worktree
    h.press("enter", cx); // Remove
    h.settle(cx);
    let calls = h.runner.calls();
    assert_eq!(
        calls[0].args,
        ["worktree", "remove", "/srv/api-worktrees/feature-login"]
    );
    assert!(!h
        .outline(cx)
        .contains(&"    worktree:feature/login".to_owned()));
}

#[gpui_kit::test]
fn escape_and_backspace_go_back_one_question_at_a_time(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("add a remote project by", cx);
    h.press("enter", cx);
    let step = |h: &Harness, cx: &mut TestAppContext| {
        h.shell(cx, |s| {
            s.palette
                .flow
                .as_ref()
                .map(|f| (f.step.prompt, f.answers.len()))
        })
    };
    assert_eq!(step(&h, cx), Some(("Machine", 0)));
    h.press("enter", cx); // the one machine on offer
    assert_eq!(step(&h, cx), Some(("Folder", 1)));
    h.press("backspace", cx); // the field is empty: back
    assert_eq!(step(&h, cx), Some(("Machine", 0)));
    h.press("escape", cx); // back from the first question: to the commands
    assert_eq!(step(&h, cx), None);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert_eq!(h.palette_titles(cx)[0], "[Commands]");
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn a_new_session_on_an_agent_that_is_not_installed_ends_in_a_clear_status(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.system = FakeSystem::without(&["codex"])
        })
    });
    // Put the keyboard on the feature worktree.
    for _ in 0..4 {
        h.press("j", cx);
    }
    assert_eq!(h.cursor_row(cx), "worktree:feature/login");
    h.press("ctrl-n", cx);
    // What this computer has comes first; Codex, which it lacks, is dimmed
    // among the rest of the catalogue.
    let titles = h.palette_titles(cx);
    assert_eq!(
        titles[..3],
        ["Claude Code", "opencode", "Codex"].map(str::to_owned)
    );
    h.press("down", cx);
    h.press("down", cx);
    h.press("enter", cx); // Codex: the fake computer has none
    h.settle(cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(
        h.status(),
        "Codex is not installed on This machine. Install it from https://github.com/openai/codex."
    );
    assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
    assert!(
        h.shell(cx, |s| s.live.ids().is_empty()),
        "nothing was started"
    );
    assert!(h.runner.calls().is_empty());
}

#[gpui_kit::test]
fn a_session_in_a_folder_that_does_not_exist_here_is_refused(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    for _ in 0..4 {
        h.press("j", cx);
    }
    h.press("ctrl-n", cx);
    h.press("enter", cx); // Claude, but /srv/api-worktrees/... is not on this computer
    h.settle(cx);
    assert_eq!(
        h.status(),
        "The folder /srv/api-worktrees/feature-login does not exist."
    );
    assert!(h.shell(cx, |s| s.live.ids().is_empty()));
}

#[gpui_kit::test]
fn a_new_session_with_nothing_selected_says_so_instead_of_pretending(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    // Nothing on the local machine: no project, no session.
    for project in h.store.projects(Some(&MachineId::local())).unwrap() {
        h.store.remove_project(&project.id).unwrap();
    }
    for session in h
        .store
        .recent_sessions(&leon_core::SessionFilter::default(), 50)
        .unwrap()
        .into_iter()
        .filter(|s| s.machine_id.is_local())
    {
        h.store.remove_session(&session.id).unwrap();
    }
    h.settle(cx);
    // The remote project's worktree is still there to ask about, so remove
    // that too: there is then nowhere at all to start a session.
    for project in h.store.projects(None).unwrap() {
        h.store.remove_project(&project.id).unwrap();
    }
    h.settle(cx);
    h.press("ctrl-n", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(h.status().contains("Add a project"), "{}", h.status());
    assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
}

#[gpui_kit::test]
fn a_session_row_leads_with_the_agents_logo_and_a_long_title_still_truncates(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let long = "a very long title that cannot possibly fit ".repeat(8);
    session_at(
        &h.store,
        &MachineId::local(),
        API_ROOT,
        &long,
        2,
        &[(Role::User, "hi")],
    );
    h.settle(cx);
    let id = h
        .store
        .recent_sessions(&leon_core::SessionFilter::default(), 50)
        .unwrap()
        .into_iter()
        .find(|session| session.title == long)
        .unwrap()
        .id;
    let at = h
        .row_of(NodeId::Session(id), cx)
        .expect("the new session's row is in the tree");
    let bounds = |name: &str, h: &Harness, cx: &mut TestAppContext| {
        h.bounds_of(format!("{name}-{at}"), cx)
            .unwrap_or_else(|| panic!("{name}-{at} is drawn"))
    };
    let row = bounds("tree-row", &h, cx);
    let icon = bounds("tree-agent", &h, cx);
    let label = bounds("tree-label", &h, cx);
    assert!(icon.right() <= label.left(), "the logo leads the title");
    assert!(
        label.right() <= row.right(),
        "the title gives way inside the row: {label:?} in {row:?}"
    );
    assert!(icon.size.width <= px(18.), "at the tree's icon size");
}

#[gpui_kit::test]
fn session_results_and_the_agent_step_of_the_palette_show_the_agents_logos(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("login", cx);
    let titles = h.palette_titles(cx);
    let at = titles
        .iter()
        .position(|title| title == "fix the login bug")
        .expect("the session is a result");
    assert!(h.shows_dynamic(format!("palette-agent-{at}"), cx));
    h.press("escape", cx);
    // The agent question.
    for _ in 0..4 {
        h.press("j", cx);
    }
    h.press("ctrl-n", cx);
    for index in 0..3 {
        assert!(
            h.shows_dynamic(format!("palette-agent-{index}"), cx),
            "agent {index}"
        );
    }
}

#[gpui_kit::test]
fn a_session_in_the_main_pane_shows_the_agents_logo_beside_its_title(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    open_session_titled(&h, "fix the login bug", cx);
    assert!(h.shows("main-agent-icon", cx));
    assert!(h.shows("main-title", cx));
}

#[gpui_kit::test]
fn the_new_session_command_is_in_the_palette_with_its_keys(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("new agent", cx);
    assert_eq!(h.palette_titles(cx)[1], "New agent session");
    assert!(
        h.shows("palette-keys-1", cx),
        "its shortcut is printed beside it"
    );
}

// ----- overlays, theme, refresh ---------------------------------------------------------------

#[gpui_kit::test]
fn the_question_mark_opens_the_shortcuts_sheet_and_escape_closes_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.type_text("?", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Shortcuts);
    assert!(h.shows("shortcuts-sheet", cx));
    // The arrows scroll the sheet and do not move the list.
    h.press("down", cx);
    assert_eq!(h.cursor_row(cx), "machine:This machine");
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert!(!h.shows("shortcuts-sheet", cx));
}

#[gpui_kit::test]
fn the_sidebar_tools_open_the_sheet(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let mut visual = VisualTestContext::from_window(h.window.into(), cx);
    let bounds = visual.debug_bounds("tool-shortcuts").unwrap();
    visual.simulate_click(bounds.center(), gpui_kit::Modifiers::none());
    visual.run_until_parked();
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Shortcuts);
}

#[gpui_kit::test]
fn secondary_shift_t_toggles_the_theme_and_the_dark_theme_is_the_default(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    cx.update(|cx| assert_eq!(settings::appearance(cx), Appearance::Dark));
    h.press("ctrl-shift-y", cx);
    cx.update(|cx| {
        assert_eq!(settings::appearance(cx), Appearance::Light);
        assert_eq!(crate::theme::palette(cx).appearance, Appearance::Light);
    });
    h.press("ctrl-shift-y", cx);
    cx.update(|cx| assert_eq!(settings::appearance(cx), Appearance::Dark));
}

#[gpui_kit::test]
fn the_interface_size_steps_with_the_keys_and_through_the_palette(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-=", cx);
    cx.update(|cx| assert_eq!(settings::get(cx).interface_scale, 110));
    h.press("ctrl--", cx);
    h.press("ctrl--", cx);
    cx.update(|cx| assert_eq!(settings::get(cx).interface_scale, 90));
    h.press("ctrl-0", cx);
    cx.update(|cx| assert_eq!(settings::get(cx).interface_scale, 100));
    h.press("ctrl-shift-p", cx);
    h.type_text("choose the interface", cx);
    h.press("enter", cx);
    assert!(h.palette_titles(cx).contains(&"125%".to_owned()));
    h.type_text("125", cx);
    h.press("enter", cx);
    cx.update(|cx| assert_eq!(settings::get(cx).interface_scale, 125));
    crate::theme::set_scale(100);
}

#[gpui_kit::test]
fn secondary_r_refreshes_and_the_status_line_reports_it(cx: &mut TestAppContext) {
    // Local: two projects. Then the remote machine: its probe, its project.
    let h = open(
        cx,
        ScriptedRunner::new()
            .reply(Output::ok(MAIN_ONLY))
            .reply(Output::ok(MAIN_ONLY))
            .reply(Output::ok(PROBE_OUTPUT))
            .reply(Output::ok(MAIN_ONLY)),
    );
    h.press("ctrl-r", cx);
    h.settle(cx);
    let status = h.status();
    assert!(status.contains("Imported"), "{status}");
    assert!(status.contains("synced 3 projects"), "{status}");
    assert!(h.shows("status-line", cx));
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|m| m.name == "build box")
        .unwrap();
    assert!(matches!(
        h.engine.machine_state(&remote.id),
        MachineState::Online(Some(_))
    ));
}

#[gpui_kit::test]
fn an_engine_failure_is_shown_in_the_status_strip_not_as_a_crash(cx: &mut TestAppContext) {
    let h = open(
        cx,
        ScriptedRunner::new().reply(Output::failed(128, "fatal: not a git repository")),
    );
    h.engine.submit(crate::engine::Op::SyncWorktrees(
        h.store.projects(Some(&MachineId::local())).unwrap()[0]
            .id
            .clone(),
    ));
    h.settle(cx);
    let line = h.engine.status().expect("a status");
    assert_eq!(line.kind, StatusKind::Error);
    assert!(h.shows("status-line", cx));
}

#[gpui_kit::test]
fn the_empty_state_lists_the_keys_from_the_registry(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    for command in [
        Command::OpenProject,
        Command::GoTo,
        Command::Commands,
        Command::SearchHistory,
        Command::NewSession,
        Command::Shortcuts,
    ] {
        let selector: &'static str = Box::leak(format!("hint-{command:?}").into_boxed_str());
        assert!(h.shows(selector, cx), "{selector}");
    }
    assert!(h.shows("maker", cx));
}

#[gpui_kit::test]
fn closing_a_session_the_store_forgot_clears_the_main_pane(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    h.type_text("api", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "project:api");
    let api = h.store.projects(Some(&MachineId::local())).unwrap()[0]
        .id
        .clone();
    h.store.remove_project(&api).unwrap();
    h.settle(cx);
    assert_eq!(h.main_kind(cx), "empty");
}

#[gpui_kit::test]
fn probing_the_machine_on_screen_updates_its_light(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new().reply(Output::ok(PROBE_OUTPUT)));
    h.press("ctrl-2", cx);
    let remote = h.shell(cx, |s| s.current_machine());
    assert_eq!(h.engine.machine_state(&remote), MachineState::Unknown);
    h.press("ctrl-shift-r", cx);
    h.settle(cx);
    assert!(matches!(
        h.engine.machine_state(&remote),
        MachineState::Online(Some(_))
    ));
    assert!(h.status().contains("build box is online"), "{}", h.status());
    assert_eq!(h.runner.calls()[0].program, "ssh");
}

// ----- the tree: folding, unsorted sessions, persistence -------------------------------------

#[gpui_kit::test]
fn pressing_l_on_a_collapsed_project_shows_its_worktrees(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("j", cx);
    h.press("h", cx);
    assert_eq!(h.outline(cx)[2], "  project:web", "h folded the project");
    h.press("l", cx);
    assert_eq!(h.outline(cx)[2], "    worktree:main", "l opened it again");
    assert_eq!(h.cursor_row(cx), "project:api", "the cursor stayed");
    h.press("l", cx);
    assert_eq!(
        h.cursor_row(cx),
        "worktree:main",
        "l on an open row goes to its first child"
    );
}

#[gpui_kit::test]
fn pressing_h_goes_to_the_parent_and_the_arrows_do_the_same_as_h_and_l(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("end", cx);
    h.press("left", cx);
    assert_eq!(
        h.cursor_row(cx),
        "worktree:main",
        "a session goes to its worktree"
    );
    h.press("left", cx);
    assert_eq!(
        h.outline(cx).last().unwrap(),
        "    worktree:main",
        "an open row folds first"
    );
    assert!(!h
        .outline(cx)
        .contains(&"      session:rotate the keys".to_owned()));
    h.press("left", cx);
    assert_eq!(h.cursor_row(cx), "project:infra");
    h.press("right", cx);
    h.press("right", cx);
    assert_eq!(h.cursor_row(cx), "worktree:main");
}

#[gpui_kit::test]
fn a_session_started_in_a_linked_worktree_appears_under_that_worktree(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    session_at(
        &h.store,
        &MachineId::local(),
        "/srv/api-worktrees/feature-login/src",
        "deep in the worktree",
        2,
        &[(Role::User, "hi")],
    );
    h.settle(cx);
    let outline = h.outline(cx);
    let linked = outline
        .iter()
        .position(|l| l == "    worktree:feature/login")
        .unwrap();
    let main = outline
        .iter()
        .position(|l| l == "    worktree:main")
        .unwrap();
    let session = outline
        .iter()
        .position(|l| l == "      session:deep in the worktree")
        .unwrap();
    assert!(
        session > linked && session < outline.iter().position(|l| l == "  project:web").unwrap()
    );
    assert!(main < linked, "and not under the main worktree");
}

#[gpui_kit::test]
fn sessions_that_belong_to_no_project_go_under_unsorted_which_starts_folded(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    session_at(
        &h.store,
        &MachineId::local(),
        "/home/me/notes",
        "jot",
        3,
        &[(Role::User, "hi")],
    );
    h.settle(cx);
    let outline = h.outline(cx);
    assert!(outline.contains(&"  unsorted".to_owned()));
    assert!(
        !outline.contains(&"    folder:/home/me/notes".to_owned()),
        "folded"
    );
    let at = h.row_of(NodeId::Unsorted(MachineId::local()), cx).unwrap();
    h.shell.update(cx, |shell, _| shell.move_cursor_to(at));
    h.press("l", cx);
    h.press("j", cx);
    assert_eq!(h.cursor_row(cx), "folder:/home/me/notes");
    h.press("l", cx);
    h.press("j", cx);
    assert_eq!(h.cursor_row(cx), "session:jot");
}

#[gpui_kit::test]
fn a_worktree_shows_eight_sessions_and_show_more_expands_the_list_in_place(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    for n in 0..10 {
        session_at(
            &h.store,
            &MachineId::local(),
            "/srv/web",
            &format!("web job {n}"),
            n + 1,
            &[(Role::User, "hi")],
        );
    }
    h.settle(cx);
    let count = |h: &Harness, cx: &mut TestAppContext| {
        h.outline(cx)
            .iter()
            .filter(|l| l.contains("session:web job"))
            .count()
    };
    assert_eq!(count(&h, cx), 8);
    let more = h
        .shell(cx, |s| {
            s.rows
                .iter()
                .position(|r| matches!(r.kind, Kind::More { hidden: 2 }))
        })
        .expect("a show more row");
    h.shell.update(cx, |shell, _| shell.move_cursor_to(more));
    h.press("enter", cx);
    assert_eq!(count(&h, cx), 10);
    assert!(h.shell(cx, |s| s.cursor.is_some()));
}

#[gpui_kit::test]
fn what_is_folded_is_remembered_next_to_the_settings(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let settings_file = directory.path().join(settings::FILE_NAME);
    let h = open_with(cx, ScriptedRunner::new(), Some(settings_file));
    h.press("j", cx);
    h.press("h", cx);
    let saved = std::fs::read_to_string(directory.path().join("tree.json")).unwrap();
    assert!(
        saved.contains("project:") && saved.contains("false"),
        "{saved}"
    );
}

#[gpui_kit::test]
fn what_was_folded_last_time_is_folded_when_the_window_opens(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let settings_file = directory.path().join(settings::FILE_NAME);
    let tree_file = directory.path().join("tree.json");
    // The file as an earlier run left it: the api project folded.
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        Some(settings_file),
        Picked::Cancelled,
        |store| {
            let api = store.projects(Some(&MachineId::local())).unwrap().remove(0);
            let mut folded = super::expansion::Expansion::default();
            folded.set_open(&NodeId::Project(api.id).key(), false);
            folded.save(&tree_file).unwrap();
        },
    );
    assert_eq!(h.outline(cx)[2], "  project:web", "api is folded");
}

// ----- opening a project ------------------------------------------------------------------------

#[gpui_kit::test]
fn a_machine_without_projects_says_how_to_open_one_and_enter_on_it_asks_where(
    cx: &mut TestAppContext,
) {
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        None,
        Picked::Cancelled,
        |store| {
            for project in store.projects(Some(&MachineId::local())).unwrap() {
                store.remove_project(&project.id).unwrap();
            }
        },
    );
    let at = h
        .row_of(NodeId::Open(MachineId::local()), &mut *cx)
        .expect("the hint is shown");
    assert!(h.shows(Box::leak(format!("tree-row-{at}").into_boxed_str()), cx));
    h.shell.update(cx, |shell, _| shell.move_cursor_to(at));
    h.press("enter", cx);
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::None,
        "the picker was cancelled"
    );
    assert!(h.runner.calls().is_empty());
}

#[gpui_kit::test]
fn opening_a_folder_adds_its_repository_and_selects_it(cx: &mut TestAppContext) {
    let listing = "worktree /srv/leon\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n";
    let h = open_full(
        cx,
        ScriptedRunner::new().reply(Output::ok(listing)),
        None,
        Picked::Folder("/srv/leon/crates".into()),
        |_| {},
    );
    h.press("ctrl-o", cx);
    h.settle(cx);
    let projects = h.store.projects(Some(&MachineId::local())).unwrap();
    let leon = projects
        .iter()
        .find(|p| p.root == "/srv/leon")
        .expect("the repository was added");
    assert_eq!(leon.name, "leon");
    assert_eq!(h.store.worktrees(&leon.id).unwrap().len(), 1);
    assert_eq!(h.cursor_row(cx), "project:leon");
    assert_eq!(h.main_kind(cx), "project:leon");
    assert_eq!(h.runner.calls()[0].cwd.as_deref(), Some("/srv/leon/crates"));
}

#[gpui_kit::test]
fn cancelling_the_folder_picker_changes_nothing(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-o", cx);
    h.settle(cx);
    assert!(h.runner.calls().is_empty());
    assert_eq!(h.store.projects(None).unwrap().len(), 3);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn without_a_folder_dialog_this_computer_does_not_fall_back_to_typing_a_path(
    cx: &mut TestAppContext,
) {
    let h = open_full(cx, ScriptedRunner::new(), None, Picked::Unavailable, |_| {});
    h.press("ctrl-o", cx);
    h.settle(cx);
    assert_eq!(h.folder_dialogs.get(), 1, "the dialog was asked for");
    assert_eq!(
        h.shell(cx, |s| s.overlay),
        Overlay::None,
        "no typed-path question"
    );
    assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
    assert!(h.status().contains("no folder dialog"), "{}", h.status());
}

#[gpui_kit::test]
fn opening_a_project_on_another_machine_asks_in_the_palette(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-2", cx);
    h.press("ctrl-o", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    assert_eq!(
        h.folder_dialogs.get(),
        0,
        "no dialog for another machine's folders"
    );
    // The machine is already answered: the path is the question.
    assert!(
        h.shell(cx, |s| s.palette.flow.as_ref().is_some_and(|flow| flow
            .step
            .prompt
            == "Folder"
            && flow.answers.len() == 1))
    );
}

#[gpui_kit::test]
fn the_path_question_offers_the_unsorted_folders_as_suggestions(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    session_at(
        &h.store,
        &MachineId::local(),
        "/home/me/notes",
        "jot",
        3,
        &[(Role::User, "hi")],
    );
    h.settle(cx);
    h.press("ctrl-shift-p", cx);
    // Folders where sessions ran, on the remote machine this time.
    let remote = h
        .store
        .machines()
        .unwrap()
        .into_iter()
        .find(|machine| machine.name == "build box")
        .unwrap();
    session_at(
        &h.store,
        &remote.id,
        "/home/me/notes",
        "jot",
        3,
        &[(Role::User, "hi")],
    );
    h.settle(cx);
    h.type_text("add a remote project by", cx);
    h.press("enter", cx);
    h.press("enter", cx); // build box
    assert_eq!(h.palette_titles(cx), ["/home/me/notes".to_owned()]);
    h.type_text("/srv/typed", cx);
    assert_eq!(
        h.palette_titles(cx)[0],
        "custom:/srv/typed",
        "what was typed comes first"
    );
}

#[gpui_kit::test]
fn removing_a_project_from_the_palette_asks_to_confirm_and_forgets_its_root(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("remove a project", cx);
    h.press("enter", cx);
    h.type_text("api", cx);
    h.press("enter", cx);
    h.press("enter", cx); // Remove api
    h.settle(cx);
    let local = MachineId::local();
    assert!(h
        .store
        .projects(Some(&local))
        .unwrap()
        .iter()
        .all(|p| p.root != API_ROOT));
    assert!(h.store.dismissed_roots(&local).unwrap().contains(API_ROOT));
    assert!(!h.outline(cx).contains(&"  project:api".to_owned()));
}

// ----- the project filter ----------------------------------------------------------------

fn full_outline() -> Vec<String> {
    [
        "machine:This machine",
        "  project:api",
        "    worktree:main",
        "      session:fix the login bug",
        "    worktree:feature/login",
        "      session:add oauth",
        "  project:web",
        "    worktree:main",
        "machine:build box",
        "  project:infra",
        "    worktree:main",
        "      session:rotate the keys",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect()
}

#[gpui_kit::test]
fn typing_in_the_filter_narrows_the_tree_to_the_matching_projects_and_hides_the_rest(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    assert_eq!(h.outline(cx), full_outline());
    let expansion = h.shell(cx, |s| s.expansion.clone());

    h.press("/", cx);
    h.type_text("web", cx);

    assert_eq!(
        h.outline(cx),
        [
            "machine:This machine",
            "  project:web",
            "    worktree:main",
            "machine:build box",
        ],
        "a machine with no match is its header alone"
    );
    assert_eq!(
        h.shell(cx, |s| s.expansion.clone()),
        expansion,
        "the filter never changes what is open"
    );
    assert_eq!(
        h.cursor_row(cx),
        "project:web",
        "the cursor goes to the best match"
    );
}

#[gpui_kit::test]
fn a_project_that_matches_keeps_all_its_worktrees_and_one_that_does_not_keeps_only_the_matching_ones(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    h.press("/", cx);
    h.type_text("api", cx);
    let outline = h.outline(cx);
    assert!(outline.contains(&"    worktree:main".to_owned()));
    assert!(
        outline.contains(&"    worktree:feature/login".to_owned()),
        "{outline:?}"
    );

    h.press("escape", cx);
    h.type_text("oauth", cx);
    assert_eq!(
        h.outline(cx)
            .iter()
            .filter(|row| row.starts_with("  project"))
            .count(),
        0,
        "session titles are not what the filter looks at"
    );
    h.press("escape", cx);
    h.type_text("login", cx);
    assert_eq!(
        h.outline(cx),
        [
            "machine:This machine",
            "  project:api",
            "    worktree:feature/login",
            "      session:add oauth",
            "machine:build box",
        ]
    );
}

#[gpui_kit::test]
fn nothing_matching_says_so_in_one_line_and_every_machine_is_its_header(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("/", cx);
    h.type_text("zzzz", cx);
    assert_eq!(
        h.outline(cx),
        ["machine:This machine", "machine:build box", "  no-match"]
    );
    assert!(h.shows("tree-no-match", cx));
}

#[gpui_kit::test]
fn escape_clears_the_filter_first_and_then_returns_to_the_tree(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("/", cx);
    h.type_text("web", cx);
    h.press("escape", cx);
    assert_eq!(h.outline(cx), full_outline(), "the first Escape clears");
    // The field still has the keyboard: typing filters again.
    h.type_text("infra", cx);
    assert_eq!(h.outline(cx).len(), 5);
    h.press("escape", cx);
    // Now the field is empty: Escape gives the keyboard to the tree, so a
    // bare `j` moves the cursor instead of being typed.
    h.press("escape", cx);
    let before = h.cursor_row(cx);
    h.press("j", cx);
    assert_ne!(h.cursor_row(cx), before);
    assert_eq!(h.outline(cx), full_outline());
}

#[gpui_kit::test]
fn escape_in_the_tree_clears_an_active_filter(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("/", cx);
    h.type_text("web", cx);
    h.press("down", cx);
    assert_eq!(
        h.outline(cx).len(),
        4,
        "the filter stays while the tree has the keyboard"
    );
    h.press("escape", cx);
    assert_eq!(h.outline(cx), full_outline());
}

#[gpui_kit::test]
fn down_moves_into_the_filtered_tree_and_enter_opens_the_best_match(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("/", cx);
    h.type_text("api", cx);
    h.press("down", cx);
    assert_eq!(h.cursor_row(cx), "project:api");
    h.press("down", cx);
    assert_eq!(
        h.cursor_row(cx),
        "worktree:main",
        "the arrows now walk the filtered tree"
    );

    h.press("/", cx);
    h.press("ctrl-a", cx);
    h.type_text("login", cx);
    h.press("enter", cx);
    assert_eq!(h.main_kind(cx), "worktree:feature/login");
    assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
}

#[gpui_kit::test]
fn the_secondary_f_chord_focuses_the_filter_and_the_history_search_keeps_its_own(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-f", cx);
    h.type_text("web", cx);
    assert_eq!(h.outline(cx).len(), 4);
    h.press("escape", cx);
    h.press_chord("cmd-shift-f", "ctrl-shift-i", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
    let typed = cx.update(|cx| h.shell.read(cx).palette.input.read(cx).value().to_string());
    assert_eq!(typed, "/");
}

#[gpui_kit::test]
fn the_filter_is_applied_to_the_snapshot_in_memory_when_the_store_changes(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("/", cx);
    h.type_text("zzzz", cx);
    h.store
        .add_project(&MachineId::local(), "zzzz-new", "/srv/zzzz-new")
        .unwrap();
    h.settle(cx);
    assert!(h.outline(cx).contains(&"  project:zzzz-new".to_owned()));
}

#[gpui_kit::test]
fn projects_with_one_name_are_told_apart_in_the_tree(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.store
        .add_project(&MachineId::local(), "monorepo", "/code/zavu/monorepo")
        .unwrap();
    h.store
        .add_project(&MachineId::local(), "monorepo", "/code/acme.io/monorepo")
        .unwrap();
    h.settle(cx);
    let labels = h.shell(cx, |s| {
        s.rows
            .iter()
            .filter_map(|row| match &row.kind {
                Kind::Project { project, .. } if project.name == "monorepo" => {
                    Some(s.project_label(project))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    let mut labels = labels;
    labels.sort();
    assert_eq!(labels, ["acme.io/monorepo", "zavu/monorepo"]);
}

/// The bug of a Windows PC: git reports the worktree with `/`, the agent
/// recorded its folder with `\`, and the session fell out of its worktree
/// into `[ UNSORTED ]`. Both spellings are one folder.
#[gpui_kit::test]
fn a_session_with_a_backslash_folder_appears_under_the_worktree_git_spelled_with_slashes(
    cx: &mut TestAppContext,
) {
    let h = open(cx, ScriptedRunner::new());
    let project = h
        .store
        .add_project(&MachineId::local(), "winapi", "C:/Users/me/code/winapi")
        .unwrap();
    let worktrees = h
        .store
        .replace_worktrees(
            &project.id,
            vec![NewWorktree {
                path: "C:/Users/me/code/winapi".into(),
                branch: Some("main".into()),
                head: None,
                is_main: true,
            }],
        )
        .unwrap();
    let at = fixed_now() - chrono::Duration::minutes(5);
    for (n, cwd) in [
        r"C:\Users\me\code\winapi",
        r"c:\users\me\code\winapi\src\",
        r"\\?\C:\Users\me\code\winapi",
    ]
    .into_iter()
    .enumerate()
    {
        h.store
            .upsert_session(
                &NewSession {
                    agent: AgentId::CLAUDE,
                    external_id: format!("win-{n}"),
                    machine_id: MachineId::local(),
                    cwd: cwd.to_owned(),
                    title: format!("windows session {n}"),
                    model: None,
                    started_at: at,
                    updated_at: at,
                },
                &[],
            )
            .unwrap();
    }
    h.settle(cx);
    let (placed, loose) = h.shell(cx, |s| {
        (
            s.placement.of_worktree(&worktrees[0].id).len(),
            s.placement
                .unsorted
                .get(&MachineId::local())
                .map_or(0, |folders| {
                    folders.iter().map(|f| f.sessions.len()).sum::<usize>()
                }),
        )
    });
    assert_eq!(placed, 3, "every spelling lands under the worktree");
    assert_eq!(loose, 0, "nothing falls into the unsorted folders");
}

// ----- project logos ---------------------------------------------------------------------

fn png_logo() -> leon_core::IconImage {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend(16u32.to_be_bytes());
    bytes.extend(16u32.to_be_bytes());
    bytes.extend([8, 6, 0, 0, 0]);
    leon_core::IconImage::from_bytes(bytes).unwrap()
}

#[gpui_kit::test]
fn a_project_row_shows_its_logo_and_falls_back_to_the_folder_glyph(cx: &mut TestAppContext) {
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        None,
        Picked::Cancelled,
        |store| {
            let api = store
                .projects(None)
                .unwrap()
                .into_iter()
                .find(|p| p.name == "api")
                .unwrap();
            store
                .set_detected_icon(
                    &api.id,
                    &leon_core::NewIcon {
                        kind: leon_core::IconKind::Detected,
                        source: "nextjs-app:app/icon.png".into(),
                        image: Some(png_logo()),
                    },
                    None,
                )
                .unwrap();
        },
    );
    h.settle(cx);
    let api = h
        .row_of(NodeId::Project(project_named(&h, "api")), cx)
        .unwrap();
    let web = h
        .row_of(NodeId::Project(project_named(&h, "web")), cx)
        .unwrap();
    assert!(
        h.shows_dynamic(format!("tree-{api}-logo-image"), cx),
        "the logo is drawn"
    );
    assert!(!h.shows_dynamic(format!("tree-{api}-logo-folder"), cx));
    assert!(
        h.shows_dynamic(format!("tree-{web}-logo-folder"), cx),
        "no image: the folder glyph"
    );
    assert!(!h.shows_dynamic(format!("tree-{web}-logo-image"), cx));
}

fn project_named(h: &Harness, name: &str) -> leon_core::ProjectId {
    h.store
        .projects(None)
        .unwrap()
        .into_iter()
        .find(|p| p.name == name)
        .unwrap()
        .id
}

#[gpui_kit::test]
fn the_logo_is_read_once_per_image_however_often_the_tree_is_drawn(cx: &mut TestAppContext) {
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        None,
        Picked::Cancelled,
        |store| {
            for project in store.projects(None).unwrap() {
                store
                    .set_detected_icon(
                        &project.id,
                        &leon_core::NewIcon {
                            kind: leon_core::IconKind::Detected,
                            source: "generic:logo.png".into(),
                            image: Some(png_logo()),
                        },
                        None,
                    )
                    .unwrap();
            }
        },
    );
    h.settle(cx);
    assert_eq!(
        h.shell(cx, |s| s.logos.len()),
        1,
        "three projects share one image"
    );
    for _ in 0..3 {
        h.press("down", cx);
    }
    assert_eq!(h.shell(cx, |s| s.logos.len()), 1);
}

#[gpui_kit::test]
fn the_detail_header_and_the_palette_show_the_logo_and_say_where_it_came_from(
    cx: &mut TestAppContext,
) {
    let h = open_full(
        cx,
        ScriptedRunner::new(),
        None,
        Picked::Cancelled,
        |store| {
            let api = store
                .projects(None)
                .unwrap()
                .into_iter()
                .find(|p| p.name == "api")
                .unwrap();
            store
                .set_detected_icon(
                    &api.id,
                    &leon_core::NewIcon {
                        kind: leon_core::IconKind::Detected,
                        source: "nextjs-app:app/icon.png".into(),
                        image: Some(png_logo()),
                    },
                    None,
                )
                .unwrap();
        },
    );
    h.settle(cx);
    cx.update(|cx| {
        let id = project_named(&h, "api");
        h.shell.update(cx, |shell, cx| shell.open_project(&id, cx))
    });
    h.settle(cx);
    assert!(
        h.shows("main-logo-image", cx),
        "the header carries the logo"
    );
    let meta = cx.update(|cx| h.shell.read(cx).main_heading(cx).2);
    assert!(meta.contains("LOGO NEXTJS-APP:APP/ICON.PNG"), "{meta}");

    h.press("ctrl-p", cx);
    h.type_text("api", cx);
    let at = h.shell(cx, |s| {
        s.palette
            .items
            .iter()
            .position(|item| matches!(item, Item::Project(p) if p.name == "api"))
    });
    let at = at.expect("the palette lists the project");
    assert!(h.shows_dynamic(format!("palette-{at}-logo-image"), cx));
}

#[gpui_kit::test]
fn choosing_an_icon_stores_the_file_and_resetting_goes_back(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mine.png");
    std::fs::write(&file, &png_logo().bytes).unwrap();
    let h = open_full(cx, ScriptedRunner::new(), None, Picked::Cancelled, |_| {});
    let chosen = file.clone();
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.options.pick_image =
                Rc::new(move |_| Task::ready(Picked::Folder(chosen.clone())));
        })
    });
    let api = project_named(&h, "api");
    put_cursor(&h, cx, NodeId::Project(api.clone()));
    h.press("ctrl-shift-p", cx);
    h.type_text("Choose project icon", cx);
    h.press("enter", cx);
    h.settle(cx);
    let icon = h
        .store
        .project_icons()
        .unwrap()
        .remove(&api)
        .expect("an icon was stored");
    assert_eq!(
        (icon.kind, icon.source.as_str()),
        (leon_core::IconKind::Custom, "mine.png")
    );

    h.press("ctrl-shift-p", cx);
    h.type_text("Reset project icon", cx);
    h.press("enter", cx);
    h.settle(cx);
    assert!(
        h.store.project_icons().unwrap().is_empty(),
        "back to nothing detected"
    );
}

fn put_cursor(h: &Harness, cx: &mut TestAppContext, node: NodeId) {
    cx.update(|cx| {
        h.shell.update(cx, |shell, _| {
            shell.show(&node);
            shell.pane = Pane::Sidebar;
        })
    });
}

#[gpui_kit::test]
fn a_project_menu_offers_the_logo_commands(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    let api = project_named(&h, "api");
    put_cursor(&h, cx, NodeId::Project(api));
    h.press("m", cx);
    let labels = h.shell(cx, |s| {
        s.menu
            .as_ref()
            .map(|menu| {
                menu.current_list()
                    .iter()
                    .map(|i| i.label.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    for wanted in ["Refresh icon", "Choose icon…", "Reset icon"] {
        assert!(labels.iter().any(|label| label == wanted), "{labels:?}");
    }
}

// ----- themes: choosing, previewing and cycling -------------------------------------------

/// The theme and the interface font on screen right now.
fn worn(cx: &mut TestAppContext) -> (crate::theme::ThemeId, &'static str) {
    cx.update(|cx| (crate::theme::current(cx), crate::theme::fonts::sans()))
}

fn kept(cx: &mut TestAppContext) -> crate::theme::ThemeId {
    cx.update(|cx| settings::kept_theme_id(cx))
}

#[gpui_kit::test]
fn a_fresh_install_wears_leon_and_its_fonts_before_anything_is_chosen(cx: &mut TestAppContext) {
    let _h = open(cx, ScriptedRunner::new());
    assert_eq!(worn(cx), (crate::theme::ThemeId::Leon, "Inter"));
    assert_eq!(crate::theme::fonts::mono(), "JetBrains Mono");
}

#[gpui_kit::test]
fn the_theme_step_lists_the_themes_opens_on_the_one_in_use_and_previews_the_selection(
    cx: &mut TestAppContext,
) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    let before = cx.update(|cx| crate::theme::palette(cx).signal);
    h.press("ctrl-shift-p", cx);
    h.type_text("choose theme", cx);
    h.press("enter", cx);
    assert_eq!(h.palette_titles(cx), ["Leon", "Zavu"].map(str::to_owned));
    assert_eq!(h.shell(cx, |s| s.palette.cursor), 0, "opens on Leon");
    assert!(
        h.shows("palette-swatch-0", cx),
        "each theme shows its swatch"
    );
    assert_eq!(
        worn(cx),
        (ThemeId::Leon, "Inter"),
        "opening previews nothing new"
    );

    h.press("down", cx);
    assert_eq!(worn(cx).0, ThemeId::Zavu, "the selection is on screen");
    assert_eq!(kept(cx), ThemeId::Leon, "and nothing is kept yet");
    assert_ne!(cx.update(|cx| crate::theme::palette(cx).signal), before);
    assert_eq!(
        worn(cx),
        (ThemeId::Zavu, "Space Grotesk"),
        "the font follows"
    );
    assert_eq!(kept(cx), ThemeId::Leon);
    h.press("enter", cx);
    assert_eq!(worn(cx), (ThemeId::Zavu, "Space Grotesk"));
    assert_eq!(kept(cx), ThemeId::Zavu, "enter keeps it");
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
}

#[gpui_kit::test]
fn escape_restores_the_theme_that_was_on_screen_when_the_step_opened(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    let before = cx.update(|cx| crate::theme::palette(cx).signal);
    h.press("ctrl-shift-p", cx);
    h.type_text("choose theme", cx);
    h.press("enter", cx);
    h.press("down", cx);
    assert_eq!(worn(cx), (ThemeId::Zavu, "Space Grotesk"));
    h.press("escape", cx);
    assert_eq!(
        worn(cx),
        (ThemeId::Leon, "Inter"),
        "back to the commands: restored"
    );
    assert_eq!(cx.update(|cx| crate::theme::palette(cx).signal), before);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(kept(cx), ThemeId::Leon);
}

#[gpui_kit::test]
fn closing_the_palette_over_a_previewed_theme_restores_it_too(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("choose theme", cx);
    h.press("enter", cx);
    h.press("down", cx);
    assert_eq!(worn(cx).0, ThemeId::Zavu);
    h.press("ctrl-shift-k", cx); // another palette replaces the question
    assert_eq!(worn(cx).0, ThemeId::Leon);
}

#[gpui_kit::test]
fn the_appearance_step_previews_light_and_dark_and_escape_restores(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("choose appearance", cx);
    h.press("enter", cx);
    assert_eq!(
        h.palette_titles(cx),
        ["System", "Light", "Dark"].map(str::to_owned)
    );
    cx.update(|cx| assert_eq!(settings::appearance(cx), Appearance::Dark));
    h.press("up", cx);
    cx.update(|cx| {
        assert_eq!(settings::appearance(cx), Appearance::Light, "previewed");
        assert_eq!(crate::theme::palette(cx).appearance, Appearance::Light);
        assert_eq!(settings::get(cx).theme, AppearanceChoice::Dark, "not kept");
    });
    h.press("escape", cx);
    cx.update(|cx| assert_eq!(settings::appearance(cx), Appearance::Dark));
}

// ----- themes and appearances as commands of the palette ------------------------------------

/// The looks the palette lists, in order.
fn looks_listed(h: &Harness, cx: &mut TestAppContext) -> Vec<Look> {
    h.shell(cx, |shell| {
        shell
            .palette
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Look(look) => Some(*look),
                _ => None,
            })
            .collect()
    })
}

/// Moves the keyboard down until it is on `look`.
fn move_to_look(h: &Harness, look: Look, cx: &mut TestAppContext) {
    for _ in 0..h.shell(cx, |s| s.palette.items.len()) {
        if h.shell(cx, |s| {
            matches!(s.palette.items.get(s.palette.cursor), Some(Item::Look(on)) if *on == look)
        }) {
            return;
        }
        h.press("down", cx);
    }
    panic!("{look:?} is not listed");
}

/// Every look the registry and the appearance choices offer.
fn every_look() -> Vec<Look> {
    use crate::theme::ThemeId;
    ThemeId::ALL
        .into_iter()
        .map(Look::Theme)
        .chain(AppearanceChoice::ALL.into_iter().map(Look::Appearance))
        .collect()
}

#[gpui_kit::test]
fn the_command_palette_lists_every_theme(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    let listed = looks_listed(&h, cx);
    for look in every_look() {
        assert!(listed.contains(&look), "{look:?} is missing");
    }
    let titles = h.palette_titles(cx);
    for name in [
        "Theme: Leon",
        "Theme: Zavu",
        "Appearance: System",
        "Appearance: Light",
        "Appearance: Dark",
    ] {
        assert!(titles.iter().any(|title| title == name), "{name}");
    }
    let heading = titles
        .iter()
        .position(|t| t == "[Themes]")
        .expect("heading");
    let choose = titles
        .iter()
        .position(|t| t == "Choose theme\u{2026}")
        .unwrap();
    assert!(choose < heading, "the regular commands come first");
    assert_eq!(titles[heading + 1], "Theme: Leon");
    assert_eq!(titles.len(), heading + 1 + every_look().len());
}

#[gpui_kit::test]
fn the_default_palette_still_lists_only_destinations(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-p", cx);
    assert!(looks_listed(&h, cx).is_empty());
    h.type_text("theme", cx);
    assert!(looks_listed(&h, cx).is_empty());
}

#[gpui_kit::test]
fn a_typed_word_finds_the_themes_and_appearances_it_names(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    for (typed, expected) in [
        ("leon", Look::Theme(ThemeId::Leon)),
        ("zavu", Look::Theme(ThemeId::Zavu)),
        ("dark", Look::Appearance(AppearanceChoice::Dark)),
        ("light", Look::Appearance(AppearanceChoice::Light)),
        ("system", Look::Appearance(AppearanceChoice::System)),
    ] {
        h.press("ctrl-shift-p", cx);
        h.type_text(typed, cx);
        assert!(looks_listed(&h, cx).contains(&expected), "{typed}");
        h.press("escape", cx);
    }
    for typed in ["theme", "colors", "colours"] {
        h.press("ctrl-shift-p", cx);
        h.type_text(typed, cx);
        let listed = looks_listed(&h, cx);
        assert!(listed.contains(&Look::Theme(ThemeId::Zavu)), "{typed}");
        h.press("escape", cx);
    }
    h.press("ctrl-shift-p", cx);
    h.type_text("appearance", cx);
    assert!(looks_listed(&h, cx).contains(&Look::Appearance(AppearanceChoice::Dark)));
}

#[gpui_kit::test]
fn selecting_a_theme_row_previews_it_and_escape_restores_the_previous_theme(
    cx: &mut TestAppContext,
) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("theme", cx);
    move_to_look(&h, Look::Theme(ThemeId::Zavu), cx);
    assert_eq!(worn(cx), (ThemeId::Zavu, "Space Grotesk"));
    assert_eq!(kept(cx), ThemeId::Leon, "nothing is kept yet");
    // Moving to a row that is not a theme puts the kept one back.
    while h.shell(cx, |s| {
        !matches!(
            s.palette.items.get(s.palette.cursor),
            Some(Item::Command(_))
        )
    }) {
        h.press("down", cx);
    }
    assert_eq!(worn(cx), (ThemeId::Leon, "Inter"));
    move_to_look(&h, Look::Theme(ThemeId::Zavu), cx);
    assert_eq!(worn(cx).0, ThemeId::Zavu);
    h.press("escape", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(worn(cx), (ThemeId::Leon, "Inter"));
    assert_eq!(kept(cx), ThemeId::Leon);
}

#[gpui_kit::test]
fn selecting_an_appearance_row_previews_it_and_escape_restores_it(cx: &mut TestAppContext) {
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    h.type_text("appearance", cx);
    move_to_look(&h, Look::Appearance(AppearanceChoice::Light), cx);
    cx.update(|cx| assert_eq!(settings::appearance(cx), Appearance::Light));
    h.press("escape", cx);
    cx.update(|cx| assert_eq!(settings::appearance(cx), Appearance::Dark));
}

#[gpui_kit::test]
fn pressing_enter_on_a_theme_row_keeps_and_saves_it(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join(settings::FILE_NAME);
    let h = open_with(cx, ScriptedRunner::new(), Some(file.clone()));
    h.press("ctrl-shift-p", cx);
    h.type_text("theme", cx);
    move_to_look(&h, Look::Theme(ThemeId::Zavu), cx);
    h.press("enter", cx);
    assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    assert_eq!(kept(cx), ThemeId::Zavu);
    assert_eq!(worn(cx), (ThemeId::Zavu, "Space Grotesk"));
    assert_eq!(settings::Settings::load(&file).theme_id, ThemeId::Zavu);

    h.press("ctrl-shift-p", cx);
    h.type_text("appearance", cx);
    move_to_look(&h, Look::Appearance(AppearanceChoice::Light), cx);
    h.press("enter", cx);
    assert_eq!(
        settings::Settings::load(&file).theme,
        AppearanceChoice::Light
    );
}

#[gpui_kit::test]
fn the_look_in_use_is_marked_and_the_others_are_not(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    h.press("ctrl-shift-p", cx);
    let world = h.shell(cx, |s| s.palette.world.clone().expect("a world"));
    assert!(Look::Theme(ThemeId::Leon).is_current(&world));
    assert!(!Look::Theme(ThemeId::Zavu).is_current(&world));
    assert!(Look::Appearance(AppearanceChoice::Dark).is_current(&world));
    assert!(!Look::Appearance(AppearanceChoice::Light).is_current(&world));
}

#[gpui_kit::test]
fn the_theme_chords_advance_and_wrap_and_the_status_strip_names_the_theme(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let h = open(cx, ScriptedRunner::new());
    let mut seen = Vec::new();
    for _ in 0..2 {
        h.press("ctrl-shift-j", cx);
        seen.push(kept(cx));
        assert_eq!(worn(cx).0, kept(cx));
        assert_eq!(h.status(), format!("Theme: {}", kept(cx).name()));
    }
    assert_eq!(
        seen,
        [ThemeId::Zavu, ThemeId::Leon],
        "forward wraps from the last to the first"
    );
    h.press("ctrl-shift-h", cx);
    assert_eq!(
        kept(cx),
        ThemeId::Zavu,
        "backward wraps from the first to the last"
    );
    h.press("ctrl-shift-h", cx);
    assert_eq!(kept(cx), ThemeId::Leon);
}

#[gpui_kit::test]
fn the_theme_is_saved_and_worn_again_when_the_window_opens(cx: &mut TestAppContext) {
    use crate::theme::ThemeId;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join(settings::FILE_NAME);
    {
        let h = open_with(cx, ScriptedRunner::new(), Some(file.clone()));
        h.press("ctrl-shift-j", cx);
        assert_eq!(kept(cx), ThemeId::Zavu);
    }
    assert_eq!(settings::Settings::load(&file).theme_id, ThemeId::Zavu);
}

// ----- live sessions: what Leon does with its terminals ---------------------------------

#[cfg(leon_posix_tests)]
mod live {
    use super::*;
    use crate::ui::live::{LiveId, LiveState};
    use std::time::{Duration, Instant};

    /// The window, with real threads allowed to wake it: the thread that types
    /// an agent's line into a new shell runs beside the window. The terminals
    /// are the scripted [`computer`]: no process is started.
    pub(super) fn open_live(cx: &mut TestAppContext) -> Harness {
        cx.executor().allow_parking();
        open(cx, ScriptedRunner::new())
    }

    /// A local project in a real folder, with the keyboard on its worktree.
    /// The folder lives as long as the returned value.
    pub(super) fn real_worktree(
        h: &Harness,
        cx: &mut TestAppContext,
    ) -> (tempfile::TempDir, String) {
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
                    branch: Some("trunk".into()),
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
        (dir, path)
    }

    /// Lets the threads beside the window and the UI catch up until
    /// `condition` holds.
    pub(super) fn wait_until(
        h: &Harness,
        cx: &mut TestAppContext,
        what: &str,
        condition: impl Fn(&Harness, &mut TestAppContext) -> bool,
    ) {
        // Only a failure waits for the whole of the patience.
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            cx.run_until_parked();
            if condition(h, cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let shown: Vec<_> = (1..=2)
            .filter_map(|id| terminal_of(h, cx, id))
            .map(|t| (t.size(), t.screen_text(), t.quiet_for(), t.exit_info()))
            .collect();
        panic!("timed out waiting for {what}: {shown:?}");
    }

    pub(super) fn terminal_of(
        h: &Harness,
        cx: &mut TestAppContext,
        id: u64,
    ) -> Option<Arc<leon_term::Terminal>> {
        cx.update(|cx| {
            let view = h.shell.read(cx).live.get(LiveId(id))?.view.clone();
            let terminal = view.read(cx).terminal().clone();
            Some(terminal)
        })
    }

    /// The script of the terminal of live session `id` (they are started in
    /// order from 1).
    pub(super) fn script_of(h: &Harness, id: u64) -> Script {
        h.computer
            .script(id as usize - 1)
            .unwrap_or_else(|| panic!("terminal {id} was not started"))
    }

    pub(super) fn screen(h: &Harness, cx: &mut TestAppContext, id: u64) -> String {
        terminal_of(h, cx, id)
            .map(|terminal| terminal.screen_text())
            .unwrap_or_default()
    }

    pub(super) fn state(h: &Harness, cx: &mut TestAppContext, id: u64) -> LiveState {
        cx.update(|cx| {
            h.shell
                .read(cx)
                .live
                .get(LiveId(id))
                .map(|session| session.state(cx))
                .expect("the session exists")
        })
    }

    #[gpui_kit::test]
    fn a_new_session_runs_the_agent_in_a_terminal_that_fills_the_main_pane(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, path) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx); // Claude Code
        wait_until(&h, cx, "the agent's first line", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        assert_eq!(h.main_kind(cx), "live:1");
        assert_eq!(
            h.shell(cx, |s| s.pane),
            Pane::Main,
            "the terminal has the keyboard"
        );
        assert!(h.shows("live-terminal", cx));
        assert!(!h.shows("terminal-placeholder", cx));
        assert!(
            h.shows("main-agent-icon", cx),
            "the header shows the agent's logo"
        );
        // The live session is in the tree under its worktree, above the history.
        let outline = h.outline(cx);
        let worktree_at = outline
            .iter()
            .position(|row| row.ends_with("worktree:trunk"))
            .unwrap();
        assert_eq!(outline[worktree_at + 1], "      live:1");
        assert_eq!(state(&h, cx, 1), LiveState::Running);
        assert!(h.shows("tree-live-led-1", cx));
        let _ = path;
        let at = h.row_of(NodeId::Live(LiveId(1)), cx).unwrap();
        assert!(
            h.shows_dynamic(format!("tree-agent-{at}"), cx),
            "a live row leads with the agent's logo"
        );
    }

    #[gpui_kit::test]
    fn the_grid_follows_the_size_of_the_pane_after_the_first_paint(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        let size = terminal_of(&h, cx, 1).unwrap().size();
        assert_ne!(
            (size.cols, size.rows),
            (120, 32),
            "resized to the pane, not left at the start size"
        );
        let pane = h.bounds_of("live-terminal".to_owned(), cx).unwrap();
        let drawn = size.cols as f32 * size.cell_width as f32;
        assert!(
            drawn <= pane.size.width.as_f32(),
            "{drawn} fits in {:?}",
            pane.size
        );
        assert!(
            size.cell_width > 0 && size.cell_height > 0,
            "the font was measured"
        );
        // The child was told too.
        let terminal = terminal_of(&h, cx, 1).unwrap();
        terminal.write(&b"\x03"[..]);
    }

    #[gpui_kit::test]
    fn switching_to_another_node_keeps_the_process_running_and_back_shows_it_again(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        let pid = terminal_of(&h, cx, 1).unwrap().process_id();

        open_session_titled(&h, "fix the login bug", cx); // some other node
        assert_eq!(h.main_kind(cx), "session:fix the login bug");
        assert_eq!(
            state(&h, cx, 1),
            LiveState::Running,
            "still running in the background"
        );
        let same = terminal_of(&h, cx, 1).unwrap().process_id();
        assert_eq!(pid, same);

        h.press("ctrl-l", cx); // the keyboard back to the sidebar
        let row = h
            .row_of(NodeId::Live(LiveId(1)), cx)
            .expect("its row is in the tree");
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| shell.cursor = Some(row));
        });
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "live:1");
        assert!(
            screen(&h, cx, 1).contains("FAKE-CLAUDE"),
            "nothing restarted"
        );
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 1);
    }

    #[gpui_kit::test]
    fn non_editing_ctrl_chords_escape_tab_and_the_arrows_reach_the_program(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        let script = script_of(&h, 1);
        let before = script.written().len();
        let mut keys = vec!["ctrl-d", "escape", "tab", "up", "left", "enter"];
        if crate::platform::is_mac() {
            keys.insert(0, "ctrl-c");
        }
        for key in keys {
            // These are the program's: the secondary chords here are not
            // Leon's while a terminal has the keyboard, on every platform.
            cx.update_window(h.window.into(), |_, window, cx| window.press(key, cx))
                .unwrap();
        }
        let expected = if crate::platform::is_mac() {
            &b"\x03\x04\x1b\t\x1b[A\x1b[D\r"[..]
        } else {
            &b"\x04\x1b\t\x1b[A\x1b[D\r"[..]
        };
        assert_eq!(script.written()[before..], *expected, "the terminal bytes");
        // Leon did not move: still on the terminal, still the main pane.
        assert_eq!(h.main_kind(cx), "live:1");
        assert_eq!(
            h.shell(cx, |s| (s.pane, s.overlay)),
            (Pane::Main, Overlay::None)
        );
    }

    #[gpui_kit::test]
    fn typed_text_reaches_the_program_through_the_text_input_path(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        let script = script_of(&h, 1);
        let before = script.written().len();
        h.type_text("héllo", cx);
        assert_eq!(script.written()[before..], *"héllo".as_bytes());
    }

    #[gpui_kit::test]
    fn the_leon_chords_still_work_in_a_terminal(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        // ctrl-shift-p is the palette on every platform, from a terminal.
        h.press("ctrl-shift-p", cx);
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
        h.press("escape", cx);
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
        // The focus-the-sidebar chord leaves the terminal for the sidebar
        // (Cmd+Shift+B on macOS, Ctrl+Shift+S elsewhere).
        h.press_chord("cmd-shift-b", "ctrl-shift-s", cx);
        assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
        assert_eq!(h.main_kind(cx), "live:1", "the terminal stays on screen");
        // And ctrl-shift-j... is not bound; ctrl-j focuses the main pane again.
        h.press("ctrl-j", cx);
        assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    }

    #[gpui_kit::test]
    fn when_the_agent_ends_the_terminal_stays_open_at_the_shell_prompt_and_takes_input(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, path) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("down", cx);
        h.press("down", cx);
        h.press("enter", cx); // opencode: prints a line and returns
        wait_until(&h, cx, "the agent to have run", |h, cx| {
            screen(h, cx, 1).contains("OPENCODE-RAN")
        });
        // The shell is still there: alive, in front again, in the folder.
        wait_until(&h, cx, "the shell to get the terminal back", |h, cx| {
            terminal_of(h, cx, 1).unwrap().shell_is_foreground() == Some(true)
        });
        assert_eq!(
            state(&h, cx, 1),
            LiveState::Running,
            "the terminal did not end"
        );
        let terminal = terminal_of(&h, cx, 1).unwrap();
        assert!(terminal.exit_info().is_none());
        terminal.write(&b"pwd -P\n"[..]);
        wait_until(&h, cx, "the shell to answer", |h, cx| {
            let text = screen(h, cx, 1);
            text.lines().filter(|line| line.trim() == path).count() >= 1
        });
        // Only the shell ending ends the session, and then its code is shown.
        terminal.write(&b"exit 7\n"[..]);
        wait_until(&h, cx, "the exit", |h, cx| {
            state(h, cx, 1) == LiveState::Exited(7)
        });
        assert!(h.status().contains("exited with code 7"), "{}", h.status());
        assert!(h.shows("tree-live-state-1", cx), "the row says the code");
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert_eq!(
            h.shell(cx, |s| s.overlay),
            Overlay::None,
            "no question for a dead session"
        );
        assert!(h.shell(cx, |s| s.live.ids().is_empty()));
    }

    #[gpui_kit::test]
    fn the_row_honestly_follows_the_agent_while_it_runs_and_after_it_returns(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("down", cx);
        h.press("enter", cx); // Codex, which waits for a line
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CODEX")
        });
        wait_until(&h, cx, "the agent to be seen in front", |h, cx| {
            h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().phase)
                == crate::ui::live::AgentPhase::Running
        });
        assert_eq!(
            h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().label()),
            "Codex"
        );
        // Enter ends it; the shell is back and is told so.
        h.press("enter", cx);
        wait_until(&h, cx, "the return to be noticed", |h, cx| {
            h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().phase)
                == crate::ui::live::AgentPhase::Returned
        });
        assert_eq!(
            h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().label()),
            "Shell"
        );
        assert_eq!(state(&h, cx, 1), LiveState::Running);
        assert!(terminal_of(&h, cx, 1).unwrap().exit_info().is_none());
    }

    #[gpui_kit::test]
    fn an_agent_session_types_its_command_line_into_the_shell(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the command line", |h, cx| {
            let text = screen(h, cx, 1);
            text.contains("READY> claude")
        });
        // The terminal is the shell, not the agent.
        let terminal = terminal_of(&h, cx, 1).unwrap();
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        assert_eq!(
            terminal.shell_is_foreground(),
            Some(false),
            "the agent is in front of the shell"
        );
    }

    #[gpui_kit::test]
    fn closing_a_running_session_asks_and_then_ends_its_process(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        let script = script_of(&h, 1);
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert_eq!(
            h.shell(cx, |s| s.overlay),
            Overlay::Palette,
            "it asks first"
        );
        assert_eq!(
            h.palette_titles(cx),
            ["Close Claude Code", "Cancel"].map(str::to_owned)
        );
        h.press("escape", cx);
        assert_eq!(
            h.shell(cx, |s| s.live.ids().len()),
            1,
            "backing out keeps it"
        );
        assert!(!script.hung_up(), "nothing was ended by asking");
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        h.press("enter", cx);
        assert!(h.shell(cx, |s| s.live.ids().is_empty()));
        assert!(script.hung_up(), "the process was told to end");
    }

    #[gpui_kit::test]
    fn a_shell_opens_in_the_selected_worktree(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, path) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        wait_until(&h, cx, "the shell", |h, cx| {
            screen(h, cx, 1).contains(&format!("FAKE-SHELL in {path}"))
        });
        assert_eq!(h.main_kind(cx), "live:1");
        let label = h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().label());
        assert_eq!(label, "Shell");
    }

    #[gpui_kit::test]
    fn shells_of_one_worktree_are_tabs_and_next_and_previous_wrap(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        h.press_chord("cmd-t", "ctrl-shift-t", cx);
        assert_eq!(h.main_kind(cx), "live:2", "the new tab is shown");
        h.press_chord("cmd-shift-]", "ctrl-shift-pagedown", cx);
        assert_eq!(h.main_kind(cx), "live:1", "wraps round");
        h.press_chord("cmd-shift-[", "ctrl-shift-pageup", cx);
        assert_eq!(h.main_kind(cx), "live:2");
        h.press_chord("cmd-alt-1", "ctrl-shift-1", cx);
        assert_eq!(h.main_kind(cx), "live:1", "tab 1");
        h.press_chord("cmd-alt-9", "ctrl-shift-9", cx);
        assert_eq!(
            h.main_kind(cx),
            "live:1",
            "there is no tab 9: nothing moves"
        );
        assert!(h.shows("terminal-tabs", cx), "two tabs: the strip is drawn");
        assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    }

    #[gpui_kit::test]
    fn the_terminal_follows_the_theme_of_the_interface(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        let background = |h: &Harness, cx: &mut TestAppContext| {
            cx.update(|cx| {
                let view = h.shell.read(cx).live.get(LiveId(1)).unwrap().view.clone();
                let colour = view.read(cx).theme().background;
                colour
            })
        };
        let dark = background(&h, cx);
        h.press("ctrl-shift-y", cx); // toggle the theme
        h.settle(cx);
        let light = background(&h, cx);
        assert_ne!(dark, light);
    }

    #[gpui_kit::test]
    fn switching_theme_gives_a_live_terminal_the_new_colours_and_font_without_a_restart(
        cx: &mut TestAppContext,
    ) {
        use crate::theme::{Appearance, ThemeId};
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        let wears = |h: &Harness, cx: &mut TestAppContext| {
            cx.update(|cx| {
                let view = h.shell.read(cx).live.get(LiveId(1)).unwrap().view.clone();
                let view = view.read(cx);
                (*view.theme(), view.font().family.to_string())
            })
        };
        let (theme, family) = wears(&h, cx);
        assert_eq!(theme, ThemeId::Leon.palette(Appearance::Dark).terminal);
        assert_eq!(family, "JetBrains Mono");
        // A keyboard cycle: Leon, Zavu.
        h.press("ctrl-shift-j", cx);
        let (theme, family) = wears(&h, cx);
        assert_eq!(theme, ThemeId::Zavu.palette(Appearance::Dark).terminal);
        assert_eq!(family, "Geist Mono");
        // A preview from the palette reaches it too, and escape takes it back.
        h.press("ctrl-shift-p", cx);
        h.type_text("choose theme", cx);
        h.press("enter", cx);
        h.press("up", cx); // Leon
        let (theme, family) = wears(&h, cx);
        assert_eq!(theme, ThemeId::Leon.palette(Appearance::Dark).terminal);
        assert_eq!(family, "JetBrains Mono");
        h.press("escape", cx);
        let (theme, _) = wears(&h, cx);
        assert_eq!(theme, ThemeId::Zavu.palette(Appearance::Dark).terminal);
    }

    #[gpui_kit::test]
    fn a_theme_row_of_the_command_palette_previews_on_a_live_terminal_too(cx: &mut TestAppContext) {
        use crate::theme::{Appearance, ThemeId};
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        let wears = |h: &Harness, cx: &mut TestAppContext| {
            cx.update(|cx| {
                *h.shell
                    .read(cx)
                    .live
                    .get(LiveId(1))
                    .unwrap()
                    .view
                    .read(cx)
                    .theme()
            })
        };
        h.press("ctrl-shift-p", cx);
        h.type_text("theme", cx);
        move_to_look(&h, Look::Theme(ThemeId::Zavu), cx);
        assert_eq!(
            wears(&h, cx),
            ThemeId::Zavu.palette(Appearance::Dark).terminal
        );
        h.press("escape", cx);
        assert_eq!(
            wears(&h, cx),
            ThemeId::Leon.palette(Appearance::Dark).terminal
        );
    }

    // ----- split panes and tabs --------------------------------------------------------

    fn layout_of(h: &Harness, cx: &mut TestAppContext, id: u64) -> crate::ui::panes::Layout {
        cx.update(|cx| {
            h.shell
                .read(cx)
                .workspaces
                .tab_of(LiveId(id))
                .expect("it is in a workspace")
                .layout
                .clone()
        })
    }

    /// A shell, split once to the right: terminals 1 and 2 in one tab.
    fn two_panes(cx: &mut TestAppContext) -> (Harness, tempfile::TempDir, String) {
        let h = open_live(cx);
        let (dir, path) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        h.press_chord("cmd-d", "ctrl-shift-d", cx);
        (h, dir, path)
    }

    #[gpui_kit::test]
    fn splitting_right_adds_a_base_shell_in_the_same_folder_and_focuses_it(
        cx: &mut TestAppContext,
    ) {
        let (h, _dir, path) = two_panes(cx);
        assert_eq!(h.main_kind(cx), "live:2", "the new pane has the focus");
        assert_eq!(
            script_of(&h, 2).spec().cwd.as_deref(),
            Some(path.as_str()),
            "the new shell starts in the same folder"
        );
        let layout = layout_of(&h, cx, 1);
        assert_eq!(layout.leaves(), [LiveId(1), LiveId(2)]);
        assert!(matches!(
            layout,
            crate::ui::panes::Layout::Split {
                axis: crate::ui::panes::Axis::Row,
                ..
            }
        ));
        assert!(h.shows("pane-1", cx) && h.shows("pane-2", cx));
        assert!(h.shows("divider-", cx), "a divider between them");
        // Both are rows of the tree, in one workspace: one tab.
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 2);
        assert!(!h.shows("terminal-tabs", cx), "one tab: no strip");
        let (a, b) = h.shell(cx, |s| {
            (
                s.live.get(LiveId(1)).unwrap().cwd.clone(),
                s.live.get(LiveId(2)).unwrap().cwd.clone(),
            )
        });
        assert_eq!(a, b);
    }

    #[gpui_kit::test]
    fn splitting_down_stacks_the_new_pane_below(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        h.press_chord("cmd-shift-d", "ctrl-shift-o", cx);
        let layout = layout_of(&h, cx, 1);
        assert!(matches!(
            layout,
            crate::ui::panes::Layout::Split {
                axis: crate::ui::panes::Axis::Column,
                ..
            }
        ));
        let top = h.bounds_of("pane-1".to_owned(), cx).unwrap();
        let bottom = h.bounds_of("pane-2".to_owned(), cx).unwrap();
        assert!(
            top.bottom() <= bottom.top() + px(2.),
            "{top:?} above {bottom:?}"
        );
    }

    #[gpui_kit::test]
    fn panes_are_drawn_side_by_side_with_a_hairline_between_them_and_resize_their_terminals(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        let before = terminal_of(&h, cx, 1).unwrap().size();
        h.press_chord("cmd-d", "ctrl-shift-d", cx);
        let (left, right) = (
            h.bounds_of("pane-1".to_owned(), cx).unwrap(),
            h.bounds_of("pane-2".to_owned(), cx).unwrap(),
        );
        assert!(
            left.right() <= right.left() + px(2.),
            "{left:?} then {right:?}"
        );
        let divider = h.bounds_of("divider-".to_owned(), cx).unwrap();
        assert_eq!(divider.size.width, px(1.), "a one pixel rule");
        wait_until(&h, cx, "the first terminal to be resized", |h, cx| {
            terminal_of(h, cx, 1).unwrap().size().cols < before.cols
        });
        let after = terminal_of(&h, cx, 1).unwrap().size();
        assert!(after.cols < before.cols && after.cols > before.cols / 3);
    }

    #[gpui_kit::test]
    fn focus_moves_between_panes_by_direction_and_in_order(cx: &mut TestAppContext) {
        let (h, _dir, _) = two_panes(cx);
        h.press_chord("cmd-alt-left", "ctrl-shift-left", cx);
        assert_eq!(h.main_kind(cx), "live:1");
        h.press_chord("cmd-alt-left", "ctrl-shift-left", cx);
        assert_eq!(h.main_kind(cx), "live:1", "nothing further left");
        h.press_chord("cmd-alt-right", "ctrl-shift-right", cx);
        assert_eq!(h.main_kind(cx), "live:2");
        h.press_chord("cmd-alt-up", "ctrl-shift-up", cx);
        assert_eq!(h.main_kind(cx), "live:2", "nothing above");
        h.press_chord("cmd-[", "ctrl-shift-[", cx);
        assert_eq!(h.main_kind(cx), "live:1", "the previous pane");
        h.press_chord("cmd-]", "ctrl-shift-]", cx);
        assert_eq!(h.main_kind(cx), "live:2", "the next pane");
        h.press_chord("cmd-]", "ctrl-shift-]", cx);
        assert_eq!(h.main_kind(cx), "live:1", "wraps round");
        assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
    }

    fn ratio(h: &Harness, cx: &mut TestAppContext) -> f32 {
        match layout_of(h, cx, 1) {
            crate::ui::panes::Layout::Split { ratio, .. } => ratio,
            other => panic!("not split: {other:?}"),
        }
    }

    #[gpui_kit::test]
    fn the_divider_moves_in_steps_with_the_resize_chords_and_equalizing_resets_it(
        cx: &mut TestAppContext,
    ) {
        let (h, _dir, _) = two_panes(cx);
        h.press_chord("cmd-alt-left", "ctrl-shift-left", cx); // focus the left pane
        let start = ratio(&h, cx);
        assert_eq!(start, 0.5);
        h.press_chord("ctrl-cmd-right", "ctrl-shift-alt-right", cx);
        let wider = ratio(&h, cx);
        assert!(wider > start, "{wider}");
        h.press_chord("ctrl-cmd-right", "ctrl-shift-alt-right", cx);
        assert!(ratio(&h, cx) > wider, "step by step");
        h.press_chord("ctrl-cmd-left", "ctrl-shift-alt-left", cx);
        h.press_chord("ctrl-cmd-left", "ctrl-shift-alt-left", cx);
        h.press_chord("ctrl-cmd-left", "ctrl-shift-alt-left", cx);
        assert!(ratio(&h, cx) < start);
        // The divider across the other axis does not exist: nothing moves.
        let now = ratio(&h, cx);
        h.press_chord("ctrl-cmd-down", "ctrl-shift-alt-down", cx);
        assert_eq!(ratio(&h, cx), now);
        h.press_chord("ctrl-cmd-=", "ctrl-shift-g", cx);
        assert_eq!(ratio(&h, cx), 0.5);
    }

    #[gpui_kit::test]
    fn the_resize_never_goes_below_the_minimum_pane_size(cx: &mut TestAppContext) {
        let (h, _dir, _) = two_panes(cx);
        h.press_chord("cmd-alt-left", "ctrl-shift-left", cx);
        for _ in 0..80 {
            h.press_chord("ctrl-cmd-right", "ctrl-shift-alt-right", cx);
        }
        let ratio = ratio(&h, cx);
        let (area, min) = h.shell(cx, |s| s.pane_area_for_test());
        let right_cols = area.w * (1.0 - ratio);
        assert!(
            right_cols >= min.cols - 1.0,
            "{right_cols} cells left of {}",
            area.w
        );
        assert!(ratio < 0.95);
    }

    #[gpui_kit::test]
    fn a_pane_is_maximised_and_restored_and_the_others_keep_running(cx: &mut TestAppContext) {
        let (h, _dir, _) = two_panes(cx);
        wait_until(&h, cx, "the first shell", |h, cx| {
            screen(h, cx, 1).contains("READY>")
        });
        h.press_chord("cmd-shift-enter", "ctrl-shift-enter", cx);
        assert!(h.shows("pane-2", cx));
        assert!(!h.shows("pane-1", cx), "the others are hidden");
        assert_eq!(state(&h, cx, 1), LiveState::Running, "and keep running");
        let alone = h.bounds_of("pane-2".to_owned(), cx).unwrap();
        h.press_chord("cmd-shift-enter", "ctrl-shift-enter", cx);
        assert!(h.shows("pane-1", cx) && h.shows("pane-2", cx));
        let shared = h.bounds_of("pane-2".to_owned(), cx).unwrap();
        assert!(alone.size.width > shared.size.width);
    }

    #[gpui_kit::test]
    fn closing_a_pane_collapses_the_layout_and_the_last_one_returns_to_the_worktree(
        cx: &mut TestAppContext,
    ) {
        let (h, _dir, _) = two_panes(cx);
        // Pane 2's shell is idle at its prompt: no question.
        wait_until(&h, cx, "the prompt", |h, cx| {
            terminal_of(h, cx, 2).unwrap().shell_is_foreground() == Some(true)
        });
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert_eq!(
            h.main_kind(cx),
            "live:1",
            "the sibling has the room and the focus"
        );
        assert_eq!(layout_of(&h, cx, 1).leaves(), [LiveId(1)]);
        assert!(!h.shows("divider-", cx));
        wait_until(&h, cx, "the prompt", |h, cx| {
            terminal_of(h, cx, 1).unwrap().shell_is_foreground() == Some(true)
        });
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        assert!(h.shell(cx, |s| s.live.ids().is_empty()));
        assert_eq!(
            h.main_kind(cx),
            "worktree:trunk",
            "back to the worktree's detail"
        );
        assert_eq!(h.shell(cx, |s| s.pane), Pane::Sidebar);
    }

    #[gpui_kit::test]
    fn a_new_tab_keeps_its_own_panes_and_the_strip_names_the_tabs(cx: &mut TestAppContext) {
        let (h, _dir, _) = two_panes(cx);
        h.press_chord("cmd-t", "ctrl-shift-t", cx); // a new tab
        assert_eq!(h.main_kind(cx), "live:3");
        assert!(h.shows("terminal-tabs", cx));
        assert!(h.shows("terminal-tab-0", cx) && h.shows("terminal-tab-1", cx));
        assert!(
            h.shows("pane-3", cx) && !h.shows("pane-1", cx),
            "only this tab's panes"
        );
        h.press_chord("cmd-shift-[", "ctrl-shift-pageup", cx);
        assert!(h.shows("pane-1", cx) && h.shows("pane-2", cx));
        // Clicking a tab shows it.
        h.mouse_on("terminal-tab-1".to_owned(), gpui_kit::MouseButton::Left, cx);
        assert_eq!(h.main_kind(cx), "live:3");
    }

    #[gpui_kit::test]
    fn every_pane_command_is_in_the_palette(cx: &mut TestAppContext) {
        let h = open_live(cx);
        h.press("ctrl-shift-p", cx);
        h.type_text("pane", cx);
        let titles = h.palette_titles(cx);
        for wanted in [
            "Split the pane to the right",
            "Split the pane downwards",
            "Focus the pane on the left",
            "Move the pane's divider left",
            "Maximise or restore the pane",
            "Close the pane",
            "Make the panes the same size",
        ] {
            assert!(
                titles.iter().any(|title| title == wanted),
                "{wanted}: {titles:?}"
            );
        }
    }

    #[gpui_kit::test]
    fn the_pane_commands_say_so_when_there_is_no_terminal(cx: &mut TestAppContext) {
        let h = open_live(cx);
        h.press_chord("cmd-d", "ctrl-shift-d", cx);
        assert!(
            h.status().contains("no terminal to split"),
            "{}",
            h.status()
        );
    }

    // ----- the context menu ------------------------------------------------------------

    fn menu_labels(h: &Harness, cx: &mut TestAppContext) -> Vec<String> {
        h.shell(cx, |s| {
            s.menu
                .as_ref()
                .map(|menu| {
                    menu.current_list()
                        .iter()
                        .map(|item| item.label.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    pub(super) fn put_cursor_on(h: &Harness, cx: &mut TestAppContext, node: NodeId) {
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                shell.show(&node);
                shell.pane = Pane::Sidebar;
            })
        });
    }

    fn project_id(h: &Harness, name: &str) -> leon_core::ProjectId {
        h.store
            .projects(None)
            .unwrap()
            .into_iter()
            .find(|project| project.name == name)
            .unwrap()
            .id
    }

    #[gpui_kit::test]
    fn the_menu_key_the_shift_f10_chord_and_m_open_the_menu_of_the_row(cx: &mut TestAppContext) {
        let h = open_live(cx);
        for key in ["shift-f10", "menu", "m"] {
            put_cursor_on(&h, cx, NodeId::Project(project_id(&h, "api")));
            h.press(key, cx);
            assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Menu, "{key}");
            assert_eq!(
                menu_labels(&h, cx),
                [
                    "New worktree…",
                    "New agent session",
                    "Open shell here",
                    "Copy path",
                    "Reveal in file manager",
                    "Refresh icon",
                    "Choose icon…",
                    "Reset icon",
                    "Remove project"
                ],
                "{key}"
            );
            assert!(h.shows("context-menu", cx));
            h.press("escape", cx);
            assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
            assert!(!h.shows("context-menu", cx));
        }
    }

    #[gpui_kit::test]
    fn a_right_click_opens_the_menu_of_that_row_and_moves_the_cursor_there(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let at = h
            .row_of(NodeId::Project(project_id(&h, "web")), cx)
            .unwrap();
        h.mouse_on(format!("tree-row-{at}"), gpui_kit::MouseButton::Right, cx);
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Menu);
        assert_eq!(h.shell(cx, |s| s.cursor), Some(at));
        assert!(menu_labels(&h, cx).contains(&"Remove project".to_owned()));
        // The card is the overlay's: elevated, anchored near the row.
        let menu = h.bounds_of("context-menu".to_owned(), cx).unwrap();
        let row = h.bounds_of(format!("tree-row-{at}"), cx).unwrap();
        assert!(menu.top() >= row.top() - px(2.), "{menu:?} for {row:?}");
        // A click outside closes it.
        let layer = h.bounds_of("menu-layer".to_owned(), cx).unwrap();
        let mut visual = VisualTestContext::from_window(h.window.into(), cx);
        let far = gpui_kit::point(layer.right() - px(4.), layer.bottom() - px(4.));
        visual.simulate_click(far, gpui_kit::Modifiers::none());
        visual.run_until_parked();
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    }

    #[gpui_kit::test]
    fn the_menu_moves_with_j_k_and_the_arrows_and_selects_by_typing(cx: &mut TestAppContext) {
        let h = open_live(cx);
        put_cursor_on(&h, cx, NodeId::Machine(MachineId::local()));
        h.press("shift-f10", cx);
        let cursor =
            |h: &Harness, cx: &mut TestAppContext| h.shell(cx, |s| s.menu.as_ref().unwrap().cursor);
        assert_eq!(
            menu_labels(&h, cx),
            [
                "Open project…",
                "New shell",
                "Probe",
                "Rename",
                "Connect a machine…"
            ]
        );
        assert_eq!(cursor(&h, cx), 0);
        h.press("j", cx);
        h.press("down", cx);
        assert_eq!(cursor(&h, cx), 2);
        h.press("k", cx);
        h.press("up", cx);
        h.press("up", cx);
        assert_eq!(cursor(&h, cx), 4, "wraps");
        h.type_text("p", cx);
        assert_eq!(cursor(&h, cx), 2, "p is Probe");
        h.type_text("r", cx);
        // "pr" still starts Probe.
        assert_eq!(cursor(&h, cx), 2);
        assert!(
            h.shows("menu-cursor", cx),
            "the selected row has the violet bar"
        );
        assert!(
            h.shows("menu-chip-0-0", cx),
            "Open project… shows its shortcut"
        );
    }

    #[gpui_kit::test]
    fn the_menu_swallows_bare_keys_so_the_tree_beneath_does_not_move(cx: &mut TestAppContext) {
        let h = open_live(cx);
        put_cursor_on(&h, cx, NodeId::Machine(MachineId::local()));
        let before = h.cursor_row(cx);
        h.press("m", cx);
        h.press("j", cx);
        h.press("j", cx);
        h.press("k", cx);
        assert_eq!(h.cursor_row(cx), before);
        h.press("escape", cx);
    }

    #[gpui_kit::test]
    fn every_entry_to_opening_a_project_here_opens_the_system_folder_dialog(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        // 1. the shortcut
        h.press("ctrl-o", cx);
        assert_eq!(h.folder_dialogs.get(), 1, "the shortcut");
        // 2. the palette
        h.press("ctrl-shift-p", cx);
        h.type_text("open project", cx);
        h.press("enter", cx);
        h.settle(cx);
        assert_eq!(h.folder_dialogs.get(), 2, "the palette");
        // 3. the context menu of the machine
        put_cursor_on(&h, cx, NodeId::Machine(MachineId::local()));
        h.press("m", cx);
        h.press("enter", cx); // Open project…
        h.settle(cx);
        assert_eq!(h.folder_dialogs.get(), 3, "the menu");
        // 4. the row of an empty machine
        for project in h.store.projects(Some(&MachineId::local())).unwrap() {
            h.store.remove_project(&project.id).unwrap();
        }
        h.settle(cx);
        let open_row = h
            .row_of(NodeId::Open(MachineId::local()), cx)
            .expect("the empty-state row");
        h.mouse_on(
            format!("tree-row-{open_row}"),
            gpui_kit::MouseButton::Left,
            cx,
        );
        assert_eq!(h.folder_dialogs.get(), 4, "the empty-state row");
        assert_eq!(
            h.shell(cx, |s| s.overlay),
            Overlay::None,
            "no typed-path flow anywhere"
        );
    }

    #[gpui_kit::test]
    fn removing_a_project_from_its_menu_asks_and_then_removes_it(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let id = project_id(&h, "web");
        put_cursor_on(&h, cx, NodeId::Project(id.clone()));
        h.press("m", cx);
        h.type_text("remove", cx);
        h.press("enter", cx);
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
        assert_eq!(
            h.palette_titles(cx),
            ["Remove web", "Cancel"].map(str::to_owned),
            "it asks"
        );
        // Back out: one question at a time, then the palette closes.
        for _ in 0..4 {
            h.press("escape", cx);
        }
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
        assert!(h.store.project(&id).is_ok(), "backing out removes nothing");
        put_cursor_on(&h, cx, NodeId::Project(id.clone()));
        h.press("m", cx);
        h.type_text("remove", cx);
        h.press("enter", cx);
        h.press("enter", cx); // Remove web
        h.settle(cx);
        assert!(h.store.project(&id).is_err());
    }

    #[gpui_kit::test]
    fn a_machines_menu_renames_and_removes_after_asking_and_the_local_one_cannot_be_removed(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let remote = h
            .store
            .machines()
            .unwrap()
            .into_iter()
            .find(|machine| machine.name == "build box")
            .unwrap();
        put_cursor_on(&h, cx, NodeId::Machine(remote.id.clone()));
        h.press("m", cx);
        assert_eq!(
            menu_labels(&h, cx),
            [
                "Open project…",
                "New shell",
                "Probe",
                "Why is it offline?",
                "Edit machine…",
                "Rename",
                "Connect a machine…",
                "Remove machine"
            ]
        );
        h.type_text("rename", cx);
        h.press("enter", cx);
        h.type_text("staging", cx);
        h.press("enter", cx);
        h.settle(cx);
        assert_eq!(h.store.machine(&remote.id).unwrap().name, "staging");
        // Remove: asks first.
        put_cursor_on(&h, cx, NodeId::Machine(remote.id.clone()));
        h.press("m", cx);
        h.type_text("remove", cx);
        h.press("enter", cx);
        assert_eq!(
            h.palette_titles(cx),
            ["Remove staging", "Cancel"].map(str::to_owned)
        );
        h.press("enter", cx);
        h.settle(cx);
        assert!(h.store.machine(&remote.id).is_err());
        put_cursor_on(&h, cx, NodeId::Machine(MachineId::local()));
        h.press("m", cx);
        assert!(!menu_labels(&h, cx).contains(&"Remove machine".to_owned()));
    }

    #[gpui_kit::test]
    fn a_projects_menu_copies_its_path_and_reveals_it_on_this_computer(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, path) = real_worktree(&h, cx);
        let real = project_id(&h, "real");
        put_cursor_on(&h, cx, NodeId::Project(real));
        h.press("m", cx);
        h.type_text("copy", cx);
        h.press("enter", cx);
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some(path.as_str())
        );
        assert!(h.status().contains("Copied the path"), "{}", h.status());
        put_cursor_on(&h, cx, NodeId::Project(project_id(&h, "real")));
        h.press("m", cx);
        h.type_text("reveal", cx);
        h.press("enter", cx);
        assert_eq!(
            h.revealed.borrow().as_slice(),
            [std::path::PathBuf::from(&path)]
        );
    }

    #[gpui_kit::test]
    fn a_worktrees_menu_copies_the_branch_and_the_main_one_cannot_be_removed(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let linked = worktree_id(&h, "feature/login");
        put_cursor_on(&h, cx, NodeId::Worktree(linked));
        h.press("m", cx);
        assert!(menu_labels(&h, cx).contains(&"Remove worktree".to_owned()));
        h.type_text("copy b", cx);
        h.press("enter", cx);
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("feature/login")
        );
        let main = h
            .store
            .all_worktrees()
            .unwrap()
            .into_iter()
            .find(|worktree| worktree.is_main)
            .unwrap();
        put_cursor_on(&h, cx, NodeId::Worktree(main.id));
        h.press("m", cx);
        assert!(!menu_labels(&h, cx).contains(&"Remove worktree".to_owned()));
    }

    #[gpui_kit::test]
    fn a_history_sessions_menu_opens_it_copies_its_id_and_removes_it_after_asking(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        session_at(
            &h.store,
            &MachineId::local(),
            &path,
            "menu me",
            3,
            &[(Role::User, "hi")],
        );
        h.settle(cx);
        let id = h
            .store
            .recent_sessions(&leon_core::SessionFilter::default(), 50)
            .unwrap()
            .into_iter()
            .find(|session| session.title == "menu me")
            .unwrap()
            .id;
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("m", cx);
        assert_eq!(
            menu_labels(&h, cx),
            [
                "Open",
                "Open transcript",
                "Copy session id",
                "Remove from history"
            ]
        );
        h.type_text("copy", cx);
        h.press("enter", cx);
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("menu me-3")
        );
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("m", cx);
        h.type_text("open t", cx);
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:menu me");
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("m", cx);
        h.press("enter", cx); // Open: resumes it in a terminal
        wait_until(&h, cx, "the resumed agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume menu me-3")
        });
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("m", cx);
        h.type_text("remove", cx);
        h.press("enter", cx);
        assert_eq!(h.palette_titles(cx)[1], "Cancel");
        h.press("enter", cx);
        h.settle(cx);
        assert!(h.store.session(&id).is_err());
    }

    #[gpui_kit::test]
    fn the_new_agent_session_submenu_starts_the_chosen_agent(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        put_cursor_on(&h, cx, NodeId::Project(project_id(&h, "real")));
        h.press("m", cx);
        h.type_text("new a", cx);
        h.press("enter", cx); // opens the submenu
        assert_eq!(menu_labels(&h, cx), ["Claude Code", "Codex", "opencode"]);
        assert!(h.shows("context-submenu", cx));
        h.press("j", cx);
        h.press("enter", cx); // Codex
        wait_until(&h, cx, "codex", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CODEX")
        });
        assert_eq!(
            h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().agent),
            Some(AgentId::CODEX)
        );
    }

    #[gpui_kit::test]
    fn the_left_arrow_closes_the_submenu_and_escape_closes_it_before_the_menu(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        put_cursor_on(&h, cx, NodeId::Project(project_id(&h, "api")));
        h.press("m", cx);
        h.type_text("new a", cx);
        h.press("right", cx);
        assert!(h.shows("context-submenu", cx));
        h.press("left", cx);
        assert!(!h.shows("context-submenu", cx));
        h.press("right", cx);
        h.press("escape", cx);
        assert!(!h.shows("context-submenu", cx));
        assert_eq!(
            h.shell(cx, |s| s.overlay),
            Overlay::Menu,
            "the menu is still open"
        );
        h.press("escape", cx);
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::None);
    }

    #[gpui_kit::test]
    fn a_live_terminals_menu_splits_renames_and_closes_with_a_question_while_a_program_runs(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _) = real_worktree(&h, cx);
        h.press("ctrl-n", cx);
        h.press("enter", cx); // Claude
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        h.press_chord("cmd-shift-b", "ctrl-shift-b", cx); // to the sidebar
        put_cursor_on(&h, cx, NodeId::Live(LiveId(1)));
        h.press("m", cx);
        assert_eq!(
            menu_labels(&h, cx),
            ["Focus", "Split right", "Split down", "Rename", "Close"]
        );
        h.type_text("split r", cx);
        h.press("enter", cx);
        assert_eq!(
            h.shell(cx, |s| s.live.ids().len()),
            2,
            "split from the menu"
        );
        put_cursor_on(&h, cx, NodeId::Live(LiveId(1)));
        h.press("m", cx);
        h.type_text("ren", cx);
        h.press("enter", cx);
        h.type_text("build", cx);
        h.press("enter", cx);
        assert_eq!(
            h.shell(cx, |s| s.live.get(LiveId(1)).unwrap().label()),
            "build"
        );
        put_cursor_on(&h, cx, NodeId::Live(LiveId(1)));
        h.press("m", cx);
        h.type_text("close", cx);
        h.press("enter", cx);
        assert_eq!(
            h.shell(cx, |s| s.overlay),
            Overlay::Palette,
            "a program runs: it asks"
        );
        h.press("enter", cx);
        assert_eq!(h.shell(cx, |s| s.live.ids().len()), 1);
    }

    // ----- the activity dot -----------------------------------------------------------------

    use crate::ui::activity::Activity;

    /// The activity the dot of the worktree `label` of project `project`
    /// shows, and the roll-up on the project's row.
    fn dots(
        h: &Harness,
        cx: &mut TestAppContext,
        project: &str,
        label: &str,
    ) -> (Activity, Activity) {
        h.shell(cx, |s| {
            let of = |wanted: &dyn Fn(&tree::Row) -> bool| {
                let row = s.rows.iter().find(|row| wanted(row)).expect("the row is in the tree");
                s.row_activity(row).expect("it has a dot")
            };
            let worktree = of(&|row| {
                matches!(&row.kind, Kind::Worktree { worktree, .. }
                    if tree::worktree_label(worktree) == label
                        && s.snapshot.project(&worktree.project_id).is_some_and(|e| e.project.name == project))
            });
            let project = of(&|row| matches!(&row.kind, Kind::Project { project: p, .. } if p.name == project));
            (worktree, project)
        })
    }

    pub(super) fn set_waiting_after(h: &Harness, cx: &mut TestAppContext, after: Duration) {
        cx.update(|cx| {
            h.shell.update(cx, |shell, cx| {
                shell.options.activity.waiting_after = after;
                if shell.refresh_activity(cx) {
                    cx.notify();
                }
            })
        });
        h.settle(cx);
    }

    pub(super) fn show_worktree_detail(
        h: &Harness,
        cx: &mut TestAppContext,
        project: &str,
        label: &str,
    ) {
        cx.update(|cx| {
            h.shell.update(cx, |shell, cx| {
                let entry = shell
                    .snapshot
                    .projects
                    .iter()
                    .find(|e| e.project.name == project)
                    .unwrap();
                let worktree = entry
                    .worktrees
                    .iter()
                    .find(|w| tree::worktree_label(w) == label)
                    .unwrap();
                let (p, w) = (entry.project.id.clone(), worktree.id.clone());
                shell.open_worktree(&p, &w, cx);
            })
        });
        h.settle(cx);
    }

    pub(super) const HOUR: Duration = Duration::from_secs(3600);

    #[gpui_kit::test]
    fn a_worktree_with_no_live_terminal_has_the_grey_dot_and_the_project_rolls_up_the_same(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Off, Activity::Off)
        );
        assert!(h.shows_dynamic(
            format!(
                "tree-activity-{}",
                h.row_of(NodeId::Project(project_id(&h, "real")), cx)
                    .unwrap()
            ),
            cx
        ));
    }

    #[gpui_kit::test]
    fn output_turns_the_dot_working_silence_turns_it_waiting_and_the_shell_back_turns_it_idle(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        set_waiting_after(&h, cx, HOUR);
        h.press("ctrl-n", cx);
        h.press("enter", cx); // Claude Code: prints, and takes the terminal
        wait_until(&h, cx, "the agent's first line", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Working, Activity::Working)
        );

        // Quiet beyond the threshold: it is waiting for the user.
        set_waiting_after(&h, cx, Duration::ZERO);
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Waiting, Activity::Waiting)
        );

        // Output again, with a threshold it has not passed: working.
        set_waiting_after(&h, cx, HOUR);
        script_of(&h, 1).print("more output\n");
        h.settle(cx);
        assert_eq!(dots(&h, cx, "real", "trunk").0, Activity::Working);

        // The agent returns to the prompt: idle, whatever the threshold.
        set_waiting_after(&h, cx, Duration::ZERO);
        script_of(&h, 1).set_foreground(true);
        h.settle(cx);
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Idle, Activity::Idle)
        );
    }

    #[gpui_kit::test]
    fn the_bell_means_waiting_until_the_session_is_looked_at(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        set_waiting_after(&h, cx, HOUR);
        h.press("ctrl-n", cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE")
        });
        show_worktree_detail(&h, cx, "real", "trunk");
        script_of(&h, 1).print("\x07");
        h.settle(cx);
        assert_eq!(
            dots(&h, cx, "real", "trunk").0,
            Activity::Waiting,
            "the bell rang out of sight"
        );
        h.press("ctrl-e", cx); // focus the terminal: it is looked at
        h.settle(cx);
        assert_eq!(dots(&h, cx, "real", "trunk").0, Activity::Working);
    }

    #[gpui_kit::test]
    fn a_failing_exit_is_the_error_colour_until_the_pane_is_visited_and_a_clean_one_is_not_live(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        set_waiting_after(&h, cx, HOUR);
        h.press("ctrl-t", cx); // a plain shell in the worktree
        wait_until(&h, cx, "the shell", |h, cx| {
            screen(h, cx, 1).contains("READY>")
        });
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Idle, Activity::Idle),
            "a plain shell is idle"
        );

        show_worktree_detail(&h, cx, "real", "trunk");
        script_of(&h, 1).exit(3);
        h.settle(cx);
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Failed, Activity::Failed)
        );

        h.press("ctrl-e", cx);
        h.settle(cx);
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Off, Activity::Off),
            "seen"
        );
    }

    #[gpui_kit::test]
    fn a_clean_exit_leaves_the_dot_grey(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        h.press("ctrl-t", cx);
        wait_until(&h, cx, "the shell", |h, cx| {
            screen(h, cx, 1).contains("READY>")
        });
        show_worktree_detail(&h, cx, "real", "trunk");
        script_of(&h, 1).exit(0);
        h.settle(cx);
        assert_eq!(dots(&h, cx, "real", "trunk").0, Activity::Off);
    }

    #[gpui_kit::test]
    fn a_worktree_shows_the_most_urgent_state_of_its_terminals_and_the_project_the_most_urgent_worktree(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        set_waiting_after(&h, cx, HOUR);
        h.press("ctrl-t", cx); // terminal 1: a plain shell, idle
        wait_until(&h, cx, "the shell", |h, cx| {
            screen(h, cx, 1).contains("READY>")
        });
        h.press_chord("cmd-t", "ctrl-shift-t", cx); // terminal 2: another one
        wait_until(&h, cx, "the second shell", |h, cx| {
            screen(h, cx, 2).contains("READY>")
        });
        assert_eq!(dots(&h, cx, "real", "trunk").0, Activity::Idle);
        // One terminal has a program in front, printing: working beats idle.
        script_of(&h, 2).set_foreground(false);
        script_of(&h, 2).print("building\n");
        h.settle(cx);
        assert_eq!(
            dots(&h, cx, "real", "trunk"),
            (Activity::Working, Activity::Working)
        );
        // Quiet: waiting beats working and idle.
        set_waiting_after(&h, cx, Duration::ZERO);
        assert_eq!(dots(&h, cx, "real", "trunk").0, Activity::Waiting);
        // Another project is unaffected.
        assert_eq!(dots(&h, cx, "api", "main"), (Activity::Off, Activity::Off));
    }

    #[gpui_kit::test]
    fn the_dot_has_a_tooltip_for_every_state(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        let at = h
            .row_of(NodeId::Worktree(worktree_id(&h, "trunk")), cx)
            .unwrap();
        assert!(
            h.shows_dynamic(format!("tree-activity-{at}"), cx),
            "the worktree row leads with a dot"
        );
    }

    #[gpui_kit::test]
    fn the_coarse_timer_runs_only_while_a_terminal_is_live(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let _real = real_worktree(&h, cx);
        assert!(
            h.shell(cx, |s| s.ticker.is_none()),
            "nothing is live: no timer"
        );
        h.press("ctrl-t", cx);
        wait_until(&h, cx, "the shell", |h, cx| {
            screen(h, cx, 1).contains("READY>")
        });
        assert!(h.shell(cx, |s| s.ticker.is_some()));
        // A program going quiet is noticed by the timer alone: no output wakes
        // the window.
        script_of(&h, 1).set_foreground(false);
        set_waiting_after(&h, cx, Duration::ZERO);
        assert_eq!(dots(&h, cx, "real", "trunk").0, Activity::Waiting);
        // The shell ends: the next tick finds nothing live and stops.
        script_of(&h, 1).exit(0);
        h.settle(cx);
        cx.executor().advance_clock(Duration::from_secs(60));
        h.settle(cx);
        assert!(
            h.shell(cx, |s| s.ticker.is_none()),
            "no live terminal: the timer is gone"
        );
    }

    // ----- opening a history session resumes it in a terminal ------------------------

    /// A session of `agent` that ran in a folder that exists on this computer
    /// and that no project contains, so it is listed under unsorted. The
    /// folder lives as long as the returned value.
    pub(super) fn local_session(
        h: &Harness,
        cx: &mut TestAppContext,
        agent: AgentId,
        title: &str,
    ) -> (tempfile::TempDir, String, leon_core::SessionId) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let id = stored_session(h, cx, agent, &MachineId::local(), &path, title);
        (dir, path, id)
    }

    /// Stores a session of `agent` in `cwd` on `machine`; its own id is the
    /// title and "-3" (it is three minutes old).
    pub(super) fn stored_session(
        h: &Harness,
        cx: &mut TestAppContext,
        agent: AgentId,
        machine: &MachineId,
        cwd: &str,
        title: &str,
    ) -> leon_core::SessionId {
        let at = Utc.with_ymd_and_hms(2026, 10, 4, 11, 57, 0).unwrap();
        let stored = h
            .store
            .upsert_session(
                &NewSession {
                    agent,
                    external_id: format!("{title}-3"),
                    machine_id: machine.clone(),
                    cwd: cwd.to_owned(),
                    title: title.to_owned(),
                    model: None,
                    started_at: at,
                    updated_at: at,
                },
                &[NewMessage {
                    role: Role::User,
                    text: format!("hello from {title}"),
                    at,
                }],
            )
            .unwrap();
        h.settle(cx);
        stored
    }

    fn session_titled(h: &Harness, title: &str) -> leon_core::SessionId {
        h.store
            .recent_sessions(&leon_core::SessionFilter::default(), 100)
            .unwrap()
            .into_iter()
            .find(|session| session.title == title)
            .expect("the session is stored")
            .id
    }

    fn live_count(h: &Harness, cx: &mut TestAppContext) -> usize {
        h.shell(cx, |s| s.live.ids().len())
    }

    /// The window over a scripted runner, with real threads allowed to wake it.
    fn open_live_with(cx: &mut TestAppContext, runner: ScriptedRunner) -> Harness {
        cx.executor().allow_parking();
        open(cx, runner)
    }

    #[gpui_kit::test]
    fn pressing_enter_on_a_history_session_opens_a_shell_and_types_the_resume_command(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, path, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the resumed agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        let shown = screen(&h, cx, 1);
        assert!(shown.contains(&format!("FAKE-SHELL in {path}")), "{shown}");
        assert!(
            shown.contains("READY> claude --resume alpha-3"),
            "the line was typed into the shell: {shown}"
        );
        let spec = script_of(&h, 1).spec().clone();
        assert_eq!(spec.program, "/bin/sh", "the terminal is the login shell");
        assert_eq!(spec.cwd.as_deref(), Some(path.as_str()));
        assert_eq!(h.main_kind(cx), "live:1");
        h.shell(cx, |s| {
            assert_eq!(s.pane, Pane::Main);
            assert!(s.terminal_focused(), "the terminal has the keyboard");
            let workspace = s.workspaces.workspace_of(LiveId(1)).unwrap();
            assert_eq!(workspace.tabs.len(), 1, "the first pane of its workspace");
            assert_eq!(
                workspace.key,
                crate::ui::workspace::key_of(MachineId::local().as_str(), &path)
            );
        });
        // Typing goes straight to the terminal.
        h.type_text("hello there", cx);
        wait_until(&h, cx, "the typed text", |h, cx| {
            screen(h, cx, 1).contains("hello there")
        });
        assert_eq!(live_count(&h, cx), 1);
    }

    #[gpui_kit::test]
    fn clicking_a_history_session_does_the_same_as_enter(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, path, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        cx.update(|cx| {
            h.shell
                .update(cx, |shell, _| shell.show(&NodeId::Session(id.clone())))
        });
        h.settle(cx);
        let at = h.row_of(NodeId::Session(id), cx).unwrap();
        h.mouse_on(format!("tree-row-{at}"), gpui_kit::MouseButton::Left, cx);
        wait_until(&h, cx, "the resumed agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        assert_eq!(h.main_kind(cx), "live:1");
        assert_eq!(script_of(&h, 1).spec().cwd.as_deref(), Some(path.as_str()));
    }

    #[gpui_kit::test]
    fn a_codex_session_resumes_with_the_codex_command(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CODEX, "beta");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the codex line", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CODEX")
        });
        assert!(
            screen(&h, cx, 1).contains("READY> codex resume beta-3"),
            "{}",
            screen(&h, cx, 1)
        );
    }

    #[gpui_kit::test]
    fn an_opencode_session_resumes_with_its_session_flag(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::OPENCODE, "gamma");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the opencode line", |h, cx| {
            screen(h, cx, 1).contains("OPENCODE-RAN")
        });
        assert!(
            screen(&h, cx, 1).contains("READY> opencode --session gamma-3"),
            "{}",
            screen(&h, cx, 1)
        );
    }

    #[gpui_kit::test]
    fn an_id_with_spaces_is_quoted_for_the_shell_the_session_is_resumed_in(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "two words");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume two words-3")
        });
        assert!(screen(&h, cx, 1).contains("READY> claude --resume 'two words-3'"));
    }

    #[gpui_kit::test]
    fn opening_the_same_session_twice_focuses_the_terminal_already_running_it(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        // A second tab in the same folder, in front, and then a look at
        // something that is not a terminal.
        h.press_chord("cmd-t", "ctrl-shift-t", cx);
        assert_eq!(h.main_kind(cx), "live:2");
        h.press_chord("cmd-b", "ctrl-shift-b", cx);
        open_session_titled(&h, "fix the login bug", cx);
        assert_eq!(h.main_kind(cx), "session:fix the login bug");
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "live:1", "the terminal that has it");
        assert_eq!(live_count(&h, cx), 2, "nothing new was started");
        assert!(h.computer.script(2).is_none(), "no third terminal exists");
        h.shell(cx, |s| {
            assert!(s.terminal_focused());
            let workspace = s.workspaces.workspace_of(LiveId(1)).unwrap();
            assert_eq!(workspace.active, 0, "its tab is the one on screen");
        });
        // The palette opens it the same way.
        h.press_chord("cmd-b", "ctrl-shift-b", cx);
        open_session_titled(&h, "fix the login bug", cx);
        h.press("ctrl-p", cx);
        h.type_text("alpha", cx);
        h.settle(cx);
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "live:1");
        assert_eq!(live_count(&h, cx), 2);
    }

    #[gpui_kit::test]
    fn closing_the_resumed_terminal_lets_the_session_be_resumed_again(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        h.press("enter", cx); // Close Claude Code
        assert_eq!(live_count(&h, cx), 0);
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent again", |h, cx| {
            screen(h, cx, 2).contains("FAKE-CLAUDE --resume alpha-3")
        });
        assert_eq!(h.main_kind(cx), "live:2", "a new terminal, not the old one");
        assert_eq!(live_count(&h, cx), 1);
    }

    #[gpui_kit::test]
    fn the_resumed_session_is_one_row_that_shows_the_live_state(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id.clone()));
        let before = h.outline(cx);
        assert!(!h.shows("tree-live-led-1", cx));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        assert_eq!(
            h.outline(cx),
            before,
            "the same rows: no second, live, row for the session"
        );
        assert!(h.shows("tree-live-led-1", cx), "the row shows the light");
        let at = h.row_of(NodeId::Session(id.clone()), cx).unwrap();
        assert_eq!(
            h.shell(cx, |s| s.here_live()),
            Some(LiveId(1)),
            "the pane commands find the terminal on the row"
        );
        let _ = at;
        h.press_chord("cmd-w", "ctrl-shift-w", cx);
        h.press("enter", cx);
        assert!(!h.shows("tree-live-led-1", cx), "the light goes with it");
        assert_eq!(h.outline(cx), before);
    }

    #[gpui_kit::test]
    fn a_resumed_session_beyond_the_eighth_row_is_still_shown(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for n in 0..10 {
            stored_session(
                &h,
                cx,
                AgentId::CLAUDE,
                &MachineId::local(),
                &path,
                &format!("s{n}"),
            );
        }
        // All have the same age; only eight are listed under the folder.
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                shell.set_open(&NodeId::Unsorted(MachineId::local()).key(), true);
                shell.set_open(
                    &NodeId::Folder(MachineId::local(), path.clone()).key(),
                    true,
                );
            })
        });
        let shown = |h: &Harness, cx: &mut TestAppContext, id: &leon_core::SessionId| {
            h.row_of(NodeId::Session(id.clone()), cx).is_some()
        };
        let hidden: Vec<_> = (0..10)
            .map(|n| session_titled(&h, &format!("s{n}")))
            .filter(|id| !shown(&h, cx, id))
            .collect();
        assert_eq!(hidden.len(), 2, "two are behind show more");
        let victim = hidden[0].clone();
        let session = h.store.session(&victim).unwrap();
        cx.update_window(h.window.into(), |_, window, cx| {
            h.shell
                .update(cx, |shell, cx| shell.resume_session(session, window, cx))
        })
        .unwrap();
        h.settle(cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume")
        });
        assert!(
            shown(&h, cx, &victim),
            "a session with a terminal is never hidden behind show more"
        );
    }

    #[gpui_kit::test]
    fn a_resumed_session_opens_as_a_new_tab_of_the_workspace_that_already_has_panes(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, path, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Folder(MachineId::local(), path.clone()));
        h.press("ctrl-t", cx); // a shell in that folder
        assert_eq!(h.main_kind(cx), "live:1");
        h.press_chord("cmd-b", "ctrl-shift-b", cx);
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 2).contains("FAKE-CLAUDE --resume alpha-3")
        });
        h.shell(cx, |s| {
            let first = s.workspaces.workspace_of(LiveId(1)).unwrap();
            let second = s.workspaces.workspace_of(LiveId(2)).unwrap();
            assert_eq!(first.key, second.key, "the workspace of the folder");
            assert_eq!(second.tabs.len(), 2, "a new tab beside the shell");
            assert_eq!(second.active, 1);
        });
    }

    #[gpui_kit::test]
    fn a_session_whose_folder_is_gone_shows_its_transcript_and_says_why(cx: &mut TestAppContext) {
        let h = open_live(cx);
        // The seeded sessions ran in /srv/api, which is not on this computer.
        put_cursor_on(
            &h,
            cx,
            NodeId::Session(session_titled(&h, "fix the login bug")),
        );
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:fix the login bug");
        assert_eq!(live_count(&h, cx), 0, "no broken shell was opened");
        assert!(h.computer.script(0).is_none());
        assert!(
            h.status().contains("/srv/api no longer exists"),
            "{}",
            h.status()
        );
        assert_eq!(h.engine.status().unwrap().kind, StatusKind::Error);
        assert!(h.shows("transcript-notice", cx), "the transcript says why");
        assert!(
            h.shows("transcript-resume-in", cx),
            "and offers another folder"
        );
        assert!(h.shows("message-0", cx), "the stored messages are shown");
    }

    #[gpui_kit::test]
    fn resume_in_offers_the_other_worktrees_of_the_project_and_resumes_there(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let base = tempfile::tempdir().unwrap();
        let base_path = base
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let gone = format!("{base_path}/old");
        let alive = format!("{base_path}/new");
        std::fs::create_dir(&alive).unwrap();
        let project = h
            .store
            .add_project(&MachineId::local(), "moved", &base_path)
            .unwrap();
        h.store
            .replace_worktrees(
                &project.id,
                vec![
                    NewWorktree {
                        path: gone.clone(),
                        branch: Some("old".into()),
                        head: None,
                        is_main: true,
                    },
                    NewWorktree {
                        path: alive.clone(),
                        branch: Some("new".into()),
                        head: None,
                        is_main: false,
                    },
                ],
            )
            .unwrap();
        let id = stored_session(&h, cx, AgentId::CLAUDE, &MachineId::local(), &gone, "omega");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:omega");
        assert_eq!(live_count(&h, cx), 0);
        h.mouse_on(
            "transcript-resume-in".to_owned(),
            gpui_kit::MouseButton::Left,
            cx,
        );
        assert_eq!(h.shell(cx, |s| s.overlay), Overlay::Palette);
        assert_eq!(
            h.palette_titles(cx),
            ["moved / new"].map(str::to_owned),
            "the other worktree of the project, not the one that is gone"
        );
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume omega-3")
        });
        assert_eq!(script_of(&h, 1).spec().cwd.as_deref(), Some(alive.as_str()));
        assert_eq!(h.main_kind(cx), "live:1");
    }

    #[gpui_kit::test]
    fn an_agent_that_is_not_installed_here_shows_the_transcript_and_says_so(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        cx.update(|cx| {
            h.shell.update(cx, |shell, _| {
                shell.options.system = FakeSystem::without(&["claude"])
            })
        });
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:alpha");
        assert_eq!(live_count(&h, cx), 0);
        assert!(
            h.status()
                .contains("Claude Code is not installed on This machine."),
            "{}",
            h.status()
        );
        assert!(h.shows("transcript-notice", cx));
    }

    #[gpui_kit::test]
    fn open_transcript_shows_the_stored_messages_without_starting_a_terminal(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press_chord("cmd-shift-l", "ctrl-shift-l", cx);
        assert_eq!(h.main_kind(cx), "session:alpha");
        assert_eq!(live_count(&h, cx), 0, "nothing was started");
        assert!(h.computer.script(0).is_none());
        assert!(h.shows("message-0", cx));
        assert!(
            h.shows("transcript-hint", cx),
            "the view says how to resume"
        );
        // Enter in the transcript resumes the session.
        h.press("tab", cx);
        assert_eq!(h.shell(cx, |s| s.pane), Pane::Main);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        assert_eq!(h.main_kind(cx), "live:1");
    }

    #[gpui_kit::test]
    fn open_transcript_works_from_the_terminal_that_resumed_the_session(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        h.press_chord("cmd-shift-l", "ctrl-shift-l", cx);
        assert_eq!(h.main_kind(cx), "session:alpha");
        assert_eq!(live_count(&h, cx), 1, "the terminal keeps running");
        // A plain shell has no transcript to show.
        h.press_chord("cmd-e", "ctrl-shift-e", cx);
        h.press_chord("cmd-t", "ctrl-shift-t", cx);
        assert_eq!(h.main_kind(cx), "live:2");
        h.press_chord("cmd-shift-l", "ctrl-shift-l", cx);
        assert_eq!(h.main_kind(cx), "live:2");
        assert!(h.status().contains("did not resume a history session"));
    }

    #[gpui_kit::test]
    fn the_menu_of_a_history_session_opens_it_in_a_terminal_first(cx: &mut TestAppContext) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "alpha");
        put_cursor_on(&h, cx, NodeId::Session(id));
        h.press("m", cx);
        assert_eq!(
            menu_labels(&h, cx),
            [
                "Open",
                "Open transcript",
                "Copy session id",
                "Remove from history"
            ]
        );
        h.press("enter", cx); // Open
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume alpha-3")
        });
        assert_eq!(h.main_kind(cx), "live:1");
    }

    #[gpui_kit::test]
    fn the_palette_resumes_a_session_found_by_title_and_shows_the_transcript_at_a_message_hit(
        cx: &mut TestAppContext,
    ) {
        let h = open_live(cx);
        let (_dir, _, id) = local_session(&h, cx, AgentId::CLAUDE, "zebra");
        // A title that only the title has, and a message that only a message has.
        h.store
            .upsert_session(
                &NewSession {
                    agent: AgentId::CLAUDE,
                    external_id: "zebra-3".into(),
                    machine_id: MachineId::local(),
                    cwd: h.store.session(&id).unwrap().cwd,
                    title: "zebra".into(),
                    model: None,
                    started_at: Utc.with_ymd_and_hms(2026, 10, 4, 11, 57, 0).unwrap(),
                    updated_at: Utc.with_ymd_and_hms(2026, 10, 4, 11, 57, 0).unwrap(),
                },
                &[NewMessage {
                    role: Role::User,
                    text: "the quokka escaped".into(),
                    at: Utc.with_ymd_and_hms(2026, 10, 4, 11, 57, 0).unwrap(),
                }],
            )
            .unwrap();
        h.settle(cx);
        // A message hit shows the transcript, scrolled to the message.
        h.press_chord("cmd-shift-f", "ctrl-shift-i", cx);
        h.type_text("quokka", cx);
        h.settle(cx);
        h.press("enter", cx);
        h.settle(cx);
        assert_eq!(h.main_kind(cx), "session:zebra");
        assert_eq!(live_count(&h, cx), 0);
        assert!(h.shows("message-hit", cx));
        // The session itself, found by its title, is resumed.
        h.press("ctrl-p", cx);
        h.type_text("zebra", cx);
        h.settle(cx);
        h.press("enter", cx);
        wait_until(&h, cx, "the agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume zebra-3")
        });
        assert_eq!(h.main_kind(cx), "live:1");
    }

    // ----- a session on another machine -----------------------------------------------

    #[gpui_kit::test]
    fn a_remote_session_resumes_over_ssh_in_its_directory(cx: &mut TestAppContext) {
        let h = open_live_with(
            cx,
            ScriptedRunner::new()
                .reply(Output::ok(PROBE_OUTPUT))
                .reply(Output::ok("")),
        );
        put_cursor_on(
            &h,
            cx,
            NodeId::Session(session_titled(&h, "rotate the keys")),
        );
        h.press("enter", cx);
        wait_until(&h, cx, "the resumed agent", |h, cx| {
            screen(h, cx, 1).contains("FAKE-CLAUDE --resume rotate the keys-120")
        });
        let spec = script_of(&h, 1).spec().clone();
        assert_eq!(spec.program, "ssh");
        let remote = spec.args.last().unwrap();
        assert!(remote.starts_with("cd /opt/infra && exec "), "{remote}");
        assert!(spec.args.contains(&"-t".to_owned()));
        assert!(
            screen(&h, cx, 1).contains("READY> claude --resume 'rotate the keys-120'"),
            "{}",
            screen(&h, cx, 1)
        );
        let calls = h.runner.calls();
        assert_eq!(calls.len(), 2, "the probe and the folder: {calls:?}");
        assert!(calls[1]
            .args
            .last()
            .unwrap()
            .starts_with("cd /opt/infra && exec "));
        assert_eq!(h.main_kind(cx), "live:1");
    }

    #[gpui_kit::test]
    fn a_remote_folder_that_is_gone_shows_the_transcript_and_says_why(cx: &mut TestAppContext) {
        let h = open_live_with(
            cx,
            ScriptedRunner::new()
                .reply(Output::ok(PROBE_OUTPUT))
                .reply(Output::failed(
                    1,
                    "cd: /opt/infra: No such file or directory",
                )),
        );
        put_cursor_on(
            &h,
            cx,
            NodeId::Session(session_titled(&h, "rotate the keys")),
        );
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:rotate the keys");
        assert_eq!(live_count(&h, cx), 0);
        assert!(
            h.status()
                .contains("/opt/infra no longer exists on build box"),
            "{}",
            h.status()
        );
        assert!(h.shows("transcript-notice", cx));
    }

    #[gpui_kit::test]
    fn a_machine_that_is_offline_shows_the_transcript_and_says_so(cx: &mut TestAppContext) {
        let h = open_live_with(
            cx,
            ScriptedRunner::new().reply(Output::failed(255, "Connection refused")),
        );
        put_cursor_on(
            &h,
            cx,
            NodeId::Session(session_titled(&h, "rotate the keys")),
        );
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:rotate the keys");
        assert_eq!(live_count(&h, cx), 0);
        assert!(
            h.status().contains("build box is offline"),
            "{}",
            h.status()
        );
        assert!(h.status().contains("Connection refused"), "{}", h.status());
        assert!(h.shows("transcript-notice", cx));
    }

    #[gpui_kit::test]
    fn an_agent_that_is_not_installed_on_the_remote_machine_shows_the_transcript(
        cx: &mut TestAppContext,
    ) {
        let h = open_live_with(
            cx,
            ScriptedRunner::new().reply(Output::ok("os=Linux\nhome=/home/dev\n")),
        );
        put_cursor_on(
            &h,
            cx,
            NodeId::Session(session_titled(&h, "rotate the keys")),
        );
        h.press("enter", cx);
        assert_eq!(h.main_kind(cx), "session:rotate the keys");
        assert!(
            h.status()
                .contains("Claude Code is not installed on build box."),
            "{}",
            h.status()
        );
        assert_eq!(
            h.runner.calls().len(),
            1,
            "the folder was not even asked for"
        );
    }

    /// The few tests that run a real child in a real PTY, end to end: the
    /// agent's line is typed into a live shell and the shell survives it,
    /// keys and text reach a real program, closing a session ends its real
    /// process, a resumed session's id is quoted for a real shell. Everything
    /// else about terminals in this file is the scripted [`computer`]. The
    /// shell is [`FakeSystem`]'s: no startup file, a fixed `PATH`, nothing from
    /// the developer's own shell or home directory.
    mod real_pty {
        use super::*;
        use leon_term::{Pty, Timings};

        /// The window with real shells: waits shortened, the same code.
        fn open_real(cx: &mut TestAppContext) -> Harness {
            let h = open_live(cx);
            cx.update(|cx| {
                h.shell.update(cx, |shell, _| {
                    shell.options.backend = Rc::new(Pty {
                        timings: Timings {
                            kill_grace: Duration::from_millis(50),
                            ..Timings::default()
                        },
                    });
                    shell.options.ready = Readiness {
                        quiet: Duration::from_millis(20),
                        timeout: Duration::from_secs(5),
                        poll: Duration::from_millis(2),
                    };
                })
            });
            h
        }

        #[gpui_kit::test]
        fn an_agent_session_types_its_command_line_into_a_real_shell(cx: &mut TestAppContext) {
            let h = open_real(cx);
            let (_dir, _) = real_worktree(&h, cx);
            h.press("ctrl-n", cx);
            h.press("enter", cx);
            wait_until(&h, cx, "the command line", |h, cx| {
                screen(h, cx, 1).contains("READY> claude")
            });
            // The terminal is the shell, not the agent.
            let terminal = terminal_of(&h, cx, 1).unwrap();
            wait_until(&h, cx, "the agent", |h, cx| {
                screen(h, cx, 1).contains("FAKE-CLAUDE")
            });
            assert_eq!(
                terminal.shell_is_foreground(),
                Some(false),
                "the agent is in front of the shell"
            );
        }

        #[gpui_kit::test]
        fn when_the_agent_ends_the_real_shell_stays_open_and_takes_input(cx: &mut TestAppContext) {
            let h = open_real(cx);
            let (_dir, path) = real_worktree(&h, cx);
            h.press("ctrl-n", cx);
            h.press("down", cx);
            h.press("down", cx);
            h.press("enter", cx); // opencode: prints a line and returns
            wait_until(&h, cx, "the agent to have run", |h, cx| {
                screen(h, cx, 1).contains("OPENCODE-RAN")
            });
            wait_until(&h, cx, "the shell to get the terminal back", |h, cx| {
                terminal_of(h, cx, 1).unwrap().shell_is_foreground() == Some(true)
            });
            assert_eq!(
                state(&h, cx, 1),
                LiveState::Running,
                "the terminal did not end"
            );
            let terminal = terminal_of(&h, cx, 1).unwrap();
            assert!(terminal.exit_info().is_none());
            terminal.write(&b"pwd -P\n"[..]);
            wait_until(&h, cx, "the shell to answer", |h, cx| {
                screen(h, cx, 1).lines().any(|line| line.trim() == path)
            });
            // Only the shell ending ends the session, and then its code is shown.
            terminal.write(&b"exit 7\n"[..]);
            wait_until(&h, cx, "the exit", |h, cx| {
                state(h, cx, 1) == LiveState::Exited(7)
            });
            // The terminal's own exit is read before the window's event that
            // words the status has run: wait for that, not for a moment.
            wait_until(&h, cx, "the status line", |h, _| {
                h.status().contains("exited with code 7")
            });
            assert!(h.status().contains("exited with code 7"), "{}", h.status());
        }

        #[gpui_kit::test]
        fn keys_and_text_reach_a_real_program_as_a_terminal_sends_them(cx: &mut TestAppContext) {
            let h = open_real(cx);
            let (_dir, _) = real_worktree(&h, cx);
            h.press("ctrl-n", cx);
            h.press("enter", cx);
            wait_until(&h, cx, "the agent", |h, cx| {
                screen(h, cx, 1).contains("FAKE-CLAUDE")
            });
            let mut keys = vec!["ctrl-d", "escape", "tab", "up", "left", "enter"];
            if crate::platform::is_mac() {
                keys.insert(0, "ctrl-c");
            }
            for key in keys {
                cx.update_window(h.window.into(), |_, window, cx| window.press(key, cx))
                    .unwrap();
            }
            let echoed = if crate::platform::is_mac() {
                "^C^D^[^I^[[A^[[D^M"
            } else {
                "^D^[^I^[[A^[[D^M"
            };
            wait_until(&h, cx, "the keys to be echoed", |h, cx| {
                screen(h, cx, 1).contains(echoed)
            });
            h.type_text("héllo", cx);
            // The fake agent shows its input with `cat -vt`, and the two `cat`
            // dialects print UTF-8 differently: BSD (macOS) leaves the letter
            // as it is, GNU (Linux) writes each byte of it as `M-` and a
            // character (`é` is the bytes C3 A9, shown `M-CM-)`). Either is
            // the two bytes of `é` having reached the program.
            wait_until(&h, cx, "the text", |h, cx| {
                let screen = screen(h, cx, 1);
                screen.contains("héllo") || screen.contains("hM-CM-)llo")
            });
        }

        #[gpui_kit::test]
        fn closing_a_running_session_ends_its_real_process(cx: &mut TestAppContext) {
            let h = open_real(cx);
            let (_dir, _) = real_worktree(&h, cx);
            h.press("ctrl-n", cx);
            h.press("enter", cx);
            wait_until(&h, cx, "the agent", |h, cx| {
                screen(h, cx, 1).contains("FAKE-CLAUDE")
            });
            let pid = terminal_of(&h, cx, 1).unwrap().process_id().unwrap();
            h.press_chord("cmd-w", "ctrl-shift-w", cx);
            h.press("enter", cx);
            assert!(h.shell(cx, |s| s.live.ids().is_empty()));
            // The process is gone: nothing is left behind.
            let deadline = Instant::now() + Duration::from_secs(20);
            while libc_kill(pid as i32) == 0 {
                assert!(
                    Instant::now() < deadline,
                    "the closed session's process is still there"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn libc_kill(pid: i32) -> i32 {
            // SAFETY: signal 0 only probes for the process.
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe { kill(pid, 0) }
        }

        #[gpui_kit::test]
        fn a_resumed_sessions_id_is_quoted_for_a_real_shell_and_runs_in_its_folder(
            cx: &mut TestAppContext,
        ) {
            let h = open_real(cx);
            let dir = tempfile::tempdir().unwrap();
            let path = dir
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            session_at(
                &h.store,
                &MachineId::local(),
                &path,
                "resume me",
                3,
                &[(Role::User, "hello")],
            );
            h.settle(cx);
            put_cursor_on(&h, cx, NodeId::Session(session_titled(&h, "resume me")));
            h.press("enter", cx);
            wait_until(&h, cx, "the resumed agent", |h, cx| {
                screen(h, cx, 1).contains("FAKE-CLAUDE --resume resume me-3")
            });
            assert!(
                terminal_of(&h, cx, 1).unwrap().process_id().is_some(),
                "a real process"
            );
        }
    }
}

// ----- what the toolkit really renders ---------------------------------------------------

/// The formats Leon accepts for a logo are the ones the toolkit decodes: this
/// feeds a tiny real file of each through the toolkit's own decoder.
#[gpui_kit::test]
fn the_toolkit_decodes_every_format_a_logo_may_have(cx: &mut TestAppContext) {
    use gpui_kit::{Image, ImageFormat};
    let b64 = |text: &str| leon_remote::icon::base64_decode(text).unwrap();
    let png = b64("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==");
    // An ICO whose one entry is that PNG, as modern icon files carry them.
    let mut ico = vec![0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 32, 0];
    ico.extend((png.len() as u32).to_le_bytes());
    ico.extend(22u32.to_le_bytes());
    ico.extend(&png);
    let webp = b64("UklGRhoAAABXRUJQVlA4TA0AAAAvAAAAEAcQERGIiP4HAA==");
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="red"/></svg>"#.to_vec();
    let samples = [
        (ImageFormat::Png, png),
        (ImageFormat::Ico, ico),
        (ImageFormat::Webp, webp),
        (ImageFormat::Svg, svg),
    ];
    for (format, bytes) in samples {
        // Leon recognises it as one of its own formats, too.
        assert!(
            leon_core::icon::sniff(&bytes).is_some(),
            "{format:?} is not recognised by Leon"
        );
        let decoded =
            cx.update(|cx| Image::from_bytes(format, bytes).to_image_data(cx.svg_renderer()));
        assert!(decoded.is_ok(), "{format:?}: {:?}", decoded.err());
    }
}

#[cfg(leon_posix_tests)]
#[path = "tests_tools.rs"]
mod tools;

#[cfg(leon_posix_tests)]
#[path = "tests_lines.rs"]
mod lines;

#[cfg(leon_posix_tests)]
#[path = "tests_prefs.rs"]
mod tests_prefs;

#[cfg(leon_posix_tests)]
#[path = "tests_screen.rs"]
mod tests_screen;

#[cfg(leon_posix_tests)]
#[path = "tests_usage.rs"]
mod tests_usage;

#[cfg(leon_posix_tests)]
#[path = "tests_notify.rs"]
mod tests_notify;

#[cfg(test)]
#[path = "tests_connect.rs"]
mod tests_connect;

#[path = "tests_settings.rs"]
mod tests_settings;

#[path = "tests_updates.rs"]
mod tests_updates;

#[cfg(leon_posix_tests)]
#[path = "tests_restore.rs"]
mod tests_restore;

#[path = "tests_settings_layout.rs"]
mod tests_settings_layout;

#[path = "tests_themes.rs"]
mod themes_files;

#[cfg(leon_posix_tests)]
#[path = "tests_header.rs"]
mod header;

#[cfg(leon_posix_tests)]
#[path = "tests_elsewhere.rs"]
mod elsewhere;

#[cfg(leon_posix_tests)]
#[path = "tests_lion.rs"]
mod lion_in_window;
