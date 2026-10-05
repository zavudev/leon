//! Tests of the element, headless: what it paints, in which colour, with
//! which geometry, and what it asks of the window.

use gpui_kit::{
    div, point, prelude::*, px, rgb, Context, Entity, Hsla, Modifiers, Render, TestAppContext,
    VisualTestContext, Window,
};

use crate::element::probe;
use crate::geometry::Variant;
use crate::motion::{GestureSet, Mood, Pose};
use crate::AnimatedMark;

#[derive(Clone)]
struct Spec {
    size: f32,
    mood: Option<Mood>,
    animate: bool,
    reduced: Option<bool>,
    intro: bool,
    hover: bool,
    color: Hsla,
}

impl Spec {
    fn new(size: f32) -> Self {
        Self {
            size,
            mood: None,
            animate: true,
            reduced: None,
            intro: false,
            hover: false,
            color: rgb(0xffea00).into(),
        }
    }
}

struct Host {
    spec: Spec,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let spec = &self.spec;
        let mut mark = AnimatedMark::new(px(spec.size))
            .id("lion")
            .seed(3)
            .color(spec.color)
            .animate(spec.animate)
            .intro(spec.intro)
            .play_on_hover(spec.hover);
        if let Some(mood) = spec.mood {
            mark = mark.mood(mood);
        }
        if let Some(reduced) = spec.reduced {
            mark = mark.reduced_motion(reduced);
        }
        div().size_full().child(mark)
    }
}

fn show(cx: &mut TestAppContext, spec: Spec) -> (Entity<Host>, &mut VisualTestContext) {
    probe::clear();
    let (host, cx) = cx.add_window_view(|_, _| Host { spec });
    // A test window is not the one in front until it says so.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    host.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    (host, cx)
}

fn change(host: &Entity<Host>, cx: &mut VisualTestContext, edit: impl FnOnce(&mut Spec)) {
    host.update(cx, |host, cx| {
        edit(&mut host.spec);
        cx.notify();
    });
    cx.run_until_parked();
}

fn lion() -> probe::Painted {
    probe::painted("lion").expect("the mark painted")
}

#[gpui_kit::test]
fn the_mark_paints_in_the_colour_it_is_given(cx: &mut TestAppContext) {
    let (_host, _cx) = show(cx, Spec::new(64.));
    let painted = lion();
    assert!(painted.paints >= 1);
    assert_eq!(painted.color, Hsla::from(rgb(0xffea00)));
}

#[gpui_kit::test]
fn a_new_colour_is_painted_on_the_next_frame(cx: &mut TestAppContext) {
    let (host, cx) = show(cx, Spec::new(64.));
    change(&host, cx, |spec| spec.color = rgb(0x0a0a0a).into());
    assert_eq!(lion().color, Hsla::from(rgb(0x0a0a0a)));
}

#[gpui_kit::test]
fn twenty_pixels_and_below_use_the_fitted_geometry_and_only_blink_and_narrow(
    cx: &mut TestAppContext,
) {
    for side in [16., 20.] {
        let (_host, _cx) = show(cx, Spec::new(side));
        let painted = lion();
        assert_eq!(painted.variant, Variant::Small, "{side} px");
        assert_eq!(painted.gestures, GestureSet::SMALL, "{side} px");
    }
}

#[gpui_kit::test]
fn larger_sizes_use_the_full_geometry_and_every_gesture(cx: &mut TestAppContext) {
    for side in [21., 32., 64., 256.] {
        let (_host, _cx) = show(cx, Spec::new(side));
        let painted = lion();
        assert_eq!(painted.variant, Variant::Full, "{side} px");
        assert_eq!(painted.gestures, GestureSet::ALL, "{side} px");
    }
}

#[gpui_kit::test]
fn a_mark_that_is_not_animated_paints_the_rest_pose_and_asks_for_nothing(cx: &mut TestAppContext) {
    let (host, cx) = show(
        cx,
        Spec {
            animate: false,
            intro: true,
            mood: Some(Mood::Working),
            ..Spec::new(64.)
        },
    );
    change(&host, cx, |_| {});
    let painted = lion();
    assert_eq!(painted.pose, Pose::REST);
    assert!(!painted.live);
    assert_eq!((painted.frames_requested, painted.timers_set), (0, 0));
}

#[gpui_kit::test]
fn under_reduced_motion_it_is_the_rest_pose_with_no_intro_and_no_timers(cx: &mut TestAppContext) {
    let (host, cx) = show(
        cx,
        Spec {
            reduced: Some(true),
            intro: true,
            ..Spec::new(64.)
        },
    );
    change(&host, cx, |_| {});
    let painted = lion();
    assert_eq!(painted.pose, Pose::REST, "no intro: the plate is whole");
    assert!(!painted.live);
    assert_eq!((painted.frames_requested, painted.timers_set), (0, 0));
    assert!(!painted.hit_testing, "no hover either");
}

#[gpui_kit::test]
fn the_toolkits_own_reduce_motion_flag_stills_the_mark_too(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_host, _cx) = show(cx, Spec::new(64.));
    let painted = lion();
    assert_eq!(painted.pose, Pose::REST);
    assert_eq!((painted.frames_requested, painted.timers_set), (0, 0));
}

#[gpui_kit::test]
fn an_explicit_reduced_motion_of_false_overrides_the_toolkit_flag(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_host, _cx) = show(
        cx,
        Spec {
            reduced: Some(false),
            ..Spec::new(64.)
        },
    );
    assert!(lion().live);
}

#[gpui_kit::test]
fn between_gestures_a_live_mark_sets_one_timer_and_requests_no_frames(cx: &mut TestAppContext) {
    let (_host, _cx) = show(cx, Spec::new(64.));
    let painted = lion();
    assert!(painted.live);
    assert_eq!(painted.pose, Pose::REST);
    assert_eq!(painted.frames_requested, 0);
    // One timer for the next gesture per paint of a live mark, each replacing
    // the last: never a frame.
    assert!(painted.timers_set >= 1 && painted.timers_set <= painted.paints);
}

#[gpui_kit::test]
fn the_intro_requests_frames_while_it_plays_and_starts_from_the_eyes(cx: &mut TestAppContext) {
    let (_host, _cx) = show(
        cx,
        Spec {
            intro: true,
            ..Spec::new(64.)
        },
    );
    let painted = lion();
    assert!(painted.frames_requested >= 1);
    assert!(painted.pose.plate < 1., "{:?}", painted.pose);
    assert!(painted.pose.glow > 0.);
}

#[gpui_kit::test]
fn an_asleep_mark_holds_its_eyes_nearly_closed_and_asks_for_nothing(cx: &mut TestAppContext) {
    let (host, cx) = show(
        cx,
        Spec {
            mood: Some(Mood::Asleep),
            ..Spec::new(64.)
        },
    );
    change(&host, cx, |_| {});
    let painted = lion();
    assert_eq!(painted.mood, Mood::Asleep);
    assert!(painted.pose.eye_open <= 0.15);
    assert_eq!((painted.frames_requested, painted.timers_set), (0, 0));
}

#[gpui_kit::test]
fn a_window_that_is_not_in_front_asks_for_nothing(cx: &mut TestAppContext) {
    let (host, cx) = show(cx, Spec::new(64.));
    assert!(lion().live);
    let before = lion();
    cx.deactivate_window();
    change(&host, cx, |_| {});
    let after = lion();
    assert!(after.paints > before.paints, "it painted again");
    assert!(!after.live, "but not alive");
    assert_eq!(after.frames_requested, before.frames_requested);
    assert_eq!(
        after.timers_set, before.timers_set,
        "no timer in a window nobody looks at"
    );
}

#[gpui_kit::test]
fn the_mood_it_is_given_is_the_mood_it_paints_in(cx: &mut TestAppContext) {
    let (host, cx) = show(cx, Spec::new(64.));
    assert_eq!(lion().mood, Mood::Idle);
    for mood in [Mood::Working, Mood::Waiting, Mood::Error, Mood::Idle] {
        change(&host, cx, |spec| spec.mood = Some(mood));
        assert_eq!(lion().mood, mood);
    }
}

#[gpui_kit::test]
fn the_pointer_arriving_on_the_mark_plays_a_narrow_once(cx: &mut TestAppContext) {
    let (host, cx) = show(
        cx,
        Spec {
            hover: true,
            ..Spec::new(64.)
        },
    );
    assert!(
        lion().hit_testing,
        "a hovering mark listens for the pointer"
    );
    let inside = point(px(30.), px(30.));
    cx.simulate_mouse_move(inside, None, Modifiers::none());
    change(&host, cx, |_| {});
    assert_eq!(lion().pokes, 1);
    assert!(lion().pose.narrow > 0., "{:?}", lion().pose);
    // Moving around inside it is the same visit.
    cx.simulate_mouse_move(point(px(31.), px(31.)), None, Modifiers::none());
    assert_eq!(lion().pokes, 1);
}

#[gpui_kit::test]
fn a_mark_that_does_not_play_on_hover_does_not_listen(cx: &mut TestAppContext) {
    let (_host, _cx) = show(cx, Spec::new(64.));
    assert!(!lion().hit_testing);
}
