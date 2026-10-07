//! The GPUI side: [`TerminalView`], an entity that draws a [`Terminal`] and
//! feeds it keys, text, the mouse and the clipboard.
//!
//! * **Painting.** One custom paint pass per frame, only when the terminal
//!   woke the UI. The visible cells are streamed under the emulator's lock
//!   into a [`Batcher`] whose buffers live across frames; the lock is
//!   released before anything is drawn. Backgrounds are merged rectangles,
//!   text is shaped per run of one style with each glyph forced onto its
//!   cell, the cursor and the selection come last.
//! * **Keys.** The window's key interceptor (the application's) offers every
//!   keystroke to [`TerminalView::handle_keystroke`] after taking its own
//!   chords; named keys, Ctrl and Alt become escape sequences. Plain text
//!   arrives through the platform text input path
//!   ([`EntityInputHandler`]), so dead keys, input methods and non-US
//!   layouts compose as the system says.
//! * **Mouse.** Wheel scrolls the history, or is reported to a program that
//!   asked for the mouse, or becomes arrow keys on the alternate screen.
//!   Click and drag select (double click a word, triple click a line); with
//!   Shift held, or when the program did not ask for the mouse, selection is
//!   always available. Clicking a link offers to open it, even under a program that
//!   captures the mouse; Ctrl+click (Command+click on macOS) opens it at once.

use crate::colors::{mix, TerminalTheme};
use crate::keys::{self, MouseAction, MouseEvent};
use crate::layout::{Batcher, CellInput};
use crate::size::GridSize;
use crate::spec::SpawnSpec;
use crate::terminal::{visible_links, Backend, ExitInfo, Pty, SpawnError, Terminal, TerminalEvent};
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::Side;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui_kit::{
    canvas, div, fill, outline, px, App, AppContext as _, BorderStyle, Bounds, ClipboardItem,
    Context, CursorStyle, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight, InteractiveElement as _,
    IntoElement, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString, Size,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, Task, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window,
};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// The font a terminal is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct FontSettings {
    /// The family. It should be monospaced.
    pub family: SharedString,
    /// The size in pixels.
    pub size: Pixels,
    /// The line height as a multiple of the size.
    pub line_height: f32,
}

/// What a view reports to the code that hosts it.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewEvent {
    /// The program set (`Some`) or reset (`None`) the title.
    Title(Option<String>),
    /// The bell rang.
    Bell,
    /// The child ended.
    Exited(ExitInfo),
    /// The view was clicked: the host should give it the keyboard.
    Clicked,
    /// A right click that is Leon's and not the program's: the host shows
    /// its menu at this point. A program that asked for the mouse keeps
    /// the right button unless Shift is held, as in other terminals.
    ContextMenu(Point<Pixels>),
}

/// Where the grid was last drawn, for turning pointer positions into cells.
#[derive(Clone, Copy, Debug, Default)]
struct Metrics {
    origin: Point<Pixels>,
    cell: Size<Pixels>,
    cols: usize,
    rows: usize,
}

/// What survives from frame to frame.
#[derive(Default)]
struct Scratch {
    batcher: Batcher,
}

#[derive(Clone, Debug, PartialEq)]
struct LinkPrompt {
    uri: String,
    col: usize,
    row: usize,
}

/// A terminal drawn on the GPU.
pub struct TerminalView {
    terminal: Arc<Terminal>,
    theme: TerminalTheme,
    font: FontSettings,
    focus: FocusHandle,
    metrics: Rc<Cell<Metrics>>,
    scratch: Rc<RefCell<Scratch>>,
    selecting: bool,
    link_clicked: bool,
    hovered_link: Option<String>,
    link_prompt: Option<LinkPrompt>,
    scroll_fraction: f32,
    marked: Option<String>,
    padding: Pixels,
    /// Whether Alt (Option) is Meta: it sends Escape before the key. Off, it
    /// types the composed character as the keyboard layout says.
    option_as_meta: bool,
    /// Whether a selection is copied to the clipboard when the mouse is let go.
    copy_on_select: bool,
    _wake: Task<()>,
}

impl EventEmitter<ViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// Starts `spec` and returns the view that shows it. The grid starts at
    /// `size` and follows the view's bounds from the first paint on.
    pub fn spawn(
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        font: FontSettings,
        cx: &mut App,
    ) -> Result<Entity<Self>, SpawnError> {
        Self::spawn_with(&Pty::default(), spec, size, theme, font, cx)
    }

    /// [`Self::spawn`] with the terminal coming from `backend`.
    pub fn spawn_with(
        backend: &dyn Backend,
        spec: &SpawnSpec,
        size: GridSize,
        theme: TerminalTheme,
        font: FontSettings,
        cx: &mut App,
    ) -> Result<Entity<Self>, SpawnError> {
        let (wake_tx, wake_rx) = flume::unbounded::<()>();
        let terminal = Arc::new(backend.spawn(
            spec,
            size,
            theme,
            Box::new(move || {
                let _ = wake_tx.send(());
            }),
        )?);
        Ok(cx.new(|cx| {
            let wake = cx.spawn(async move |this, cx| {
                while wake_rx.recv_async().await.is_ok() {
                    while wake_rx.try_recv().is_ok() {}
                    if this
                        .update(cx, |this: &mut Self, cx| this.woken(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            });
            Self {
                terminal,
                theme,
                font,
                focus: cx.focus_handle(),
                metrics: Rc::new(Cell::new(Metrics::default())),
                scratch: Rc::new(RefCell::new(Scratch::default())),
                selecting: false,
                link_clicked: false,
                hovered_link: None,
                link_prompt: None,
                scroll_fraction: 0.0,
                marked: None,
                padding: px(0.),
                option_as_meta: true,
                copy_on_select: false,
                _wake: wake,
            }
        }))
    }

    /// The terminal model.
    pub fn terminal(&self) -> &Arc<Terminal> {
        &self.terminal
    }

    /// The colours in use.
    pub fn theme(&self) -> &TerminalTheme {
        &self.theme
    }

    /// The font in use.
    pub fn font(&self) -> &FontSettings {
        &self.font
    }

    /// The view's focus handle.
    pub fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// Sets the padding around the grid.
    pub fn set_padding(&mut self, padding: Pixels) {
        self.padding = padding;
    }

    /// Chooses whether Alt is Meta (the default) or types composed characters.
    pub fn set_option_as_meta(&mut self, on: bool) {
        self.option_as_meta = on;
    }

    /// Chooses whether a selection is copied as soon as the mouse is let go.
    pub fn set_copy_on_select(&mut self, on: bool) {
        self.copy_on_select = on;
    }

    /// Changes the colours.
    pub fn set_theme(&mut self, theme: TerminalTheme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        self.terminal.set_theme(theme);
        cx.notify();
    }

    /// Changes the font.
    pub fn set_font(&mut self, font: FontSettings, cx: &mut Context<Self>) {
        if self.font != font {
            self.font = font;
            cx.notify();
        }
    }

    /// The terminal has something new: report its events and ask for a
    /// repaint.
    fn woken(&mut self, cx: &mut Context<Self>) {
        for event in self.terminal.drain_events() {
            match event {
                TerminalEvent::Title(title) => cx.emit(ViewEvent::Title(Some(title))),
                TerminalEvent::ResetTitle => cx.emit(ViewEvent::Title(None)),
                TerminalEvent::Bell => cx.emit(ViewEvent::Bell),
                TerminalEvent::ClipboardStore(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                TerminalEvent::Exited(info) => cx.emit(ViewEvent::Exited(info)),
            }
        }
        cx.notify();
    }

    // ----- keyboard ------------------------------------------------------------------------

    /// Sends a keystroke to the program when it is a terminal key. `false`
    /// for plain text, which comes through the text input path.
    pub fn handle_keystroke(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        // Alt that is not Meta types what the layout composes (å, é): that is
        // the text input path's, so no escape sequence is made for it.
        if !self.option_as_meta
            && stroke.modifiers.alt
            && !stroke.modifiers.control
            && !stroke.modifiers.platform
            && stroke
                .key_char
                .as_deref()
                .is_some_and(|text| !text.is_empty())
        {
            return false;
        }
        match keys::encode_key(stroke, self.terminal.mode()) {
            Some(bytes) => {
                self.terminal.scroll_to_bottom();
                self.terminal.write(bytes);
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// Copies the selection to the clipboard. `true` when there was one.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        match self.terminal.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    /// Pastes the clipboard's text.
    pub fn paste(&mut self, cx: &mut Context<Self>) -> bool {
        match cx.read_from_clipboard().and_then(|item| item.text()) {
            Some(text) if !text.is_empty() => {
                self.terminal.scroll_to_bottom();
                self.terminal.paste(&text);
                true
            }
            _ => false,
        }
    }

    /// A page back into the history.
    pub fn scroll_page_up(&mut self, cx: &mut Context<Self>) {
        self.terminal.scroll_page_up();
        cx.notify();
    }

    /// A page forward.
    pub fn scroll_page_down(&mut self, cx: &mut Context<Self>) {
        self.terminal.scroll_page_down();
        cx.notify();
    }

    // ----- mouse -----------------------------------------------------------------------------

    /// The cell under `position` and which half of it, clamped to the grid.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let m = self.metrics.get();
        let (cw, ch) = (
            m.cell.width.as_f32().max(1.0),
            m.cell.height.as_f32().max(1.0),
        );
        let x = (position.x - m.origin.x).as_f32();
        let y = (position.y - m.origin.y).as_f32();
        let col = (x / cw).floor().max(0.0) as usize;
        let row = (y / ch).floor().max(0.0) as usize;
        let side = if x.rem_euclid(cw) < cw / 2.0 {
            Side::Left
        } else {
            Side::Right
        };
        (
            col.min(m.cols.saturating_sub(1)),
            row.min(m.rows.saturating_sub(1)),
            side,
        )
    }

    fn report(
        &self,
        button: keys::MouseButton,
        action: MouseAction,
        position: Point<Pixels>,
        modifiers: &gpui_kit::Modifiers,
    ) -> bool {
        let (col, row, _) = self.cell_at(position);
        let event = MouseEvent {
            button,
            action,
            col,
            row,
            shift: modifiers.shift,
            alt: modifiers.alt,
            ctrl: modifiers.control,
        };
        match keys::mouse_report(&event, self.terminal.mode()) {
            Some(bytes) => {
                self.terminal.write(bytes);
                true
            }
            None => false,
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        cx.emit(ViewEvent::Clicked);
        // A click on a link offers to open it even where a program captures
        // the mouse (Claude Code does, and prints links); the secondary key
        // (Cmd or Ctrl) opens it at once.
        if event.button == MouseButton::Left {
            let (col, row, _) = self.cell_at(event.position);
            if let Some(link) = self.terminal.link_at(col, row) {
                if event.modifiers.secondary() {
                    self.link_prompt = None;
                    self.link_clicked = true;
                    cx.open_url(&link);
                } else {
                    self.link_prompt = Some(LinkPrompt {
                        uri: link,
                        col,
                        row,
                    });
                    cx.notify();
                }
                return;
            }
        }
        if self.link_prompt.take().is_some() {
            cx.notify();
        }
        let reporting = keys::mouse_reporting(self.terminal.mode()) && !event.modifiers.shift;
        if reporting {
            let button = match event.button {
                MouseButton::Left => keys::MouseButton::Left,
                MouseButton::Middle => keys::MouseButton::Middle,
                MouseButton::Right => keys::MouseButton::Right,
                _ => return,
            };
            self.report(button, MouseAction::Press, event.position, &event.modifiers);
            return;
        }
        match event.button {
            MouseButton::Left => {
                let (col, row, side) = self.cell_at(event.position);
                let kind = match event.click_count {
                    0 | 1 => SelectionType::Simple,
                    2 => SelectionType::Semantic,
                    _ => SelectionType::Lines,
                };
                self.terminal.start_selection(kind, col, row, side);
                self.selecting = true;
                cx.notify();
            }
            MouseButton::Middle => {
                self.paste(cx);
            }
            MouseButton::Right => cx.emit(ViewEvent::ContextMenu(event.position)),
            _ => {}
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hovered_link = if event.dragging() {
            None
        } else {
            let (col, row, _) = self.cell_at(event.position);
            self.terminal.link_at(col, row)
        };
        if self.hovered_link != hovered_link {
            self.hovered_link = hovered_link;
            cx.notify();
        }
        if self.selecting && event.dragging() {
            let (col, row, side) = self.cell_at(event.position);
            self.terminal.update_selection(col, row, side);
            cx.notify();
            return;
        }
        let reporting = keys::mouse_reporting(self.terminal.mode()) && !event.modifiers.shift;
        if reporting {
            let (button, action) = match event.pressed_button {
                Some(MouseButton::Left) => (keys::MouseButton::Left, MouseAction::Drag),
                Some(MouseButton::Middle) => (keys::MouseButton::Middle, MouseAction::Drag),
                Some(MouseButton::Right) => (keys::MouseButton::Right, MouseAction::Drag),
                _ => (keys::MouseButton::Left, MouseAction::Move),
            };
            self.report(button, action, event.position, &event.modifiers);
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.link_clicked && event.button == MouseButton::Left {
            self.link_clicked = false;
            return;
        }
        if self.selecting {
            self.selecting = false;
            match self.terminal.selection_text() {
                None => self.terminal.clear_selection(),
                Some(text) if self.copy_on_select => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                Some(_) => {}
            }
            cx.notify();
            return;
        }
        if keys::mouse_reporting(self.terminal.mode()) && !event.modifiers.shift {
            let button = match event.button {
                MouseButton::Left => keys::MouseButton::Left,
                MouseButton::Middle => keys::MouseButton::Middle,
                MouseButton::Right => keys::MouseButton::Right,
                _ => return,
            };
            self.report(
                button,
                MouseAction::Release,
                event.position,
                &event.modifiers,
            );
        }
    }

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cell_height = self.metrics.get().cell.height.as_f32().max(1.0);
        let lines = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y.as_f32() / cell_height,
            ScrollDelta::Lines(delta) => delta.y,
        };
        self.scroll_fraction += lines;
        let whole = self.scroll_fraction.trunc();
        if whole == 0.0 {
            return;
        }
        self.scroll_fraction -= whole;
        let count = whole.abs() as usize;
        // Positive is the content moving down: back into the history.
        let up = whole > 0.0;
        let mode = self.terminal.mode();
        if keys::mouse_reporting(mode) && !event.modifiers.shift {
            let button = if up {
                keys::MouseButton::WheelUp
            } else {
                keys::MouseButton::WheelDown
            };
            for _ in 0..count {
                self.report(button, MouseAction::Press, event.position, &event.modifiers);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            self.terminal.write(keys::wheel_as_arrows(up, count, mode));
        } else {
            self.terminal
                .scroll_lines(if up { count as i32 } else { -(count as i32) });
        }
        cx.notify();
        cx.stop_propagation();
    }

    fn open_link_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.link_prompt.take() {
            cx.open_url(&prompt.uri);
            cx.notify();
        }
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.terminal.scroll_to_bottom();
            self.terminal.write(text.as_bytes().to_vec());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The input method's candidate window sits under the cursor.
        let m = self.metrics.get();
        let (col, row) = self.terminal.with_term(|term| {
            (
                term.grid().cursor.point.column.0,
                term.grid().cursor.point.line.0,
            )
        });
        let origin = Point {
            x: m.origin.x + m.cell.width * col as f32,
            y: m.origin.y + m.cell.height * row.max(0) as f32,
        };
        Some(Bounds {
            origin: Point {
                x: origin.x.max(element_bounds.origin.x),
                y: origin.y,
            },
            size: m.cell,
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let paint_view = view.clone();
        let terminal = self.terminal.clone();
        let theme = self.theme;
        let font = self.font.clone();
        let focus = self.focus.clone();
        let metrics = self.metrics.clone();
        let scratch = self.scratch.clone();
        let padding = self.padding;
        let hovered_link = self.hovered_link.clone();
        let link_prompt = self.link_prompt.clone();
        let current_metrics = self.metrics.get();
        let prompt = link_prompt.map(|prompt| {
            let uri = prompt.uri.clone();
            let approximate_cols =
                (80.0 / current_metrics.cell.width.as_f32().max(1.0)).ceil() as usize;
            let col = prompt
                .col
                .min(current_metrics.cols.saturating_sub(approximate_cols));
            let below = prompt.row + 2 < current_metrics.rows;
            let top = if below {
                current_metrics.cell.height * (prompt.row + 1) as f32
            } else {
                current_metrics.cell.height * prompt.row.saturating_sub(1) as f32
            };
            div()
                .id("terminal-open-link")
                .absolute()
                .left(padding + current_metrics.cell.width * col as f32)
                .top(padding + top)
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(theme.cursor)
                .bg(theme.background)
                .text_color(theme.cursor)
                .text_size(font.size * 0.9)
                .shadow_sm()
                .cursor_pointer()
                .hover(move |style| style.bg(theme.cursor).text_color(theme.background))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this
                        .link_prompt
                        .as_ref()
                        .is_some_and(|prompt| prompt.uri == uri)
                    {
                        this.open_link_prompt(cx);
                    }
                    cx.stop_propagation();
                }))
                .child("Open link")
        });
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(theme.background)
            .cursor(if hovered_link.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if !hovered && this.hovered_link.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        paint_grid(
                            &paint_view,
                            &terminal,
                            &theme,
                            &font,
                            &focus,
                            padding,
                            hovered_link.as_deref(),
                            bounds,
                            &metrics,
                            &mut scratch.borrow_mut(),
                            window,
                            cx,
                        );
                    },
                )
                .size_full(),
            )
            .children(prompt)
    }
}

fn font_for(settings: &FontSettings, bold: bool, italic: bool) -> Font {
    Font {
        family: settings.family.clone(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_grid(
    view: &Entity<TerminalView>,
    terminal: &Terminal,
    theme: &TerminalTheme,
    font: &FontSettings,
    focus: &FocusHandle,
    padding: Pixels,
    hovered_link: Option<&str>,
    bounds: Bounds<Pixels>,
    metrics: &Cell<Metrics>,
    scratch: &mut Scratch,
    window: &mut Window,
    cx: &mut App,
) {
    // Cell metrics from the font: the advance of one glyph, the line height.
    let regular = font_for(font, false, false);
    let font_id = window.text_system().resolve_font(&regular);
    let cell_width = window
        .text_system()
        .advance(font_id, font.size, 'm')
        .map_or(font.size * 0.6, |advance| advance.width);
    let cell_height = (font.size * font.line_height).round();
    let origin = Point {
        x: bounds.origin.x + padding,
        y: bounds.origin.y + padding,
    };
    let inner_width = (bounds.size.width - padding * 2.0).as_f32();
    let inner_height = (bounds.size.height - padding * 2.0).as_f32();
    if let Some(size) = GridSize::fit(
        inner_width,
        inner_height,
        cell_width.as_f32(),
        cell_height.as_f32(),
    ) {
        terminal.resize(size);
    }
    terminal.take_dirty();

    let focused = focus.is_focused(window);
    let batcher = &mut scratch.batcher;
    batcher.clear();
    let mut cursor: Option<(usize, usize, CursorShape)> = None;
    // The matches of the find bar, none while it is closed.
    let highlights = terminal.highlights();
    let mut next_match = 0usize;
    let (cols, rows) = terminal.with_term(|term| {
        let links = visible_links(term);
        let columns = term.grid().columns();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let overrides = content.colors;
        let selection = content.selection;
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let block_cursor = focused && cursor_shape == CursorShape::Block && offset == 0;
        if offset == 0 && cursor_shape != CursorShape::Hidden {
            cursor = Some((
                cursor_point.column.0,
                cursor_point.line.0.max(0) as usize,
                cursor_shape,
            ));
        }
        for indexed in content.display_iter {
            let point = indexed.point;
            let cell = indexed.cell;
            let row = point.line.0 + offset;
            if row < 0 {
                continue;
            }
            let mut fg = theme.resolve(cell.fg, overrides);
            let mut bg = theme.resolve(cell.bg, overrides);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.flags.contains(Flags::DIM) {
                fg = mix(fg, bg, 0.4);
            }
            let link = links
                .get(row.max(0) as usize * columns + point.column.0)
                .and_then(Option::as_deref);
            let mut flags = cell.flags;
            if link.is_some() {
                flags.insert(Flags::UNDERLINE);
            }
            if link_is_hovered(link, hovered_link) {
                fg = theme.cursor;
            }
            let mut paint_bg = cell.flags.contains(Flags::INVERSE) || bg != theme.background;
            if let Some(found) = &highlights {
                while next_match < found.matches.len() && *found.matches[next_match].end() < point {
                    next_match += 1;
                }
                if next_match < found.matches.len() && *found.matches[next_match].start() <= point {
                    paint_bg = true;
                    if found.current == Some(next_match) {
                        bg = theme.find_match_current;
                        fg = theme.background;
                    } else {
                        bg = theme.find_match;
                    }
                }
            }
            if selection.is_some_and(|range| range.contains(point)) {
                bg = theme.selection;
                paint_bg = true;
            }
            if block_cursor && point == cursor_point {
                fg = theme.background;
                bg = theme.cursor;
                paint_bg = true;
            }
            batcher.push(&CellInput {
                row: row as usize,
                col: point.column.0,
                ch: cell.c,
                zerowidth: cell.zerowidth().unwrap_or(&[]),
                fg,
                bg: paint_bg.then_some(bg),
                flags,
            });
        }
        batcher.finish();
        (term.grid().columns(), term.grid().screen_lines())
    });
    metrics.set(Metrics {
        origin,
        cell: Size {
            width: cell_width,
            height: cell_height,
        },
        cols,
        rows,
    });

    // ----- painting, the lock released -----
    window.paint_quad(fill(bounds, theme.background));
    let at = |col: usize, row: usize| Point {
        x: origin.x + cell_width * col as f32,
        y: origin.y + cell_height * row as f32,
    };
    for rect in &batcher.rects {
        window.paint_quad(fill(
            Bounds {
                origin: at(rect.col, rect.row),
                size: Size {
                    width: cell_width * rect.cols as f32,
                    height: cell_height,
                },
            },
            rect.color,
        ));
    }
    for run in &batcher.runs {
        let text = batcher.text_of(run);
        let shaped = window.text_system().shape_line(
            SharedString::from(text.to_owned()),
            font.size,
            &[TextRun {
                len: text.len(),
                font: font_for(font, run.bold, run.italic),
                color: run.fg,
                background_color: None,
                underline: run.underline.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(run.fg),
                    wavy: false,
                }),
                strikethrough: run.strikethrough.then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(run.fg),
                }),
            }],
            Some(if run.wide {
                cell_width * 2.0
            } else {
                cell_width
            }),
        );
        let _ = shaped.paint(
            at(run.col, run.row),
            cell_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    if let Some((col, row, shape)) = cursor {
        let cell = Bounds {
            origin: at(col, row),
            size: Size {
                width: cell_width,
                height: cell_height,
            },
        };
        match (shape, focused) {
            (CursorShape::Block, true) | (CursorShape::Hidden, _) => {}
            (CursorShape::Beam, true) => window.paint_quad(fill(
                Bounds {
                    origin: cell.origin,
                    size: Size {
                        width: px(2.),
                        height: cell_height,
                    },
                },
                theme.cursor,
            )),
            (CursorShape::Underline, true) => window.paint_quad(fill(
                Bounds {
                    origin: Point {
                        x: cell.origin.x,
                        y: cell.origin.y + cell_height - px(2.),
                    },
                    size: Size {
                        width: cell_width,
                        height: px(2.),
                    },
                },
                theme.cursor,
            )),
            // Unfocused, or asked for: the outline of the cell.
            _ => window.paint_quad(outline(cell, theme.cursor, BorderStyle::Solid)),
        }
    }

    window.handle_input(focus, ElementInputHandler::new(bounds, view.clone()), cx);
}

/// Whether the cell's link is the one under the pointer. Cells that are no
/// link at all are never hovered, whatever the pointer is over: two absent
/// links are not the same link.
fn link_is_hovered(link: Option<&str>, hovered: Option<&str>) -> bool {
    link.is_some() && link == hovered
}

#[cfg(test)]
mod link_tests {
    use super::link_is_hovered;

    #[test]
    fn only_a_cell_of_the_hovered_link_is_hovered() {
        assert!(!link_is_hovered(None, None), "plain text is not a link");
        assert!(!link_is_hovered(None, Some("https://a.example")));
        assert!(!link_is_hovered(Some("https://a.example"), None));
        assert!(!link_is_hovered(
            Some("https://a.example"),
            Some("https://b.example")
        ));
        assert!(link_is_hovered(
            Some("https://a.example"),
            Some("https://a.example")
        ));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testing::{sh_c, theme, PATIENCE};
    use crate::Timings;
    use gpui_kit::{
        px, size, Modifiers, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
    };
    use std::time::{Duration, Instant};

    fn font() -> FontSettings {
        FontSettings {
            family: "Menlo".into(),
            size: px(13.),
            line_height: 1.3,
        }
    }

    fn show(cx: &mut TestAppContext, script: &str) -> (Entity<TerminalView>, Arc<Terminal>) {
        cx.executor().allow_parking();
        let pty = Pty {
            timings: Timings {
                kill_grace: Duration::from_millis(50),
                ..Timings::default()
            },
        };
        let view = cx.update(|cx| {
            TerminalView::spawn_with(
                &pty,
                &sh_c(script),
                GridSize::new(80, 24),
                theme(),
                font(),
                cx,
            )
            .expect("sh starts")
        });
        let root = view.clone();
        cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(640.), px(400.)),
                    })),
                    ..Default::default()
                },
                move |_, _| root,
            )
            .unwrap();
        });
        cx.run_until_parked();
        let terminal = cx.update(|cx| view.read(cx).terminal().clone());
        (view, terminal)
    }

    fn wait_until(cx: &mut TestAppContext, what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            cx.run_until_parked();
            if condition() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[gpui_kit::test]
    fn painting_measures_the_font_and_fits_the_grid_to_the_window(cx: &mut TestAppContext) {
        let (_view, terminal) = show(cx, "printf VIEW-OK; read hold");
        wait_until(cx, "the output", || {
            terminal.screen_text().contains("VIEW-OK")
        });
        let grid = terminal.size();
        assert!(grid.cell_width > 0 && grid.cell_height > 0);
        assert_ne!((grid.cols, grid.rows), (80, 24), "resized to 640x400");
        // The cell size is kept in whole pixels for the child, rounded up at
        // most half a pixel: the grid still fits within a pixel per cell.
        assert!(f32::from(grid.cols) * (f32::from(grid.cell_width) - 1.0) <= 640.0);
        assert!(f32::from(grid.rows) * (f32::from(grid.cell_height) - 1.0) <= 400.0);
    }

    #[gpui_kit::test]
    fn a_keystroke_that_is_a_terminal_key_is_sent_and_plain_text_is_not(cx: &mut TestAppContext) {
        let (view, terminal) = show(cx, "stty -echo -icanon -isig; printf SET; exec cat -vt");
        // Input sent before `stty` has run would be echoed and line-buffered
        // by the terminal driver, not seen raw by `cat`.
        wait_until(cx, "the raw mode", || {
            terminal.screen_text().contains("SET")
        });
        let sent = cx.update(|cx| {
            view.update(cx, |view, cx| {
                let up = view.handle_keystroke(&Keystroke::parse("up").unwrap(), cx);
                let mut letter = Keystroke::parse("a").unwrap();
                letter.key_char = Some("a".into());
                let text = view.handle_keystroke(&letter, cx);
                (up, text)
            })
        });
        assert_eq!(
            sent,
            (true, false),
            "arrows are keys; letters are text input"
        );
        wait_until(cx, "the echo", || terminal.screen_text().contains("^[[A"));
    }

    #[gpui_kit::test]
    fn text_from_the_input_method_reaches_the_program(cx: &mut TestAppContext) {
        let (view, terminal) = show(cx, "stty -echo -icanon -isig; printf SET; exec cat");
        wait_until(cx, "the raw mode", || {
            terminal.screen_text().contains("SET")
        });
        let window = cx.windows()[0];
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.replace_text_in_range(None, "ñandú", window, cx);
            });
        })
        .unwrap();
        wait_until(cx, "the text", || terminal.screen_text().contains("ñandú"));
    }

    #[gpui_kit::test]
    fn a_program_that_captures_the_mouse_still_offers_the_link_and_ctrl_click_opens_it(
        cx: &mut TestAppContext,
    ) {
        let (view, terminal) = show(
            cx,
            "printf '\\033[?1000h\\033[?1006hSee https://one.example '; read hold",
        );
        wait_until(cx, "the link", || {
            terminal.screen_text().contains("https://one.example")
        });
        let metrics = cx.update(|cx| view.read(cx).metrics.get());
        let at = Point {
            x: metrics.origin.x + metrics.cell.width * 10.5,
            y: metrics.origin.y + metrics.cell.height * 0.5,
        };
        let window = cx.windows()[0];
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        drop(visual);
        assert_eq!(cx.opened_url(), None, "the offer does not open it yet");
        assert_eq!(
            cx.update(|cx| view.read(cx).link_prompt.clone())
                .map(|prompt| prompt.uri),
            Some("https://one.example".to_owned()),
            "a plain click on a link offers it even under mouse capture"
        );

        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::secondary_key());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::secondary_key());
        assert_eq!(cx.opened_url().as_deref(), Some("https://one.example"));
    }

    #[gpui_kit::test]
    fn link_clicks_offer_or_open_and_hover_identifies_the_target(cx: &mut TestAppContext) {
        let (view, terminal) = show(
            cx,
            "printf 'See https://one.example and https://two.example'; read hold",
        );
        wait_until(cx, "the link", || {
            terminal.screen_text().contains("https://two.example")
        });
        let metrics = cx.update(|cx| view.read(cx).metrics.get());
        let first = Point {
            x: metrics.origin.x + metrics.cell.width * 10.5,
            y: metrics.origin.y + metrics.cell.height * 0.5,
        };
        let second = Point {
            x: metrics.origin.x + metrics.cell.width * 34.5,
            y: first.y,
        };
        let window = cx.windows()[0];
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_move(first, None, Modifiers::none());
        drop(visual);
        assert_eq!(
            cx.update(|cx| view.read(cx).hovered_link.clone())
                .as_deref(),
            Some("https://one.example")
        );

        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(first, MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_up(first, MouseButton::Left, Modifiers::none());
        drop(visual);
        assert_eq!(cx.opened_url(), None, "the offer does not open it yet");
        assert_eq!(
            cx.update(|cx| view.read(cx).link_prompt.clone()),
            Some(LinkPrompt {
                uri: "https://one.example".into(),
                col: 10,
                row: 0,
            })
        );
        cx.update(|cx| view.update(cx, |view, cx| view.open_link_prompt(cx)));
        assert_eq!(cx.opened_url().as_deref(), Some("https://one.example"));

        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(second, MouseButton::Left, Modifiers::secondary_key());
        visual.simulate_mouse_up(second, MouseButton::Left, Modifiers::secondary_key());
        assert_eq!(cx.opened_url().as_deref(), Some("https://two.example"));
        assert_eq!(terminal.selection_text(), None);
    }

    #[gpui_kit::test]
    fn the_view_reports_the_title_and_the_end_of_the_program(cx: &mut TestAppContext) {
        let (view, terminal) = show(cx, "printf '\\033]0;hello\\007'; read go; exit 5");
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &ViewEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
        });
        terminal.write(&b"\n"[..]);
        wait_until(cx, "the exit", || terminal.exit_info().is_some());
        wait_until(cx, "the events", || {
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, ViewEvent::Exited(info) if info.code == 5))
        });
        // The title may have been reported before the subscription: it is
        // always on the terminal.
        assert_eq!(terminal.title().as_deref(), Some("hello"));
    }
}
