//! The Den as a GPUI view.
//!
//! [`DenView`] owns a [`Den`]. The room is one picture, handed to GPUI as a
//! single image: the pixel art, composited in software ([`crate::paint`]),
//! or the room in 2.5D, drawn by the GPU ([`crate::iso`]) when the host
//! asked for it ([`DenView::set_three_d`]) and the computer can; the
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
//!
//! # The room in 2.5D
//!
//! The same rules hold: a picture is drawn only when it differs from the
//! last one. What differs is the walk. The pixel art steps a lion a quarter
//! of a tile ten times a second; in 2.5D it glides ([`Den::glide`]), so
//! while a lion is on its way, and only then, the view asks for a picture
//! every [`GLIDE_PACE`]. The name plates and the bubbles are not in that
//! picture: they are the host's font, drawn over it
//! ([`crate::iso::overlay`]).
//!
//! The pixel art is what is shown when 2.5D was not asked for, when the
//! build has no GPU in it, when the computer has none to give, and when the
//! GPU fails later.
//!
//! The room is edited in the picture it is shown in. In 2.5D the pointer is
//! turned into a tile of the plan by [`crate::iso::room::hit`], by what is
//! in hand: a piece for the floor follows the floor, one for a table the
//! tables' tops, one that hangs the back wall, and an empty hand takes the
//! piece that shows under it, which then follows at the height it was taken
//! by. The editor is the same, and its marks are drawn in the room
//! ([`crate::iso::room::marks`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    canvas, div, fill, point, prelude::*, px, size, App, Bounds, ContentMask, Context, Corners,
    EventEmitter, FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, Task, TextAlign, TextRun, Window,
};

use crate::editor::{advice_for, Brush, Editor, Marks};
use crate::feed::Past;
use crate::iso;
use crate::layout::{DenLayout, Why};
use crate::model::{Cub, Happening};
use crate::paint::{compose, Wardrobe};
use crate::palette::DenPalette;
use crate::pose::TILE;
use crate::scene::{build, build_around, Layout, Rect, Scene, TextMetrics};
use crate::sim::{Den, Frame, Wake};
use crate::world::Tile;

/// How the Den looks: the host's theme.
#[derive(Clone, Debug, PartialEq)]
pub struct DenStyle {
    /// The colours.
    pub palette: DenPalette,
    /// The colours of the room in 2.5D: the same theme
    /// ([`iso::Theme::from_tokens`]).
    pub room: iso::Theme,
    /// The monospace font of the text box, the roster and the truth card.
    pub font_family: SharedString,
    /// Its size.
    pub font_size: Pixels,
}

/// What the view tells its host.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// The menu of a lion is asked for: it was clicked with the right
    /// button, or the host called [`DenView::menu_selection`]. The lion is
    /// the selected one by then.
    Menu {
        /// The lion.
        id: u64,
        /// Where the menu goes, in the window: the pointer, or the lion.
        at: Point<Pixels>,
    },
    /// A session at home is to be woken: its row of the roster was clicked.
    /// The id is the one the host gave it ([`DenView::set_home`]).
    Wake(u64),
}

/// The time since the view was made. A test hands its own.
pub type Clock = Rc<dyn Fn() -> Duration>;

/// How long the view waits between two redraws in a window that is not the
/// active one.
const BACKGROUND_PACE: Duration = Duration::from_millis(500);
/// How long the view waits between two pictures of the room in 2.5D while
/// a lion walks.
pub const GLIDE_PACE: Duration = Duration::from_millis(33);
/// How much smaller than the text of the chrome the letters of a name
/// plate are.
const PLATE_TEXT: f32 = 0.84;

/// Which picture of the room the view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// The pixel art: 2.5D was not asked for.
    Pixels,
    /// 2.5D was asked for and the GPU is not opened yet: it is on the first
    /// paint that needs it.
    Waiting,
    /// The room in 2.5D, by this adapter.
    Iso(String),
    /// The pixel art, because 2.5D cannot be drawn here: why.
    Failed(String),
}

enum ThreeD {
    Off,
    Wanted,
    On(Box<iso::Renderer>),
    Failed(String),
}

/// What the pointer needs of the last picture in 2.5D.
struct Shown {
    overlay: iso::overlay::Overlay,
    camera: iso::Camera,
    /// The top left of the picture in the view.
    origin: (i32, i32),
}

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
    /// Whether the room is drawn in 2.5D.
    three_d: ThreeD,
    /// The last picture in 2.5D, for the pointer; none while the pixel art
    /// is shown.
    shown: Option<Shown>,
    /// How high the pointer took the piece it drags in 2.5D: it follows
    /// the pointer at that height.
    grab: f32,
    /// How many times the view has painted, how many pictures it has
    /// composited, and how many timers it has set.
    paints: usize,
    pictures: usize,
    timers: usize,
    /// How long the pictures took to make, all of them.
    spent: Duration,
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
    /// What a picture in 2.5D is a function of besides: none for the pixel
    /// art.
    iso: Option<IsoKey>,
}

#[derive(Clone, PartialEq)]
struct IsoKey {
    /// Where every lion is, to a thirty-second of a tile.
    glide: Vec<(u64, i32, i32)>,
    /// The stride of those that walk, to a sixteenth of a turn.
    stride: i64,
    size: (u32, u32),
    theme: iso::Theme,
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
                three_d: ThreeD::Off,
                shown: None,
                grab: 0.,
                paints: 0,
                pictures: 0,
                timers: 0,
                spent: Duration::ZERO,
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

    /// Asks for the room in 2.5D, or for the pixel art. 2.5D is shown when
    /// the build has it and the computer has a GPU for it; otherwise the
    /// pixel art is. Nothing is opened before a paint needs it.
    pub fn set_three_d(&mut self, wanted: bool, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let changed = match (&state.three_d, wanted) {
            (ThreeD::Off, true) => {
                state.three_d = if iso::Renderer::BUILT {
                    ThreeD::Wanted
                } else {
                    ThreeD::Failed("this build has no 2.5D renderer".to_owned())
                };
                true
            }
            (ThreeD::Off, false) => false,
            (_, false) => {
                state.three_d = ThreeD::Off;
                state.shown = None;
                true
            }
            (_, true) => false,
        };
        drop(state);
        if changed {
            cx.notify();
        }
    }

    /// Which picture of the room is shown, and why.
    pub fn drawn(&self) -> Drawn {
        match &self.state.borrow().three_d {
            ThreeD::Off => Drawn::Pixels,
            ThreeD::Wanted => Drawn::Waiting,
            ThreeD::On(renderer) => Drawn::Iso(renderer.adapter()),
            ThreeD::Failed(why) => Drawn::Failed(why.clone()),
        }
    }

    /// A picture of a room in 2.5D with nobody in it, `size` device pixels:
    /// what a host shows of a den to choose it by. `None` while the room is
    /// not drawn in 2.5D: the host shows the pixel art then
    /// ([`crate::paint::thumbnail`]).
    pub fn room_picture(&self, layout: &DenLayout, size: (u32, u32)) -> Option<RenderImage> {
        let mut state = self.state.borrow_mut();
        let ThreeD::On(renderer) = &mut state.three_d else {
            return None;
        };
        let pixels = renderer.room_picture(layout, &self.style.room, size).ok()?;
        Some(picture_of(pixels, size))
    }

    /// A picture of a piece of the catalogue in 2.5D on `ground`, the
    /// colour of what it is shown on. `None` while the room is not drawn in
    /// 2.5D: the host shows the pixel art then ([`crate::paint::piece`]).
    pub fn piece_picture(
        &self,
        id: &str,
        turn: u8,
        ground: gpui_kit::Hsla,
        size: (u32, u32),
    ) -> Option<RenderImage> {
        let mut state = self.state.borrow_mut();
        let ThreeD::On(renderer) = &mut state.three_d else {
            return None;
        };
        let [r, g, b, _] = crate::palette::bytes(ground);
        let theme = iso::Theme {
            background: [r, g, b].map(|part| f32::from(part) / 255.),
            ..self.style.room
        };
        let pixels = renderer.piece_picture(id, turn, &theme, size).ok()?;
        Some(picture_of(pixels, size))
    }

    /// Whether the room is drawn in 2.5D right now.
    pub fn is_three_d(&self) -> bool {
        matches!(self.state.borrow().three_d, ThreeD::On(_))
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

    /// Selects the next lion that needs the user: one that waits, asks for
    /// a permission or fainted, the most pressing first. It answers whether
    /// there is one.
    pub fn select_needy(&mut self, cx: &mut Context<Self>) -> bool {
        let before = self.selected();
        let found = self.state.borrow_mut().den.select_needy().is_some();
        self.selection_moved(before, cx);
        found
    }

    /// How many lions need the user.
    pub fn needy(&self) -> usize {
        self.state.borrow().den.needy().len()
    }

    /// Says which sessions were sent home and can be woken: the roster
    /// lists them under the pride, and a click on one asks the host to wake
    /// it ([`DenEvent::Wake`]).
    pub fn set_home(&mut self, home: Vec<crate::sim::HomeEntry>, cx: &mut Context<Self>) {
        if self.state.borrow_mut().den.set_home(home) {
            cx.notify();
        }
    }

    /// Shows the keys of the Den over the room, each with what it does, or
    /// takes them away with `None`. A click anywhere takes them away too.
    pub fn show_keys(&mut self, keys: Option<Vec<(String, String)>>, cx: &mut Context<Self>) {
        if self.state.borrow_mut().den.set_keys(keys) {
            cx.notify();
        }
    }

    /// Whether the keys are shown.
    pub fn keys_shown(&self) -> bool {
        self.state.borrow().den.keys().is_some()
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

    /// Asks the host for the menu of the selected lion, anchored under it.
    /// It answers whether there was one.
    pub fn menu_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.selected() else {
            return false;
        };
        let now = self.now();
        // In 2.5D the lion is where its box is in the picture.
        let boxed = {
            let state = self.state.borrow();
            state
                .shown
                .as_ref()
                .and_then(|shown| iso::overlay::box_of(&shown.overlay, id))
        };
        let foot = {
            let state = self.state.borrow();
            state
                .den
                .frame(now)
                .actors
                .iter()
                .find(|actor| actor.id == id)
                .map(|actor| {
                    let (left, top, w, h) = actor.bounds();
                    (left + w / 2, top + h)
                })
        };
        let at = match boxed {
            Some(rect) => self.window_point_of(rect.x + rect.w / 2, rect.y + rect.h),
            None => foot
                .and_then(|(x, y)| self.window_point(x, y))
                .unwrap_or_else(|| self.window_point_of(0, 0)),
        };
        cx.emit(DenEvent::Menu { id, at });
        true
    }

    /// What a lion's truth card says under its state: the host's own lines,
    /// a summary of its session. A lion that is not named has none.
    pub fn set_notes(
        &mut self,
        notes: HashMap<u64, Vec<crate::sim::Note>>,
        cx: &mut Context<Self>,
    ) {
        if self.state.borrow_mut().den.set_notes(notes) {
            cx.notify();
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

    /// How long the view has spent making its pictures of the room, all of
    /// them together: with [`Self::cost`], what one picture takes.
    pub fn picture_time(&self) -> Duration {
        self.state.borrow().spent
    }

    /// The point of the window over a pixel of the room's art, once the view
    /// has painted.
    pub fn window_point(&self, x: i32, y: i32) -> Option<Point<Pixels>> {
        let state = self.state.borrow();
        let layout = state.layout?;
        if let Some(shown) = &state.shown {
            // In 2.5D a pixel of the art is a point of the floor.
            let (px_x, px_y) =
                shown
                    .camera
                    .project([x as f32 / TILE as f32, 0., y as f32 / TILE as f32]);
            return Some(point(
                px((state.origin.0 + shown.origin.0) as f32 / state.scale + px_x / state.scale),
                px((state.origin.1 + shown.origin.1) as f32 / state.scale + px_y / state.scale),
            ));
        }
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
            if let Some(shown) = &state.shown {
                return iso::overlay::pick(&shown.overlay, x, y);
            }
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

    /// What the editor's pointer is after in 2.5D, by what is in hand.
    fn aim(state: &State) -> iso::room::Aim {
        use crate::catalogue::Placement;
        use iso::room::Aim;
        let Some(editor) = &state.editor else {
            return Aim::Any;
        };
        let of = |id: &str| match crate::catalogue::find(id).map(|entry| entry.placement) {
            Some(Placement::Wall) => Aim::Wall,
            Some(Placement::Surface) => Aim::Surface,
            _ => Aim::Level(0.),
        };
        match editor.brush() {
            Brush::Piece { id, .. } => of(id),
            Brush::Carpet(_) | Brush::BareFloor => Aim::Level(0.),
            Brush::Hand => match editor.dragging() {
                // A piece that is dragged follows the pointer at the height
                // it was taken by; one on the wall stays on the wall.
                Some(index) => match editor.layout().items.get(index).map(|piece| of(&piece.id)) {
                    Some(Aim::Wall) => Aim::Wall,
                    _ => Aim::Level(state.grab),
                },
                None => Aim::Any,
            },
        }
    }

    /// What is under a point of the window in the room in 2.5D.
    fn hit_at(&self, position: Point<Pixels>) -> Option<iso::room::Hit> {
        let state = self.state.borrow();
        let shown = state.shown.as_ref()?;
        let x = (position.x.as_f32() * state.scale).floor() as i32 - state.origin.0;
        let y = (position.y.as_f32() * state.scale).floor() as i32 - state.origin.1;
        Some(iso::room::hit(
            state.den.layout(),
            &shown.camera,
            (x - shown.origin.0) as f32,
            (y - shown.origin.1) as f32,
            Self::aim(&state),
        ))
    }

    /// The tile of the room under a point of the window.
    fn tile_at(&self, position: Point<Pixels>) -> Option<Tile> {
        if let Some(hit) = self.hit_at(position) {
            return Some(hit.tile);
        }
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
        let (x, y) = self.view_point(event.position);
        let mut state = self.state.borrow_mut();
        let home = state.scene.home_at(x, y);
        let moved = state.den.hover(over) | state.den.hover_home(home);
        drop(state);
        if moved {
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
            // In 2.5D the piece that is taken is held at the height the
            // pointer found it.
            let grab = self.hit_at(event.position).map_or(0., |hit| {
                if hit.item.is_some() {
                    hit.height
                } else {
                    0.
                }
            });
            self.state.borrow_mut().grab = grab;
            if let Some(tile) = self.tile_at(event.position) {
                self.edit(|editor| editor.press(tile), cx);
            }
            return;
        }
        // The keys, while they are shown, are over everything: a click
        // takes them away and does nothing else.
        if self.state.borrow_mut().den.set_keys(None) {
            cx.notify();
            return;
        }
        // The feed first: the way back to its end, the name that narrows it,
        // a message to open.
        let (x, y) = self.view_point(event.position);
        // A session at home: its row wakes it.
        let home = self.state.borrow().scene.home_at(x, y);
        if let Some(id) = home {
            cx.emit(DenEvent::Wake(id));
            return;
        }
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

    /// The right button on a lion selects it and asks for its menu.
    fn on_right_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if self.is_editing() {
            return;
        }
        let Some(id) = self.lion_at(event.position) else {
            return;
        };
        let before = self.selected();
        self.state.borrow_mut().den.select(Some(id));
        self.selection_moved(before, cx);
        cx.emit(DenEvent::Menu {
            id,
            at: event.position,
        });
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
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
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

    let (scene, wake, image, stale, labels) = {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        state.den.set_reduced_motion(reduced, now);
        let (cols, rows) = (state.den.world().cols, state.den.world().rows);
        let (lions, home) = (state.den.order().len(), state.den.home().len());
        let layout =
            Layout::compute_with_home(width, height, scale, metrics, cols, rows, lions, home);
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
        let mut wake = frame.wake;
        // The room does not show what is said: typing redraws the feed, not
        // the picture.
        let still = Frame {
            said: Default::default(),
            typing: None,
            wake: Wake::Never,
            tick: if crate::paint::animated(&frame) {
                frame.tick
            } else {
                0
            },
            ..frame.clone()
        };

        // The room in 2.5D, when it was asked for and can be drawn.
        if matches!(state.three_d, ThreeD::Wanted) {
            state.three_d = match iso::Renderer::start() {
                Ok(renderer) => ThreeD::On(Box::new(renderer)),
                Err(why) => {
                    eprintln!("leon-den: the room stays in pixel art: {why}");
                    ThreeD::Failed(why)
                }
            };
        }
        let mut iso_picture = None;
        if let ThreeD::On(renderer) = &mut state.three_d {
            let field = layout.field;
            let size = (field.w.max(16) as u32, field.h.max(16) as u32);
            let camera = iso::Camera::fit(
                cols,
                rows,
                size.0 as f32,
                size.1 as f32,
                layout.px(26.) as f32,
            );
            let glide = state.den.glide(now);
            let walking = state.den.walking(now);
            let seconds = now.as_secs_f32();
            let stands = iso::room::stands(&state.den, &still, &glide, seconds);
            let small = metrics.char_w * PLATE_TEXT;
            let lettering = iso::overlay::Lettering {
                char_w: small,
                high: (metrics.line_h * PLATE_TEXT).ceil() as i32 + layout.px(2.),
                pad: layout.px(5.),
            };
            let overlay = iso::overlay::lay(
                &stands,
                &camera,
                (field.x, field.y),
                &style.palette,
                &lettering,
            );
            let wanted = Picture {
                frame: still.clone(),
                palette: style.palette,
                room: state.room,
                unit: 0,
                marks: marks.clone(),
                iso: Some(IsoKey {
                    glide: glide
                        .iter()
                        .map(|(id, x, y)| (*id, (x * 32.).round() as i32, (y * 32.).round() as i32))
                        .collect(),
                    stride: if walking {
                        (seconds * 9. * 16. / std::f32::consts::TAU) as i64
                    } else {
                        0
                    },
                    size,
                    theme: style.room,
                }),
            };
            let mut stale = None;
            let image = match &state.picture {
                Some((painted, image)) if *painted == wanted => Some(image.clone()),
                _ => {
                    let began = Instant::now();
                    let built = iso::room::build(
                        &state.den,
                        &still,
                        &glide,
                        seconds,
                        &style.room,
                        true,
                        marks.as_ref(),
                    );
                    match renderer.draw(&built.mesh, &camera, &style.room, size) {
                        Ok(pixels) => {
                            // The pixels come blue first already.
                            let buffer = image::RgbaImage::from_raw(size.0, size.1, pixels)
                                .expect("a picture is w*h*4 bytes");
                            let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
                            stale = state
                                .picture
                                .replace((wanted, image.clone()))
                                .map(|(_, old)| old);
                            state.pictures += 1;
                            state.spent += began.elapsed();
                            Some(image)
                        }
                        Err(why) => {
                            eprintln!("leon-den: the room goes back to pixel art: {why}");
                            state.three_d = ThreeD::Failed(why);
                            None
                        }
                    }
                }
            };
            if let Some(image) = image {
                let room = Layout {
                    map: field,
                    ..layout
                };
                let boxes = |id: u64| iso::overlay::box_of(&overlay, id);
                let scene = build_around(
                    &state.den,
                    &frame,
                    &room,
                    &style.palette,
                    note.as_deref(),
                    Some(&boxes),
                );
                if walking {
                    wake = wake.sooner(Wake::At(now + GLIDE_PACE));
                }
                let labels = overlay.labels.clone();
                state.shown = Some(Shown {
                    overlay,
                    camera,
                    origin: (field.x, field.y),
                });
                iso_picture = Some((scene, image, stale, labels));
            }
        }

        let (scene, image, stale, labels) = match iso_picture {
            Some(picture) => picture,
            None => {
                state.shown = None;
                let scene = build(&state.den, &frame, &layout, &style.palette, note.as_deref());
                let wanted = Picture {
                    frame: still,
                    palette: style.palette,
                    room: state.room,
                    unit: layout.unit,
                    marks,
                    iso: None,
                };
                // The same frame as last time is the same picture.
                let mut stale = None;
                let image = match &state.picture {
                    Some((painted, image)) if *painted == wanted => image.clone(),
                    _ => {
                        let began = Instant::now();
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
                        state.spent += began.elapsed();
                        image
                    }
                };
                (scene, image, stale, Vec::new())
            }
        };
        state.layout = Some(layout);
        state.origin = origin;
        state.scale = scale;
        state.paints += 1;
        (scene, wake, image, stale, labels)
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
    let area = |rect: Rect| Bounds {
        origin: at(rect.x, rect.y),
        size: size(px(rect.w as f32 / scale), px(rect.h as f32 / scale)),
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for quad in &scene.under {
            window.paint_quad(fill(area(quad.rect), quad.color));
        }
        let room = area(scene.room);
        let _ = window.paint_image(room, room, Corners::default(), image, 0, false);
        // What is written over the room in 2.5D: the plates and the bubbles.
        let plate_size = style.font_size * PLATE_TEXT;
        let plate_line = (line_height * PLATE_TEXT).round();
        for label in &labels {
            if let Some(leader) = label.leader {
                window.paint_quad(fill(area(leader), label.line));
            }
            window.paint_quad(fill(area(label.rect), label.line));
            let inner = Rect {
                x: label.rect.x + 1,
                y: label.rect.y + 1,
                w: label.rect.w - 2,
                h: label.rect.h - 2,
            };
            window.paint_quad(fill(area(inner), label.ground));
            let shaped = window.text_system().shape_line(
                SharedString::from(label.text.clone()),
                plate_size,
                &[TextRun {
                    len: label.text.len(),
                    font: font(style, true),
                    color: label.ink,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            // The word in the middle of its box, whatever the font measures.
            let left = label.rect.x as f32
                + (label.rect.w as f32 - shaped.width.as_f32() * scale).max(0.) / 2.;
            let top = label.rect.y as f32
                + (label.rect.h as f32 - plate_line.as_f32() * scale).max(0.) / 2.;
            let _ = shaped.paint(
                point(
                    px((origin.0 as f32 + left) / scale),
                    px((origin.1 as f32 + top) / scale),
                ),
                plate_line,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
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

/// The pixels of the GPU, blue first already, as a picture for GPUI.
fn picture_of(pixels: Vec<u8>, size: (u32, u32)) -> RenderImage {
    let buffer =
        image::RgbaImage::from_raw(size.0, size.1, pixels).expect("a picture is w*h*4 bytes");
    RenderImage::new([image::Frame::new(buffer)])
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
