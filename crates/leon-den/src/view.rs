//! The Den as a GPUI view.
//!
//! [`DenView`] owns a [`Den`]. The room is one picture, composited in
//! software ([`crate::paint`]) and handed to GPUI as a single image; the
//! chrome around it ([`Scene`]) is filled rectangles and lines of text in
//! the host's monospace font. The host feeds the view
//! ([`DenView::set_cubs`], [`DenView::happen`]), moves its selection from
//! its own shortcuts ([`DenView::select_next`],
//! [`DenView::open_selection`]) and listens to its [`DenEvent`]s. The view
//! binds no key itself: shortcuts belong to the host's registry.
//!
//! # Crisp at any scale
//!
//! GPUI samples images with a linear filter, which would blur pixel art
//! that it scales. So the view does the scaling itself, by a whole number
//! of device pixels and with no smoothing, and gives GPUI a picture that is
//! exactly as many device pixels as it covers, at an origin snapped to a
//! device pixel: one texel a pixel, which no filter can smear. A display
//! scale of 1.25 or 2 is taken into the sum.
//!
//! # Cost
//!
//! The view draws when it is told something, when the pointer moves onto or
//! off a lion, and when the frame it last drew says the picture changes
//! ([`Wake`]): then it sets one timer and nothing else. It never asks for an
//! animation frame: the art moves ten times a second at most, on a timer.
//! A den where nothing moves sets no timer at all. While its window is not
//! the active one the view redraws at most twice a second.
//!
//! A frame is composited only when it differs from the last one painted:
//! the room is a few hundred small blits into a buffer of some hundred
//! thousand pixels, then one scaled copy. The image of the frame before is
//! dropped from GPUI's atlas as the new one goes in.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    canvas, div, fill, point, prelude::*, px, size, App, Bounds, ContentMask, Context, Corners,
    EventEmitter, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, Task, TextAlign, TextRun, Window,
};

use crate::editor::{advice_for, Editor, Marks};
use crate::feed::Past;
use crate::layout::{DenLayout, Why};
use crate::model::{Cub, Happening};
use crate::paint::{compose, Wardrobe};
use crate::palette::DenPalette;
use crate::pose::TILE;
use crate::scene::{build, Layout, Scene, TextMetrics};
use crate::sim::{Den, Frame, Wake};
use crate::world::Tile;

/// How the Den looks: the host's theme.
#[derive(Clone, Debug, PartialEq)]
pub struct DenStyle {
    /// The colours.
    pub palette: DenPalette,
    /// The monospace font of the text box, the roster and the truth card.
    pub font_family: SharedString,
    /// Its size.
    pub font_size: Pixels,
}

/// What the view tells its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenEvent {
    /// A lion, or its row in the roster, was clicked.
    Clicked(u64),
    /// The selection changed, by the pointer or by the host's keys.
    Selected(Option<u64>),
    /// The selected lion is to be opened: it was double-clicked, or the host
    /// called [`DenView::open_selection`].
    Opened(u64),
    /// The room was changed in the editor: the host keeps
    /// [`DenView::layout`].
    LayoutChanged,
}

/// The time since the view was made. A test hands its own.
pub type Clock = Rc<dyn Fn() -> Duration>;

/// How long the view waits between two redraws in a window that is not the
/// active one.
const BACKGROUND_PACE: Duration = Duration::from_millis(500);

struct State {
    den: Den,
    /// The layout and the scene of the last paint, for the pointer.
    layout: Option<Layout>,
    scene: Scene,
    /// The top left of the view in the window, in device pixels.
    origin: (i32, i32),
    scale: f32,
    /// Wakes the view when the picture changes next. Replaced, and so
    /// cancelled, on every paint.
    timer: Option<Task<()>>,
    /// The room being edited, while it is.
    editor: Option<Editor>,
    /// Why the last change was refused, until the next one.
    refused: Option<Why>,
    /// Goes up every time the room changes.
    room: u64,
    /// The lions' sheets, dyed.
    wardrobe: Wardrobe,
    /// The picture of the room last painted, and what it was painted from.
    picture: Option<(Picture, Arc<RenderImage>)>,
    /// How many times the view has painted, how many pictures it has
    /// composited, and how many timers it has set.
    paints: usize,
    pictures: usize,
    timers: usize,
}

/// What a picture of the room is a function of.
#[derive(Clone, PartialEq)]
struct Picture {
    frame: Frame,
    palette: DenPalette,
    /// Which layout the room had: a number that goes up with every change.
    room: u64,
    unit: i32,
    marks: Option<Marks>,
}

/// The Den.
pub struct DenView {
    state: Rc<RefCell<State>>,
    style: DenStyle,
    clock: Clock,
    reduced_motion: Option<bool>,
    focus: FocusHandle,
}

impl EventEmitter<DenEvent> for DenView {}

impl Focusable for DenView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DenView {
    /// An empty den in this style.
    pub fn new(style: DenStyle, cx: &mut Context<Self>) -> Self {
        let started = Instant::now();
        Self::with_clock(style, Rc::new(move || started.elapsed()), cx)
    }

    /// An empty den that reads the time from `clock`.
    pub fn with_clock(style: DenStyle, clock: Clock, cx: &mut Context<Self>) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                den: Den::new(),
                layout: None,
                scene: Scene::default(),
                origin: (0, 0),
                scale: 1.,
                timer: None,
                editor: None,
                refused: None,
                room: 0,
                wardrobe: Wardrobe::new(),
                picture: None,
                paints: 0,
                pictures: 0,
                timers: 0,
            })),
            style,
            clock,
            reduced_motion: None,
            focus: cx.focus_handle(),
        }
    }

    fn now(&self) -> Duration {
        (self.clock)()
    }

    /// Changes the colours and the font: the theme changed.
    pub fn set_style(&mut self, style: DenStyle, cx: &mut Context<Self>) {
        if self.style != style {
            self.style = style;
            cx.notify();
        }
    }

    /// Tells the den who is in it now: the whole list, on every change.
    pub fn set_cubs(&mut self, cubs: &[Cub], cx: &mut Context<Self>) {
        let now = self.now();
        let before = self.selected();
        self.state.borrow_mut().den.update(cubs, now);
        if self.selected() != before {
            cx.emit(DenEvent::Selected(self.selected()));
        }
        cx.notify();
    }

    /// Tells the den what happened, for the narrator.
    pub fn happen(&mut self, happening: &Happening, cx: &mut Context<Self>) {
        let now = self.now();
        self.state.borrow_mut().den.happen(happening, now);
        cx.notify();
    }

    /// `Some(true)` holds the den still, `Some(false)` lets it move, and
    /// `None`, the default, follows GPUI's own [`App::reduce_motion`], which
    /// the desktop's setting feeds.
    pub fn set_reduced_motion(&mut self, reduced: Option<bool>, cx: &mut Context<Self>) {
        self.reduced_motion = reduced;
        cx.notify();
    }

    /// `false` turns the narrator off: the text box then says the state of
    /// the den in plain words.
    pub fn set_narrator(&mut self, narrator: bool, cx: &mut Context<Self>) {
        self.state.borrow_mut().den.set_plain_status(!narrator);
        cx.notify();
    }

    /// The selected lion.
    pub fn selected(&self) -> Option<u64> {
        self.state.borrow().den.selected()
    }

    /// Selects a lion, or nobody.
    pub fn select(&mut self, id: Option<u64>, cx: &mut Context<Self>) {
        let before = self.selected();
        self.state.borrow_mut().den.select(id);
        self.selection_moved(before, cx);
    }

    /// Selects the next lion of the roster, around the end.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let before = self.selected();
        self.state.borrow_mut().den.select_by(1);
        self.selection_moved(before, cx);
    }

    /// Selects the previous lion of the roster, around the start.
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let before = self.selected();
        self.state.borrow_mut().den.select_by(-1);
        self.selection_moved(before, cx);
    }

    /// Asks the host to open the selected lion's session. It answers whether
    /// there was one to open.
    pub fn open_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selected() {
            Some(id) => {
                cx.emit(DenEvent::Opened(id));
                true
            }
            None => false,
        }
    }

    fn selection_moved(&mut self, before: Option<u64>, cx: &mut Context<Self>) {
        let now = self.selected();
        if now != before {
            // Another lion's feed, or everybody's: from its end.
            self.state.borrow_mut().den.feed_mut().follow();
            cx.emit(DenEvent::Selected(now));
            cx.notify();
        }
    }

    /// Tells the den what time it is, in seconds since the Unix epoch: the
    /// time of what is told from now on, and what the feed counts "5m"
    /// from. The view reads no clock for this.
    pub fn set_wall_time(&mut self, now: i64, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        if state.den.wall_time() != Some(now) {
            state.den.set_wall_time(now);
            drop(state);
            cx.notify();
        }
    }

    /// Puts in the feed what an agent just said to the user, as it said it.
    /// `at` is when, in seconds since the Unix epoch, if the transcript
    /// says; otherwise it is now.
    pub fn speak(
        &mut self,
        cub: u64,
        name: &str,
        text: &str,
        at: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let now = self.now();
        self.state.borrow_mut().den.speak(cub, name, text, at, now);
        cx.notify();
    }

    /// Puts the past of a session in the feed: its last messages, and one
    /// line for the tools of each turn ([`crate::feed::backfill`]).
    pub fn remember(&mut self, cub: u64, name: &str, past: &[Past], cx: &mut Context<Self>) {
        self.state.borrow_mut().den.remember(cub, name, past);
        cx.notify();
    }

    /// Moves the feed by so many of its pages: back with a negative number.
    pub fn scroll_feed(&mut self, pages: i32, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let (total, height) = (state.scene.feed_lines.len(), state.scene.feed_height);
        let rows = i64::from(pages) * (height.max(2) as i64 - 1);
        state.den.feed_mut().scroll(rows, total, height);
        drop(state);
        cx.notify();
    }

    /// Goes to the start of the feed (`false`) or back to its end, which it
    /// then follows (`true`).
    pub fn feed_to(&mut self, end: bool, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let (total, height) = (state.scene.feed_lines.len(), state.scene.feed_height);
        if end {
            state.den.feed_mut().follow();
        } else {
            state.den.feed_mut().scroll(-(total as i64), total, height);
        }
        drop(state);
        cx.notify();
    }

    /// Moves the keyboard's entry of the feed to the one before (`back`) or
    /// after it, and brings it into view.
    pub fn feed_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        state
            .den
            .feed_mut()
            .step_cursor(&state.scene.feed_lines, state.scene.feed_height, back);
        cx.notify();
    }

    /// Opens the message the keyboard is on, or cuts it again. `false`
    /// when the keyboard is on no entry.
    pub fn feed_toggle(&mut self, cx: &mut Context<Self>) -> bool {
        let mut state = self.state.borrow_mut();
        let Some(id) = state.den.feed().cursor() else {
            return false;
        };
        state.den.feed_mut().toggle(id);
        drop(state);
        cx.notify();
        true
    }

    /// Takes the keyboard off the feed. `false` when it was not on it.
    pub fn feed_release(&mut self, cx: &mut Context<Self>) -> bool {
        let dropped = self.state.borrow_mut().den.feed_mut().drop_cursor();
        if dropped {
            cx.notify();
        }
        dropped
    }

    /// Reads the den: its roster, the truth about a lion, its frame.
    pub fn read<R>(&self, read: impl FnOnce(&Den) -> R) -> R {
        read(&self.state.borrow().den)
    }

    /// How many times the view has painted, how many pictures of the room
    /// it has composited and how many timers it has set: what it costs.
    pub fn cost(&self) -> (usize, usize, usize) {
        let state = self.state.borrow();
        (state.paints, state.pictures, state.timers)
    }

    /// The point of the window over a pixel of the room's art, once the view
    /// has painted.
    pub fn window_point(&self, x: i32, y: i32) -> Option<Point<Pixels>> {
        let state = self.state.borrow();
        let layout = state.layout?;
        let device = |origin: i32, map: i32, art: i32| {
            (origin + map + art * layout.unit) as f32 + layout.unit as f32 / 2.
        };
        Some(point(
            px(device(state.origin.0, layout.map.x, x) / state.scale),
            px(device(state.origin.1, layout.map.y, y) / state.scale),
        ))
    }

    /// A point of the window in the view's device pixels.
    fn view_point(&self, position: Point<Pixels>) -> (i32, i32) {
        let state = self.state.borrow();
        (
            (position.x.as_f32() * state.scale).floor() as i32 - state.origin.0,
            (position.y.as_f32() * state.scale).floor() as i32 - state.origin.1,
        )
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = self.view_point(event.position);
        let mut state = self.state.borrow_mut();
        if !state.scene.feed.is_some_and(|feed| feed.contains(x, y)) {
            return;
        }
        let line = state
            .layout
            .map_or(18., |layout| layout.metrics.line_h / layout.scale.max(0.1));
        let moved = event.delta.pixel_delta(px(line)).y.as_f32() / line.max(1.);
        // A row at least for the smallest turn of a wheel.
        let rows = if moved.abs() < 1. {
            -moved.signum() as i64
        } else {
            -moved.round() as i64
        };
        if rows == 0 {
            return;
        }
        let (total, height) = (state.scene.feed_lines.len(), state.scene.feed_height);
        state.den.feed_mut().scroll(rows, total, height);
        drop(state);
        cx.notify();
    }

    /// Reads the scene last painted: where its parts are.
    pub fn scene<R>(&self, read: impl FnOnce(&Scene) -> R) -> R {
        read(&self.state.borrow().scene)
    }

    /// The point of the window over a device pixel of the view.
    pub fn window_point_of(&self, x: i32, y: i32) -> Point<Pixels> {
        let state = self.state.borrow();
        point(
            px((state.origin.0 + x) as f32 / state.scale),
            px((state.origin.1 + y) as f32 / state.scale),
        )
    }

    /// The lion under a point of the window, on the map or in the roster.
    fn lion_at(&self, position: Point<Pixels>) -> Option<u64> {
        let state = self.state.borrow();
        let layout = state.layout?;
        let x = (position.x.as_f32() * state.scale).floor() as i32 - state.origin.0;
        let y = (position.y.as_f32() * state.scale).floor() as i32 - state.origin.1;
        state.scene.row_at(x, y).or_else(|| {
            let (ax, ay) = layout.art_point(x, y)?;
            state.den.cub_at(ax, ay, self.now())
        })
    }

    /// How many device pixels a pixel of the host is, as of the last paint:
    /// what a host scales its own pictures of the Den by.
    pub fn scale(&self) -> f32 {
        self.state.borrow().scale
    }

    /// The layout of the room: the one being edited, while it is.
    pub fn layout(&self) -> DenLayout {
        let state = self.state.borrow();
        match &state.editor {
            Some(editor) => editor.layout().clone(),
            None => state.den.layout().clone(),
        }
    }

    /// Gives the den another room. The lions are put in it at once. An
    /// editor that is open starts again on the new room.
    pub fn set_layout(&mut self, layout: &DenLayout, cx: &mut Context<Self>) {
        let now = self.now();
        let mut state = self.state.borrow_mut();
        if state.editor.is_some() {
            state.editor = Some(Editor::new(layout.clone()));
        }
        if state.den.layout() != layout {
            state.den.set_layout(layout, false, now);
            state.room += 1;
        }
        drop(state);
        cx.notify();
    }

    /// Whether the room is being edited.
    pub fn is_editing(&self) -> bool {
        self.state.borrow().editor.is_some()
    }

    /// Opens the editor on the room as it is. The lions go on living in it.
    pub fn start_editing(&mut self, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        if state.editor.is_none() {
            let layout = state.den.layout().clone();
            state.editor = Some(Editor::new(layout));
            state.refused = None;
            state.den.hover(None);
        }
        drop(state);
        cx.notify();
    }

    /// Closes the editor. The room stays as it was left: the host keeps
    /// [`Self::layout`].
    pub fn stop_editing(&mut self, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        state.editor = None;
        state.refused = None;
        drop(state);
        cx.notify();
    }

    /// Why the editor refused the last change, until the next one.
    pub fn refusal(&self) -> Option<Why> {
        self.state.borrow().refused
    }

    /// Reads the editor, while it is open.
    pub fn editor<R>(&self, read: impl FnOnce(&Editor) -> R) -> Option<R> {
        self.state.borrow().editor.as_ref().map(read)
    }

    /// Does something with the editor, while it is open: picks a piece,
    /// turns it, undoes. The room follows, the lions walk to where their
    /// seats went, and the host is told when the layout changed. An
    /// operation that is refused is said in the text box. It answers
    /// whether the operation was accepted.
    pub fn edit(
        &mut self,
        change: impl FnOnce(&mut Editor) -> Result<(), Why>,
        cx: &mut Context<Self>,
    ) -> bool {
        let now = self.now();
        let mut state = self.state.borrow_mut();
        let state_ref = &mut *state;
        let Some(editor) = state_ref.editor.as_mut() else {
            return false;
        };
        let result = change(editor);
        state_ref.refused = result.err();
        let changed = editor.layout() != state_ref.den.layout();
        if changed {
            let layout = editor.layout().clone();
            state_ref.den.set_layout(&layout, true, now);
            state_ref.room += 1;
        }
        drop(state);
        if changed {
            cx.emit(DenEvent::LayoutChanged);
        }
        cx.notify();
        result.is_ok()
    }

    /// The tile of the room under a point of the window.
    fn tile_at(&self, position: Point<Pixels>) -> Option<Tile> {
        let state = self.state.borrow();
        let layout = state.layout?;
        let x = (position.x.as_f32() * state.scale).floor() as i32 - state.origin.0;
        let y = (position.y.as_f32() * state.scale).floor() as i32 - state.origin.1;
        let (ax, ay) = layout.art_point(x, y)?;
        Some(Tile::new(ax.div_euclid(TILE), ay.div_euclid(TILE)))
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_editing() {
            let tile = self.tile_at(event.position);
            // Moving the pointer refuses nothing: what was said stays.
            let said = self.state.borrow().refused;
            self.edit(
                |editor| {
                    editor.point(tile);
                    Ok(())
                },
                cx,
            );
            self.state.borrow_mut().refused = said;
            return;
        }
        let over = self.lion_at(event.position);
        if self.state.borrow_mut().den.hover(over) {
            cx.notify();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if self.is_editing() {
            if let Some(tile) = self.tile_at(event.position) {
                self.edit(|editor| editor.press(tile), cx);
            }
            return;
        }
        // The feed first: the way back to its end, the name that narrows it,
        // a message to open.
        let (x, y) = self.view_point(event.position);
        let on_feed = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let inside = |rect: Option<crate::scene::Rect>| rect.is_some_and(|r| r.contains(x, y));
            if inside(state.scene.jump) {
                state.den.feed_mut().follow();
                Some(false)
            } else if inside(state.scene.all) {
                Some(true)
            } else if let Some(entry) = state.scene.entry_at(x, y) {
                state.den.feed_mut().toggle(entry);
                Some(false)
            } else if inside(state.scene.feed) {
                Some(false)
            } else {
                None
            }
        };
        match on_feed {
            Some(true) => {
                self.select(None, cx);
                return;
            }
            Some(false) => {
                cx.notify();
                return;
            }
            None => {}
        }
        let over = self.lion_at(event.position);
        let before = self.selected();
        self.state.borrow_mut().den.select(over);
        self.selection_moved(before, cx);
        if let Some(id) = over {
            cx.emit(DenEvent::Clicked(id));
            if event.click_count >= 2 {
                cx.emit(DenEvent::Opened(id));
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_editing() {
            let said = self.state.borrow().refused;
            self.edit(
                |editor| {
                    editor.release();
                    Ok(())
                },
                cx,
            );
            self.state.borrow_mut().refused = said;
        }
    }
}

impl Render for DenView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let style = self.style.clone();
        let now = self.now();
        let reduced = self.reduced_motion.unwrap_or_else(|| cx.reduce_motion());
        div()
            .id("den")
            .track_focus(&self.focus)
            .size_full()
            .bg(self.style.palette.ground)
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        paint(&state, &style, bounds, now, reduced, window, cx);
                    },
                )
                .size_full(),
            )
    }
}

fn font(style: &DenStyle, bold: bool) -> Font {
    Font {
        family: style.font_family.clone(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: if bold {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        },
        style: FontStyle::Normal,
    }
}

fn paint(
    state: &Rc<RefCell<State>>,
    style: &DenStyle,
    bounds: Bounds<Pixels>,
    now: Duration,
    reduced: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let device = |value: Pixels| (value.as_f32() * scale).round() as i32;
    let origin = (device(bounds.origin.x), device(bounds.origin.y));
    let (width, height) = (
        (bounds.size.width.as_f32() * scale).floor() as i32,
        (bounds.size.height.as_f32() * scale).floor() as i32,
    );
    if width <= 0 || height <= 0 {
        return;
    }

    // The measures of the font: the advance of one glyph, the line height.
    let font_id = window.text_system().resolve_font(&font(style, false));
    let advance = window
        .text_system()
        .advance(font_id, style.font_size, 'm')
        .map_or(style.font_size * 0.6, |advance| advance.width);
    let line_height = (style.font_size * 1.45).round();
    let metrics = TextMetrics {
        char_w: advance.as_f32() * scale,
        line_h: line_height.as_f32() * scale,
    };

    let (scene, wake, image, stale) = {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        state.den.set_reduced_motion(reduced, now);
        let (cols, rows) = (state.den.world().cols, state.den.world().rows);
        let lions = state.den.order().len();
        let layout = Layout::compute(width, height, scale, metrics, cols, rows, lions);
        let frame = state.den.frame(now);
        let marks = state.editor.as_ref().map(Editor::marks);
        // While the room is edited the box says what it lacks, or why the
        // last change was refused.
        let note: Option<Vec<String>> = state.editor.as_ref().map(|editor| {
            if let Some(why) = state.refused {
                return vec!["It will not go there.".to_owned(), why.text().to_owned()];
            }
            match advice_for(editor.layout()).first() {
                Some((voiced, plain)) => vec![(*voiced).to_owned(), (*plain).to_owned()],
                None => vec![
                    "The den is being rearranged.".to_owned(),
                    "Every place is there: desk, shelf, board, rack, lookout, rest, rug and nest."
                        .to_owned(),
                ],
            }
        });
        let scene = build(&state.den, &frame, &layout, &style.palette, note.as_deref());
        let wake = frame.wake;
        // The room does not show what is said: typing redraws the feed, not
        // the picture.
        let wanted = Picture {
            frame: Frame {
                said: Default::default(),
                typing: None,
                wake: Wake::Never,
                tick: if crate::paint::animated(&frame) {
                    frame.tick
                } else {
                    0
                },
                ..frame
            },
            palette: style.palette,
            room: state.room,
            unit: layout.unit,
            marks,
        };
        // The same frame as last time is the same picture.
        let mut stale = None;
        let image = match &state.picture {
            Some((painted, image)) if *painted == wanted => image.clone(),
            _ => {
                let room = compose(
                    &state.den,
                    &wanted.frame,
                    &style.palette,
                    &mut state.wardrobe,
                    wanted.marks.as_ref(),
                )
                .scaled(layout.unit);
                let image = Arc::new(render_image(room));
                stale = state
                    .picture
                    .replace((wanted, image.clone()))
                    .map(|(_, old)| old);
                state.pictures += 1;
                image
            }
        };
        state.layout = Some(layout);
        state.origin = origin;
        state.scale = scale;
        state.paints += 1;
        (scene, wake, image, stale)
    };
    if let Some(stale) = stale {
        let _ = window.drop_image(stale);
    }

    let at = |x: i32, y: i32| {
        point(
            px((origin.0 + x) as f32 / scale),
            px((origin.1 + y) as f32 / scale),
        )
    };
    let area = |rect: crate::scene::Rect| Bounds {
        origin: at(rect.x, rect.y),
        size: size(px(rect.w as f32 / scale), px(rect.h as f32 / scale)),
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for quad in &scene.under {
            window.paint_quad(fill(area(quad.rect), quad.color));
        }
        let room = area(scene.room);
        let _ = window.paint_image(room, room, Corners::default(), image, 0, false);
        for quad in &scene.quads {
            window.paint_quad(fill(area(quad.rect), quad.color));
        }
        for text in &scene.texts {
            let shaped = window.text_system().shape_line(
                SharedString::from(text.text.clone()),
                style.font_size,
                &[TextRun {
                    len: text.text.len(),
                    font: font(style, text.bold),
                    color: text.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let _ = shaped.paint(
                at(text.x, text.y),
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    });

    // One timer for the next change of the picture, and nothing else.
    let view = window.current_view();
    let timer = match wake {
        Wake::Never => None,
        Wake::At(when) => {
            let mut sleep = when.saturating_sub(now).max(Duration::from_millis(1));
            if !window.is_window_active() {
                sleep = sleep.max(BACKGROUND_PACE);
            }
            Some(window.spawn(cx, async move |cx| {
                cx.background_executor().timer(sleep).await;
                cx.update(|_, cx| cx.notify(view)).ok();
            }))
        }
    };
    let mut state = state.borrow_mut();
    state.timers += usize::from(timer.is_some());
    state.timer = timer;
    state.scene = scene;
}

/// A picture as GPUI wants it: the same pixels, blue first. For a host that
/// shows pictures of the Den's own, a thumbnail of a piece or of a room: give
/// it one already scaled to the device pixels it will cover.
pub fn render_image(picture: crate::bitmap::Bitmap) -> RenderImage {
    let (w, h) = (picture.w as u32, picture.h as u32);
    let mut bytes = picture.into_bytes();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(w, h, bytes).expect("a bitmap is w*h*4 bytes");
    RenderImage::new([image::Frame::new(buffer)])
}
