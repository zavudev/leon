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
                    room: crate::iso::Theme::from_tokens(&crate::testing::tokens(true)),
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
fn the_right_button_and_the_host_ask_for_the_menu_of_a_lion(cx: &mut TestAppContext) {
    let (shown, cx) = show(cx);
    feed(
        &shown,
        cx,
        &[
            (1, "moss", CubState::Editing),
            (2, "fern", CubState::Running),
        ],
    );
    shown.clock.set(Duration::from_secs(60));
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    // The right button on a lion selects it and asks for its menu there.
    let at = middle_of(&shown, cx, 2);
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        *shown.events.borrow(),
        vec![DenEvent::Selected(Some(2)), DenEvent::Menu { id: 2, at }]
    );
    // On the floor it does nothing: the selection stays.
    shown.events.borrow_mut().clear();
    let floor = shown
        .view
        .update(cx, |view, _| view.window_point(40, 60).unwrap());
    cx.simulate_mouse_down(floor, MouseButton::Right, Modifiers::none());
    cx.run_until_parked();
    assert!(shown.events.borrow().is_empty());
    assert_eq!(shown.view.read_with(cx, |view, _| view.selected()), Some(2));

    // The host's key: the menu of the selected lion, anchored under it.
    assert!(shown.view.update(cx, |view, cx| view.menu_selection(cx)));
    cx.run_until_parked();
    let events = shown.events.borrow().clone();
    let [DenEvent::Menu { id: 2, at: under }] = events[..] else {
        panic!("one menu of fern: {events:?}");
    };
    assert!(under.y > at.y, "under the lion, not on it");
    // With nobody selected there is no menu to ask for.
    shown.view.update(cx, |view, cx| {
        view.select(None, cx);
        assert!(!view.menu_selection(cx));
    });
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

#[gpui_kit::test]
fn the_pixel_art_is_the_picture_until_two_and_a_half_d_is_asked_for_and_whenever_it_cannot_be_drawn(
    cx: &mut TestAppContext,
) {
    use crate::view::Drawn;
    let (shown, cx) = show(cx);
    feed(&shown, cx, &[(1, "moss", CubState::WaitingForUser)]);
    assert_eq!(
        shown.view.read_with(cx, |view, _| view.drawn()),
        Drawn::Pixels
    );
    let (_, before, _) = shown.view.read_with(cx, |view, _| view.cost());

    shown.view.update(cx, |view, cx| view.set_three_d(true, cx));
    cx.run_until_parked();
    let (_, after, _) = shown.view.read_with(cx, |view, _| view.cost());
    match shown.view.read_with(cx, |view, _| view.drawn()) {
        // No GPU in this build or on this computer: the same pixel art,
        // and the view says why.
        Drawn::Failed(why) => {
            assert!(!why.is_empty());
            assert_eq!(after, before, "the picture did not change");
        }
        // A GPU: another picture of the same room.
        Drawn::Iso(_) => assert!(after > before),
        other => panic!("a paint decides: {other:?}"),
    }
    // Either way the lion is found where it is drawn, and its menu opens
    // under it.
    shown.clock.set(Duration::from_secs(30));
    shown.view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    shown.view.update(cx, |view, cx| {
        view.select(Some(1), cx);
        assert!(view.menu_selection(cx));
    });
    cx.run_until_parked();
    assert!(shown
        .events
        .borrow()
        .iter()
        .any(|event| matches!(event, DenEvent::Menu { id: 1, .. })));

    // The room is edited in the picture it is shown in: opening the editor
    // changes nothing of that, and a piece in hand is one more picture.
    let drawn = shown.view.read_with(cx, |view, _| view.drawn());
    shown.view.update(cx, |view, cx| view.start_editing(cx));
    cx.run_until_parked();
    assert_eq!(shown.view.read_with(cx, |view, _| view.drawn()), drawn);
    let (_, before, _) = shown.view.read_with(cx, |view, _| view.cost());
    shown.view.update(cx, |view, cx| {
        view.edit(
            |editor| {
                editor.pick("plant");
                editor.point(Some(crate::world::Tile::new(5, 6)));
                Ok(())
            },
            cx,
        );
    });
    cx.run_until_parked();
    let (_, after, _) = shown.view.read_with(cx, |view, _| view.cost());
    assert!(after > before, "the piece in hand is drawn");
    assert_eq!(shown.view.read_with(cx, |view, _| view.drawn()), drawn);
    // The pictures a host lists are in 2.5D exactly when the room is.
    let layout = shown.view.read_with(cx, |view, _| view.layout());
    let (iso, room, piece) = shown.view.read_with(cx, |view, _| {
        (
            view.is_three_d(),
            view.room_picture(&layout, (120, 72)).is_some(),
            view.piece_picture("desk", 0, gpui_kit::black(), (56, 56))
                .is_some(),
        )
    });
    assert_eq!(iso, matches!(drawn, Drawn::Iso(_)));
    assert_eq!((room, piece), (iso, iso));
    // Asking for the pixel art again gives it back.
    shown.view.update(cx, |view, cx| {
        view.stop_editing(cx);
        view.set_three_d(false, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        shown.view.read_with(cx, |view, _| view.drawn()),
        Drawn::Pixels
    );
    // Asking twice for the same thing changes nothing.
    shown
        .view
        .update(cx, |view, cx| view.set_three_d(false, cx));
    assert_eq!(
        shown.view.read_with(cx, |view, _| view.drawn()),
        Drawn::Pixels
    );
}

#[gpui_kit::test]
fn the_host_goes_to_who_needs_the_user_wakes_who_is_at_home_and_shows_the_keys(
    cx: &mut TestAppContext,
) {
    let (shown, cx) = show(cx);
    feed(
        &shown,
        cx,
        &[
            (1, "moss", CubState::Editing),
            (2, "fern", CubState::WaitingForUser),
            (3, "ash", CubState::NeedsPermission),
        ],
    );
    // The most pressing first, then around.
    shown.view.update(cx, |view, cx| {
        assert_eq!(view.needy(), 2);
        assert!(view.select_needy(cx));
        assert_eq!(view.selected(), Some(3));
        assert!(view.select_needy(cx));
        assert_eq!(view.selected(), Some(2));
        assert!(view.select_needy(cx));
        assert_eq!(view.selected(), Some(3));
    });
    cx.run_until_parked();
    assert_eq!(
        *shown.events.borrow(),
        vec![
            DenEvent::Selected(Some(3)),
            DenEvent::Selected(Some(2)),
            DenEvent::Selected(Some(3)),
        ]
    );
    shown.events.borrow_mut().clear();

    // A session at home has a row under the roster: a click wakes it.
    shown.view.update(cx, |view, cx| {
        view.set_home(
            vec![crate::HomeEntry {
                id: 70,
                name: "rowan".to_owned(),
                tint: gpui_kit::rgb(0xd97757).into(),
            }],
            cx,
        );
    });
    cx.run_until_parked();
    let row = shown.view.read_with(cx, |view, _| {
        let (rect, id) = view.scene(|scene| scene.home_rows[0]);
        assert_eq!(id, 70);
        view.window_point_of(rect.x + rect.w / 2, rect.y + rect.h / 2)
    });
    cx.simulate_mouse_move(row, None, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        shown
            .view
            .read_with(cx, |view, _| view.read(|den| den.home_hovered())),
        Some(70)
    );
    cx.simulate_mouse_down(row, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(row, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(*shown.events.borrow(), vec![DenEvent::Wake(70)]);
    assert_eq!(
        shown.view.read_with(cx, |view, _| view.selected()),
        Some(3),
        "the selection is nobody's business here"
    );
    shown.events.borrow_mut().clear();

    // The keys, over the room: a click anywhere takes them away and does
    // nothing else, not even on a lion.
    let keys = vec![("N".to_owned(), "the next lion that needs you".to_owned())];
    shown.view.update(cx, |view, cx| {
        assert!(!view.keys_shown());
        view.show_keys(Some(keys.clone()), cx);
        assert!(view.keys_shown());
    });
    cx.run_until_parked();
    assert!(shown
        .view
        .read_with(cx, |view, _| view.scene(|scene| scene.keys.is_some())));
    let at = middle_of(&shown, cx, 1);
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert!(!shown.view.read_with(cx, |view, _| view.keys_shown()));
    assert!(shown.events.borrow().is_empty());
    assert_eq!(shown.view.read_with(cx, |view, _| view.selected()), Some(3));
}
