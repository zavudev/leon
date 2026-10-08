//! The terminal model: emulator, pseudo-terminal and child process, with no
//! GPUI in it.
//!
//! Four threads serve one terminal:
//!
//! * **reader** blocks on the PTY and forwards chunks (a bounded channel
//!   gives a flooding child back-pressure);
//! * **processor** feeds the chunks to the parser, which updates the grid
//!   under a lock, and flushes a synchronized update (DEC mode 2026) that
//!   never ended;
//! * **writer** owns the PTY's input: keys, pastes and the replies the
//!   emulator makes to a program's queries all go through its channel, so
//!   neither the UI nor the parser can block on a full PTY;
//! * **waiter** reaps the child and reports how it ended.
//!
//! Output wakes the UI through a callback, but only when the UI has not
//! already been woken since it last looked ([`Terminal::take_dirty`]): a
//! child that prints megabytes causes at most one repaint per frame.
//!
//! Dropping a [`Terminal`] hangs the child up and, if it ignores that,
//! kills its process group a moment later; the waiter thread reaps it, so no
//! zombie is left behind. The two waits that involve ([`Timings`]) can be
//! shortened by the caller.
//!
//! Terminals come from a [`Backend`]: [`Pty`] starts the real thing;
//! [`Scripted`] (in `scripted.rs`) makes terminals with the same emulator but
//! no process and no threads, for tests of code that only uses a terminal.

use crate::buffer::{self, Cleared, Extent};
use crate::colors::{to_rgb8, TerminalTheme};
use crate::files::{file_refs, Files};
use crate::find::Highlights;
use crate::size::GridSize;
use crate::spec::{command_builder, SpawnSpec};
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor, Rgb};
use parking_lot::{Mutex, RwLock};
use portable_pty::{ChildKiller, MasterPty};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod scripted;

pub use scripted::{HeldForeground, RemoteFeed, RemoteLink, Script, Scripted};

/// How many lines of scrollback a terminal keeps.
pub const SCROLLBACK_LINES: usize = 10_000;

/// The waits a terminal makes on its own account. Production uses
/// [`Timings::default`]; tests shorten them so that nothing sleeps for
/// real.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    /// How long a hung-up child has to exit before its process group is
    /// killed.
    pub kill_grace: Duration,
    /// How long, after the child exits, the last of its output may still
    /// arrive.
    pub drain_grace: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            kill_grace: Duration::from_millis(1_500),
            drain_grace: Duration::from_millis(250),
        }
    }
}

/// How the UI is woken from a background thread.
pub type Wake = Box<dyn Fn() + Send + Sync>;

/// Where terminals come from: a real PTY in the application, a scripted
/// stand-in (see [`Scripted`]) in tests of code that only uses a terminal.
pub trait Backend {
    /// Starts `spec` in a terminal of `size`; `wake` is called when there is
    /// something new to draw.
    fn spawn(
        &self,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
    ) -> Result<Terminal, SpawnError>;
}

/// The real thing: the program runs in a pseudo-terminal.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pty {
    /// The waits it makes.
    pub timings: Timings,
}

impl Backend for Pty {
    fn spawn(
        &self,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
    ) -> Result<Terminal, SpawnError> {
        Terminal::spawn_with(spec, size, theme, self.timings, wake)
    }
}

/// A cell of the grid: a line (negative in the scrollback) and a column.
pub type GridPoint = Point;

/// How the child ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    /// The exit code (a process killed by a signal reports 1 on most
    /// systems).
    pub code: u32,
    /// The name of the signal that ended it, when one did.
    pub signal: Option<String>,
}

impl ExitInfo {
    /// Whether it ended with code 0.
    pub fn success(&self) -> bool {
        self.code == 0 && self.signal.is_none()
    }
}

/// Something the terminal reports besides its picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The program set the window title (OSC 0 or 2).
    Title(String),
    /// The program reset the title.
    ResetTitle,
    /// The bell rang.
    Bell,
    /// The program asked to put text in the clipboard (OSC 52).
    ClipboardStore(String),
    /// The child ended.
    Exited(ExitInfo),
}

/// Why a terminal could not be started.
#[derive(Debug)]
pub struct SpawnError(pub String);

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SpawnError {}

/// What the threads and the UI share.
struct Shared {
    dirty: AtomicBool,
    events_pending: AtomicBool,
    has_output: AtomicBool,
    started: Instant,
    last_output_ms: AtomicU64,
    reader_done: AtomicBool,
    exit: Mutex<Option<ExitInfo>>,
    events: Mutex<VecDeque<TerminalEvent>>,
    title: Mutex<Option<String>>,
    theme: RwLock<TerminalTheme>,
    size: Mutex<GridSize>,
    wake: Wake,
    /// Counts the chunks of output fed to the emulator: a search that last ran
    /// at another count is out of date.
    generation: AtomicU64,
    /// The matches of the find bar, painted over the grid.
    highlights: Mutex<Option<Arc<Highlights>>>,
    /// The scrollback and the cursor the emulator was last configured with.
    options: Mutex<(usize, CursorShape)>,
    /// Where a `#123` printed in the terminal points to (see
    /// [`Terminal::set_reference_base`]).
    reference_base: Mutex<Option<Arc<str>>>,
    /// The folder whose files a printed path names (see
    /// [`Terminal::set_file_base`]).
    files: Mutex<Option<Arc<Files>>>,
}

impl Shared {
    fn new(size: GridSize, theme: TerminalTheme, wake: Wake) -> Arc<Self> {
        Arc::new(Self {
            dirty: AtomicBool::new(false),
            events_pending: AtomicBool::new(false),
            has_output: AtomicBool::new(false),
            started: Instant::now(),
            last_output_ms: AtomicU64::new(0),
            reader_done: AtomicBool::new(false),
            exit: Mutex::new(None),
            events: Mutex::new(VecDeque::new()),
            title: Mutex::new(None),
            theme: RwLock::new(theme),
            size: Mutex::new(size),
            wake,
            generation: AtomicU64::new(0),
            highlights: Mutex::new(None),
            options: Mutex::new((SCROLLBACK_LINES, CursorShape::Block)),
            reference_base: Mutex::new(None),
            files: Mutex::new(None),
        })
    }

    /// Wakes the UI unless it has been woken since it last looked.
    fn wake_once(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }

    /// Queues an event for the UI and wakes it, unless it has been woken for
    /// events and has not read them yet. Separate from the picture's flag so
    /// that a terminal nobody is drawing still reports its title and its end.
    fn push(&self, event: TerminalEvent) {
        self.events.lock().push_back(event);
        if !self.events_pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

/// The listener the emulator reports to. It answers the program's queries
/// itself and queues the rest for the UI.
#[derive(Clone)]
pub struct EventProxy {
    shared: Arc<Shared>,
    input: Sender<Vec<u8>>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                *self.shared.title.lock() = Some(title.clone());
                self.shared.push(TerminalEvent::Title(title));
            }
            Event::ResetTitle => {
                *self.shared.title.lock() = None;
                self.shared.push(TerminalEvent::ResetTitle);
            }
            Event::Bell => self.shared.push(TerminalEvent::Bell),
            Event::ClipboardStore(_, text) => self.shared.push(TerminalEvent::ClipboardStore(text)),
            // Reply to a program that asks what a colour is, as full-screen
            // programs do to pick a light or a dark style.
            Event::ColorRequest(slot, format) => {
                let (r, g, b) = to_rgb8(self.shared.theme.read().by_slot(slot));
                let _ = self.input.send(format(Rgb { r, g, b }).into_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let size = *self.shared.size.lock();
                let reply = format(WindowSize {
                    num_lines: size.rows,
                    num_cols: size.cols,
                    cell_width: size.cell_width,
                    cell_height: size.cell_height,
                });
                let _ = self.input.send(reply.into_bytes());
            }
            // Cursor position reports, device attributes and the like.
            Event::PtyWrite(text) => {
                let _ = self.input.send(text.into_bytes());
            }
            // A program reading the clipboard is refused: nothing is exposed
            // to whatever runs in the terminal, remote hosts included.
            Event::ClipboardLoad(..) => {}
            Event::Wakeup
            | Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }
}

struct Dims {
    cols: usize,
    rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// A running (or finished) program in a pseudo-terminal.
pub struct Terminal {
    term: Arc<Mutex<Term<EventProxy>>>,
    shared: Arc<Shared>,
    input: Option<Sender<Vec<u8>>>,
    /// The PTY and the child's killer; none for a scripted terminal.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    pid: Option<u32>,
    /// Only the Unix reaper reads it (see `kill`).
    #[cfg_attr(not(unix), allow(dead_code))]
    timings: Timings,
    /// What stands in for the child of a scripted terminal.
    script: Option<scripted::Script>,
}

impl Terminal {
    /// Starts `spec` in a new PTY of `size`. `wake` is called, from a
    /// background thread, when there is something new to draw; call
    /// [`Self::take_dirty`] when drawing it.
    pub fn spawn(
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, SpawnError> {
        Self::spawn_with(spec, size, theme, Timings::default(), Box::new(wake))
    }

    /// [`Self::spawn`] with the waits chosen by the caller.
    pub fn spawn_with(
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        timings: Timings,
        wake: Wake,
    ) -> Result<Self, SpawnError> {
        let pty = portable_pty::native_pty_system()
            .openpty(size.pty_size())
            .map_err(|error| SpawnError(format!("cannot open a terminal: {error}")))?;
        let mut child = pty
            .slave
            .spawn_command(command_builder(spec))
            .map_err(|error| SpawnError(format!("cannot start {}: {error}", spec.program)))?;
        // The child holds the only other end: reading ends when it is gone.
        drop(pty.slave);
        let reader = pty
            .master
            .try_clone_reader()
            .map_err(|error| SpawnError(format!("cannot read the terminal: {error}")))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|error| SpawnError(format!("cannot write to the terminal: {error}")))?;
        let pid = child.process_id();
        let killer = child.clone_killer();

        let shared = Shared::new(size, theme, wake);
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        let proxy = EventProxy {
            shared: shared.clone(),
            input: input_tx.clone(),
        };
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term = Arc::new(Mutex::new(Term::new(
            config,
            &Dims {
                cols: usize::from(size.cols),
                rows: usize::from(size.rows),
            },
            proxy,
        )));

        spawn_thread("leon-term-writer", move || write_loop(writer, input_rx));
        let (chunks_tx, chunks_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        spawn_thread("leon-term-reader", move || read_loop(reader, chunks_tx));
        {
            let (term, shared) = (term.clone(), shared.clone());
            spawn_thread("leon-term-parser", move || {
                process_loop(chunks_rx, &term, &shared)
            });
        }
        {
            let shared = shared.clone();
            spawn_thread("leon-term-waiter", move || {
                let status = child.wait();
                // Let the last of the output arrive before reporting the end.
                let deadline = Instant::now() + timings.drain_grace;
                while !shared.reader_done.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let info = match status {
                    Ok(status) => ExitInfo {
                        code: status.exit_code(),
                        signal: status.signal().map(str::to_owned),
                    },
                    Err(_) => ExitInfo {
                        code: 1,
                        signal: None,
                    },
                };
                *shared.exit.lock() = Some(info.clone());
                shared.push(TerminalEvent::Exited(info));
            });
        }

        Ok(Self {
            term,
            shared,
            input: Some(input_tx),
            master: Mutex::new(Some(pty.master)),
            killer: Mutex::new(Some(killer)),
            pid,
            timings,
            script: None,
        })
    }

    /// Whether anything changed since the last call, clearing the flag. The
    /// UI calls this when it is about to draw.
    pub fn take_dirty(&self) -> bool {
        self.shared.dirty.swap(false, Ordering::AcqRel)
    }

    /// How many chunks of output the emulator has taken so far.
    pub fn output_generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// Shows these matches over the grid, or none.
    pub fn set_highlights(&self, highlights: Option<Highlights>) {
        *self.shared.highlights.lock() = highlights.map(Arc::new);
        self.shared.wake_once();
    }

    /// The matches being shown.
    pub fn highlights(&self) -> Option<Arc<Highlights>> {
        self.shared.highlights.lock().clone()
    }

    /// The buffer as plain text (see [`buffer::text`]).
    pub fn buffer_text(&self, extent: Extent) -> String {
        buffer::text(&self.term.lock(), extent)
    }

    /// The buffer with its colours as ANSI text (see [`buffer::ansi`]).
    pub fn buffer_ansi(&self, extent: Extent) -> String {
        buffer::ansi(&self.term.lock(), extent)
    }

    /// Empties the history and the screen but the cursor's line.
    pub fn clear_buffer(&self) -> Cleared {
        let cleared = buffer::clear_buffer(&mut self.term.lock());
        self.shared.wake_once();
        cleared
    }

    /// Empties the history.
    pub fn clear_scrollback(&self) -> Cleared {
        let cleared = buffer::clear_scrollback(&mut self.term.lock());
        self.shared.wake_once();
        cleared
    }

    /// Selects the whole buffer.
    pub fn select_all(&self) {
        buffer::select_all(&mut self.term.lock());
        self.shared.wake_once();
    }

    /// Whether the child has printed anything yet.
    pub fn has_output(&self) -> bool {
        self.shared.has_output.load(Ordering::Acquire)
    }

    /// How long it has been since the child last printed, `None` while it has
    /// printed nothing. A shell that has printed its prompt and then been
    /// quiet for a moment is ready for typed input.
    pub fn quiet_for(&self) -> Option<Duration> {
        let stored = self.shared.last_output_ms.load(Ordering::Acquire);
        let last = Duration::from_millis(stored.checked_sub(1)?);
        Some(self.shared.started.elapsed().saturating_sub(last))
    }

    /// Whether the terminal's own process (the shell) is the foreground
    /// process group of the terminal, so that nothing else, such as an agent
    /// started from it, is running in front of it. `None` where the system
    /// cannot say (Windows) or the process has no pid.
    pub fn shell_is_foreground(&self) -> Option<bool> {
        if let Some(script) = &self.script {
            // Nothing is claimed about a program on another computer. The
            // keeper reports for one it holds on this computer.
            if script.is_held() {
                return script.held_foreground()?.shell_in_front;
            }
            return (!script.is_remote()).then(|| script.shell_is_foreground());
        }
        #[cfg(unix)]
        {
            let pid = self.pid?;
            let leader = self.master.lock().as_ref()?.process_group_leader()?;
            Some(leader as u32 == pid)
        }
        // The pseudo-console has no foreground process group.
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// The name of the program in front of the shell (the `comm` of the
    /// terminal's foreground process group leader), `None` when the shell
    /// itself is in front, the terminal is on another computer or the system
    /// cannot say.
    pub fn foreground_command(&self) -> Option<String> {
        if let Some(script) = &self.script {
            return script.foreground_command();
        }
        #[cfg(target_os = "linux")]
        {
            let pid = self.pid?;
            let leader = self.master.lock().as_ref()?.process_group_leader()?;
            if leader as u32 == pid {
                return None;
            }
            let name = std::fs::read_to_string(format!("/proc/{leader}/comm")).ok()?;
            let name = name.trim();
            (!name.is_empty()).then(|| name.to_owned())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// Asks the program running in front of the shell to end: SIGTERM to the
    /// terminal's foreground process group. `false` when nothing was sent:
    /// the shell itself is in front, the terminal is on another computer, the
    /// foreground cannot be told (Windows) or the signal failed.
    pub fn terminate_foreground(&self) -> bool {
        if let Some(script) = &self.script {
            return script.terminate();
        }
        #[cfg(unix)]
        {
            let Some(pid) = self.pid else { return false };
            let Some(leader) = self
                .master
                .lock()
                .as_ref()
                .and_then(|master| master.process_group_leader())
            else {
                return false;
            };
            if leader as u32 == pid || leader <= 1 {
                return false;
            }
            // SAFETY: plain signal delivery to the terminal's foreground
            // process group, which the pseudo-terminal itself reported.
            unsafe { libc::kill(-leader, libc::SIGTERM) == 0 }
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// The events since the last call.
    pub fn drain_events(&self) -> Vec<TerminalEvent> {
        self.shared.events_pending.store(false, Ordering::Release);
        self.shared.events.lock().drain(..).collect()
    }

    /// How the child ended, once it has.
    pub fn exit_info(&self) -> Option<ExitInfo> {
        self.shared.exit.lock().clone()
    }

    /// The title the program set, if any.
    pub fn title(&self) -> Option<String> {
        self.shared.title.lock().clone()
    }

    /// The child's process id, where the system has one.
    pub fn process_id(&self) -> Option<u32> {
        self.pid.or_else(|| {
            self.script
                .as_ref()
                .and_then(Script::held_foreground)
                .and_then(|held| held.pid)
        })
    }

    /// Keeps `lines` lines of scrollback from now on; what is already kept is
    /// cut to that when it is more. Does nothing when it is the number in
    /// force.
    pub fn set_scrollback(&self, lines: usize) {
        let mut options = self.shared.options.lock();
        if options.0 == lines {
            return;
        }
        options.0 = lines;
        self.configure(*options);
    }

    /// Draws the cursor in `shape` until a program asks for another.
    pub fn set_cursor_shape(&self, shape: CursorShape) {
        let mut options = self.shared.options.lock();
        if options.1 == shape {
            return;
        }
        options.1 = shape;
        self.configure(*options);
    }

    /// How many lines of scrollback hold something now.
    pub fn history_size(&self) -> usize {
        use alacritty_terminal::grid::Dimensions as _;
        self.term.lock().grid().history_size()
    }

    /// The scrollback and the cursor shape in force.
    pub fn options(&self) -> (usize, CursorShape) {
        *self.shared.options.lock()
    }

    fn configure(&self, (scrollback, shape): (usize, CursorShape)) {
        let config = Config {
            scrolling_history: scrollback,
            default_cursor_style: CursorStyle {
                shape,
                blinking: false,
            },
            ..Config::default()
        };
        self.term.lock().set_options(config);
        self.shared.wake_once();
    }

    /// Replaces the colours the emulator answers colour queries with.
    pub fn set_theme(&self, theme: TerminalTheme) {
        *self.shared.theme.write() = theme;
    }

    /// Sends bytes to the child.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        if let Some(script) = &self.script {
            script.input(bytes.into());
            return;
        }
        if let Some(input) = &self.input {
            let _ = input.send(bytes.into());
        }
    }

    /// Whether the program asked for pasted text to be bracketed: only then
    /// do the line breaks of a paste not act as the Enter key.
    pub fn bracketed_paste(&self) -> bool {
        self.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Sends pasted text, bracketed when the program asked for that.
    pub fn paste(&self, text: &str) {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.write(crate::keys::paste_bytes(text, bracketed));
    }

    /// The emulator's mode flags.
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    /// The grid size in force.
    pub fn size(&self) -> GridSize {
        *self.shared.size.lock()
    }

    /// Resizes the grid and tells the child. Does nothing when the size is
    /// the one in force.
    pub fn resize(&self, size: GridSize) {
        {
            let mut current = self.shared.size.lock();
            if *current == size {
                return;
            }
            *current = size;
        }
        self.term.lock().resize(Dims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        });
        // A child that is gone cannot be told; that is not an error.
        if let Some(master) = self.master.lock().as_ref() {
            let _ = master.resize(size.pty_size());
        }
        if let Some(script) = &self.script {
            script.resized(size);
        }
        self.shared.wake_once();
    }

    /// Runs `f` with the emulator locked. Keep it short: the parser waits.
    pub fn with_term<R>(&self, f: impl FnOnce(&mut Term<EventProxy>) -> R) -> R {
        f(&mut self.term.lock())
    }

    // ----- scrollback and selection ---------------------------------------------------

    /// How many lines the view is scrolled back.
    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    /// Scrolls the view: positive is back into the history.
    pub fn scroll_lines(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.shared.wake_once();
    }

    /// One page back into the history.
    pub fn scroll_page_up(&self) {
        self.term.lock().scroll_display(Scroll::PageUp);
        self.shared.wake_once();
    }

    /// One page forward.
    pub fn scroll_page_down(&self) {
        self.term.lock().scroll_display(Scroll::PageDown);
        self.shared.wake_once();
    }

    /// Back to the live screen.
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
            drop(term);
            self.shared.wake_once();
        }
    }

    /// Starts a selection at a cell of the viewport (row and column from 0).
    pub fn start_selection(&self, kind: SelectionType, col: usize, row: usize, side: Side) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, col, row);
        term.selection = Some(Selection::new(kind, point, side));
        drop(term);
        self.shared.wake_once();
    }

    /// Extends the selection to a cell of the viewport.
    pub fn update_selection(&self, col: usize, row: usize, side: Side) {
        let mut term = self.term.lock();
        let point = viewport_point(&term, col, row);
        if let Some(selection) = &mut term.selection {
            selection.update(point, side);
        }
        drop(term);
        self.shared.wake_once();
    }

    /// Drops the selection.
    pub fn clear_selection(&self) {
        let mut term = self.term.lock();
        if term.selection.take().is_some() {
            drop(term);
            self.shared.wake_once();
        }
    }

    /// The selected text, if any.
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// The OSC 8 or HTTP(S) hyperlink under a viewport cell, across wraps. A
    /// `#123` is one too when a [reference base](Self::set_reference_base) is set,
    /// and a path to a file when a [file base](Self::set_file_base) is.
    pub fn link_at(&self, col: usize, row: usize) -> Option<String> {
        let base = self.reference_base();
        let files = self.files();
        let term = self.term.lock();
        link_at(&term, col, row, base.as_deref(), files.as_deref())
    }

    /// Makes a path printed in the output a link when it names a file of
    /// `cwd` (or an absolute one, or `~/`): the host opens it in its editor.
    /// `None` turns it off.
    pub fn set_file_base(&self, cwd: Option<String>) {
        *self.shared.files.lock() = cwd.map(|cwd| Arc::new(Files::new(cwd)));
        self.shared.wake_once();
    }

    /// What [`Self::set_file_base`] set.
    pub(crate) fn files(&self) -> Option<Arc<Files>> {
        self.shared.files.lock().clone()
    }

    /// Makes `#123` in the output a link to `{base}123`: the pull request
    /// page of the repository the terminal works in (GitHub sends an issue
    /// number to its issue). `None` turns it off.
    pub fn set_reference_base(&self, base: Option<String>) {
        *self.shared.reference_base.lock() = base.map(Arc::from);
        self.shared.wake_once();
    }

    /// What [`Self::set_reference_base`] set.
    pub fn reference_base(&self) -> Option<Arc<str>> {
        self.shared.reference_base.lock().clone()
    }

    /// The text of the visible screen, one line per row, trailing blanks
    /// trimmed. For tests and diagnostics.
    pub fn screen_text(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let offset = grid.display_offset() as i32;
        let mut out = String::new();
        for row in 0..grid.screen_lines() {
            let mut line = String::new();
            for col in 0..grid.columns() {
                let cell = &grid[Point::new(Line(row as i32 - offset), Column(col))];
                if cell
                    .flags
                    .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
                {
                    continue;
                }
                line.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.trim_end().to_owned()
    }

    /// Hangs the child up now (SIGHUP; the process is terminated on
    /// Windows). The process group is killed if it is still there after a
    /// grace period.
    pub fn kill(&self) {
        if let Some(script) = &self.script {
            script.hang_up();
            return;
        }
        if let Some(killer) = self.killer.lock().as_mut() {
            let _ = killer.kill();
        }
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            let shared = self.shared.clone();
            let kill_grace = self.timings.kill_grace;
            spawn_thread("leon-term-reaper", move || {
                let deadline = Instant::now() + kill_grace;
                while Instant::now() < deadline {
                    if shared.exit.lock().is_some() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                if shared.exit.lock().is_none() {
                    // The child leads its own session, so its pid is its
                    // process group: this reaches whatever it started too.
                    // SAFETY: plain signal delivery to a pid we spawned.
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
            });
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Closing the input channel ends the writer; the PTY's own drop and
        // the hang-up end the rest.
        self.input = None;
        if let Some(script) = &self.script {
            if script.is_linked() {
                // Dropping a view lets go of the connection; it does not end
                // the program on the other computer. Closing it is `kill`.
                script.detach();
                return;
            }
        }
        if self.shared.exit.lock().is_none() {
            self.kill();
        }
    }
}

fn viewport_point(term: &Term<EventProxy>, col: usize, row: usize) -> Point {
    let grid = term.grid();
    let col = col.min(grid.columns().saturating_sub(1));
    let row = row.min(grid.screen_lines().saturating_sub(1));
    Point::new(Line(row as i32 - grid.display_offset() as i32), Column(col))
}

/// Whether an address that a program wrote (an OSC 8 hyperlink) may be handed
/// to the system to open: only the web's own schemes. Anything else
/// (`file:`, an application's scheme, `javascript:`...) would let output that
/// is merely printed start something, so it is not a link at all.
pub(crate) fn openable(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !uri.chars().any(char::is_control)
}

fn link_at(
    term: &Term<EventProxy>,
    col: usize,
    row: usize,
    base: Option<&str>,
    files: Option<&Files>,
) -> Option<String> {
    let point = viewport_point(term, col, row);
    let grid = term.grid();
    if let Some(link) = grid[point].hyperlink() {
        return openable(link.uri()).then(|| link.uri().to_owned());
    }

    let (columns, first, final_line) = logical_line_bounds(term, point);
    if columns == 0 {
        return None;
    }
    let mut text = Vec::new();
    let mut clicked = None;
    for line in first..=final_line {
        for column in 0..columns {
            let cell = &grid[Point::new(Line(line), Column(column))];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                if line == point.line.0 && column == point.column.0 {
                    clicked = text.len().checked_sub(1);
                }
                continue;
            }
            if line == point.line.0 && column == point.column.0 {
                clicked = Some(text.len());
            }
            text.push(if cell.c == '\0' { ' ' } else { cell.c });
        }
    }
    plain_link_at(&text, clicked?, base, files)
}

fn plain_link_at(
    text: &[char],
    clicked: usize,
    base: Option<&str>,
    files: Option<&Files>,
) -> Option<String> {
    plain_links(text, base, files)
        .into_iter()
        .find(|(range, _)| range.contains(&clicked))
        .map(|(_, uri)| uri)
}

/// The HTTP(S) links in `text`, with `files` the paths of files that exist
/// and with a `base` the `#123` references too, as character ranges and the
/// address each opens.
pub(crate) fn plain_links(
    text: &[char],
    base: Option<&str>,
    files: Option<&Files>,
) -> Vec<(std::ops::Range<usize>, String)> {
    let mut links = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let scheme_len = if starts_with(text, start, "https://") {
            8
        } else if starts_with(text, start, "http://") {
            7
        } else {
            start += 1;
            continue;
        };
        let mut end = (start + scheme_len..text.len())
            .find(|&index| link_separator(text[index]))
            .unwrap_or(text.len());
        while end > start + scheme_len && matches!(text[end - 1], '.' | ',' | ';' | ':' | '!') {
            end -= 1;
        }
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            while end > start + scheme_len
                && text[end - 1] == close
                && text[start..end].iter().filter(|&&ch| ch == close).count()
                    > text[start..end].iter().filter(|&&ch| ch == open).count()
            {
                end -= 1;
            }
        }
        if end > start + scheme_len {
            links.push((start..end, text[start..end].iter().collect()));
        }
        start = end.max(start + 1);
    }
    if let Some(files) = files {
        let mut paths = file_refs(text)
            .into_iter()
            .filter(|(range, _, _)| {
                !links
                    .iter()
                    .any(|(link, _): &(std::ops::Range<usize>, String)| {
                        link.start < range.end && range.start < link.end
                    })
            })
            .filter_map(|(range, path, line)| Some((range, files.link(&path, line)?)))
            .collect::<Vec<_>>();
        links.append(&mut paths);
        links.sort_by_key(|(range, _)| range.start);
    }
    if let Some(base) = base {
        let mut references = references(text)
            .into_iter()
            .filter(|(range, _)| {
                !links
                    .iter()
                    .any(|(link, _): &(std::ops::Range<usize>, String)| {
                        link.start < range.end && range.start < link.end
                    })
            })
            .map(|(range, number)| (range, format!("{base}{number}")))
            .collect::<Vec<_>>();
        links.append(&mut references);
        links.sort_by_key(|(range, _)| range.start);
    }
    links
}

/// The `#123` references in `text` with their numbers: a `#` that starts a
/// word, followed by one to five digits that end it. That leaves out colours
/// (`#123456`), anchors (`page#12`, `/#12`) and headings (`# 3`).
fn references(text: &[char]) -> Vec<(std::ops::Range<usize>, String)> {
    let mut found = Vec::new();
    for (start, &ch) in text.iter().enumerate() {
        if ch != '#' {
            continue;
        }
        let starts_word = start == 0
            || text[start - 1].is_whitespace()
            || matches!(
                text[start - 1],
                '(' | '[' | '{' | ',' | ';' | ':' | '"' | '\'' | '`'
            );
        if !starts_word {
            continue;
        }
        let digits = text[start + 1..]
            .iter()
            .take_while(|ch| ch.is_ascii_digit())
            .count();
        let end = start + 1 + digits;
        let ends_word = text
            .get(end)
            .is_none_or(|ch| !ch.is_alphanumeric() && !matches!(ch, '_' | '-' | '#'));
        if (1..=5).contains(&digits) && ends_word {
            found.push((start..end, text[start + 1..end].iter().collect()));
        }
    }
    found
}

pub(crate) fn visible_links(
    term: &Term<EventProxy>,
    base: Option<&str>,
    files: Option<&Files>,
) -> Vec<Option<String>> {
    let grid = term.grid();
    let columns = grid.columns();
    let rows = grid.screen_lines();
    let offset = grid.display_offset() as i32;
    let mut links = vec![None; columns * rows];
    let mut logical_start = None;

    for viewport_row in 0..rows {
        let point = Point::new(Line(viewport_row as i32 - offset), Column(0));
        let (_, first, final_line) = logical_line_bounds(term, point);
        if logical_start == Some(first) {
            continue;
        }
        logical_start = Some(first);

        let mut text = Vec::new();
        let mut points = Vec::new();
        for line in first..=final_line {
            for column in 0..columns {
                let point = Point::new(Line(line), Column(column));
                let cell = &grid[point];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
                points.push(point);
            }
        }
        for (range, uri) in plain_links(&text, base, files) {
            for point in &points[range] {
                let row = point.line.0 + offset;
                if row >= 0 && row < rows as i32 {
                    links[row as usize * columns + point.column.0] = Some(uri.clone());
                }
            }
        }
    }

    for row in 0..rows {
        for column in 0..columns {
            let point = Point::new(Line(row as i32 - offset), Column(column));
            if let Some(link) = grid[point].hyperlink() {
                if openable(link.uri()) {
                    links[row * columns + column] = Some(link.uri().to_owned());
                }
            }
        }
    }
    links
}

fn logical_line_bounds(term: &Term<EventProxy>, point: Point) -> (usize, i32, i32) {
    let grid = term.grid();
    let columns = grid.columns();
    let last = Column(columns.saturating_sub(1));
    let top = -(grid.total_lines().saturating_sub(grid.screen_lines()) as i32);
    let bottom = grid.screen_lines().saturating_sub(1) as i32;
    let mut first = point.line.0;
    while columns > 0
        && first > top
        && grid[Point::new(Line(first - 1), last)]
            .flags
            .contains(Flags::WRAPLINE)
    {
        first -= 1;
    }
    let mut final_line = point.line.0;
    while columns > 0
        && final_line < bottom
        && grid[Point::new(Line(final_line), last)]
            .flags
            .contains(Flags::WRAPLINE)
    {
        final_line += 1;
    }
    (columns, first, final_line)
}

fn starts_with(text: &[char], at: usize, prefix: &str) -> bool {
    text.get(at..at + prefix.len())
        .is_some_and(|candidate| candidate.iter().copied().eq(prefix.chars()))
}

fn link_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '<' | '>' | '\'' | '"' | '`')
}

fn spawn_thread(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(error) = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
    {
        tracing::error!(%error, name, "could not start a terminal thread");
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, input: mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = input.recv() {
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            break;
        }
    }
}

fn read_loop(mut reader: Box<dyn Read + Send>, chunks: SyncSender<Vec<u8>>) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if chunks.send(buffer[..n].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
    // Dropping the sender tells the parser thread that this was the last chunk.
}

fn process_loop(chunks: mpsc::Receiver<Vec<u8>>, term: &Mutex<Term<EventProxy>>, shared: &Shared) {
    let mut parser: Processor = Processor::new();
    loop {
        let wait = parser
            .sync_timeout()
            .sync_timeout()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let received = match wait {
            Some(wait) => chunks.recv_timeout(wait),
            None => chunks.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(bytes) => {
                feed(&mut parser, &mut term.lock(), &bytes);
                shared.has_output.store(true, Ordering::Release);
                shared.generation.fetch_add(1, Ordering::AcqRel);
                let elapsed = shared.started.elapsed().as_millis() as u64;
                shared.last_output_ms.store(elapsed + 1, Ordering::Release);
                shared.wake_once();
            }
            Err(RecvTimeoutError::Timeout) => {
                // A synchronized update that never ended: show what we have.
                parser.stop_sync(&mut *term.lock());
                shared.wake_once();
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    shared.reader_done.store(true, Ordering::Release);
    shared.wake_once();
}

/// Feeds output bytes to the parser, which updates the grid.
pub fn feed(parser: &mut Processor, term: &mut Term<EventProxy>, bytes: &[u8]) {
    parser.advance(term, bytes);
}

/// A headless emulator for tests and measurements: the parser and a grid,
/// with no PTY and no threads.
pub struct Headless {
    parser: Processor,
    term: Term<EventProxy>,
    replies: mpsc::Receiver<Vec<u8>>,
}

impl Headless {
    /// A grid of `cols` by `rows` with the scrollback a real terminal has.
    pub fn new(cols: u16, rows: u16, theme: TerminalTheme) -> Self {
        let shared = Shared::new(GridSize::new(cols, rows), theme, Box::new(|| {}));
        let (input, replies) = mpsc::channel();
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term = Term::new(
            config,
            &Dims {
                cols: usize::from(cols),
                rows: usize::from(rows),
            },
            EventProxy { shared, input },
        );
        Self {
            parser: Processor::new(),
            term,
            replies,
        }
    }

    /// Feeds output bytes.
    pub fn feed(&mut self, bytes: &[u8]) {
        feed(&mut self.parser, &mut self.term, bytes);
    }

    /// The emulator.
    pub fn term(&self) -> &Term<EventProxy> {
        &self.term
    }

    /// The emulator, to change it.
    pub fn term_mut(&mut self) -> &mut Term<EventProxy> {
        &mut self.term
    }

    /// The bytes the emulator wrote back to the program so far.
    pub fn replies(&self) -> Vec<u8> {
        self.replies.try_iter().flatten().collect()
    }

    /// The visible text, one line per row, trailing blanks trimmed.
    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let mut out = Vec::new();
        for row in 0..grid.screen_lines() {
            let mut line = String::new();
            for col in 0..grid.columns() {
                let cell = &grid[Point::new(Line(row as i32), Column(col))];
                line.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
            out.push(line.trim_end().to_owned());
        }
        while out.last().is_some_and(String::is_empty) {
            out.pop();
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::from_rgb8;

    fn theme() -> TerminalTheme {
        TerminalTheme {
            foreground: from_rgb8(250, 250, 250),
            background: from_rgb8(0, 0, 0),
            cursor: from_rgb8(97, 95, 255),
            selection: from_rgb8(30, 30, 90),
            find_match: from_rgb8(60, 60, 20),
            find_match_current: from_rgb8(250, 200, 0),
            ansi: std::array::from_fn(|i| from_rgb8(i as u8 * 16, 0, 0)),
        }
    }

    #[test]
    fn text_lands_on_the_grid() {
        let mut headless = Headless::new(20, 4, theme());
        headless.feed(b"hello\r\nworld");
        assert_eq!(headless.text(), "hello\nworld");
    }

    #[test]
    fn http_links_are_found_at_the_clicked_cell_and_trim_sentence_punctuation() {
        let mut headless = Headless::new(80, 4, theme());
        headless.feed(b"See (https://example.com/a_(b)). Next");
        assert_eq!(
            link_at(headless.term(), 10, 0, None, None).as_deref(),
            Some("https://example.com/a_(b)")
        );
        assert_eq!(link_at(headless.term(), 31, 0, None, None), None);
    }

    #[test]
    fn a_plain_link_is_found_across_wrapped_rows() {
        let mut headless = Headless::new(12, 4, theme());
        headless.feed(b"https://example.com/docs");
        assert_eq!(
            link_at(headless.term(), 3, 1, None, None).as_deref(),
            Some("https://example.com/docs")
        );
        let links = visible_links(headless.term(), None, None);
        assert_eq!(links[3].as_deref(), Some("https://example.com/docs"));
        assert_eq!(links[12 + 3].as_deref(), Some("https://example.com/docs"));
    }

    #[test]
    fn a_pull_request_number_is_a_link_only_with_a_reference_base() {
        let mut headless = Headless::new(60, 4, theme());
        headless.feed(b"created PR #29 (see #7, not #123456 or a/#5 or c#4)");
        let base = Some("https://github.com/o/r/pull/");
        assert_eq!(link_at(headless.term(), 12, 0, None, None), None);
        assert_eq!(
            link_at(headless.term(), 12, 0, base, None).as_deref(),
            Some("https://github.com/o/r/pull/29")
        );
        assert_eq!(
            link_at(headless.term(), 21, 0, base, None).as_deref(),
            Some("https://github.com/o/r/pull/7")
        );
        let numbers = visible_links(headless.term(), base, None)
            .into_iter()
            .flatten()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(numbers.len(), 2, "{numbers:?}");
    }

    #[test]
    fn a_printed_path_to_an_existing_file_is_a_link_with_a_file_base() {
        let dir = std::env::temp_dir().join(format!("leon-term-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "x").unwrap();
        let files = Files::new(&dir);
        let mut headless = Headless::new(60, 4, theme());
        headless.feed(b"wrote notes.md:3 and gone.md");
        assert_eq!(link_at(headless.term(), 8, 0, None, None), None);
        let link = link_at(headless.term(), 8, 0, None, Some(&files)).unwrap();
        assert_eq!(
            crate::files::parse(&link),
            Some((dir.join("notes.md").to_string_lossy().into_owned(), Some(3)))
        );
        assert_eq!(link_at(headless.term(), 24, 0, None, Some(&files)), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_reference_inside_a_url_stays_part_of_the_url() {
        let text: Vec<char> = "https://example.com/x #12".chars().collect();
        let links = plain_links(&text, Some("https://example.com/pull/"), None);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].1, "https://example.com/x");
        assert_eq!(links[1].1, "https://example.com/pull/12");
    }

    #[test]
    fn an_osc_8_label_opens_its_declared_uri() {
        let mut headless = Headless::new(40, 4, theme());
        headless.feed(b"\x1b]8;;https://example.com/target\x1b\\read me\x1b]8;;\x1b\\");
        assert_eq!(
            link_at(headless.term(), 3, 0, None, None).as_deref(),
            Some("https://example.com/target")
        );
    }

    #[test]
    fn colour_queries_are_answered_from_the_theme() {
        let mut headless = Headless::new(20, 4, theme());
        // OSC 11 ; ? asks for the background, OSC 10 for the foreground.
        headless.feed(b"\x1b]11;?\x07\x1b]10;?\x07");
        let replies = String::from_utf8(headless.replies()).unwrap();
        assert!(
            replies.contains("\x1b]11;rgb:0000/0000/0000"),
            "{replies:?}"
        );
        assert!(
            replies.contains("\x1b]10;rgb:fafa/fafa/fafa"),
            "{replies:?}"
        );
    }

    #[test]
    fn a_cursor_position_query_is_answered() {
        let mut headless = Headless::new(20, 4, theme());
        headless.feed(b"ab\x1b[6n");
        assert_eq!(String::from_utf8(headless.replies()).unwrap(), "\x1b[1;3R");
    }

    #[test]
    fn a_synchronized_update_is_shown_when_it_ends() {
        let mut headless = Headless::new(20, 4, theme());
        headless.feed(b"\x1b[?2026hhidden");
        assert_eq!(headless.text(), "", "held back until the update ends");
        headless.feed(b"\x1b[?2026l");
        assert_eq!(headless.text(), "hidden");
    }

    #[test]
    fn a_few_megabytes_go_through_the_parser() {
        // Throughput sanity check, no clock: it must simply complete with a
        // sane grid. A flood of colourful lines, scrolling the whole time.
        let mut headless = Headless::new(120, 40, theme());
        let line = b"\x1b[31mred\x1b[0m \x1b[1;38;5;208mbold orange\x1b[0m \x1b[48;2;1;2;3mtruecolor\x1b[0m lorem ipsum dolor sit amet, consectetur adipiscing elit\r\n";
        let mut fed = 0usize;
        while fed < 8 * 1024 * 1024 {
            headless.feed(line);
            fed += line.len();
        }
        assert!(headless.text().contains("lorem ipsum"));
        assert_eq!(headless.term().grid().screen_lines(), 40);
        assert!(headless.term().grid().history_size() <= SCROLLBACK_LINES);
        assert!(headless.term().grid().history_size() > 1_000);
    }

    #[test]
    fn the_scrollback_is_ten_thousand_lines() {
        assert_eq!(SCROLLBACK_LINES, 10_000);
        let mut headless = Headless::new(10, 5, theme());
        for i in 0..12_000 {
            headless.feed(format!("{i}\r\n").as_bytes());
        }
        assert_eq!(headless.term().grid().history_size(), SCROLLBACK_LINES);
    }
}

/// The tests that run real children in real PTYs: output reaches the grid,
/// input reaches the child, a resize reaches the child, the exit status is
/// reported, a stubborn child is killed. Each runs `/bin/sh` with no startup
/// file and a fixed `PATH` (see [`crate::testing::sh`]), with the waits
/// shortened by [`FAST`]; conditions are polled, never slept for.
#[cfg(test)]
mod link_scheme_tests {
    use super::openable;

    #[test]
    fn only_web_addresses_are_ever_handed_to_the_system() {
        assert!(openable("https://example.com/a?b=c"));
        assert!(openable("HTTP://example.com"));
        for refused in [
            "file:///Applications/Calculator.app",
            "javascript:alert(1)",
            "ssh://host",
            "x-apple.systempreferences:",
            "mailto:a@b",
            "",
            "https://exa\nmple.com",
            "  file:///etc/passwd",
        ] {
            assert!(!openable(refused), "{refused:?}");
        }
    }
}

#[cfg(all(test, unix))]
mod pty_tests {
    use super::*;
    use crate::testing::{sh, sh_c, theme, wait_for};
    use std::sync::atomic::AtomicUsize;

    /// The waits shortened: a child that ignores the hang-up is killed after
    /// 50 ms rather than a second and a half.
    pub(super) const FAST: Timings = Timings {
        kill_grace: Duration::from_millis(50),
        drain_grace: Duration::from_millis(250),
    };

    fn spawn(spec: &SpawnSpec, size: GridSize) -> Terminal {
        Terminal::spawn_with(spec, size, theme(), FAST, Box::new(|| {})).expect("sh starts")
    }

    fn start(script: &str) -> Terminal {
        spawn(&sh_c(script), GridSize::new(80, 24))
    }

    #[test]
    fn a_real_child_prints_a_marker_that_reaches_the_grid() {
        let terminal = start("printf 'MARKER-%s' 42");
        wait_for("the marker", || {
            terminal.screen_text().contains("MARKER-42")
        });
    }

    #[test]
    fn the_exit_status_is_reported_after_the_output() {
        let terminal = start("printf 'bye'; exit 3");
        wait_for("the exit", || terminal.exit_info().is_some());
        let info = terminal.exit_info().unwrap();
        assert_eq!(info.code, 3);
        assert!(!info.success());
        assert!(
            terminal.screen_text().contains("bye"),
            "the last output is not lost"
        );
        assert!(terminal
            .drain_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exited(i) if i.code == 3)));
    }

    #[test]
    fn a_clean_exit_is_a_success() {
        let terminal = start("exit 0");
        wait_for("the exit", || terminal.exit_info().is_some());
        assert!(terminal.exit_info().unwrap().success());
    }

    #[test]
    fn input_written_to_the_terminal_reaches_the_child() {
        let terminal = start("read line; printf 'got:%s' \"$line\"");
        terminal.write(&b"ping\n"[..]);
        wait_for("the echo", || terminal.screen_text().contains("got:ping"));
    }

    #[test]
    fn a_resize_reaches_the_child() {
        let terminal = start("read go; stty size");
        terminal.resize(GridSize::new(100, 30));
        terminal.write(&b"\n"[..]);
        wait_for("the size", || terminal.screen_text().contains("30 100"));
        assert_eq!(terminal.size().cols, 100);
    }

    #[test]
    fn the_child_sees_the_terminal_type_and_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = sh_c("printf '%s|%s|%s' \"$TERM\" \"$COLORTERM\" \"$(pwd -P)\"");
        spec.cwd = Some(dir.path().to_string_lossy().into_owned());
        let terminal = spawn(&spec, GridSize::new(120, 24));
        let wanted = dir.path().canonicalize().unwrap();
        wait_for("the report", || {
            terminal
                .screen_text()
                .contains(&format!("xterm-256color|truecolor|{}", wanted.display()))
        });
    }

    #[test]
    fn the_title_a_program_sets_is_reported() {
        let terminal = start("printf '\\033]0;my title\\007'; read hold");
        wait_for("the title", || {
            terminal.title().as_deref() == Some("my title")
        });
        assert!(terminal
            .drain_events()
            .contains(&TerminalEvent::Title("my title".into())));
    }

    #[test]
    fn the_bell_is_reported() {
        let terminal = start("printf '\\007'; read hold");
        wait_for("the bell", || {
            terminal.drain_events().contains(&TerminalEvent::Bell)
        });
    }

    #[test]
    fn a_program_that_does_not_exist_is_an_error_not_a_panic() {
        let spec = SpawnSpec::new("/nonexistent/leon-no-such-program");
        let error =
            Terminal::spawn_with(&spec, GridSize::new(80, 24), theme(), FAST, Box::new(|| {}))
                .err()
                .expect("no such program");
        assert!(error.to_string().contains("leon-no-such-program"));
    }

    #[test]
    fn a_flood_of_output_wakes_the_ui_far_less_often_than_it_has_chunks() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let terminal = Terminal::spawn_with(
            &sh_c("i=0; while [ $i -lt 3000 ]; do echo line-$i; i=$((i+1)); done; echo FLOOD-DONE"),
            GridSize::new(80, 24),
            theme(),
            FAST,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .unwrap();
        // The UI never looks (never calls take_dirty): one wake in all.
        wait_for("the end of the flood", || {
            terminal.screen_text().contains("FLOOD-DONE")
        });
        assert!(
            wakes.load(Ordering::SeqCst) <= 3,
            "{} wakes",
            wakes.load(Ordering::SeqCst)
        );
        // After looking, the next output wakes it again.
        assert!(terminal.take_dirty());
        assert!(!terminal.take_dirty());
    }

    #[test]
    fn selecting_and_reading_back_text() {
        let terminal = start("printf 'select me please'; read hold");
        wait_for("the text", || {
            terminal.screen_text().contains("select me please")
        });
        terminal.start_selection(SelectionType::Simple, 0, 0, Side::Left);
        terminal.update_selection(5, 0, Side::Right);
        assert_eq!(terminal.selection_text().as_deref(), Some("select"));
        terminal.clear_selection();
        assert_eq!(terminal.selection_text(), None);
    }

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 only probes for the process.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[test]
    fn dropping_a_terminal_leaves_no_child_behind() {
        let terminal = start("read hold");
        let pid = terminal.process_id().expect("a pid");
        assert!(alive(pid));
        drop(terminal);
        wait_for("the child to be gone and reaped", || !alive(pid));
    }

    #[test]
    fn a_child_that_ignores_the_hangup_is_killed_after_the_grace_period() {
        let terminal = start("trap '' HUP; echo TRAPPED; while :; do sleep 1; done");
        let pid = terminal.process_id().expect("a pid");
        wait_for("the trap to be set", || {
            terminal.screen_text().contains("TRAPPED")
        });
        drop(terminal);
        wait_for("the stubborn child to be killed", || !alive(pid));
    }

    #[test]
    fn a_shell_with_no_startup_file_is_the_one_tests_use() {
        // The prompt is the one the test chose, not one from a startup file.
        let terminal = spawn(&sh(&["-i"]), GridSize::new(80, 24));
        wait_for("the prompt", || terminal.screen_text().contains("READY>"));
    }
}

/// The Windows counterpart of the PTY round trips. These were written without
/// a Windows machine to run them on: they use only `cmd`, which every Windows
/// has, and are unverified. The Unix suite above covers the shared logic.
#[cfg(all(test, windows))]
mod windows_pty_tests {
    use super::*;
    use crate::colors::from_rgb8;

    fn theme() -> TerminalTheme {
        TerminalTheme {
            foreground: from_rgb8(250, 250, 250),
            background: from_rgb8(0, 0, 0),
            cursor: from_rgb8(97, 95, 255),
            selection: from_rgb8(30, 30, 90),
            find_match: from_rgb8(60, 60, 20),
            find_match_current: from_rgb8(250, 200, 0),
            ansi: std::array::from_fn(|i| from_rgb8(i as u8 * 16, 0, 0)),
        }
    }

    fn cmd(script: &str) -> SpawnSpec {
        SpawnSpec {
            program: "cmd".into(),
            args: vec!["/c".into(), script.into()],
            env: Vec::new(),
            cwd: None,
            route: None,
        }
    }

    fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_real_child_prints_a_marker_that_reaches_the_grid() {
        let terminal = Terminal::spawn(
            &cmd("echo MARKER-42"),
            GridSize::new(80, 24),
            theme(),
            || {},
        )
        .unwrap();
        wait_for("the marker", || {
            terminal.screen_text().contains("MARKER-42")
        });
    }

    #[test]
    fn the_exit_status_is_reported() {
        let terminal =
            Terminal::spawn(&cmd("exit 3"), GridSize::new(80, 24), theme(), || {}).unwrap();
        wait_for("the exit", || terminal.exit_info().is_some());
        assert_eq!(terminal.exit_info().unwrap().code, 3);
    }
}

#[cfg(all(test, unix))]
mod foreground_tests {
    use super::pty_tests::FAST;
    use super::*;
    use crate::testing::{sh, theme, wait_for};

    fn shell() -> Terminal {
        Terminal::spawn_with(
            &sh(&["-i"]),
            GridSize::new(80, 24),
            theme(),
            FAST,
            Box::new(|| {}),
        )
        .unwrap()
    }

    #[test]
    fn a_program_in_front_is_asked_to_end_and_the_shell_is_left_alone() {
        let terminal = shell();
        wait_for("the prompt", || terminal.screen_text().contains("READY>"));
        // Nothing runs in front of the shell: there is nobody to ask.
        assert!(!terminal.terminate_foreground());
        terminal.write(&b"cat\n"[..]);
        wait_for("cat to take the terminal", || {
            terminal.shell_is_foreground() == Some(false)
        });
        assert!(terminal.terminate_foreground(), "SIGTERM reached cat");
        wait_for("the shell to get the terminal back", || {
            terminal.shell_is_foreground() == Some(true)
        });
        assert!(terminal.exit_info().is_none(), "the shell was not touched");
    }

    #[test]
    fn the_shell_is_in_front_until_it_runs_a_program_and_again_after() {
        let terminal = shell();
        wait_for("the prompt", || terminal.screen_text().contains("READY>"));
        assert_eq!(terminal.shell_is_foreground(), Some(true));
        // `cat` is a program of its own that holds the terminal until it
        // reads the end of its input.
        terminal.write(&b"cat\n"[..]);
        wait_for("cat to take the terminal", || {
            terminal.shell_is_foreground() == Some(false)
        });
        terminal.write(&b"\x04"[..]);
        wait_for("the shell to get it back", || {
            terminal.shell_is_foreground() == Some(true)
        });
        // The shell is alive and takes input after the program ended.
        assert!(terminal.exit_info().is_none());
        terminal.write(&b"echo STILL-HERE\n"[..]);
        wait_for("the echo", || terminal.screen_text().contains("STILL-HERE"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_program_in_front_of_the_shell_is_named() {
        let terminal = shell();
        wait_for("the prompt", || terminal.screen_text().contains("READY>"));
        assert_eq!(terminal.foreground_command(), None);
        terminal.write(&b"cat\n"[..]);
        wait_for("cat to take the terminal", || {
            terminal.foreground_command().as_deref() == Some("cat")
        });
        terminal.write(&b"\x04"[..]);
        wait_for("the shell to get it back", || {
            terminal.foreground_command().is_none()
        });
    }

    #[test]
    fn a_terminal_is_quiet_after_its_prompt() {
        let terminal = shell();
        // Whether the shell has already printed when the terminal is first
        // looked at depends on the machine's load, so nothing is asserted
        // before the output exists.
        wait_for("the first output", || terminal.quiet_for().is_some());
        wait_for("a quiet prompt", || {
            terminal
                .quiet_for()
                .is_some_and(|quiet| quiet > Duration::from_millis(20))
        });
    }
}
