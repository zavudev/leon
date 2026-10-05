//! The lion as a GPUI element.
//!
//! [`AnimatedMark`] paints the lion's shapes as vector paths in one colour:
//! the plate and its cuts are one path with the `evenodd` rule, exactly as
//! the SVG is, so the cuts show whatever is behind the mark. At rest it
//! snaps to device pixels, so the still pose is as crisp as the static SVG.
//!
//! # Cost
//!
//! The element asks for an animation frame only while a part is moving.
//! Between gestures it sets one timer for the start of the next one and
//! paints nothing. It does no work at all, and keeps no timer, when:
//!
//! * [`animate(false)`](AnimatedMark::animate), or reduced motion is on
//!   (then it paints the rest pose, and no intro);
//! * the window is not the active one (it holds the baseline of its mood);
//! * the mood is [`Mood::Asleep`] (the eyes settle, then nothing moves);
//! * the mark is not drawn: its state, and with it the timer, is dropped by
//!   GPUI as soon as a frame is drawn without it.
//!
//! # Size
//!
//! At 20 pixels and below the element draws the fitted small geometry and
//! plays only the blink and the glare: a glance, a lean, a breath or a
//! twitch of the nose cannot be seen at that size.

use std::cell::RefCell;
use std::panic::Location;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    point, px, size, App, Bounds, DispatchPhase, Element, ElementId, FillOptions, FillRule,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseMoveEvent, PathBuilder, PathStyle, Pixels, Point, Style, Task, Window,
};

use crate::geometry::{shapes, Pt, Shapes, Variant, GRID};
use crate::motion::{Frame, GestureSet, Mood, Pose, Timeline, Wake};

/// The lion, alive.
///
/// It is a square of the given size. Build it with [`AnimatedMark::new`] and
/// tint it with [`color`](Self::color): the theme's `logo` colour.
///
/// ```no_run
/// use leon_mark::{AnimatedMark, Mood};
/// use gpui_kit::{px, rgb};
///
/// let mark = AnimatedMark::new(px(32.))
///     .color(rgb(0xffea00))
///     .mood(Mood::Working)
///     .play_on_hover(true);
/// ```
pub struct AnimatedMark {
    id: ElementId,
    size: Pixels,
    color: Hsla,
    seed: u64,
    mood: Option<Mood>,
    timeline: Option<Timeline>,
    animate: bool,
    reduced_motion: Option<bool>,
    intro: bool,
    hover: bool,
}

impl AnimatedMark {
    /// A mark `size` wide and high, idle, in white, with no intro.
    ///
    /// Its state is keyed by the place in the code that calls this, which
    /// also seeds its schedule so that two marks do not blink together. Marks
    /// built in a loop need an [`id`](Self::id) each.
    #[track_caller]
    pub fn new(size: Pixels) -> Self {
        let location = Location::caller();
        Self {
            id: ElementId::CodeLocation(*location),
            size,
            color: Hsla {
                h: 0.,
                s: 0.,
                l: 1.,
                a: 1.,
            },
            seed: u64::from(location.line()) * 131 + u64::from(location.column()),
            mood: None,
            timeline: None,
            animate: true,
            reduced_motion: None,
            intro: false,
            hover: false,
        }
    }

    /// Identifies this mark among its siblings. The animation restarts when
    /// the id changes.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// The colour of the lion: the plate. The cuts are not painted, so they
    /// show what is behind.
    pub fn color(mut self, color: impl Into<Hsla>) -> Self {
        self.color = color.into();
        self
    }

    /// The seed of the schedule: the same seed always blinks at the same
    /// moments.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// How the lion feels. It may change from one frame to the next: the eyes
    /// ease to the new baseline and the gestures of the new mood begin.
    pub fn mood(mut self, mood: Mood) -> Self {
        self.mood = Some(mood);
        self
    }

    /// What it plays, in place of the idle timeline of its seed. A mood set
    /// with [`mood`](Self::mood) still applies.
    pub fn timeline(mut self, timeline: Timeline) -> Self {
        self.timeline = Some(timeline);
        self
    }

    /// False paints the rest pose and stops every timer and frame. Turning
    /// it back on starts the timeline from zero.
    pub fn animate(mut self, animate: bool) -> Self {
        self.animate = animate;
        self
    }

    /// True paints the rest pose, no intro, as the user asked for less
    /// motion. Without this the element follows GPUI's own
    /// [`App::reduce_motion`], which the desktop's setting feeds.
    pub fn reduced_motion(mut self, reduced_motion: bool) -> Self {
        self.reduced_motion = Some(reduced_motion);
        self
    }

    /// Whether the lion resolves from its eyes when it first appears. Off by
    /// default; it plays once, when the element's state is created.
    pub fn intro(mut self, intro: bool) -> Self {
        self.intro = intro;
        self
    }

    /// Whether the pointer arriving on the mark makes it narrow its eyes.
    /// Off by default. The mark never swallows the pointer's events.
    pub fn play_on_hover(mut self, hover: bool) -> Self {
        self.hover = hover;
        self
    }

    fn variant(&self) -> Variant {
        Variant::for_side(self.size.as_f32())
    }

    fn gestures(&self) -> GestureSet {
        match self.variant() {
            Variant::Small => GestureSet::SMALL,
            Variant::Full => GestureSet::ALL,
        }
    }

    fn is_still(&self, cx: &App) -> bool {
        !self.animate || self.reduced_motion.unwrap_or_else(|| cx.reduce_motion())
    }

    fn start_timeline(&self) -> Timeline {
        let base = self.timeline.unwrap_or_else(|| {
            if self.intro {
                Timeline::intro(self.seed)
            } else {
                Timeline::idle(self.seed)
            }
        });
        let base = base.with_gestures(self.gestures());
        match self.mood {
            Some(mood) => base.with_mood(mood),
            None => base,
        }
    }
}

/// What a live mark remembers between frames. GPUI drops it, and with it the
/// timer, as soon as a frame is drawn without the mark.
struct Runtime {
    /// When the timeline began: when the lion was first drawn in the window
    /// in front, so that an intro is not spent in a window nobody looks at.
    started: Instant,
    /// Whether the window has been in front since the state was made.
    seen: bool,
    timeline: Timeline,
    /// When the pointer last arrived, on the timeline.
    poked_at: Option<Duration>,
    hovered: bool,
    /// Wakes the view for the next gesture. Replaced, and so cancelled, on
    /// every paint.
    timer: Option<Task<()>>,
}

type SharedRuntime = Rc<RefCell<Runtime>>;

impl IntoElement for AnimatedMark {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AnimatedMark {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: size(self.size.into(), self.size.into()),
            flex_shrink: 0.,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        (self.hover && !self.is_still(cx))
            .then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let variant = self.variant();
        let element = self.id.clone();
        let _ = &element;

        let Some(id) = id.filter(|_| !self.is_still(cx)) else {
            // Not touching the element state lets GPUI drop it: no timer
            // survives a pause.
            paint_pose(bounds, &Pose::REST, variant, self.color, window);
            #[cfg(any(test, feature = "test-support"))]
            probe::record(&element, |painted| {
                painted.paints += 1;
                painted.pose = Pose::REST;
                painted.live = false;
                painted.variant = variant;
                painted.color = self.color;
                painted.mood = self.mood.unwrap_or_default();
                painted.hit_testing = false;
                painted.gestures = self.gestures();
            });
            return;
        };

        let now = Instant::now();
        let start = self.start_timeline();
        let runtime = window.with_element_state::<SharedRuntime, _>(id, |runtime, _| {
            let runtime = runtime.unwrap_or_else(|| {
                Rc::new(RefCell::new(Runtime {
                    started: now,
                    seen: false,
                    timeline: start,
                    poked_at: None,
                    hovered: false,
                    timer: None,
                }))
            });
            (runtime.clone(), runtime)
        });

        let active = window.is_window_active();
        let view = window.current_view();
        let gestures = self.gestures();
        let small = variant == Variant::Small;
        let mut requested_frame = false;
        let mut set_timer = false;
        let (pose, mood) = {
            let mut state = runtime.borrow_mut();
            if active && !state.seen {
                state.seen = true;
                state.started = now;
            }
            let at = now.saturating_duration_since(state.started);
            // A change of mood takes effect from this frame on.
            state.timeline = state.timeline.with_gestures(gestures);
            if let Some(mood) = self.mood {
                state.timeline = state.timeline.mood_changed(mood, at);
            }
            let mood = state.timeline.mood();
            if !active {
                // Not the window in front: hold the baseline, ask for
                // nothing. Coming back redraws the window, which resumes.
                state.timer = None;
                (mood.baseline(), mood)
            } else {
                let poked = state.poked_at;
                let Frame { pose, wake } = state.timeline.frame(at, poked);
                state.timer = match wake {
                    Wake::NextFrame => {
                        window.request_animation_frame();
                        requested_frame = true;
                        None
                    }
                    Wake::At(wake) => {
                        set_timer = true;
                        // Sleep until the next gesture starts, then redraw
                        // the view that holds the mark.
                        let sleep = wake.saturating_sub(at).max(Duration::from_millis(1));
                        Some(window.spawn(cx, async move |cx| {
                            cx.background_executor().timer(sleep).await;
                            cx.update(|_, cx| cx.notify(view)).ok();
                        }))
                    }
                    Wake::Never => None,
                };
                (pose, mood)
            }
        };
        let pose = if small { pose.for_small() } else { pose };

        let hit_testing = hitbox.is_some();
        if let Some(hitbox) = hitbox.take() {
            let runtime = runtime.clone();
            window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let hovered = hitbox.is_hovered(window);
                let mut state = runtime.borrow_mut();
                if hovered && !state.hovered {
                    state.poked_at = Some(state.started.elapsed());
                    #[cfg(any(test, feature = "test-support"))]
                    probe::record(&element, |painted| painted.pokes += 1);
                    cx.notify(view);
                }
                state.hovered = hovered;
            });
        }

        paint_pose(bounds, &pose, variant, self.color, window);

        #[cfg(any(test, feature = "test-support"))]
        probe::record(&self.id, |painted| {
            painted.paints += 1;
            painted.pose = pose;
            painted.live = active;
            painted.variant = variant;
            painted.color = self.color;
            painted.mood = mood;
            painted.frames_requested += usize::from(requested_frame);
            painted.timers_set += usize::from(set_timer);
            painted.hit_testing = hit_testing;
            painted.gestures = gestures;
        });
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = (mood, requested_frame, set_timer, hit_testing);
    }
}

/// Paints the lion in `pose` into `bounds` (a square) in one colour. This
/// is all [`AnimatedMark`] draws; call it from a `canvas` to place the lion
/// in a custom element. Paint phase only.
pub fn paint_pose(
    bounds: Bounds<Pixels>,
    pose: &Pose,
    variant: Variant,
    color: Hsla,
    window: &mut Window,
) {
    let drawn = shapes(pose, variant);
    paint_shapes(bounds, &drawn, color, window);
}

/// Paints already computed [`Shapes`].
pub fn paint_shapes(bounds: Bounds<Pixels>, drawn: &Shapes, color: Hsla, window: &mut Window) {
    let (origin, side) = snap(bounds, window.scale_factor());
    let unit = side / GRID;
    let at = |(x, y): Pt| point(origin.x + unit * x, origin.y + unit * y);

    // The plate and its cuts are one path with the even-odd rule, as in the
    // SVG: a cut is a hole whatever its winding.
    if drawn.plate_opacity > 0. {
        let mut path = PathBuilder::fill().with_style(PathStyle::Fill(
            FillOptions::default().with_fill_rule(FillRule::EvenOdd),
        ));
        for contour in drawn.contours() {
            let mut points = contour.into_iter();
            if let Some(first) = points.next() {
                path.move_to(at(first));
                for next in points {
                    path.line_to(at(next));
                }
                path.close();
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color.opacity(drawn.plate_opacity));
        }
    }

    // The slits as solid shapes, before the plate has arrived around them.
    if drawn.glow_opacity > 0. {
        let mut path = PathBuilder::fill();
        for eye in &drawn.glow {
            let mut points = eye.iter().copied();
            if let Some(first) = points.next() {
                path.move_to(at(first));
                for next in points {
                    path.line_to(at(next));
                }
                path.close();
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color.opacity(drawn.glow_opacity));
        }
    }
}

/// The square the lion is drawn in, on whole device pixels: its origin and
/// its side. A still lion drawn on whole pixels is as crisp as the SVG.
fn snap(bounds: Bounds<Pixels>, scale: f32) -> (Point<Pixels>, Pixels) {
    let side = bounds.size.width.min(bounds.size.height);
    let device = |value: Pixels| px((value.as_f32() * scale).round() / scale);
    let side = px(((side.as_f32() * scale).round() / scale).max(1. / scale));
    (
        point(device(bounds.origin.x), device(bounds.origin.y)),
        side,
    )
}

/// What the tests of other crates can read of a painted mark.
///
/// Every paint of an [`AnimatedMark`] is recorded here by its element id,
/// per thread, so that a test can ask what a mark last painted and how many
/// frames and timers it has asked for. Only with the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod probe {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use gpui_kit::{ElementId, Hsla};

    use crate::geometry::Variant;
    use crate::motion::{GestureSet, Mood, Pose};

    /// What a mark last painted, and what it has asked for since it first
    /// painted.
    #[derive(Clone, Debug)]
    pub struct Painted {
        /// How many times it has painted.
        pub paints: usize,
        /// The pose it last painted.
        pub pose: Pose,
        /// Whether it was live: animated, and in the window in front.
        pub live: bool,
        /// The drawing it used.
        pub variant: Variant,
        /// The colour it was given.
        pub color: Hsla,
        /// The mood it was in.
        pub mood: Mood,
        /// Animation frames it has asked for in all.
        pub frames_requested: usize,
        /// Timers it has set in all.
        pub timers_set: usize,
        /// Whether it listens for the pointer.
        pub hit_testing: bool,
        /// How many times the pointer arrived on it.
        pub pokes: usize,
        /// The families of gesture it may play.
        pub gestures: GestureSet,
    }

    impl Default for Painted {
        fn default() -> Self {
            Self {
                paints: 0,
                pose: Pose::REST,
                live: false,
                variant: Variant::Full,
                color: Hsla::default(),
                mood: Mood::Idle,
                frames_requested: 0,
                timers_set: 0,
                hit_testing: false,
                pokes: 0,
                gestures: GestureSet::ALL,
            }
        }
    }

    thread_local! {
        static PAINTED: RefCell<HashMap<String, Painted>> = RefCell::new(HashMap::new());
    }

    pub(crate) fn record(id: &ElementId, update: impl FnOnce(&mut Painted)) {
        PAINTED.with(|painted| update(painted.borrow_mut().entry(id.to_string()).or_default()));
    }

    /// What the mark with this element id (a name, as given to
    /// [`AnimatedMark::id`](crate::AnimatedMark::id)) last painted on this
    /// thread.
    pub fn painted(id: &str) -> Option<Painted> {
        PAINTED.with(|painted| painted.borrow().get(id).cloned())
    }

    /// Forgets everything recorded on this thread.
    pub fn clear() {
        PAINTED.with(|painted| painted.borrow_mut().clear());
    }
}
