//! The window: a sidebar tree and the main pane on a hairline grid, and
//! everything that moves the keyboard between them.
//!
//! ```text
//! ┌──────────────────┬──────────────────────────────┐
//! │     sidebar      │             main             │
//! └──────────────────┴──────────────────────────────┘
//! ```
//!
//! [`Shell`] owns the UI state: the rows of the tree, where the cursor is, what
//! the main pane shows and which overlay is open. Everything displayed is read
//! from the local store ([`Snapshot`]) and read again when the store announces
//! a change; the shell never calls the engine for data, only to ask for work.
//!
//! Keys are resolved against the one registry (`crate::keys`). A keystroke
//! interceptor sees every key before the text field that may have the
//! keyboard, so the shortcuts work while typing in the palette; keys with no
//! modifier only count while nothing is being typed.

use super::activity::Thresholds;
use super::expansion::{self, Expansion};
use super::filter::{self, Filter};
use super::find::{self, FindBar};
use super::lines;
use super::live::{LiveId, Sessions};
use super::logos::Logos;
use super::menu::Menu;
use super::model::Snapshot;
use super::notify;
use super::palette::PaletteState;
use super::panes::{Axis, Dir};
use super::settings_screen::SettingsUi;
use super::steps::{LiveInfo, Where, World};
use super::terminals::Readiness;
use super::tree::{self, build_rows_filtered, Kind, LiveEntry, NodeId, Placement, Row};
use super::workspace::Workspaces;
use crate::elsewhere::{self, Found, OwnTerminal};
use crate::engine::Engine;
use crate::keys::{self, Command};
use crate::launch::{self, System};
use crate::settings::{self, AppearanceChoice};
use crate::theme::{self, metrics, palette, px, Appearance, Palette as Colours};
use chrono::{DateTime, Utc};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, App, Context, Div, Entity, FocusHandle, FontWeight, Keystroke, ListAlignment, ListOffset,
    ListState, MouseButton, MouseDownEvent, PathPromptOptions, ScrollHandle, ScrollStrategy, Size,
    Stateful, Subscription, Task, UniformListScrollHandle, Window,
};
use leon_core::{MachineId, Message, ProjectId, Result as StoreResult, Session, WorktreeId};
use leon_term::{Backend, Pty};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// A drag of the sidebar's edge in progress: a plain press, move, release state
/// machine, driven by window-level listeners (see `render`), because a real
/// pointer leaves the narrow handle at once.
#[derive(Clone, Copy, Debug)]
pub struct SidebarDrag {
    /// The width (at 100 %) before the press.
    pub start: u16,
    /// The width the pointer asks for now, between the limits.
    pub width: u16,
    /// Whether the pointer is far enough below the minimum to close.
    pub closing: bool,
}

/// The two panes the keyboard moves between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The tree of machines, projects, worktrees and sessions.
    Sidebar,
    /// What was opened.
    Main,
}

/// What is open over the panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// Nothing.
    None,
    /// The palette.
    Palette,
    /// The sheet of shortcuts.
    Shortcuts,
    /// The context menu of a row of the tree.
    Menu,
    /// The About panel.
    About,
    /// The problems of the theme files.
    Problems,
    /// The Settings screen.
    Settings,
    /// The "Connect a machine" screen (SSH).
    Connect,
    /// The usage view: how much of each agent's limits is left.
    Usage,
    /// "Connect a machine, with a code".
    Pair,
    /// "Share this machine".
    Share,
}

/// What the folder picker answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    /// The folder that was chosen.
    Folder(PathBuf),
    /// Nothing was chosen.
    Cancelled,
    /// The platform has no folder picker to show.
    Unavailable,
}

/// Shows a folder in the system's file manager.
pub type Reveal = Rc<dyn Fn(&mut App, &std::path::Path)>;

/// Brings an application forward, given the path of its bundle.
pub type RevealApp = Rc<dyn Fn(&mut App, &str)>;

/// Shows the folder picker and says what was chosen.
pub type PickFolder = Rc<dyn Fn(&mut App) -> Task<Picked>>;

/// The system's own folder picker. Every way of opening a project on this
/// computer ends here; the log line is how a run shows the call was reached.
pub fn system_picker(cx: &mut App) -> Task<Picked> {
    tracing::info!("opening the system folder dialog (prompt_for_paths)");
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open project".into()),
    });
    cx.spawn(async move |_| match paths.await {
        Ok(Ok(Some(mut paths))) => paths.pop().map_or(Picked::Cancelled, Picked::Folder),
        Ok(Ok(None)) => Picked::Cancelled,
        _ => Picked::Unavailable,
    })
}

/// What tests replace so that nothing waits on the clock, the calendar or a
/// system dialog.
#[derive(Clone)]
pub struct Options {
    /// After a reload, how long further store changes are batched into the
    /// next one.
    pub reload_throttle: Duration,
    /// How long the palette waits after the last keystroke before it reads
    /// the store.
    pub search_debounce: Duration,
    /// What time it is: ages and "worked on this week" are read from it.
    pub now: fn() -> DateTime<Utc>,
    /// How a folder is picked on this computer.
    pub pick_folder: PickFolder,
    /// What this computer is asked when a session starts: where the agents
    /// are, which shell is the login shell, whether a folder exists.
    pub system: Rc<dyn System>,
    /// Shows a folder in the system's file manager.
    pub reveal: Reveal,
    /// Where terminals come from: a real PTY, or a scripted stand-in in tests
    /// of what Leon does with a terminal rather than of the terminal.
    pub backend: Rc<dyn Backend>,
    /// When a new shell is ready for an agent's command line.
    pub ready: Readiness,
    /// How a worktree's activity dot reads its terminals.
    pub activity: Thresholds,
    /// How long the lion shows an error after a session ends with one.
    pub error_flash: Duration,
    /// How a note reaches the desktop's notification centre; tests record the
    /// notes instead.
    pub notify: notify::Notify,
    /// How long a banner stays on screen.
    pub banner_duration: Duration,
    /// How an image file is picked on this computer, for a project's logo.
    pub pick_image: PickFolder,
    /// Where a file is saved: the system's save dialog.
    pub save_file: super::terminal_tools::SaveDialog,
    /// Reads the system clipboard: tests answer with their own.
    pub read_clipboard: super::paste::ReadClipboard,
    /// How the application ends: tests record it instead.
    pub quit: Rc<dyn Fn(&mut App)>,
    /// How often the themes folder is listed; none for no timer (tests ask).
    pub theme_poll: Option<Duration>,
    /// How often, while the window is focused, the processes of this
    /// computer are listed for sessions running in another terminal; none for
    /// no timer (tests ask).
    pub elsewhere_poll: Option<Duration>,
    /// Whether the usage limits are read again on a timer (the setting names
    /// the interval). Tests turn it off.
    pub usage_timer: bool,
    /// Brings an application forward: the terminal a session runs in, given
    /// the path of its bundle (macOS).
    pub reveal_app: RevealApp,
    /// Opens a file in the system's editor: `settings.json`.
    pub open_file: Reveal,
    /// How a private key file is picked, for the Connect screen.
    pub pick_key: PickFolder,
    /// The user's `~/.ssh`, as far as Leon looks at it.
    pub ssh_dir: Arc<dyn leon_remote::connect::SshDir>,
    /// Connections to other computers and sharing this one; absent in tests
    /// that do not need them.
    pub remote: Option<Arc<crate::remote::Remote>>,
}

/// A `~/.ssh` that is not there: the home folder is unknown.
struct NoSshDir;

impl leon_remote::connect::SshDir for NoSshDir {
    fn config(&self) -> Option<String> {
        None
    }
    fn known_hosts(&self) -> Option<String> {
        None
    }
    fn public_keys(&self) -> Vec<leon_remote::connect::KeyFile> {
        Vec::new()
    }
    fn append_known_hosts(&self, _: &[String]) -> std::io::Result<()> {
        Err(std::io::Error::other("the home folder is not known"))
    }
}

/// How many polls of this computer pass between two of the SSH machines.
pub const REMOTE_EVERY: u32 = 6;

/// How often the banners' expiry is looked at while any is on screen.
const BANNER_TICK: Duration = Duration::from_millis(250);

impl Default for Options {
    fn default() -> Self {
        Self {
            reload_throttle: Duration::from_millis(50),
            search_debounce: super::palette::SEARCH_DEBOUNCE,
            now: Utc::now,
            pick_folder: Rc::new(system_picker),
            system: Rc::new(launch::RealSystem),
            reveal: Rc::new(|cx, path| cx.reveal_path(path)),
            backend: Rc::new(Pty::default()),
            ready: Readiness::default(),
            activity: Thresholds::default(),
            error_flash: Duration::from_secs(4),
            notify: Rc::new(notify::system),
            banner_duration: Duration::from_secs(8),
            pick_image: super::projects::default_image_picker(),
            save_file: Rc::new(super::terminal_tools::system_save_dialog),
            read_clipboard: Rc::new(|cx| cx.read_from_clipboard()),
            quit: Rc::new(|cx| cx.quit()),
            theme_poll: Some(crate::theme::watch::POLL),
            elsewhere_poll: Some(Duration::from_secs(5)),
            usage_timer: true,
            open_file: Rc::new(|cx, path| cx.open_with_system(path)),
            pick_key: Rc::new(super::connect::system_key_picker),
            ssh_dir: match leon_remote::connect::RealSshDir::home() {
                Some(dir) => Arc::new(dir),
                None => Arc::new(NoSshDir),
            },
            remote: None,
            reveal_app: Rc::new(|_, bundle| {
                // `open` on a running application brings it forward.
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(bundle).spawn();
                #[cfg(not(target_os = "macos"))]
                let _ = bundle;
            }),
        }
    }
}

/// A session's transcript, as far as it has been read.
pub struct Transcript {
    /// The session.
    pub session: Session,
    /// Its messages, once read.
    pub messages: Option<Arc<Vec<Message>>>,
    /// The scroll state of the virtualised list.
    pub list: ListState,
    /// The message a search hit pointed at, by `seq`.
    pub hit: Option<u32>,
    /// Why it could not be read.
    pub failed: Option<String>,
    /// Why this is shown instead of a terminal that resumes the session.
    pub notice: Option<Notice>,
}

/// What a transcript says about why it is on screen where a terminal was
/// asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// What went wrong: the folder is gone, the machine is off, the agent is
    /// not installed.
    pub text: String,
    /// Whether another worktree could host the session instead ("Resume
    /// in…").
    pub other_folder: bool,
    /// Set when the session is shown because it runs in another terminal.
    pub elsewhere: Option<ElsewhereNote>,
}

/// What a transcript knows of the process that holds its session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElsewhereNote {
    /// The process.
    pub pid: u32,
    /// Whether the match is only the best fit (`Enter` then resumes).
    pub likely: bool,
    /// Whether the terminal application can be brought forward.
    pub can_reveal: bool,
}

impl Notice {
    /// A notice that only says what went wrong.
    pub fn plain(text: String, other_folder: bool) -> Self {
        Self {
            text,
            other_folder,
            elsewhere: None,
        }
    }
}

impl Transcript {
    /// The position in the list of the message the hit pointed at.
    pub fn hit_index(&self) -> Option<usize> {
        let seq = self.hit?;
        self.messages
            .as_ref()?
            .iter()
            .position(|message| message.seq == seq)
    }
}

/// What the main pane shows.
pub enum Main {
    /// Nothing is open.
    Empty,
    /// A project.
    Project(ProjectId),
    /// A worktree of a project.
    Worktree(ProjectId, WorktreeId),
    /// A session's transcript.
    Session(Box<Transcript>),
    /// A live terminal session.
    Live(LiveId),
}

/// The application window's view.
pub struct Shell {
    pub(super) engine: Engine,
    pub(super) options: Options,
    pub(super) focus: FocusHandle,
    pub(super) pane: Pane,
    pub(super) overlay: Overlay,
    pub(super) snapshot: Snapshot,
    pub(super) placement: Placement,
    pub(super) expansion: Expansion,
    expansion_file: Option<PathBuf>,
    pub(super) rows: Vec<Row>,
    pub(super) cursor: Option<usize>,
    pub(super) tree_scroll: UniformListScrollHandle,
    pub(super) main: Main,
    /// The terminals that are running.
    pub(super) live: Sessions,
    /// How the terminals are laid out: workspaces of tabs of split panes.
    pub(super) workspaces: Workspaces,
    pub(super) palette: PaletteState,
    /// The count of settings changes the window last acted on.
    pub(super) applied_settings: u64,
    /// The Settings screen: where the keyboard is in it, what is searched.
    pub(super) settings_ui: SettingsUi,
    /// The "Connect a machine" screen.
    pub(super) connect_ui: super::connect::ConnectUi,
    /// The usage bar and view.
    pub(super) usage: super::usage_view::UsageUi,
    /// "Connect a machine, with a code".
    pub(super) pair_ui: super::pair::PairUi,
    /// "Share this machine".
    pub(super) share_ui: super::share::ShareUi,
    /// The sidebar's filter field, the text it holds, and what that leaves of
    /// the tree (`None` while it is empty).
    pub(super) filter_input: Entity<InputState>,
    pub(super) filter_query: String,
    pub(super) filter: Option<Filter>,
    /// What each project is called, where several share a name.
    pub(super) labels: std::collections::HashMap<ProjectId, String>,
    /// The project logos read from the store.
    pub(super) logos: Logos,
    /// Watches the terminals' activity while any is live.
    pub(super) ticker: Option<Task<()>>,
    /// The notifications shown for what sessions just did, oldest first.
    pub(super) banners: Vec<notify::Banner>,
    /// Watches the banners' expiry while any is on screen.
    pub(super) banner_ticker: Option<Task<()>>,
    /// The id the next banner gets.
    pub(super) next_banner: u64,
    pub(super) choosing: Option<Task<()>>,
    /// The context menu, while one is open.
    pub(super) menu: Option<Menu>,
    pub(super) sheet_scroll: ScrollHandle,
    /// The find bar of each terminal pane that has had one.
    pub(super) find: std::collections::HashMap<LiveId, FindBar>,
    /// The field of the find bar, and the pane whose query it shows.
    pub(super) find_input: Entity<InputState>,
    pub(super) find_shown: Option<LiveId>,
    /// The drag of the sidebar's edge, while the button is down.
    pub(super) sidebar_drag: Option<SidebarDrag>,
    /// The write of a saved terminal, while it runs.
    pub(super) saving: Option<Task<()>>,
    /// The themes folder and how it is followed.
    pub(super) themes: super::themes::ThemeFiles,
    /// What the menu bar was last told is available.
    menu_state: crate::menus::Availability,
    pub(super) viewport: Size<gpui_kit::Pixels>,
    loading: Option<Task<()>>,
    picking: Option<Task<()>>,
    opening: Option<Task<()>>,
    /// The check of a remote machine and folder before a session is resumed
    /// there.
    pub(super) resuming: Option<Task<()>>,
    /// The look for a process that holds a session, before it is opened.
    pub(super) checking: Option<Task<()>>,
    /// The coarse timer of that look, while the window lives.
    elsewhere_ticker: Option<Task<()>>,
    /// Whether the window has the focus: the lion sleeps without it.
    pub(super) window_active: bool,
    /// A session ended with an error a moment ago: the lion shows it.
    pub(super) mark_error: bool,
    /// Ends [`Shell::mark_error`] after [`Options::error_flash`].
    pub(super) mark_error_timer: Option<Task<()>>,
    /// Whether the lion's entrance has been played: it plays once, when the
    /// window opens.
    pub(super) intro_played: std::cell::Cell<bool>,
    _watchers: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// The window's view over what `engine` keeps in the store.
    pub fn new(
        engine: Engine,
        options: Options,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = engine.store().clone();
        let snapshot = Snapshot::load(&store).unwrap_or_default();
        let placement = Placement::compute(&snapshot);
        let expansion_file = settings::sibling(expansion::FILE_NAME, cx);
        let expansion = expansion_file
            .as_deref()
            .map(Expansion::load)
            .unwrap_or_default();
        let rows = build_rows_filtered(&snapshot, &placement, &expansion, (options.now)(), None);
        let cursor = (!rows.is_empty()).then_some(0);
        let palette = PaletteState::new(window, cx);
        let settings_ui = SettingsUi::new(window, cx);
        let connect_ui = super::connect::ConnectUi::new(window, cx);
        let pair_ui = super::pair::PairUi::new(window, cx);
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter projects"));
        let labels = filter::project_labels(&snapshot);
        let find_input = find::new_input(window, cx);

        let own_window = window.window_handle();
        let weak = cx.weak_entity();
        let subscriptions = vec![
            // Every key is seen here before the text field that may have the
            // keyboard, so the shortcuts work while typing in the palette.
            cx.intercept_keystrokes(move |event, window, cx| {
                if window.window_handle() != own_window {
                    return;
                }
                let taken = weak
                    .update(cx, |this, cx| this.handle_key(&event.keystroke, window, cx))
                    .unwrap_or(false);
                if taken {
                    cx.stop_propagation();
                }
            }),
            cx.subscribe_in(
                &filter_input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.filter_changed(cx);
                    }
                },
            ),
            cx.subscribe_in(&find_input, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.find_changed(cx);
                }
            }),
            cx.subscribe_in(
                &settings_ui.search,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.settings_search_changed(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &palette.input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.palette_changed(cx);
                    }
                },
            ),
            // Back in the window: what runs in other terminals may have changed.
            cx.observe_window_activation(window, |this, window, cx| {
                this.window_active = window.is_window_active();
                if window.is_window_active() {
                    this.scan_elsewhere_now(true, cx);
                }
                cx.notify();
            }),
            // When the focused element goes away (the palette closes) the
            // keyboard comes back to the window, so the shortcuts always have
            // somewhere to land.
            cx.on_focus_lost(window, |this, window, cx| match this.overlay {
                Overlay::Palette => this
                    .palette
                    .input
                    .update(cx, |field, cx| field.focus(window, cx)),
                Overlay::Settings => this.settings_focus(window, cx),
                Overlay::Connect => this.connect_focus(window, cx),
                Overlay::Pair => this.pair_focus(window, cx),
                _ => {
                    this.focus.focus(window, cx);
                    this.sync_focus(window, cx);
                }
            }),
        ];

        // The store announces each write; the lists are read again, batched.
        let mut changes = store.subscribe();
        let throttle = options.reload_throttle;
        let store_watcher = cx.spawn(async move |this, cx| {
            while changes.next().await.is_some() {
                while changes.try_next().is_some() {}
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
                if !throttle.is_zero() {
                    cx.background_executor().timer(throttle).await;
                }
            }
        });
        // The engine's own status line and machine lights.
        let mut events = engine.subscribe();
        let engine_watcher = cx.spawn(async move |this, cx| {
            use tokio::sync::broadcast::error::RecvError;
            while !matches!(events.recv().await, Err(RecvError::Closed)) {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });

        // Closing the window ends the application, so it asks what quitting
        // asks.
        let closing = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            closing
                .update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let mut shell = Self {
            engine,
            options,
            focus,
            pane: Pane::Sidebar,
            overlay: Overlay::None,
            snapshot,
            placement,
            expansion,
            expansion_file,
            rows,
            cursor,
            tree_scroll: UniformListScrollHandle::new(),
            main: Main::Empty,
            live: Sessions::default(),
            workspaces: Workspaces::default(),
            palette,
            applied_settings: 0,
            settings_ui,
            connect_ui,
            usage: super::usage_view::UsageUi::default(),
            pair_ui,
            share_ui: super::share::ShareUi::default(),
            filter_input,
            filter_query: String::new(),
            filter: None,
            labels,
            logos: Logos::default(),
            ticker: None,
            banners: Vec::new(),
            banner_ticker: None,
            next_banner: 0,
            choosing: None,
            menu: None,
            sheet_scroll: ScrollHandle::new(),
            find: std::collections::HashMap::new(),
            find_input,
            find_shown: None,
            sidebar_drag: None,
            saving: None,
            themes: super::themes::ThemeFiles::new(settings::sibling(super::themes::FOLDER, cx)),
            menu_state: crate::menus::Availability::default(),
            viewport: window.viewport_size(),
            loading: None,
            picking: None,
            opening: None,
            resuming: None,
            checking: None,
            elsewhere_ticker: None,
            window_active: window.is_window_active(),
            mark_error: false,
            mark_error_timer: None,
            intro_played: std::cell::Cell::new(false),
            _watchers: vec![store_watcher, engine_watcher],
            _subscriptions: subscriptions,
        };
        shell.load_logos(cx);
        shell.watch_themes(cx);
        shell.watch_elsewhere(window, cx);
        shell.usage_reload();
        shell.watch_usage(window, cx);
        // Values of settings.json that could not be used were read as defaults.
        let problems = settings::take_problems(cx);
        shell.report_problems(&problems);
        // A saved theme whose file is gone was replaced by the default.
        for missing in crate::theme::registry::take_missing() {
            shell.engine.report(
                crate::engine::StatusKind::Info,
                format!(
                    "Theme {missing:?} is not installed: using {}.",
                    crate::theme::ThemeId::DEFAULT.name()
                ),
            );
        }
        shell
    }

    // ----- sessions running in another terminal ----------------------------------------

    /// Lists the processes of this computer, and of the SSH machines that
    /// answer when `remote`, for sessions running in another terminal. Does
    /// nothing while the engine has no scanner.
    pub(super) fn scan_elsewhere_now(&self, remote: bool, cx: &App) {
        if !self.detecting_elsewhere(cx) {
            return;
        }
        for machine in &self.snapshot.machines {
            let online = matches!(
                self.engine.machine_state(&machine.id),
                crate::engine::MachineState::Online(_)
            );
            if machine.id.is_local() || (remote && online) {
                self.engine
                    .submit(crate::engine::Op::Scan(machine.id.clone()));
            }
        }
    }

    /// The coarse timer: while the window is focused, this computer is looked
    /// at every `elsewhere_poll`, the SSH machines every few polls.
    fn watch_elsewhere(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.options.elsewhere_poll.is_none() {
            return;
        }
        self.elsewhere_ticker = Some(cx.spawn_in(window, async move |this, cx| {
            let mut tick = 0u32;
            loop {
                // The interval is a setting, read again at each turn.
                let Ok(poll) = this.read_with(cx, |_, cx| Self::elsewhere_interval(cx)) else {
                    return;
                };
                cx.background_executor().timer(poll).await;
                tick = tick.wrapping_add(1);
                let alive = this.update_in(cx, |this, window, cx| {
                    if window.is_window_active() {
                        this.scan_elsewhere_now(tick % REMOTE_EVERY == 0, cx);
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
        }));
    }

    /// Leon's own agent terminals on a machine, as far as the process tree of
    /// another machine cannot tell them from strangers.
    fn own_terminals(&self, machine: &MachineId) -> Vec<OwnTerminal> {
        self.live
            .all()
            .iter()
            .filter(|session| &session.machine == machine)
            .filter_map(|session| {
                Some(OwnTerminal {
                    agent: session.agent?,
                    cwd: session.cwd.clone(),
                    session: session.history.clone(),
                })
            })
            .collect()
    }

    /// The process in another terminal that holds `session`, as the last scan
    /// saw it; a certain match wins over a likely one.
    pub(super) fn elsewhere_of(&self, session: &Session) -> Option<Found> {
        let report = self.engine.elsewhere(&session.machine_id)?;
        let away = elsewhere::foreign(
            &report.found,
            session.machine_id.is_local(),
            &self.own_terminals(&session.machine_id),
        );
        let mut mine: Vec<Found> = away
            .into_iter()
            .filter(|found| found.session.as_ref() == Some(&session.id))
            .collect();
        mine.sort_by_key(|found| !found.is_certain());
        mine.into_iter().next()
    }

    // ----- reading the store ---------------------------------------------------------

    /// What time it is, for ages and for what counts as recent.
    pub(super) fn now(&self) -> DateTime<Utc> {
        (self.options.now)()
    }

    /// Reads the store again and rebuilds the tree, keeping the cursor on the
    /// same node when it is still there.
    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let snapshot = match Snapshot::load(self.engine.store()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "could not read the store");
                return;
            }
        };
        self.placement = self.place(&snapshot);
        self.snapshot = snapshot;
        self.refilter();
        self.rebuild_rows();
        self.load_logos(cx);
        self.usage_reload();
        // What was open may be gone.
        let gone = match &self.main {
            Main::Project(id) => self.snapshot.project(id).is_none(),
            Main::Worktree(project, worktree) => !self
                .snapshot
                .project(project)
                .is_some_and(|entry| entry.worktrees.iter().any(|w| &w.id == worktree)),
            Main::Live(id) => self.live.get(*id).is_none(),
            Main::Empty | Main::Session(_) => false,
        };
        if gone {
            self.main = Main::Empty;
        }
        if self.overlay == Overlay::Settings {
            self.refresh_dismissed();
        }
        if self.overlay == Overlay::Palette {
            self.palette.world = Some(self.world(cx));
            if self.palette.flow.is_none() {
                self.fill_palette(cx);
            }
        }
        cx.notify();
    }

    /// Where everything goes in the tree: the history and the live sessions.
    pub(super) fn place(&self, snapshot: &Snapshot) -> Placement {
        Placement::compute(snapshot).with_live(snapshot, self.live_entries())
    }

    /// The live sessions as the tree sees them.
    fn live_entries(&self) -> Vec<LiveEntry> {
        self.live
            .all()
            .iter()
            .map(|session| LiveEntry {
                id: session.id,
                machine: session.machine.clone(),
                cwd: session.cwd.clone(),
                agent: session.agent,
                history: session.history.clone(),
            })
            .collect()
    }

    /// Flattens the tree again, from what is open. The cursor follows its node.
    pub(super) fn rebuild_rows(&mut self) {
        let key = self
            .cursor
            .and_then(|index| self.rows.get(index))
            .map(Row::key);
        self.rows = build_rows_filtered(
            &self.snapshot,
            &self.placement,
            &self.expansion,
            self.now(),
            self.filter.as_ref(),
        );
        self.cursor = tree::follow(&self.rows, key.as_deref(), self.cursor.unwrap_or(0));
    }

    /// The machines, projects and folders, and where the keyboard is, for the
    /// palette's questions.
    pub(super) fn world(&self, cx: &Context<Self>) -> World {
        let chosen = settings::get(cx);
        World::build(
            &self.snapshot,
            &self.placement,
            Some(self.current_machine()),
            self.here(),
            chosen.theme,
            settings::kept_theme_id(cx),
            chosen.interface_scale,
        )
        .with_prefs(Self::step_prefs(cx))
        .with_repositories(
            self.connect_ui
                .repos
                .iter()
                .flat_map(|(machine, roots)| {
                    roots
                        .iter()
                        .map(move |root| (machine.clone(), root.clone()))
                })
                .collect(),
        )
        .with_live(self.live_infos(cx), self.here_live())
        .with_targets(
            self.machine_row(),
            self.here_session()
                .map(|session| (session.id, session.title)),
            self.rename_target(cx),
        )
        .with_elsewhere(self.here_session().and_then(|session| {
            let found = self.elsewhere_of(&session)?;
            Some(super::steps::ElsewhereTarget {
                session: session.id,
                title: session.title,
                pid: found.pid,
                likely: !found.is_certain(),
            })
        }))
        .with_resume(self.here_session().map(|session| {
            let root = tree::workspace_root(&self.snapshot, &session.machine_id, &session.cwd);
            let project = tree::detail_of_root(&self.snapshot, &session.machine_id, &root)
                .map(|(project, _)| project);
            super::steps::ResumeTarget {
                session: session.id,
                machine: session.machine_id,
                cwd: session.cwd,
                project,
            }
        }))
    }

    /// The machine whose row the cursor is on.
    fn machine_row(&self) -> Option<MachineId> {
        match &self.rows.get(self.cursor?)?.kind {
            Kind::Machine(machine) => Some(machine.id.clone()),
            _ => None,
        }
    }

    /// The history session the keyboard is on: the cursor's row while the
    /// sidebar has the keyboard, else the open transcript.
    pub(super) fn here_session(&self) -> Option<Session> {
        let from_row = || match &self.rows.get(self.cursor?)?.kind {
            Kind::Session(session) => Some(session.clone()),
            _ => None,
        };
        let from_main = || match &self.main {
            Main::Session(transcript) => Some(transcript.session.clone()),
            _ => None,
        };
        if self.pane == Pane::Sidebar {
            from_row().or_else(from_main)
        } else {
            from_main().or_else(from_row)
        }
    }

    /// What a rename would rename: the machine or the live terminal the
    /// keyboard is on.
    fn rename_target(&self, cx: &App) -> Option<super::steps::RenameTarget> {
        use super::steps::RenameTarget;
        let machine = || {
            let id = self.machine_row()?;
            let machine = self.snapshot.machine(&id)?;
            (!id.is_local()).then(|| RenameTarget::Machine(id, machine.name.clone()))
        };
        let live = || {
            let id = self.here_live()?;
            let session = self.live.get(id)?;
            let _ = cx;
            Some(RenameTarget::Live(id, session.label()))
        };
        if self.pane == Pane::Sidebar {
            machine().or_else(live)
        } else {
            live().or_else(machine)
        }
    }

    // ----- what the tree's commands do to the row under the cursor --------------------

    /// Copies a text to the clipboard and says so.
    pub(super) fn copy_text(&mut self, what: &str, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
        self.engine.report(
            crate::engine::StatusKind::Info,
            format!("Copied the {what}: {text}"),
        );
    }

    /// The row's own kind, when the cursor is on a row.
    fn cursor_kind(&self) -> Option<Kind> {
        Some(self.rows.get(self.cursor?)?.kind.clone())
    }

    pub(super) fn copy_path_here(&mut self, cx: &mut Context<Self>) {
        match self.here() {
            Some(place) => self.copy_text("path", place.cwd, cx),
            None => self
                .engine
                .report(crate::engine::StatusKind::Info, "Select a folder first."),
        }
    }

    pub(super) fn copy_branch_here(&mut self, cx: &mut Context<Self>) {
        match self.cursor_kind() {
            Some(Kind::Worktree { worktree, .. }) => match worktree.branch {
                Some(branch) => self.copy_text("branch name", branch, cx),
                None => self.engine.report(
                    crate::engine::StatusKind::Info,
                    "This worktree has no branch: its head is detached.",
                ),
            },
            _ => self
                .engine
                .report(crate::engine::StatusKind::Info, "Select a worktree first."),
        }
    }

    pub(super) fn copy_session_id_here(&mut self, cx: &mut Context<Self>) {
        let id = match (self.cursor_kind(), &self.main) {
            (Some(Kind::Session(session)), _) if self.pane == Pane::Sidebar => {
                Some(session.external_id)
            }
            (_, Main::Session(transcript)) => Some(transcript.session.external_id.clone()),
            (Some(Kind::Session(session)), _) => Some(session.external_id),
            _ => None,
        };
        match id {
            Some(id) => self.copy_text("session id", id, cx),
            None => self.engine.report(
                crate::engine::StatusKind::Info,
                "Select a history session first.",
            ),
        }
    }

    pub(super) fn reveal_here(&mut self, cx: &mut Context<Self>) {
        match self.here() {
            Some(place) if place.machine.is_local() => {
                let reveal = self.options.reveal.clone();
                reveal(cx, std::path::Path::new(&place.cwd));
            }
            Some(_) => self.engine.report(
                crate::engine::StatusKind::Info,
                "Showing a folder in the file manager only works on this computer.",
            ),
            None => self
                .engine
                .report(crate::engine::StatusKind::Info, "Select a folder first."),
        }
    }

    /// Shows the stored transcript of the history session the keyboard is
    /// on: the row under the cursor, or, from a terminal, the session it
    /// resumed. Nothing is started.
    pub(super) fn open_transcript_here(&mut self, cx: &mut Context<Self>) {
        let from_live = |id: LiveId| {
            self.live
                .get(id)
                .and_then(|live| live.history.as_ref())
                .and_then(|history| {
                    self.snapshot
                        .sessions
                        .iter()
                        .find(|session| &session.id == history)
                        .cloned()
                })
        };
        let session = match (self.pane, &self.main, self.cursor_kind()) {
            (Pane::Main, Main::Live(id), _) => match from_live(*id) {
                Some(session) => session,
                None => {
                    self.engine.report(
                        crate::engine::StatusKind::Info,
                        "This terminal did not resume a history session.",
                    );
                    return;
                }
            },
            (_, _, Some(Kind::Session(session))) => session,
            (_, _, Some(Kind::Live(entry))) => match from_live(entry.id) {
                Some(session) => session,
                None => {
                    self.engine.report(
                        crate::engine::StatusKind::Info,
                        "This terminal did not resume a history session.",
                    );
                    return;
                }
            },
            (_, Main::Session(transcript), _) => transcript.session.clone(),
            _ => {
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    "Select a history session first.",
                );
                return;
            }
        };
        self.open_session(session, None, cx);
    }

    fn live_infos(&self, cx: &App) -> Vec<LiveInfo> {
        self.live
            .all()
            .iter()
            .map(|session| LiveInfo {
                id: session.id,
                label: session.label(),
                busy: session.busy(cx),
            })
            .collect()
    }

    /// The live session on screen or under the cursor, whichever has the
    /// keyboard.
    pub(super) fn here_live(&self) -> Option<LiveId> {
        let from_row = || match &self.rows.get(self.cursor?)?.kind {
            Kind::Live(entry) => Some(entry.id),
            // A history session with a terminal is that terminal's row.
            Kind::Session(session) => self.live.of_history(&session.id).map(|live| live.id),
            _ => None,
        };
        let from_main = || match &self.main {
            Main::Live(id) => Some(*id),
            _ => None,
        };
        if self.pane == Pane::Sidebar {
            from_row().or_else(from_main)
        } else {
            from_main().or_else(from_row)
        }
    }

    /// The machine the keyboard is on: the one of the row under the cursor
    /// while the sidebar has the keyboard, else of what is open, else this
    /// computer.
    pub(super) fn current_machine(&self) -> MachineId {
        let from_row = || Some(self.rows.get(self.cursor?)?.machine.clone());
        let from_main = || match &self.main {
            Main::Project(id) | Main::Worktree(id, _) => self
                .snapshot
                .project(id)
                .map(|entry| entry.project.machine_id.clone()),
            Main::Session(transcript) => Some(transcript.session.machine_id.clone()),
            Main::Live(id) => self.live.get(*id).map(|session| session.machine.clone()),
            Main::Empty => None,
        };
        let found = if self.pane == Pane::Sidebar {
            from_row().or_else(from_main)
        } else {
            from_main().or_else(from_row)
        };
        found.unwrap_or_else(MachineId::local)
    }

    /// Where a new session or worktree would go: the row under the cursor
    /// while the sidebar has the keyboard, else what is open, else that row.
    pub(super) fn here(&self) -> Option<Where> {
        let from_row = || -> Option<(MachineId, Option<ProjectId>, String)> {
            let row = self.rows.get(self.cursor?)?;
            match &row.kind {
                Kind::Project { project, .. } => Some((
                    row.machine.clone(),
                    Some(project.id.clone()),
                    project.root.clone(),
                )),
                Kind::Worktree { worktree, .. } => Some((
                    row.machine.clone(),
                    Some(worktree.project_id.clone()),
                    worktree.path.clone(),
                )),
                Kind::Session(session) => Some((
                    row.machine.clone(),
                    session.project_id.clone(),
                    session.cwd.clone(),
                )),
                Kind::Folder { cwd, .. } => Some((row.machine.clone(), None, cwd.clone())),
                Kind::Live(entry) => Some((row.machine.clone(), None, entry.cwd.clone())),
                Kind::Machine(_)
                | Kind::Unsorted { .. }
                | Kind::More { .. }
                | Kind::Open
                | Kind::NoMatch => None,
            }
        };
        let from_main = || -> Option<(MachineId, Option<ProjectId>, String)> {
            match &self.main {
                Main::Project(id) => {
                    let entry = self.snapshot.project(id)?;
                    Some((
                        entry.project.machine_id.clone(),
                        Some(id.clone()),
                        entry.project.root.clone(),
                    ))
                }
                Main::Worktree(project, worktree) => {
                    let entry = self.snapshot.project(project)?;
                    let worktree = entry.worktrees.iter().find(|w| &w.id == worktree)?;
                    Some((
                        entry.project.machine_id.clone(),
                        Some(project.clone()),
                        worktree.path.clone(),
                    ))
                }
                Main::Session(transcript) => Some((
                    transcript.session.machine_id.clone(),
                    transcript.session.project_id.clone(),
                    transcript.session.cwd.clone(),
                )),
                Main::Live(id) => {
                    let session = self.live.get(*id)?;
                    Some((session.machine.clone(), None, session.cwd.clone()))
                }
                Main::Empty => None,
            }
        };
        let (machine, project, cwd) = if self.pane == Pane::Sidebar {
            from_row().or_else(from_main)
        } else {
            from_main().or_else(from_row)
        }?;
        let machine_name = self.snapshot.machine(&machine)?.name.clone();
        Some(Where {
            machine,
            machine_name,
            project,
            cwd,
        })
    }

    // ----- the tree ------------------------------------------------------------------

    pub(super) fn move_cursor_to(&mut self, index: usize) {
        self.cursor = Some(index);
        let strategy = if index == 0 {
            ScrollStrategy::Top
        } else {
            ScrollStrategy::Nearest
        };
        self.tree_scroll.scroll_to_item(index, strategy);
    }

    /// Puts the cursor on `node`, opening what hides it.
    pub(super) fn show(&mut self, node: &NodeId) {
        let key = node.key();
        if !self.rows.iter().any(|row| row.key() == key) {
            let now = self.now();
            tree::reveal(
                &self.snapshot,
                &self.placement,
                node,
                &mut self.expansion,
                now,
            );
            self.save_expansion();
            self.rebuild_rows();
        }
        if let Some(index) = self.rows.iter().position(|row| row.key() == key) {
            self.move_cursor_to(index);
        }
    }

    fn save_expansion(&self) {
        if let Some(file) = &self.expansion_file {
            if let Err(error) = self.expansion.save(file) {
                tracing::warn!(%error, "could not remember what is open");
            }
        }
    }

    /// Opens or folds a node, and remembers the choice.
    pub(super) fn set_open(&mut self, key: &str, open: bool) {
        if self.expansion.set_open(key, open) {
            self.save_expansion();
        }
        self.rebuild_rows();
    }

    /// Opens or folds the node of the row at `index`, if it can be.
    pub(super) fn toggle(&mut self, index: usize) {
        if let Some(row) = self.rows.get(index) {
            if let Some(open) = row.open {
                let key = row.key();
                self.set_open(&key, !open);
            }
        }
    }

    /// The right arrow: open a folded row, or go to the first child of an
    /// open one.
    fn expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor else { return };
        let Some(row) = self.rows.get(index) else {
            return;
        };
        match row.open {
            Some(false) => self.toggle(index),
            Some(true) => {
                if let Some(child) = tree::first_child_of(&self.rows, index) {
                    self.move_cursor_to(child);
                }
            }
            None => {
                if matches!(row.kind, Kind::More { .. }) {
                    self.activate(index, window, cx);
                }
            }
        }
    }

    /// The left arrow: fold an open row, or go to the parent of a folded or
    /// childless one.
    fn collapse(&mut self) {
        let Some(index) = self.cursor else { return };
        let Some(row) = self.rows.get(index) else {
            return;
        };
        if row.open == Some(true) {
            self.toggle(index);
        } else if let Some(parent) = tree::parent_of(&self.rows, index) {
            self.move_cursor_to(parent);
        }
    }

    /// Enter, or a click, on the row at `index`: a history session opens in a
    /// terminal that resumes it, a worktree shows its detail; a machine,
    /// project or folder opens or folds; "show more" shows the rest; "open a
    /// project" opens one.
    pub(super) fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        if let Kind::Machine(machine) = &row.kind {
            // Selecting a machine that does not answer shows why, where the
            // problem is seen.
            if matches!(machine.kind, leon_core::MachineKind::Ssh { .. })
                && self.engine.offline_reason(&machine.id).is_some()
            {
                self.open_connect(Some(machine.id.clone()), true, window, cx);
                return;
            }
        }
        match &row.kind {
            Kind::Machine(_)
            | Kind::Project { .. }
            | Kind::Unsorted { .. }
            | Kind::Folder { .. } => {
                self.toggle(index);
            }
            Kind::Worktree { worktree, .. } => {
                self.open_worktree(&worktree.project_id, &worktree.id, cx)
            }
            Kind::Session(session) => self.open_history(session.clone(), window, cx),
            Kind::Live(entry) => self.open_live(entry.id, window, cx),
            Kind::More { .. } => {
                if let NodeId::More(parent) = &row.id {
                    self.expansion.show_all_under(&parent.key());
                    self.rebuild_rows();
                }
            }
            Kind::Open => self.open_project_on(&row.machine, window, cx),
            Kind::NoMatch => {}
        }
    }

    // ----- selecting and opening -------------------------------------------------------

    /// Opens a project in the main pane.
    pub(super) fn open_project(&mut self, id: &ProjectId, cx: &mut Context<Self>) {
        if self.snapshot.project(id).is_none() {
            return;
        }
        self.main = Main::Project(id.clone());
        self.show(&NodeId::Project(id.clone()));
        cx.notify();
    }

    /// Opens a worktree in the main pane.
    pub(super) fn open_worktree(
        &mut self,
        project: &ProjectId,
        worktree: &WorktreeId,
        cx: &mut Context<Self>,
    ) {
        let known = self
            .snapshot
            .project(project)
            .is_some_and(|entry| entry.worktrees.iter().any(|w| &w.id == worktree));
        if !known {
            return;
        }
        self.main = Main::Worktree(project.clone(), worktree.clone());
        self.show(&NodeId::Worktree(worktree.clone()));
        cx.notify();
    }

    /// Opens a session's transcript, read off the UI thread. With `hit`, the
    /// list scrolls to that message once it has been read.
    pub(super) fn open_session(
        &mut self,
        session: Session,
        hit: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.open_transcript(session, hit, None, cx);
    }

    /// [`Self::open_session`], with the reason it is shown in the place of a
    /// terminal when there is one.
    pub(super) fn open_transcript(
        &mut self,
        session: Session,
        hit: Option<u32>,
        notice: Option<Notice>,
        cx: &mut Context<Self>,
    ) {
        let id = session.id.clone();
        self.show(&NodeId::Session(id.clone()));
        self.main = Main::Session(Box::new(Transcript {
            session,
            messages: None,
            list: ListState::new(0, ListAlignment::Top, px(800.)),
            hit,
            failed: None,
            notice,
        }));
        let store = self.engine.store().clone();
        self.loading = Some(cx.spawn(async move |this, cx| {
            let wanted = id.clone();
            let loaded = cx
                .background_spawn(async move { store.session_messages(&wanted) })
                .await;
            this.update(cx, |this, cx| this.transcript_loaded(&id, loaded, cx))
                .ok();
        }));
        cx.notify();
    }

    // ----- opening a folder as a project ----------------------------------------------

    /// Opens a folder as a project on `machine`: the system's folder picker
    /// on this computer, the palette's questions on any other machine.
    pub(super) fn open_project_on(
        &mut self,
        machine: &MachineId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Typing a path is for other machines only: they have no dialog to
        // show on this screen.
        if !machine.is_local() {
            self.begin_flow_with(
                Command::AddProject,
                vec![machine.as_str().to_owned()],
                window,
                cx,
            );
            return;
        }
        self.close_overlay(window, cx);
        let picking = (self.options.pick_folder)(cx);
        self.picking = Some(cx.spawn_in(window, async move |this, cx| {
            let picked = picking.await;
            this.update_in(cx, |this, window, cx| match picked {
                Picked::Folder(path) => this.open_folder(path, cx),
                Picked::Cancelled => {}
                Picked::Unavailable => {
                    let _ = window;
                    this.engine.report(
                        crate::engine::StatusKind::Error,
                        "This system has no folder dialog to open a project with.",
                    );
                }
            })
            .ok();
        }));
    }

    /// Adds the repository of a folder of this computer, and selects it once
    /// it is in the store.
    fn open_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let opening = self
            .engine
            .open_project(MachineId::local(), path.to_string_lossy().into_owned());
        self.opening = Some(cx.spawn(async move |this, cx| {
            if let Ok(Some(id)) = opening.await {
                this.update(cx, |this, cx| {
                    this.reload(cx);
                    this.expansion
                        .set_open(&NodeId::Project(id.clone()).key(), true);
                    this.save_expansion();
                    this.rebuild_rows();
                    this.pane = Pane::Sidebar;
                    this.open_project(&id, cx);
                })
                .ok();
            }
        }));
    }

    fn transcript_loaded(
        &mut self,
        id: &leon_core::SessionId,
        loaded: StoreResult<Vec<Message>>,
        cx: &mut Context<Self>,
    ) {
        let Main::Session(transcript) = &mut self.main else {
            return;
        };
        if &transcript.session.id != id {
            return;
        }
        match loaded {
            Ok(messages) => {
                transcript.list.reset(messages.len());
                transcript.messages = Some(Arc::new(messages));
                if let Some(index) = transcript.hit_index() {
                    transcript.list.scroll_to(ListOffset {
                        item_ix: index,
                        offset_in_item: px(0.),
                    });
                }
            }
            Err(error) => transcript.failed = Some(error.to_string()),
        }
        cx.notify();
    }

    // ----- keys -------------------------------------------------------------------------------

    /// What a keystroke does, if anything. `true` when it was taken.
    pub(super) fn handle_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.overlay == Overlay::Menu && self.menu_key(stroke, window, cx) {
            return true;
        }
        if self.overlay == Overlay::Settings && self.settings_key(stroke, window, cx) {
            return true;
        }
        if self.overlay == Overlay::Connect && self.connect_key(stroke, window, cx) {
            return true;
        }
        if self.overlay == Overlay::Usage && self.usage_key(stroke, window, cx) {
            return true;
        }
        if self.overlay == Overlay::Pair && self.pair_key(stroke, window, cx) {
            return true;
        }
        if self.overlay == Overlay::Share && self.share_key(stroke, window, cx) {
            return true;
        }
        let filtering = self.overlay == Overlay::None && self.filter_focused(window, cx);
        if filtering && self.filter_key(stroke, window, cx) {
            return true;
        }
        let finding = self.overlay == Overlay::None && self.find_focused(window, cx);
        if finding && self.find_key(stroke, window, cx) {
            return true;
        }
        let typing = matches!(
            self.overlay,
            Overlay::Palette | Overlay::Settings | Overlay::Connect | Overlay::Pair
        ) || filtering
            || finding;
        if self.overlay == Overlay::Palette && self.palette_key(stroke, window, cx) {
            return true;
        }
        // The find bar's field belongs to a terminal pane: the chords that stay
        // Leon's in a terminal (find next, clear...) work from it as well.
        let terminal = self.terminal_focused();
        let context = keys::Context {
            typing,
            terminal: terminal || finding,
        };
        let Some(command) = keys::resolve(stroke, context) else {
            // Every key that is not Leon's goes to the terminal that has the
            // keyboard; plain text reaches it through the text input path.
            return terminal && !finding && self.terminal_key(stroke, cx);
        };
        self.run_command(command, window, cx)
    }

    /// Does what `command` says. `true` when it did something.
    pub(super) fn run_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use Command as C;
        // What acts on the tree has the tree showing first.
        if matches!(
            command,
            C::FilterProjects | C::Machine(_) | C::ContextMenu | C::FocusSidebar
        ) {
            self.reveal_sidebar(cx);
        }
        match command {
            C::GoTo => self.open_palette("", window, cx),
            C::Commands => self.open_palette(">", window, cx),
            C::SearchHistory => self.open_palette("/", window, cx),
            C::FilterProjects => self.focus_filter(window, cx),
            C::RefreshIcon => self.refresh_icon_here(),
            C::ChooseIcon => self.choose_icon_here(window, cx),
            C::ResetIcon => self.reset_icon_here(),
            C::Machine(number) => {
                let machine = self
                    .snapshot
                    .machines
                    .get(usize::from(number).saturating_sub(1))
                    .map(|machine| machine.id.clone());
                if let Some(id) = machine {
                    self.close_overlay(window, cx);
                    self.show(&NodeId::Machine(id));
                    self.pane = Pane::Sidebar;
                }
            }
            C::FocusSidebar => self.focus_pane(Pane::Sidebar, window, cx),
            C::FocusMain => self.focus_pane(Pane::Main, window, cx),
            C::NextPane | C::PreviousPane => {
                if self.overlay == Overlay::None {
                    self.pane = match self.pane {
                        Pane::Sidebar => Pane::Main,
                        Pane::Main => Pane::Sidebar,
                    };
                }
            }
            C::Down | C::Up | C::Top | C::Bottom | C::PageDown | C::PageUp => {
                self.move_cursor(command, cx)
            }
            C::Expand => {
                if self.overlay == Overlay::None && self.pane == Pane::Sidebar {
                    self.expand(window, cx);
                }
            }
            C::Collapse => {
                if self.overlay == Overlay::None && self.pane == Pane::Sidebar {
                    self.collapse();
                }
            }
            C::Open => self.open_here(window, cx),
            C::OpenProject => {
                let machine = self.current_machine();
                self.open_project_on(&machine, window, cx);
            }
            C::OpenShell => self.open_shell_here(window, cx),
            C::ContextMenu => self.open_menu_here(window, cx),
            C::Rename | C::RemoveMachine | C::RemoveFromHistory | C::ResumeIn | C::ResumeAnyway => {
                self.begin_flow(command, window, cx)
            }
            C::RevealTerminal => self.reveal_terminal_here(cx),
            C::CopyPath => self.copy_path_here(cx),
            C::CopyBranch => self.copy_branch_here(cx),
            C::CopySessionId => self.copy_session_id_here(cx),
            C::Reveal => self.reveal_here(cx),
            C::OpenTranscript => self.open_transcript_here(cx),
            C::FocusTerminal => self.focus_terminal(window, cx),
            C::SplitRight => self.split_pane(Axis::Row, window, cx),
            C::SplitDown => self.split_pane(Axis::Column, window, cx),
            C::FocusPaneLeft => self.focus_pane_dir(Dir::Left, window, cx),
            C::FocusPaneRight => self.focus_pane_dir(Dir::Right, window, cx),
            C::FocusPaneUp => self.focus_pane_dir(Dir::Up, window, cx),
            C::FocusPaneDown => self.focus_pane_dir(Dir::Down, window, cx),
            C::NextSplit => self.focus_pane_step(1, window, cx),
            C::PreviousSplit => self.focus_pane_step(-1, window, cx),
            C::ResizeLeft => self.resize_pane(Dir::Left, cx),
            C::ResizeRight => self.resize_pane(Dir::Right, cx),
            C::ResizeUp => self.resize_pane(Dir::Up, cx),
            C::ResizeDown => self.resize_pane(Dir::Down, cx),
            C::EqualizeSplits => self.equalize_panes(cx),
            C::ToggleZoom => self.toggle_zoom(window, cx),
            C::NextTab => self.step_tab(1, window, cx),
            C::PreviousTab => self.step_tab(-1, window, cx),
            C::Tab(number) => self.goto_tab(usize::from(number), window, cx),
            C::Paste => self.paste_terminal(Self::paste_mode(cx), cx),
            C::PasteText => self.paste_terminal(super::paste::How::Text, cx),
            C::PasteImage => self.paste_terminal(super::paste::How::Image, cx),
            C::Copy | C::ScrollPageUp | C::ScrollPageDown => self.terminal_action(command, cx),
            C::NewSession
            | C::CloseSession
            | C::NewWorktree
            | C::AddProject
            | C::RemoveProject
            | C::RemoveWorktree
            | C::SetAppearance
            | C::ChooseTheme
            | C::SetInterfaceSize => self.begin_flow(command, window, cx),
            C::AddMachine => self.open_pair(window, cx),
            C::ShareMachine => self.open_share(window, cx),
            C::WhyOffline if self.relay_machine_here().is_some() => {
                if let Some(machine) = self.relay_machine_here() {
                    self.open_pair_why(machine, window, cx);
                }
            }
            C::EditMachine | C::WhyOffline => match self.ssh_machine_here() {
                Some(machine) => {
                    self.open_connect(Some(machine), command == C::WhyOffline, window, cx)
                }
                None => self.engine.report(
                    crate::engine::StatusKind::Info,
                    "Select a machine first: this one is not reached over SSH.",
                ),
            },
            C::Settings => self.open_settings(window, cx),
            C::ShowUsage => self.toggle_usage(window, cx),
            C::RefreshUsage => self.refresh_usage(cx),
            C::Refresh => self.engine.submit(crate::engine::Op::Refresh),
            C::ProbeMachine => {
                let machine = self.current_machine();
                self.engine.submit(crate::engine::Op::Probe(machine));
            }
            C::NextTheme | C::PreviousTheme => {
                let next = settings::kept_theme_id(cx).step(command == C::NextTheme);
                settings::set_theme_id(cx, next);
                self.engine.report(
                    crate::engine::StatusKind::Info,
                    format!("Theme: {}", next.name()),
                );
            }
            C::ToggleAppearance => {
                let next = match settings::appearance(cx) {
                    Appearance::Dark => AppearanceChoice::Light,
                    Appearance::Light => AppearanceChoice::Dark,
                };
                settings::set_appearance(cx, next);
            }
            C::Find => self.open_find(window, cx),
            C::FindNext => self.find_step(true, window, cx),
            C::FindPrevious => self.find_step(false, window, cx),
            C::ClearBuffer => self.clear_terminal(true, cx),
            C::ClearScrollback => self.clear_terminal(false, cx),
            C::CopyAll => self.copy_terminal_text(leon_term::Extent::All, cx),
            C::CopyScreen => self.copy_terminal_text(leon_term::Extent::Screen, cx),
            C::SelectAll => self.select_all_terminal(cx),
            C::SaveOutput => self.save_terminal(false, cx),
            C::SaveOutputAnsi => self.save_terminal(true, cx),
            C::Quit | C::CloseWindow => self.request_quit(window, cx),
            C::OpenSettingsFile => self.open_settings_file(cx),
            C::RevealSettingsFolder => self.reveal_settings_folder(cx),
            C::About => {
                self.overlay = Overlay::About;
                self.focus.focus(window, cx);
            }
            C::NewThemeFromCurrent => self.begin_flow(command, window, cx),
            C::ExportTheme => self.export_theme(cx),
            C::OpenThemesFolder => self.open_themes_folder(cx),
            C::ReloadThemes => self.reload_themes(cx, true),
            C::ShowThemeProblems => self.show_theme_problems(window, cx),
            C::ToggleSidebar => self.toggle_sidebar(window, cx),
            C::WidenSidebar => self.step_sidebar(1, cx),
            C::NarrowSidebar => self.step_sidebar(-1, cx),
            C::ResetSidebarWidth => self.reset_sidebar_width(window, cx),
            C::Larger | C::Smaller | C::ActualSize => {
                let current = settings::get(cx).interface_scale;
                let next = match command {
                    C::Larger => theme::next_step(current, true),
                    C::Smaller => theme::next_step(current, false),
                    _ => 100,
                };
                settings::update(cx, |settings| settings.interface_scale = next);
            }
            C::Shortcuts => {
                if self.overlay == Overlay::Shortcuts {
                    self.close_overlay(window, cx);
                } else {
                    self.overlay = Overlay::Shortcuts;
                    self.sheet_scroll
                        .set_offset(gpui_kit::point(px(0.), px(0.)));
                    self.focus.focus(window, cx);
                }
            }
            C::Close => match self.overlay {
                Overlay::Palette => {
                    if !self.palette_back(window, cx) {
                        self.close_palette(window, cx);
                    }
                }
                Overlay::Shortcuts
                | Overlay::Menu
                | Overlay::About
                | Overlay::Problems
                | Overlay::Connect
                | Overlay::Usage
                | Overlay::Pair
                | Overlay::Share
                | Overlay::Settings => self.close_overlay(window, cx),
                Overlay::None => {
                    if self.pane == Pane::Sidebar {
                        // The first Escape puts the whole tree back.
                        if self.filter.is_none() {
                            return false;
                        }
                        self.clear_filter(window, cx);
                    } else {
                        self.pane = Pane::Sidebar;
                    }
                }
            },
        }
        self.sync_focus(window, cx);
        cx.notify();
        true
    }

    pub(super) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.overlay {
            Overlay::Palette => self.close_palette(window, cx),
            Overlay::Shortcuts => {
                self.overlay = Overlay::None;
                self.focus.focus(window, cx);
            }
            Overlay::Menu => self.close_menu(window, cx),
            Overlay::Settings => self.close_settings(window, cx),
            Overlay::Connect => self.close_connect(window, cx),
            Overlay::Pair => self.close_pair(window, cx),
            Overlay::Share => self.close_share(window, cx),
            Overlay::About | Overlay::Problems | Overlay::Usage => {
                self.overlay = Overlay::None;
                self.focus.focus(window, cx);
            }
            Overlay::None => {}
        }
    }

    fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(window, cx);
        self.pane = pane;
    }

    /// Moves the cursor of the pane that has the keyboard (or scrolls the
    /// sheet, while it is open).
    fn move_cursor(&mut self, command: Command, _cx: &mut Context<Self>) {
        use Command as C;
        if matches!(self.overlay, Overlay::Shortcuts | Overlay::Problems) {
            let page = (self.viewport.height.as_f32() * 0.6).max(120.);
            let delta = match command {
                C::Down => 48.,
                C::Up => -48.,
                C::PageDown => page,
                C::PageUp => -page,
                C::Top => -1e9,
                C::Bottom => 1e9,
                _ => 0.,
            };
            let max = self.sheet_scroll.max_offset().y.as_f32();
            let mut offset = self.sheet_scroll.offset();
            offset.y = px(-(((-offset.y.as_f32()) + delta).clamp(0., max)));
            self.sheet_scroll.set_offset(offset);
            return;
        }
        match self.pane {
            Pane::Sidebar => {
                let from = self.cursor.unwrap_or(0);
                let next = match command {
                    C::Down => tree::step(&self.rows, from, 1),
                    C::Up => tree::step(&self.rows, from, -1),
                    C::PageDown => tree::step(&self.rows, from, 8),
                    C::PageUp => tree::step(&self.rows, from, -8),
                    C::Top => tree::first(&self.rows),
                    C::Bottom => tree::last(&self.rows),
                    _ => None,
                };
                if let Some(next) = next {
                    self.move_cursor_to(next);
                }
            }
            Pane::Main => {
                if let Main::Session(transcript) = &self.main {
                    let page = px((self.viewport.height.as_f32() * 0.7).max(120.));
                    match command {
                        C::Down => transcript.list.scroll_by(px(48.)),
                        C::Up => transcript.list.scroll_by(px(-48.)),
                        C::PageDown => transcript.list.scroll_by(page),
                        C::PageUp => transcript.list.scroll_by(-page),
                        C::Top => transcript.list.scroll_to(ListOffset {
                            item_ix: 0,
                            offset_in_item: px(0.),
                        }),
                        C::Bottom => {
                            let last = transcript.list.item_count().saturating_sub(1);
                            transcript.list.scroll_to(ListOffset {
                                item_ix: last,
                                offset_in_item: px(0.),
                            })
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Enter: opens the row the cursor is on; in the main pane, the open
    /// transcript is resumed in a terminal.
    fn open_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::None {
            return;
        }
        match (self.pane, &self.main) {
            (Pane::Sidebar, _) => {
                if let Some(index) = self.cursor {
                    self.activate(index, window, cx);
                }
            }
            (Pane::Main, Main::Session(transcript)) => {
                let session = transcript.session.clone();
                // On the notice of a session that runs elsewhere, `Enter`
                // resumes it only when that was a guess; a certain one asks.
                match transcript
                    .notice
                    .as_ref()
                    .and_then(|n| n.elsewhere.as_ref())
                {
                    Some(note) if note.likely => self.resume_anyway(session, window, cx),
                    Some(_) => {
                        self.run_command(Command::ResumeAnyway, window, cx);
                    }
                    None => self.resume_session(session, window, cx),
                }
            }
            (Pane::Main, _) => {}
        }
    }

    // ----- the sidebar ---------------------------------------------------------------------------

    /// Shows or hides the sidebar. Showing it gives the keyboard to the tree;
    /// hiding it gives the keyboard to the main pane.
    pub(super) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if settings::get(cx).sidebar_visible {
            settings::update(cx, |settings| settings.sidebar_visible = false);
            self.close_overlay(window, cx);
            self.pane = Pane::Main;
            if self.filter_focused(window, cx) {
                self.focus.focus(window, cx);
            }
        } else {
            settings::update(cx, |settings| settings.sidebar_visible = true);
            self.close_overlay(window, cx);
            self.pane = Pane::Sidebar;
        }
        cx.notify();
    }

    /// Shows the sidebar if it is hidden, without moving the keyboard.
    pub(super) fn reveal_sidebar(&mut self, cx: &mut Context<Self>) {
        if !settings::get(cx).sidebar_visible {
            settings::update(cx, |settings| settings.sidebar_visible = true);
            cx.notify();
        }
    }

    /// Makes the sidebar `steps` steps wider (narrower when negative).
    pub(super) fn step_sidebar(&mut self, steps: i32, cx: &mut Context<Self>) {
        let width =
            i32::from(settings::get(cx).sidebar_width) + steps * i32::from(theme::SIDEBAR_STEP);
        self.set_sidebar_width(width, cx);
    }

    /// Gives the sidebar its width, in design pixels, kept between the limits.
    pub(super) fn set_sidebar_width(&mut self, width: i32, cx: &mut Context<Self>) {
        let width = width.clamp(0, i32::from(u16::MAX)) as u16;
        self.reveal_sidebar(cx);
        settings::update(cx, |settings| settings.sidebar_width = width);
        cx.notify();
    }

    /// The sidebar's default width again.
    pub(super) fn reset_sidebar_width(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_width(i32::from(theme::SIDEBAR_DEFAULT), cx);
    }

    /// The button went down on the sidebar's edge. A double click resets the
    /// width; a single one starts a drag.
    pub(super) fn begin_sidebar_drag(&mut self, clicks: usize, cx: &mut Context<Self>) {
        if clicks >= 2 {
            self.sidebar_drag = None;
            self.set_sidebar_width(i32::from(theme::SIDEBAR_DEFAULT), cx);
            return;
        }
        let start = settings::get(cx).sidebar_width;
        tracing::debug!(width = start, "sidebar drag: start");
        self.sidebar_drag = Some(SidebarDrag {
            start,
            width: start,
            closing: false,
        });
        cx.notify();
    }

    /// The pointer moved to `x` (window coordinates) with or without the button
    /// down. The width follows at once, on the next layout, and is saved only
    /// on release; a move with no button held means the release was missed.
    pub(super) fn sidebar_drag_move(
        &mut self,
        x: gpui_kit::Pixels,
        held: bool,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_drag.is_none() {
            return;
        }
        if !held {
            self.end_sidebar_drag(cx);
            return;
        }
        let wanted = (x.as_f32() / theme::scale()).round() as i32;
        let closing = wanted < i32::from(theme::SIDEBAR_MIN) - i32::from(theme::SIDEBAR_SNAP);
        let width = theme::clamp_sidebar(wanted.clamp(0, i32::from(u16::MAX)) as u16);
        if let Some(drag) = self.sidebar_drag.as_mut() {
            drag.width = width;
            drag.closing = closing;
        }
        tracing::debug!(width, closing, "sidebar drag: move");
        theme::set_sidebar(width, true);
        cx.notify();
    }

    /// The drag is over: the width is kept and saved, or the sidebar closes.
    pub(super) fn end_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_drag.take() else {
            return;
        };
        tracing::debug!(
            width = drag.width,
            closing = drag.closing,
            "sidebar drag: end"
        );
        if drag.closing {
            settings::update(cx, |settings| {
                settings.sidebar_width = drag.start;
                settings.sidebar_visible = false;
            });
            self.pane = Pane::Main;
        } else {
            settings::update(cx, |settings| settings.sidebar_width = drag.width);
        }
        cx.notify();
    }

    /// The handle on the sidebar's rule, above every pane, the marks and the
    /// header buttons: eight pixels wide, centred on the rule, the window's
    /// height. It takes the press (`occlude`: nothing beneath sees it).
    fn render_sidebar_handle(&self, colours: &Colours, cx: &mut Context<Self>) -> Stateful<Div> {
        let dragging = self.sidebar_drag.is_some();
        let line = if dragging {
            colours.signal
        } else {
            colours.elevated_border
        };
        div()
            .id("sidebar-resize")
            .debug_selector(|| "sidebar-resize".into())
            .group("sidebar-edge")
            .occlude()
            .absolute()
            .top_0()
            .bottom_0()
            .left(metrics::SIDEBAR_WIDTH() - gpui_kit::px(4.))
            .w(gpui_kit::px(8.))
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.begin_sidebar_drag(event.click_count, cx);
                    cx.stop_propagation();
                }),
            )
            // The rule shows it is the one that moves, with the tokens it has.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui_kit::px(3.))
                    .w(theme::hairline())
                    .when(dragging, |this| this.bg(line))
                    .group_hover("sidebar-edge", move |style| style.bg(line)),
            )
    }

    /// While the sidebar is dragged, listeners for the whole window, in the
    /// capture phase, so nothing under the pointer (a terminal, a row) sees the
    /// moves or the release.
    fn sidebar_drag_listeners(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = self.sidebar_drag.is_some();
        let entity = cx.entity();
        gpui_kit::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                if !dragging {
                    return;
                }
                let moving = entity.clone();
                window.on_mouse_event(move |event: &gpui_kit::MouseMoveEvent, phase, _, cx| {
                    if phase == gpui_kit::DispatchPhase::Capture {
                        moving.update(cx, |this, cx| {
                            this.sidebar_drag_move(
                                event.position.x,
                                event.pressed_button == Some(MouseButton::Left),
                                cx,
                            );
                        });
                        cx.stop_propagation();
                    }
                });
                let released = entity.clone();
                window.on_mouse_event(move |_: &gpui_kit::MouseUpEvent, phase, _, cx| {
                    if phase == gpui_kit::DispatchPhase::Capture {
                        released.update(cx, |this, cx| this.end_sidebar_drag(cx));
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    // ----- the menu bar -------------------------------------------------------------------------

    /// What the menu bar needs to know to enable its items.
    pub(super) fn availability(&self, cx: &App) -> crate::menus::Availability {
        crate::menus::Availability {
            terminal: matches!(self.main, Main::Live(id) if self.live.get(id).is_some()),
            sidebar: settings::get(cx).sidebar_visible,
        }
    }

    /// Whether a command of a menu applies now, by the rules its chord follows:
    /// a command of the terminal needs the terminal (or its find bar), one of
    /// the tree's bare keys needs nothing to be typed.
    pub(super) fn applies_now(&self, command: Command, window: &Window, cx: &App) -> bool {
        use keys::When;
        match keys::binding(command).map(|binding| binding.when) {
            Some(When::Terminal) => self.terminal_focused() || self.find_focused(window, cx),
            Some(When::Panes) => {
                self.overlay == Overlay::None
                    && !self.terminal_focused()
                    && !self.filter_focused(window, cx)
                    && !self.find_focused(window, cx)
            }
            _ => true,
        }
    }

    /// A menu item was chosen: the same code as its chord, when it applies.
    fn menu_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        // A menu click is not a key: the terminal commands run for the terminal
        // on screen even when the keyboard is elsewhere.
        let by_click_ok =
            crate::menus::needs_terminal(command) || matches!(command, Command::FilterProjects);
        if by_click_ok || self.applies_now(command, window, cx) {
            self.run_command(command, window, cx);
        }
    }

    /// Copy, paste or select all from the menu: for the terminal that has the
    /// keyboard.
    fn terminal_edit(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_focused() && !self.find_focused(window, cx) {
            self.run_command(command, window, cx);
        } else {
            cx.propagate();
        }
    }

    // ----- quitting ------------------------------------------------------------------------------

    /// How many live sessions have a program running that quitting would end
    /// (or may: over SSH nothing can be told).
    pub(super) fn busy_sessions(&self, cx: &App) -> usize {
        self.live
            .all()
            .iter()
            .filter(|session| session.busy(cx))
            .count()
    }

    /// Quits, asking first while a program runs in a terminal.
    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.quit_asks(cx) {
            self.quit_now(cx);
        } else {
            self.begin_flow(Command::Quit, window, cx);
        }
    }

    /// The window is being closed: it may be, unless a program is running.
    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.quit_asks(cx) {
            self.flush(cx);
            return true;
        }
        self.begin_flow(Command::Quit, window, cx);
        false
    }

    /// Hangs every terminal up (those on other computers are let go of: their
    /// programs keep running there), saves what is kept, and ends the
    /// application.
    pub(super) fn quit_now(&mut self, cx: &mut Context<Self>) {
        self.flush(cx);
        (self.options.quit)(cx);
    }

    /// What must not be lost when the application ends: the terminals are hung
    /// up (no program is left running without a window) and the state kept
    /// beside the settings is written. The settings themselves are written at
    /// every change.
    pub(super) fn flush(&mut self, cx: &mut Context<Self>) {
        for id in self.live.ids() {
            if let Some(session) = self.live.get(id) {
                let terminal = session.view.read(cx).terminal();
                if terminal.is_remote() {
                    // The program on the other computer keeps running; it can
                    // be attached to again.
                    terminal.detach();
                } else {
                    terminal.kill();
                }
            }
        }
        if let Some(path) = &self.expansion_file {
            if let Err(error) = self.expansion.save(path) {
                tracing::warn!(%error, "the tree's open rows could not be saved");
            }
        }
    }

    // ----- drawing --------------------------------------------------------------------------------

    /// The card overlays are drawn on: lifted by its outline, not by a shadow.
    pub(super) fn card(&self, id: &'static str, colours: &Colours) -> Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .relative()
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(colours.elevated_border)
            .bg(colours.surface)
            .text_color(colours.text)
            .on_click(|_, _, cx| cx.stop_propagation())
            // The corners of a framed surface, in a theme that draws them.
            .children(lines::frame_ticks(id, colours))
    }

    /// The numbers the window's rules are laid out from, for the marks that sit
    /// on them.
    fn line_frame(&self, window: &Window) -> lines::Frame<'_> {
        let live = match &self.main {
            Main::Live(id) => self.workspaces.workspace_of(*id).map(|workspace| {
                (
                    workspace.tabs.len() > 1,
                    workspace.tabs.get(workspace.active),
                )
            }),
            _ => None,
        };
        let tab = live.and_then(|(_, tab)| tab);
        lines::Frame {
            viewport: (
                window.viewport_size().width.as_f32(),
                window.viewport_size().height.as_f32(),
            ),
            sidebar: metrics::SIDEBAR_WIDTH().as_f32(),
            header: metrics::HEADER_HEIGHT().as_f32(),
            status: metrics::FOOTER_HEIGHT().as_f32(),
            tools: metrics::TOOLS_HEIGHT().as_f32(),
            tabs: live
                .filter(|(many, _)| *many)
                .map(|_| metrics::TAB_BAR_HEIGHT().as_f32()),
            layout: tab.filter(|tab| !tab.zoomed).map(|tab| &tab.layout),
            scale_factor: window.scale_factor(),
        }
    }

    /// Watches the banners' expiry while any is on screen. One timer for all
    /// of them, and it ends itself once the last one is gone.
    pub(super) fn keep_banners(&mut self, cx: &mut Context<Self>) {
        if self.banner_ticker.is_some() {
            return;
        }
        self.banner_ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(BANNER_TICK).await;
            let alive = this.update(cx, |this, cx| {
                let now = cx.background_executor().now();
                let before = this.banners.len();
                this.banners.retain(|banner| banner.until > now);
                if this.banners.len() != before {
                    cx.notify();
                }
                let alive = !this.banners.is_empty();
                if !alive {
                    // Dropping the handle ends the task after this turn.
                    this.banner_ticker = None;
                }
                alive
            });
            if !matches!(alive, Ok(true)) {
                return;
            }
        }));
    }

    /// The geek banners: what the sessions just did, over the main pane. A
    /// click opens the session the note came from; the cross dismisses it.
    fn render_notifications(
        &self,
        colours: &Colours,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if self.banners.is_empty() {
            return None;
        }
        let cards = self.banners.iter().rev().map(|banner| {
            let id = banner.id;
            let session = banner.note.session;
            let (marker, label) = match banner.note.kind {
                notify::Kind::Waiting => (colours.warning, "WAIT"),
                notify::Kind::Finished => (colours.success, "DONE"),
                notify::Kind::Failed => (colours.error, "FAIL"),
            };
            div()
                .id(("notify", id as usize))
                .debug_selector(move || format!("notify-{id}"))
                .w(px(320.))
                .flex()
                .flex_row()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.elevated_border)
                .bg(colours.surface)
                .overflow_hidden()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.banners.retain(|banner| banner.id != id);
                    if let Some(session) = session {
                        this.open_live(session, window, cx);
                    }
                    cx.notify();
                }))
                .child(div().w(px(3.)).flex_none().bg(marker))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_none()
                                        .font_family(theme::fonts::mono())
                                        .font_features(theme::fonts::mono_features())
                                        .text_size(metrics::TEXT_LABEL())
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(marker)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(theme::fonts::mono())
                                        .text_size(metrics::TEXT_SMALL())
                                        .text_color(colours.text)
                                        .child(banner.note.title.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_size(metrics::TEXT_SMALL())
                                .text_color(colours.text_muted)
                                .child(banner.note.body.clone()),
                        ),
                )
                .child(
                    div()
                        .id(("notify-dismiss", id as usize))
                        .debug_selector(move || format!("notify-dismiss-{id}"))
                        .px_2()
                        .py_2()
                        .flex_none()
                        .cursor_pointer()
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.text_faint)
                        .hover(move |style| style.text_color(colours.text))
                        .child("✕")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.banners.retain(|banner| banner.id != id);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
        });
        Some(
            div()
                .id("notifications")
                .debug_selector(|| "notifications".into())
                .absolute()
                .right(px(12.))
                .bottom(metrics::FOOTER_HEIGHT() + px(12.))
                .occlude()
                .flex()
                .flex_col()
                .gap_2()
                .children(cards),
        )
    }

    fn render_overlay(&self, colours: &Colours, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let content = match self.overlay {
            Overlay::None | Overlay::Menu => return None,
            Overlay::Palette => self.render_palette(colours, cx).into_any_element(),
            Overlay::Shortcuts => self.render_sheet(colours).into_any_element(),
            Overlay::About => self.render_about(colours, cx).into_any_element(),
            Overlay::Problems => self.render_problems(colours).into_any_element(),
            Overlay::Settings => self.render_settings(colours, cx).into_any_element(),
            Overlay::Connect => self.render_connect(colours, cx).into_any_element(),
            Overlay::Usage => self.render_usage(colours, cx).into_any_element(),
            Overlay::Pair => self.render_pair(colours, cx).into_any_element(),
            Overlay::Share => self.render_share(colours, cx).into_any_element(),
        };
        let top = match self.overlay {
            Overlay::Palette => self.palette_top(),
            _ => px(56.),
        };
        Some(
            div()
                .id("overlay")
                .debug_selector(|| "overlay".into())
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .bg(colours.scrim)
                .flex()
                .flex_col()
                .items_center()
                .pt(top)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_overlay(window, cx);
                    cx.notify();
                }))
                .child(content),
        )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colours = palette(cx);
        self.viewport = window.viewport_size();
        let available = self.availability(cx);
        if available != self.menu_state {
            self.menu_state = available;
            crate::menus::refresh(available, cx);
        }
        // Terminals wear the theme and the interface size in use.
        self.sync_settings(cx);
        let (terminal_theme, terminal_font) = (colours.terminal, Self::terminal_font(cx));
        for session in self.live.all() {
            session.view.update(cx, |view, cx| {
                view.set_theme(terminal_theme, cx);
                view.set_font(terminal_font.clone(), cx);
            });
        }
        div()
            .id("shell")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .bg(colours.background)
            .text_color(colours.text)
            .font_family(theme::fonts::sans())
            .font_features(theme::fonts::sans_features())
            .text_size(metrics::TEXT_BODY())
            // A menu item is a command of the registry, run as its chord is.
            .on_action(cx.listener(|this, run: &crate::menus::Run, window, cx| {
                this.menu_command(run.0, window, cx);
            }))
            .on_action(cx.listener(|_, _: &crate::menus::Minimize, window, _| {
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &crate::menus::Zoom, window, _| {
                window.zoom_window();
            }))
            .on_action(
                cx.listener(|_, _: &crate::menus::ToggleFullScreen, window, _| {
                    window.toggle_fullscreen();
                }),
            )
            // The platform's Copy, Paste and Select All reach the terminal when
            // it has the keyboard; a text field answers them itself first.
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Copy, window, cx| {
                    this.terminal_edit(Command::Copy, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::Paste, window, cx| {
                    this.terminal_edit(Command::Paste, window, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::base::input::SelectAll, window, cx| {
                    this.terminal_edit(Command::SelectAll, window, cx);
                }),
            )
            // First, so that its capture-phase listeners run before every
            // other element's (a terminal's "mouse up outside" included).
            .child(self.sidebar_drag_listeners(cx))
            .when(settings::get(cx).sidebar_visible, |this| {
                this.child(self.render_sidebar(&colours, cx))
            })
            .child(self.render_main_pane(&colours, cx))
            // The crosshairs, where the rules of the window meet.
            .children(lines::crosshairs(&self.line_frame(window), &colours))
            .when(settings::get(cx).sidebar_visible, |this| {
                this.child(self.render_sidebar_handle(&colours, cx))
            })
            // The notifications of what the sessions just did, over the main
            // pane and under any overlay.
            .children(self.render_notifications(&colours, cx))
            // While anything is dragged (the sidebar's edge, a pane divider) a
            // sheet over the window takes the pointer: no terminal beneath
            // selects text or is sent mouse reports, and the cursor stays the
            // resize one.
            .when(
                self.sidebar_drag.is_some() || cx.has_active_drag(),
                |this| {
                    this.child(
                        div()
                            .debug_selector(|| "drag-sheet".into())
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .occlude()
                            .cursor_col_resize(),
                    )
                },
            )
            .children(self.render_overlay(&colours, cx))
            .children(self.render_menu(&colours, cx))
    }
}
