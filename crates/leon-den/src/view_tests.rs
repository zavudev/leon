//! Tests of the view, headless: what it tells its host and what it costs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    point, px, size, AppContext, Bounds, Entity, Modifiers, MouseButton, TestAppContext,
    VisualTestContext, WindowBounds, WindowOptions,
};

use crate::model::{CubState, Event, Happening};
use crate::testing::{cub, palette};
use crate::view::{DenEvent, DenStyle, DenView};

struct Shown {
    view: Entity<DenView>,
    clock: Rc<Cell<Duration>>,
    events: Rc<RefCell<Vec<DenEvent>>>,
}

fn show(cx: &mut TestAppContext) -> (Shown, &mut VisualTestContext) {
    let clock = Rc::new(Cell::new(Duration::ZERO));
    let events = Rc::new(RefCell::new(Vec::new()));
    let (ticking, heard) = (clock.clone(), events.clone());
    let window = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1280.), px(800.)),
            })),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| {
            cx.new(|cx| {
                let style = DenStyle {
                    palette: palette(true),
                    font_family: "monospace".into(),
                    font_size: px(13.),
                };
                DenView::with_clock(style, Rc::new(move || ticking.get()), cx)
            })
        })
        .expect("the window opens")
    });
    let view = window.root(cx).expect("the view is the root");
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &DenEvent, _| {
            heard.borrow_mut().push(*event)
        })
        .detach();
    });
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (
        Shown {
            view,
            clock,
            events,
        },
        cx,
    )
}

fn feed(shown: &Shown, cx: &mut VisualTestContext, states: &[(u64, &str, CubState)]) {
    let cubs: Vec<_> = states
        .iter()
        .map(|(id, name, state)| cub(*id, name, *state))
        .collect();
    shown.view.update(cx, |view, cx| view.set_cubs(&cubs, cx));
    cx.run_until_parked();
}

/// The middle of a settled lion, in the window.
fn middle_of(
    shown: &Shown,
    cx: &mut VisualTestContext,
    id: u64,
) -> gpui_kit::Point<gpui_kit::Pixels> {
    let now = shown.clock.get();
    shown.view.update(cx, |view, _| {
        let (left, top, w, h) = view.read(|den| {
            den.frame(now)
                .actors
                .iter()
                .find(|actor| actor.id == id)
                .expect("the lion is drawn")
                .bounds()
        });
        view.window_point(left + w / 2, top + h / 2)
            .expect("the view has painted")
    })
}

#[gpui_kit::test]
fn the_view_paints_a_picture_of_the_room_and_only_a_new_one_when_the_frame_changes(
    cx: &mut TestAppContext,
) {
    let (shown, cx) = show(cx);
    let (paints, pictures, _) = shown.view.read_with(cx, |view, _| view.cost());
    assert!(paints >= 1 && pictures >= 1);
    // The same moment again: the same picture.
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let (again, same, _) = shown.view.read_with(cx, |view, _| view.cost());
    assert!(again > paints);
    assert_eq!(same, pictures, "nothing changed: nothing was composited");
    feed(&shown, cx, &[(1, "moss", CubState::Editing)]);
    let (_, more, _) = shown.view.read_with(cx, |view, _| view.cost());
    assert!(more > pictures);
}

#[gpui_kit::test]
fn a_den_where_nothing_moves_sets_no_timer(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    // An empty den types its one line, then it is still.
    shown.clock.set(Duration::from_secs(30));
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let (_, _, before) = shown.view.read_with(cx, |view, _| view.cost());
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let (_, _, after) = shown.view.read_with(cx, |view, _| view.cost());
    assert_eq!(after, before, "nothing moves: no timer");
    // A lion walks in: one timer a paint.
    feed(&shown, cx, &[(1, "moss", CubState::Editing)]);
    let (_, _, walking) = shown.view.read_with(cx, |view, _| view.cost());
    assert!(walking > after);
}

#[gpui_kit::test]
fn a_click_on_a_lion_selects_it_and_tells_the_host(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    feed(
        &shown,
        cx,
        &[
            (1, "moss", CubState::WaitingForUser),
            (2, "fern", CubState::Running),
        ],
    );
    shown.clock.set(Duration::from_secs(60));
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    let at = middle_of(&shown, cx, 1);
    cx.simulate_mouse_move(at, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        shown
            .view
            .read_with(cx, |view, _| view.read(|den| den.hovered())),
        Some(1)
    );
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(shown.view.read_with(cx, |view, _| view.selected()), Some(1));
    assert_eq!(
        *shown.events.borrow(),
        vec![DenEvent::Selected(Some(1)), DenEvent::Clicked(1)]
    );

    // A click on the floor selects nobody.
    shown.events.borrow_mut().clear();
    let floor = shown
        .view
        .update(cx, |view, _| view.window_point(40, 60).unwrap());
    cx.simulate_mouse_down(floor, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(floor, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(*shown.events.borrow(), vec![DenEvent::Selected(None)]);
}

#[gpui_kit::test]
fn the_host_moves_the_selection_and_opens_it(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    feed(
        &shown,
        cx,
        &[(1, "moss", CubState::Editing), (2, "fern", CubState::Idle)],
    );
    let opened = shown.view.update(cx, |view, cx| view.open_selection(cx));
    assert!(!opened, "nobody is selected");
    shown.view.update(cx, |view, cx| {
        view.select_next(cx);
        view.select_next(cx);
        view.select_next(cx);
        view.select_previous(cx);
        assert!(view.open_selection(cx));
        view.select(None, cx);
        view.select(None, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        *shown.events.borrow(),
        vec![
            DenEvent::Selected(Some(1)),
            DenEvent::Selected(Some(2)),
            DenEvent::Selected(Some(1)),
            DenEvent::Selected(Some(2)),
            DenEvent::Opened(2),
            DenEvent::Selected(None),
        ]
    );
}

#[gpui_kit::test]
fn a_selected_lion_that_goes_home_is_no_longer_selected(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    feed(&shown, cx, &[(1, "moss", CubState::Editing)]);
    shown.view.update(cx, |view, cx| view.select(Some(1), cx));
    shown.events.borrow_mut().clear();
    feed(&shown, cx, &[]);
    assert_eq!(*shown.events.borrow(), vec![DenEvent::Selected(None)]);
    shown.view.update(cx, |view, cx| {
        view.happen(&Happening::new(1, "moss", Event::WentHome), cx);
    });
    cx.run_until_parked();
    let said = shown
        .view
        .read_with(cx, |view, _| view.read(crate::testing::told));
    assert_eq!(said, vec!["MOSS went home."]);
}

/// Where a rectangle of the last scene is in the window: its middle.
fn middle(
    shown: &Shown,
    cx: &mut VisualTestContext,
    pick: impl FnOnce(&crate::scene::Scene) -> Option<crate::scene::Rect>,
) -> gpui_kit::Point<gpui_kit::Pixels> {
    shown.view.update(cx, |view, _| {
        let rect = view.scene(pick).expect("it is drawn");
        view.window_point_of(rect.x + rect.w / 2, rect.y + rect.h / 2)
    })
}

fn chatter(shown: &Shown, cx: &mut VisualTestContext) {
    feed(
        shown,
        cx,
        &[(1, "moss", CubState::Editing), (2, "fern", CubState::Idle)],
    );
    shown.view.update(cx, |view, cx| {
        view.set_wall_time(10_000, cx);
        for round in 0..14u64 {
            let (id, name) = if round % 2 == 0 {
                (1, "moss")
            } else {
                (2, "fern")
            };
            let text = format!("Message {round}.\n\n{}", "It goes on and on. ".repeat(20));
            view.speak(id, name, &text, None, cx);
        }
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_wheel_moves_the_feed_and_a_click_on_the_mark_goes_back_to_its_end(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    chatter(&shown, cx);
    let follows = |cx: &mut VisualTestContext| {
        shown
            .view
            .read_with(cx, |view, _| view.read(|den| den.feed().follows()))
    };
    assert!(follows(cx));
    assert!(shown
        .view
        .read_with(cx, |view, _| view.scene(|scene| scene.jump).is_none()));
    let over = middle(&shown, cx, |scene| scene.feed);
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: over,
        delta: gpui_kit::ScrollDelta::Lines(point(0., 6.)),
        ..Default::default()
    });
    cx.run_until_parked();
    assert!(!follows(cx), "the reader went back");
    // The wheel over the room moves nothing.
    shown.view.update(cx, |view, cx| view.feed_to(true, cx));
    cx.run_until_parked();
    let room = shown
        .view
        .update(cx, |view, _| view.window_point(40, 40).unwrap());
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: room,
        delta: gpui_kit::ScrollDelta::Lines(point(0., 6.)),
        ..Default::default()
    });
    cx.run_until_parked();
    assert!(follows(cx));

    // The keys of the host: a page back, the start, the end.
    shown.view.update(cx, |view, cx| view.scroll_feed(-1, cx));
    cx.run_until_parked();
    assert!(!follows(cx));
    shown.view.update(cx, |view, cx| view.feed_to(false, cx));
    cx.run_until_parked();
    let first = shown.view.read_with(cx, |view, _| {
        view.scene(|scene| scene.feed_rows.first().map(|(_, id)| *id))
    });
    assert_eq!(first, Some(0), "the oldest entry is in view");
    // A click on the mark is the way back; it selects no lion.
    let jump = middle(&shown, cx, |scene| scene.jump);
    shown.events.borrow_mut().clear();
    cx.simulate_click(jump, Modifiers::none());
    cx.run_until_parked();
    assert!(follows(cx));
    assert!(shown.events.borrow().is_empty());
}

#[gpui_kit::test]
fn a_click_opens_a_long_message_and_the_name_over_the_feed_shows_everybody_again(
    cx: &mut TestAppContext,
) {
    let (shown, cx) = show(cx);
    chatter(&shown, cx);
    let open = |cx: &mut VisualTestContext, id: u64| {
        shown
            .view
            .read_with(cx, |view, _| view.read(|den| den.feed().is_open(id)))
    };
    let (at, entry) = shown.view.update(cx, |view, _| {
        let (rect, entry) = view
            .scene(|scene| scene.feed_rows.last().copied())
            .expect("the feed has rows");
        (
            view.window_point_of(rect.x + rect.w / 2, rect.y + rect.h / 2),
            entry,
        )
    });
    assert!(!open(cx, entry));
    cx.simulate_click(at, Modifiers::none());
    cx.run_until_parked();
    assert!(open(cx, entry));
    assert_eq!(shown.view.read_with(cx, |view, _| view.selected()), None);

    // The keyboard: an entry at a time, Enter on it, and away.
    shown.view.update(cx, |view, cx| {
        assert!(!view.feed_toggle(cx), "the keyboard is on no entry");
        view.feed_step(true, cx);
        assert!(view.feed_toggle(cx));
    });
    assert!(!open(cx, entry), "cut again");
    shown.view.update(cx, |view, cx| {
        assert!(view.feed_release(cx));
        assert!(!view.feed_release(cx));
    });

    // Selecting a lion narrows the feed; its name over the feed undoes it.
    shown.view.update(cx, |view, cx| view.select(Some(2), cx));
    cx.run_until_parked();
    let all = middle(&shown, cx, |scene| scene.all);
    shown.events.borrow_mut().clear();
    cx.simulate_click(all, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(shown.view.read_with(cx, |view, _| view.selected()), None);
    assert_eq!(*shown.events.borrow(), vec![DenEvent::Selected(None)]);
}

#[gpui_kit::test]
fn typing_in_the_feed_composites_no_new_picture_of_the_room(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    feed(&shown, cx, &[(1, "moss", CubState::Fainted)]);
    shown.clock.set(Duration::from_secs(60));
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let (_, pictures, _) = shown.view.read_with(cx, |view, _| view.cost());
    shown.view.update(cx, |view, cx| {
        view.happen(&Happening::new(1, "moss", Event::FellAsleep), cx);
    });
    cx.run_until_parked();
    for tick in 1..8 {
        shown
            .clock
            .set(Duration::from_secs(60) + Duration::from_millis(100 * tick));
        shown.view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }
    let (_, after, _) = shown.view.read_with(cx, |view, _| view.cost());
    assert_eq!(after, pictures, "the line was typed over the same picture");
}
