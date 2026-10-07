//! A terminal with no process behind it, for tests of code that only uses a
//! terminal: how it is laid out, focused, closed, what keys it is sent.
//!
//! A [`Scripted`] backend hands out ordinary [`Terminal`]s (the same emulator,
//! grid, events and wake-ups as with a PTY) whose child is a [`Script`]: the
//! test prints what the child would print, reads what the terminal was
//! sent, and ends it. Nothing is spawned, no thread is started and nothing
//! waits on the clock, so what happens is decided by the test alone.
//!
//! What a real PTY gives that this does not (line discipline, echo, a real
//! shell, signals, resizes seen by a child) is covered by the PTY tests of
//! this crate and a few end-to-end tests in the application.

use super::*;

/// What a script does with the bytes the terminal is sent.
type Handler = Arc<dyn Fn(&Script, &[u8]) + Send + Sync>;

struct Inner {
    spec: SpawnSpec,
    term: Arc<Mutex<Term<EventProxy>>>,
    shared: Arc<Shared>,
    parser: Mutex<Processor>,
    /// Everything the terminal was sent, in order.
    written: Mutex<Vec<u8>>,
    /// The emulator's own replies (colour and cursor queries).
    replies: Mutex<mpsc::Receiver<Vec<u8>>>,
    handler: Mutex<Option<Handler>>,
    foreground: AtomicBool,
    /// The name of the program in front of the shell, as the system names it.
    program: Mutex<Option<String>>,
    hung_up: AtomicBool,
    /// Whether the program in front of the shell was sent SIGTERM.
    terminated: AtomicBool,
    /// Where a remote terminal's input, resizes and hang-up go.
    link: Mutex<Option<Arc<dyn RemoteLink>>>,
}

/// The child of a scripted terminal, as the test sees it.
#[derive(Clone)]
pub struct Script(Arc<Inner>);

impl Script {
    /// Prints what the child would print. A bare `\n` becomes `\r\n`, as a
    /// terminal's line discipline makes it.
    pub fn print(&self, text: impl AsRef<[u8]>) {
        let mut bytes = Vec::new();
        let mut last = 0u8;
        for &byte in text.as_ref() {
            if byte == b'\n' && last != b'\r' {
                bytes.push(b'\r');
            }
            bytes.push(byte);
            last = byte;
        }
        self.feed_raw(&bytes);
    }

    /// Feeds output bytes exactly as they are.
    pub(super) fn feed_raw(&self, bytes: &[u8]) {
        let inner = &self.0;
        feed(&mut inner.parser.lock(), &mut inner.term.lock(), bytes);
        inner.shared.has_output.store(true, Ordering::Release);
        inner.shared.generation.fetch_add(1, Ordering::AcqRel);
        let elapsed = inner.shared.started.elapsed().as_millis() as u64;
        inner
            .shared
            .last_output_ms
            .store(elapsed + 1, Ordering::Release);
        inner.shared.wake_once();
    }

    /// Ends the child with `code`.
    pub fn exit(&self, code: u32) {
        self.end(ExitInfo { code, signal: None });
    }

    /// Whether the shell is the foreground process of the terminal, as
    /// [`Terminal::shell_is_foreground`] reports it. A new script starts with
    /// the shell in front.
    pub fn set_foreground(&self, shell_in_front: bool) {
        self.0.foreground.store(shell_in_front, Ordering::Release);
        self.0.shared.wake_once();
    }

    /// The name of the program in front of the shell, as
    /// [`Terminal::foreground_command`] reports it while the shell is not.
    pub fn set_program(&self, program: Option<&str>) {
        *self.0.program.lock() = program.map(str::to_owned);
        self.0.shared.wake_once();
    }

    /// Decides what happens when the terminal is sent bytes: `handler` runs
    /// on the thread that sent them, once per send. Replaces any earlier one.
    pub fn on_input(&self, handler: impl Fn(&Script, &[u8]) + Send + Sync + 'static) {
        *self.0.handler.lock() = Some(Arc::new(handler));
    }

    /// Everything the terminal has been sent so far: keys, text, pastes and
    /// the emulator's replies.
    pub fn written(&self) -> Vec<u8> {
        let inner = &self.0;
        let mut log = inner.written.lock();
        log.extend(inner.replies.lock().try_iter().flatten());
        log.clone()
    }

    /// [`Self::written`] as text.
    pub fn written_text(&self) -> String {
        String::from_utf8_lossy(&self.written()).into_owned()
    }

    /// What was asked to run.
    pub fn spec(&self) -> &SpawnSpec {
        &self.0.spec
    }

    /// The grid size in force: what a child would be told.
    pub fn size(&self) -> GridSize {
        *self.0.shared.size.lock()
    }

    /// How the child ended, once it has.
    pub fn exit_info(&self) -> Option<ExitInfo> {
        self.0.shared.exit.lock().clone()
    }

    /// Whether the terminal hung the child up (it was closed while running).
    pub fn hung_up(&self) -> bool {
        self.0.hung_up.load(Ordering::Acquire)
    }

    pub(super) fn input(&self, bytes: Vec<u8>) {
        if let Some(link) = self.link() {
            link.write(bytes);
            return;
        }
        self.0.written.lock().extend_from_slice(&bytes);
        let handler = self.0.handler.lock().clone();
        if let Some(handler) = handler {
            handler(self, &bytes);
        }
    }

    fn link(&self) -> Option<Arc<dyn RemoteLink>> {
        self.0.link.lock().clone()
    }

    pub(super) fn is_remote(&self) -> bool {
        self.0.link.lock().is_some()
    }

    pub(super) fn resized(&self, size: GridSize) {
        if let Some(link) = self.link() {
            link.resize(size);
        }
    }

    /// Lets go of a remote terminal without ending its program.
    pub(super) fn detach(&self) {
        if let Some(link) = self.link() {
            link.detach();
        }
    }

    pub(super) fn hang_up(&self) {
        if let Some(link) = self.link() {
            // The program on the other computer is hung up; its end arrives
            // as the host's report.
            link.close();
            return;
        }
        self.0.hung_up.store(true, Ordering::Release);
        self.end(ExitInfo {
            code: 1,
            signal: Some("Hangup".into()),
        });
    }

    /// Whether the program in front of the shell was asked to end with a
    /// termination signal ([`Terminal::terminate_foreground`]).
    pub fn was_terminated(&self) -> bool {
        self.0.terminated.load(Ordering::Acquire)
    }

    pub(super) fn terminate(&self) -> bool {
        if self.is_remote() || self.0.foreground.load(Ordering::Acquire) {
            return false;
        }
        self.0.terminated.store(true, Ordering::Release);
        true
    }

    pub(super) fn foreground_command(&self) -> Option<String> {
        if self.is_remote() || self.0.foreground.load(Ordering::Acquire) {
            return None;
        }
        self.0.program.lock().clone()
    }

    pub(super) fn shell_is_foreground(&self) -> bool {
        self.0.foreground.load(Ordering::Acquire)
    }

    fn end(&self, info: ExitInfo) {
        let shared = &self.0.shared;
        {
            let mut exit = shared.exit.lock();
            if exit.is_some() {
                return;
            }
            *exit = Some(info.clone());
        }
        shared.push(TerminalEvent::Exited(info));
    }
}

impl Terminal {
    /// A terminal whose child is the returned [`Script`]. `spec` is only
    /// recorded.
    pub fn scripted(
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
    ) -> (Self, Script) {
        let shared = Shared::new(size, theme, wake);
        let (replies_tx, replies_rx) = mpsc::channel();
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
            EventProxy {
                shared: shared.clone(),
                input: replies_tx,
            },
        )));
        let script = Script(Arc::new(Inner {
            spec: spec.clone(),
            term: term.clone(),
            shared: shared.clone(),
            parser: Mutex::new(Processor::new()),
            written: Mutex::new(Vec::new()),
            replies: Mutex::new(replies_rx),
            handler: Mutex::new(None),
            foreground: AtomicBool::new(true),
            program: Mutex::new(None),
            hung_up: AtomicBool::new(false),
            terminated: AtomicBool::new(false),
            link: Mutex::new(None),
        }));
        let terminal = Self {
            term,
            shared,
            input: None,
            master: Mutex::new(None),
            killer: Mutex::new(None),
            pid: None,
            timings: Timings::default(),
            script: Some(script.clone()),
        };
        (terminal, script)
    }
}

/// Where a terminal whose program runs on another computer sends what the
/// user does. The terminal itself is the same emulator as for a local PTY; only
/// its two ends differ: bytes arrive through a [`RemoteFeed`] and leave through
/// this link.
pub trait RemoteLink: Send + Sync {
    /// Keystrokes, pastes and the emulator's replies.
    fn write(&self, bytes: Vec<u8>);
    /// The grid changed size.
    fn resize(&self, size: GridSize);
    /// The user closed the terminal: hang the remote program up.
    fn close(&self);
    /// The application lets go (it is quitting): the remote program keeps
    /// running and can be attached to again.
    fn detach(&self);
}

/// The producing end of a remote terminal: what arrives from the other
/// computer is fed in here.
#[derive(Clone)]
pub struct RemoteFeed(Script);

impl RemoteFeed {
    /// Output bytes from the remote program, in order.
    pub fn data(&self, bytes: &[u8]) {
        self.0.feed_raw(bytes);
    }

    /// The other computer no longer had some output: start the screen over.
    pub fn gap(&self) {
        // RIS: full reset, so a half-received screen does not linger.
        self.0.feed_raw(b"\x1bc");
    }

    /// Text for the person, in the terminal's own scrollback (connection
    /// notices). A bare `\n` becomes `\r\n`.
    pub fn notice(&self, text: &str) {
        self.0.print(text);
    }

    /// The remote program ended.
    pub fn exit(&self, code: u32, signal: Option<String>) {
        self.0.end(ExitInfo { code, signal });
    }
}

impl Terminal {
    /// A terminal whose program runs on another computer. Returns the terminal
    /// and the feed through which the other computer's output is delivered;
    /// what the user does goes to `link`.
    pub fn remote(
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
        link: Arc<dyn RemoteLink>,
    ) -> (Self, RemoteFeed) {
        let (terminal, script) = Self::scripted(spec, size, theme, wake);
        *script.0.link.lock() = Some(link);
        (terminal, RemoteFeed(script))
    }

    /// Whether the program runs on another computer.
    pub fn is_remote(&self) -> bool {
        self.script.as_ref().is_some_and(Script::is_remote)
    }

    /// Lets go of a remote terminal without ending its program (the
    /// application is quitting). Does nothing for a local one.
    pub fn detach(&self) {
        if let Some(script) = &self.script {
            script.detach();
        }
    }
}

/// A backend of scripted terminals that remembers each one's [`Script`], in
/// the order they were started.
pub struct Scripted {
    spawned: Mutex<Vec<Script>>,
    setup: Box<dyn Fn(&Script) + Send + Sync>,
}

impl Scripted {
    /// Terminals that start silent and ignore what they are sent.
    pub fn new() -> Self {
        Self::with_setup(|_| {})
    }

    /// Terminals that `setup` prepares as they start: it can print a banner
    /// and install an [`Script::on_input`] handler.
    pub fn with_setup(setup: impl Fn(&Script) + Send + Sync + 'static) -> Self {
        Self {
            spawned: Mutex::new(Vec::new()),
            setup: Box::new(setup),
        }
    }

    /// The scripts of the terminals started so far, oldest first.
    pub fn scripts(&self) -> Vec<Script> {
        self.spawned.lock().clone()
    }

    /// The script of the `n`th terminal started (from 0).
    pub fn script(&self, n: usize) -> Option<Script> {
        self.spawned.lock().get(n).cloned()
    }
}

impl Default for Scripted {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Scripted {
    fn spawn(
        &self,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        wake: Wake,
    ) -> Result<Terminal, SpawnError> {
        let (terminal, script) = Terminal::scripted(spec, size, theme, wake);
        (self.setup)(&script);
        self.spawned.lock().push(script);
        Ok(terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::theme;

    fn start() -> (Terminal, Script) {
        Terminal::scripted(
            &SpawnSpec::new("sh"),
            GridSize::new(40, 5),
            theme(),
            Box::new(|| {}),
        )
    }

    #[test]
    fn what_the_script_prints_is_on_the_screen_and_counts_as_output() {
        let (terminal, script) = start();
        assert!(!terminal.has_output());
        assert_eq!(terminal.quiet_for(), None);
        script.print("hello\nworld");
        assert_eq!(terminal.screen_text(), "hello\nworld");
        assert!(terminal.has_output());
        assert!(terminal.quiet_for().is_some());
    }

    #[test]
    fn what_the_terminal_is_sent_is_recorded_and_offered_to_the_handler() {
        let (terminal, script) = start();
        script.on_input(|script, bytes| {
            if bytes == b"ping" {
                script.print("pong");
            }
        });
        terminal.write(&b"ping"[..]);
        terminal.write(&b"!"[..]);
        assert_eq!(script.written_text(), "ping!");
        assert_eq!(terminal.screen_text(), "pong");
    }

    #[test]
    fn the_emulators_own_replies_are_part_of_what_was_sent() {
        let (_terminal, script) = start();
        script.print("\x1b[6n"); // where is the cursor?
        assert_eq!(script.written_text(), "\x1b[1;1R");
    }

    #[test]
    fn ending_it_reports_the_exit_once() {
        let (terminal, script) = start();
        script.exit(7);
        script.exit(9);
        assert_eq!(terminal.exit_info().map(|info| info.code), Some(7));
        let exited = terminal
            .drain_events()
            .into_iter()
            .filter(|event| matches!(event, TerminalEvent::Exited(_)))
            .count();
        assert_eq!(exited, 1);
    }

    #[test]
    fn closing_a_running_terminal_hangs_the_child_up() {
        let (terminal, script) = start();
        terminal.kill();
        assert!(script.hung_up());
        assert_eq!(
            script.exit_info().and_then(|info| info.signal).as_deref(),
            Some("Hangup")
        );
    }

    #[test]
    fn the_foreground_is_the_scripts_to_say() {
        let (terminal, script) = start();
        assert_eq!(terminal.shell_is_foreground(), Some(true));
        script.set_foreground(false);
        assert_eq!(terminal.shell_is_foreground(), Some(false));
    }

    #[test]
    fn the_backend_remembers_each_script_and_what_was_asked_to_run() {
        let backend = Scripted::with_setup(|script| script.print("READY> "));
        let spec = SpawnSpec::new("zsh");
        let terminal = backend
            .spawn(&spec, GridSize::new(40, 5), theme(), Box::new(|| {}))
            .unwrap();
        assert_eq!(terminal.screen_text(), "READY>");
        assert_eq!(backend.script(0).unwrap().spec(), &spec);
        assert!(backend.script(1).is_none());
    }

    fn history(terminal: &Terminal) -> usize {
        use alacritty_terminal::grid::Dimensions as _;
        terminal.with_term(|term| term.grid().history_size())
    }

    #[test]
    fn the_scrollback_setting_bounds_the_buffer() {
        let (terminal, script) = start();
        for n in 0..300 {
            script.print(format!("line {n}\n"));
        }
        assert!(history(&terminal) > 250, "the default keeps them all");
        terminal.set_scrollback(100);
        assert!(history(&terminal) <= 100, "what was kept is cut");
        for n in 0..300 {
            script.print(format!("more {n}\n"));
        }
        assert_eq!(history(&terminal), 100, "and the buffer stays at the bound");
        terminal.set_scrollback(0);
        script.print("again\n");
        assert_eq!(history(&terminal), 0, "zero keeps none");
        assert_eq!(terminal.options().0, 0);
    }

    #[test]
    fn the_cursor_shape_setting_is_the_one_a_program_gets_until_it_asks_for_another() {
        use alacritty_terminal::vte::ansi::CursorShape;
        let (terminal, script) = start();
        let shape = |terminal: &Terminal| terminal.with_term(|term| term.cursor_style().shape);
        assert_eq!(shape(&terminal), CursorShape::Block);
        terminal.set_cursor_shape(CursorShape::Beam);
        assert_eq!(shape(&terminal), CursorShape::Beam);
        script.print("\x1b[4 q"); // the program asks for an underline
        assert_eq!(shape(&terminal), CursorShape::Underline);
        terminal.set_cursor_shape(CursorShape::Block);
        script.print("\x1b[0 q"); // and then for the default again
        assert_eq!(shape(&terminal), CursorShape::Block);
    }

    #[derive(Default)]
    struct Recording {
        events: Mutex<Vec<String>>,
    }

    impl RemoteLink for Recording {
        fn write(&self, bytes: Vec<u8>) {
            self.events
                .lock()
                .push(format!("write {}", String::from_utf8_lossy(&bytes)));
        }
        fn resize(&self, size: GridSize) {
            self.events
                .lock()
                .push(format!("resize {}x{}", size.cols, size.rows));
        }
        fn close(&self) {
            self.events.lock().push("close".into());
        }
        fn detach(&self) {
            self.events.lock().push("detach".into());
        }
    }

    fn remote() -> (Terminal, RemoteFeed, Arc<Recording>) {
        let link = Arc::new(Recording::default());
        let (terminal, feed) = Terminal::remote(
            &SpawnSpec::new("claude"),
            GridSize::new(40, 5),
            crate::testing::theme(),
            Box::new(|| {}),
            link.clone(),
        );
        (terminal, feed, link)
    }

    #[test]
    fn a_remote_terminal_draws_what_arrives_and_sends_what_the_user_does() {
        let (terminal, feed, link) = remote();
        assert!(terminal.is_remote());
        feed.data(b"hello\r\nworld");
        assert_eq!(terminal.screen_text(), "hello\nworld");
        terminal.write(&b"ls\r"[..]);
        terminal.resize(GridSize::new(100, 30));
        assert_eq!(*link.events.lock(), ["write ls\r", "resize 100x30"]);
    }

    #[test]
    fn closing_a_remote_terminal_hangs_the_remote_program_up_and_waits_for_its_report() {
        let (terminal, feed, link) = remote();
        terminal.kill();
        assert_eq!(*link.events.lock(), ["close"]);
        assert!(
            terminal.exit_info().is_none(),
            "the end is the host's to report"
        );
        feed.exit(1, Some("Hangup".into()));
        assert_eq!(
            terminal.exit_info().and_then(|i| i.signal).as_deref(),
            Some("Hangup")
        );
    }

    #[test]
    fn dropping_a_remote_terminal_detaches_instead_of_ending_the_program() {
        let (terminal, _feed, link) = remote();
        drop(terminal);
        assert_eq!(*link.events.lock(), ["detach"]);
    }

    #[test]
    fn nothing_is_claimed_about_the_foreground_of_a_remote_program() {
        let (terminal, ..) = remote();
        assert_eq!(terminal.shell_is_foreground(), None);
    }

    #[test]
    fn a_gap_starts_the_screen_over() {
        let (terminal, feed, _link) = remote();
        feed.data(b"stale screen");
        feed.gap();
        feed.data(b"fresh");
        assert_eq!(terminal.screen_text(), "fresh");
    }

    #[test]
    fn a_remote_terminal_does_not_keep_a_log_of_what_it_was_sent() {
        let (terminal, ..) = remote();
        terminal.write(&b"secret"[..]);
        terminal.with_term(|_| ());
        assert!(terminal.script.as_ref().unwrap().written().is_empty());
    }
}
