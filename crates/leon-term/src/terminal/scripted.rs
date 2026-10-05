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
    hung_up: AtomicBool,
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
        let inner = &self.0;
        feed(&mut inner.parser.lock(), &mut inner.term.lock(), &bytes);
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
        self.0.written.lock().extend_from_slice(&bytes);
        let handler = self.0.handler.lock().clone();
        if let Some(handler) = handler {
            handler(self, &bytes);
        }
    }

    pub(super) fn hang_up(&self) {
        self.0.hung_up.store(true, Ordering::Release);
        self.end(ExitInfo {
            code: 1,
            signal: Some("Hangup".into()),
        });
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
            hung_up: AtomicBool::new(false),
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
}
