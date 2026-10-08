//! Tests of the simulation: the den at any time is a function of what it
//! was told.

use std::collections::HashSet;
use std::time::Duration;

use crate::model::{Cub, CubState, Event, Happening, ToolKind};
use crate::narrator::LINGER;
use crate::pose::{Act, Bubble, Facing, TILE};
use crate::sim::{
    act_of, place_of, tick_of, time_of, wanted, Den, Frame, Wake, EGG_TICKS, ENTRANCE_LINE,
    HATCH_TICKS, PONDER, STEP_TICKS,
};
use crate::testing::{at, cub, little, told};
use crate::world::{Ground, Place, Tile, World};

fn den(cubs: &[Cub]) -> Den {
    let mut den = Den::new();
    den.update(cubs, Duration::ZERO);
    den
}

/// A time by which everybody has arrived.
const LATER: u64 = 400;

fn actor(frame: &Frame, id: u64) -> &crate::sim::Actor {
    frame
        .actors
        .iter()
        .find(|actor| actor.id == id)
        .unwrap_or_else(|| panic!("lion {id} is drawn"))
}

#[test]
fn every_state_has_its_place_and_its_act() {
    let table = [
        (CubState::Editing, Place::Desks, Act::Type),
        (CubState::UsingTool, Place::Desks, Act::Type),
        (CubState::Thinking, Place::Desks, Act::Think),
        (CubState::Mystery, Place::Desks, Act::Wonder),
        (CubState::Reading, Place::Shelf, Act::Browse),
        (CubState::Searching, Place::Shelf, Act::Rummage),
        (CubState::Running, Place::Rack, Act::Operate),
        (CubState::Web, Place::Lookout, Act::Peer),
        (CubState::Planning, Place::Board, Act::Plan),
        (CubState::Delegating, Place::Watch, Act::Guard),
        (CubState::WaitingForUser, Place::Entrance, Act::Wait),
        (CubState::NeedsPermission, Place::Entrance, Act::Stare),
        (CubState::Idle, Place::Sun, Act::Lounge),
        (CubState::Asleep, Place::Desks, Act::Sleep),
    ];
    for (state, place, act) in table {
        assert_eq!(place_of(state, false, false), place, "{state:?}");
        assert_eq!(act_of(state, false), act, "{state:?}");
    }
    // A little one does its work at the small table, whatever the tool.
    for state in [
        CubState::Editing,
        CubState::Reading,
        CubState::Running,
        CubState::Web,
    ] {
        assert_eq!(place_of(state, true, false), Place::Bench);
        assert_eq!(act_of(state, true), Act::Type);
    }
    // A lion sleeps at its own desk; the lounge is for the idle.
    assert_eq!(wanted(CubState::Asleep, false), None);
    assert_eq!(wanted(CubState::Idle, false), Some(Place::Sun));
    assert_eq!(wanted(CubState::Idle, true), Some(Place::Sun));
    assert_eq!(
        wanted(CubState::Reading, true),
        None,
        "a little one works at its desk"
    );
}

#[test]
fn a_lion_walks_in_by_the_door_tile_by_tile_and_then_does_its_work() {
    let den = den(&[cub(1, "moss", CubState::Editing)]);
    let world = den.world();
    let first = den.frame(Duration::ZERO);
    let moss = actor(&first, 1);
    assert_eq!(
        (moss.x, moss.y),
        (world.outside().x * TILE, world.outside().y * TILE)
    );

    let mut last = (moss.x, moss.y);
    let mut walked = false;
    let mut arrived_at = None;
    for tick in 1..LATER {
        let frame = den.frame(at(tick));
        let moss = actor(&frame, 1);
        let (dx, dy) = ((moss.x - last.0).abs(), (moss.y - last.1).abs());
        assert!(
            dx + dy <= TILE / STEP_TICKS as i32,
            "tick {tick}: a jump of {dx},{dy}"
        );
        assert!(dx == 0 || dy == 0, "tick {tick}: it walks along the grid");
        if moss.look.act == Act::Walk {
            walked = true;
            assert_eq!(frame.wake, Wake::At(at(tick + 1)), "walking: the next tick");
        } else if arrived_at.is_none() && walked {
            arrived_at = Some(tick);
        }
        last = (moss.x, moss.y);
    }
    assert!(walked);
    let settled = den.frame(at(LATER));
    let moss = actor(&settled, 1);
    let desk = world.spots(Place::Desks)[0];
    assert_eq!(moss.tile, desk.tile);
    assert_eq!((moss.x, moss.y), (desk.tile.x * TILE, desk.tile.y * TILE));
    assert_eq!(moss.look.act, Act::Type);
    assert!(moss.look.seat, "it sits at its desk");
    assert_eq!(moss.look.facing, Facing::Up, "facing its computer");
    assert!(arrived_at.unwrap() < 120);
}

#[test]
fn the_den_at_a_time_is_the_same_however_often_it_is_asked() {
    let cubs = [
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Running),
    ];
    let (a, b) = (den(&cubs), den(&cubs));
    for tick in [0, 3, 17, 60, 61, 200] {
        assert_eq!(a.frame(at(tick)), b.frame(at(tick)));
        assert_eq!(a.frame(at(tick)), a.frame(at(tick)));
    }
    // Within a tick nothing changes.
    assert_eq!(
        a.frame(at(17)).actors,
        a.frame(at(17) + Duration::from_millis(99)).actors
    );
    assert_eq!(tick_of(time_of(42)), 42);
}

#[test]
fn a_change_of_state_sends_the_lion_to_its_new_place_from_where_it_is() {
    let mut den = den(&[cub(1, "moss", CubState::Editing)]);
    den.update(&[cub(1, "moss", CubState::Running)], at(LATER));
    let before = den.frame(at(LATER));
    let desk = den.world().spots(Place::Desks)[0].tile;
    assert_eq!(actor(&before, 1).tile, desk, "it starts from its desk");
    let on_the_way = den.frame(at(LATER + 6));
    assert_eq!(actor(&on_the_way, 1).look.act, Act::Walk);
    let there = den.frame(at(2 * LATER));
    let moss = actor(&there, 1);
    assert_eq!(moss.tile, den.world().spots(Place::Rack)[0].tile);
    assert_eq!(moss.look.act, Act::Operate);
    assert_eq!(moss.look.facing, Facing::Up);
    assert!(!moss.look.seat);
}

#[test]
fn a_lion_turned_around_mid_step_finishes_the_step_first() {
    let mut den = den(&[cub(1, "moss", CubState::Editing)]);
    // Two ticks into a step.
    let turn = 4 * STEP_TICKS + 2;
    let before = den.frame(at(turn));
    den.update(&[cub(1, "moss", CubState::WaitingForUser)], at(turn));
    let after = den.frame(at(turn));
    assert_eq!(
        (actor(&before, 1).x, actor(&before, 1).y),
        (actor(&after, 1).x, actor(&after, 1).y),
        "it does not jump"
    );
    let mut last = (actor(&after, 1).x, actor(&after, 1).y);
    for tick in turn + 1..turn + 80 {
        let frame = den.frame(at(tick));
        let moss = actor(&frame, 1);
        assert!(
            (moss.x - last.0).abs() + (moss.y - last.1).abs() <= 4,
            "tick {tick}"
        );
        last = (moss.x, moss.y);
    }
    let rug = den.world().spots(Place::Entrance)[0].tile;
    assert_eq!(actor(&den.frame(at(turn + 80)), 1).tile, rug);
}

#[test]
fn what_floats_over_a_lion_says_its_state() {
    let table = [
        (CubState::WaitingForUser, Some(Bubble::Bang)),
        (CubState::NeedsPermission, Some(Bubble::Urgent)),
        (CubState::Thinking, Some(Bubble::Thought)),
        (CubState::Mystery, Some(Bubble::Question)),
        (CubState::Asleep, Some(Bubble::Zzz)),
        (CubState::Fainted, Some(Bubble::Cross)),
        (CubState::Editing, None),
        (CubState::Idle, None),
    ];
    for (state, bubble) in table {
        let den = den(&[cub(1, "moss", state)]);
        let frame = den.frame(at(LATER));
        assert_eq!(actor(&frame, 1).look.bubble, bubble, "{state:?}");
    }
    // What is urgent shows on the way too; the rest only once there.
    let urgent = den(&[cub(1, "moss", CubState::NeedsPermission)]);
    assert_eq!(
        actor(&urgent.frame(at(5)), 1).look.bubble,
        Some(Bubble::Urgent)
    );
    let waiting = den(&[cub(1, "moss", CubState::WaitingForUser)]);
    assert_eq!(actor(&waiting.frame(at(5)), 1).look.bubble, None);
}

#[test]
fn nobody_shares_a_tile_in_a_full_den() {
    let mut cubs: Vec<Cub> = (0..16)
        .map(|index| cub(index + 1, "lion", CubState::ALL[(index % 13) as usize]))
        .collect();
    for index in 0..6 {
        cubs.push(little(100 + index, index + 1, CubState::Editing));
    }
    for room in ["office", "server-room", "open-plan", "nook"] {
        let (cols, rows) = (room, "");
        let mut den = crate::testing::den_in(room);
        den.update(&cubs, Duration::ZERO);
        let frame = den.frame(at(3 * LATER));
        assert_eq!(frame.actors.len(), cubs.len());
        let tiles: HashSet<Tile> = frame.actors.iter().map(|actor| actor.tile).collect();
        assert_eq!(
            tiles.len(),
            cubs.len(),
            "{cols}x{rows}: two lions on a tile"
        );
        for actor in &frame.actors {
            assert_ne!(
                actor.look.act,
                Act::Walk,
                "{cols}x{rows}: everybody has arrived"
            );
            assert!(matches!(
                den.world().ground(actor.tile),
                Ground::Floor | Ground::Seat
            ));
        }
    }
}

#[test]
fn a_little_one_hatches_in_the_nest_and_goes_to_the_small_table() {
    let mut den = den(&[cub(1, "moss", CubState::Editing)]);
    let born = LATER;
    den.update(
        &[
            cub(1, "moss", CubState::Delegating),
            little(9, 1, CubState::Editing),
        ],
        at(born),
    );
    let nest = den.world().spots(Place::Nest)[0].tile;
    let egg = den.frame(at(born + 1));
    let small = actor(&egg, 9);
    assert_eq!(small.look.act, Act::Egg);
    assert_eq!(small.tile, nest);
    assert!(small.look.little);
    assert_eq!(small.look.bubble, None);
    assert_eq!(egg.wake, Wake::At(at(born + 2)), "an egg wobbles");
    let hatching = den.frame(at(born + EGG_TICKS + 1));
    assert_eq!(actor(&hatching, 9).look.act, Act::Hatch);
    let walking = den.frame(at(born + EGG_TICKS + HATCH_TICKS + 2));
    assert_eq!(actor(&walking, 9).look.act, Act::Walk);
    let working = den.frame(at(born + LATER));
    let small = actor(&working, 9);
    assert_eq!(small.tile, den.world().spots(Place::Bench)[0].tile);
    assert_eq!(small.look.act, Act::Type);
    assert!(small.look.seat && small.look.little);
    // Its parent watches the nest meanwhile.
    let moss = actor(&working, 1);
    assert_eq!(moss.tile, den.world().spots(Place::Watch)[0].tile);
    assert_eq!(moss.look.act, Act::Guard);
}

#[test]
fn a_lion_that_is_gone_or_no_longer_listed_walks_out_and_is_forgotten() {
    for leave in [
        vec![
            cub(1, "moss", CubState::Gone),
            cub(2, "fern", CubState::Idle),
        ],
        vec![cub(2, "fern", CubState::Idle)],
    ] {
        let mut den = den(&[
            cub(1, "moss", CubState::Editing),
            cub(2, "fern", CubState::Idle),
        ]);
        den.update(&leave, at(LATER));
        assert_eq!(den.order(), vec![2], "it is off the roster at once");
        let leaving = den.frame(at(LATER + 5));
        assert_eq!(actor(&leaving, 1).look.act, Act::Walk);
        let gone = den.frame(at(2 * LATER));
        assert!(gone.actors.iter().all(|actor| actor.id != 1));
        // Told again later, the den has forgotten it: it would walk in anew.
        den.update(&leave, at(2 * LATER));
        assert_eq!(den.frame(at(2 * LATER)).actors.len(), 1);
    }
}

#[test]
fn a_lion_faints_where_it_stands_and_then_nothing_of_it_moves() {
    let mut den = den(&[cub(1, "moss", CubState::Running)]);
    den.update(&[cub(1, "moss", CubState::Fainted)], at(12));
    let (down, later) = (den.frame(at(12 + STEP_TICKS)), den.frame(at(LATER)));
    let moss = actor(&later, 1);
    assert_eq!(moss.look.act, Act::Faint);
    assert_eq!(moss.look.bubble, Some(Bubble::Cross));
    assert_eq!(
        (actor(&down, 1).x, actor(&down, 1).y),
        (moss.x, moss.y),
        "it went no farther than the end of its step"
    );
    assert_ne!(moss.tile, den.world().spots(Place::Rack)[0].tile);
    assert_eq!(later.wake, Wake::Never, "a den of fainted lions is still");
}

#[test]
fn an_empty_den_asks_for_nothing_once_it_has_said_so() {
    let den = den(&[]);
    let frame = den.frame(at(LATER));
    assert!(frame.actors.is_empty());
    assert_eq!(frame.said.rows, vec!["The den is quiet. Too quiet."]);
    assert_eq!(frame.wake, Wake::Never);
}

#[test]
fn settled_lions_wake_the_den_at_the_pace_of_their_pose() {
    let typing = den(&[cub(1, "moss", CubState::Editing)]);
    let asleep = den(&[cub(1, "moss", CubState::Asleep)]);
    let pace = |den: &Den| {
        let mut changes = 0;
        let mut last = den.frame(at(LATER)).actors;
        for tick in LATER + 1..LATER + 101 {
            let now = den.frame(at(tick)).actors;
            if now != last {
                changes += 1;
            }
            last = now;
        }
        changes
    };
    assert!(
        pace(&typing) > 3 * pace(&asleep),
        "typing is livelier than sleep"
    );
    assert!(
        pace(&asleep) <= 12,
        "a sleeping den redraws about once a second"
    );
    // The wake is never later than the next change of the picture.
    for den in [&typing, &asleep] {
        let mut tick = LATER;
        for _ in 0..40 {
            let frame = den.frame(at(tick));
            let Wake::At(next) = frame.wake else {
                panic!("a lion that breathes is not still");
            };
            let next = tick_of(next);
            assert!(next > tick);
            for between in tick + 1..next {
                assert_eq!(
                    den.frame(at(between)).actors,
                    frame.actors,
                    "tick {between}"
                );
            }
            tick = next;
        }
    }
}

#[test]
fn with_reduced_motion_lions_are_put_on_their_spot_in_a_still_pose() {
    let cubs = [
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::NeedsPermission),
        little(9, 1, CubState::Editing),
    ];
    let moving = den(&cubs);
    let mut still = Den::new();
    still.set_reduced_motion(true, Duration::ZERO);
    still.update(&cubs, Duration::ZERO);
    let (now, settled) = (still.frame(Duration::ZERO), moving.frame(at(LATER)));
    for id in [1, 2, 9] {
        let (a, b) = (actor(&now, id), actor(&settled, id));
        // The same information: the same place, act and bubble, at once.
        assert_eq!((a.tile, a.x, a.y), (b.tile, b.x, b.y), "lion {id}");
        assert_eq!(
            (a.look.act, a.look.bubble, a.look.seat),
            (b.look.act, b.look.bubble, b.look.seat)
        );
        assert_eq!(a.look.beat, 0, "a still pose");
    }
    assert_eq!(now.actors, still.frame(at(LATER)).actors, "nothing moves");
    // Leaving is at once too.
    still.update(&[cub(1, "moss", CubState::Editing)], at(5));
    assert_eq!(still.frame(at(5)).actors.len(), 1);
    // Turned on in a den that was moving, it puts everybody in place.
    let mut den = moving;
    den.set_reduced_motion(true, at(3));
    assert_eq!(actor(&den.frame(at(3)), 1).look.act, Act::Type);
}

#[test]
fn another_room_puts_everybody_on_a_spot_of_it_at_once() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Running),
    ]);
    let room = crate::prefabs::prefab("open-plan").unwrap().layout;
    den.set_layout(&room, false, at(10));
    let frame = den.frame(at(10));
    assert_eq!((den.world().cols, den.world().rows), (26, 15));
    assert_eq!(den.layout(), &room);
    assert_eq!(
        actor(&frame, 1).tile,
        den.world().spots(Place::Desks)[0].tile
    );
    assert_eq!(
        actor(&frame, 2).tile,
        den.world().spots(Place::Rack)[0].tile
    );
    assert_eq!(actor(&frame, 2).look.act, Act::Operate);
    // The same room again changes nothing.
    den.update(
        &[
            cub(1, "moss", CubState::Web),
            cub(2, "fern", CubState::Running),
        ],
        at(20),
    );
    den.set_layout(&room, false, at(22));
    assert_eq!(actor(&den.frame(at(22)), 1).look.act, Act::Walk);
}

#[test]
fn when_the_furniture_moves_under_them_the_lions_walk_to_where_their_seats_went() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Reading),
    ]);
    let before = den.frame(at(LATER));
    let seat = actor(&before, 1).tile;
    // Its desk, with its computers and its benches, goes two tiles down.
    let mut room = den.layout().clone();
    for piece in &mut room.items {
        if matches!(piece.id.as_str(), "desk" | "pc" | "bench") && piece.x <= 4 && piece.y <= 6 {
            piece.y += 1;
        }
    }
    den.set_layout(&room, true, at(LATER));
    let moving = den.frame(at(LATER + 2));
    assert_eq!(
        actor(&moving, 1).look.act,
        Act::Walk,
        "it walks, it does not jump"
    );
    let after = den.frame(at(2 * LATER));
    let moss = actor(&after, 1);
    assert_eq!(moss.tile, Tile::new(seat.x, seat.y + 1));
    assert_eq!(moss.look.act, Act::Type);
    assert!(moss.look.seat);
    // The reader did not have to move.
    assert_eq!(actor(&after, 2).tile, actor(&before, 2).tile);
}

#[test]
fn a_room_with_nothing_in_it_still_holds_every_lion() {
    let mut den = Den::new();
    den.set_layout(
        &crate::layout::DenLayout::empty("Bare", 9, 8),
        false,
        Duration::ZERO,
    );
    let cubs: Vec<Cub> = (0..13)
        .map(|index| cub(index + 1, "lion", CubState::ALL[index as usize]))
        .chain([little(100, 1, CubState::Editing)])
        .collect();
    den.update(&cubs, Duration::ZERO);
    let frame = den.frame(at(3 * LATER));
    assert_eq!(frame.actors.len(), cubs.len());
    let tiles: HashSet<Tile> = frame.actors.iter().map(|actor| actor.tile).collect();
    assert_eq!(tiles.len(), cubs.len(), "nobody shares a tile");
    for actor in &frame.actors {
        assert!(!actor.look.seat, "there is no seat");
        assert_eq!(den.world().ground(actor.tile), Ground::Floor);
    }
    assert_eq!(
        den.world().missing().len(),
        9,
        "every place is missing, and it works"
    );
}

#[test]
fn the_roster_lists_each_lion_in_the_order_it_came_with_its_little_ones_after_it() {
    let mut den = den(&[
        cub(1, "moss", CubState::Delegating),
        cub(2, "fern", CubState::Running),
    ]);
    den.update(
        &[
            cub(1, "moss", CubState::Delegating),
            cub(2, "fern", CubState::Mystery),
            little(9, 1, CubState::Reading),
            cub(3, "ash", CubState::Asleep),
        ],
        at(5),
    );
    assert_eq!(den.order(), vec![1, 9, 2, 3]);
    let roster = den.roster();
    let rows: Vec<(&str, &str, bool)> = roster
        .iter()
        .map(|entry| (entry.name.as_str(), entry.tag, entry.little))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("moss", "EGG", false),
            ("explore", "READ", true),
            ("fern", "???", false),
            ("ash", "ZZZ", false)
        ]
    );
    assert_eq!(roster[0].level, 3);
    assert!(roster[2].status.is_some() && roster[0].status.is_none());
    assert!(roster.iter().all(|entry| !entry.needs));
}

#[test]
fn those_that_need_the_user_come_first_the_most_pressing_and_the_longest_waiting_ahead() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::WaitingForUser),
        cub(3, "ash", CubState::Fainted),
    ]);
    assert_eq!(den.needy(), vec![2, 3]);
    // Later: one more waits, one asks for a permission, and a little one
    // waits too, which is its parent's business.
    den.update(
        &[
            cub(1, "moss", CubState::Editing),
            cub(2, "fern", CubState::WaitingForUser),
            cub(3, "ash", CubState::Fainted),
            cub(4, "wren", CubState::WaitingForUser),
            cub(5, "pike", CubState::NeedsPermission),
            little(9, 1, CubState::WaitingForUser),
        ],
        at(LATER),
    );
    // The permission prompt, then those that wait, the longest first, then
    // the one that fainted.
    assert_eq!(den.needy(), vec![5, 2, 4, 3]);
    assert_eq!(den.order(), vec![5, 2, 4, 3, 1, 9]);
    let roster = den.roster();
    let needs: Vec<bool> = roster.iter().map(|entry| entry.needs).collect();
    assert_eq!(needs, [true, true, true, true, false, false]);
    // The key walks them in that order, around the end, from anywhere.
    assert_eq!(den.select_needy(), Some(5));
    assert_eq!(den.select_needy(), Some(2));
    assert_eq!(den.select_needy(), Some(4));
    assert_eq!(den.select_needy(), Some(3));
    assert_eq!(den.select_needy(), Some(5), "around the end");
    den.select(Some(1));
    assert_eq!(den.select_needy(), Some(5), "from one that needs nothing");
    // From a little one, as from its parent.
    den.select(Some(9));
    assert_eq!(den.select_needy(), Some(5));
    // Once it works again it is no longer listed, and the rest keep their order.
    den.update(
        &[
            cub(1, "moss", CubState::Editing),
            cub(2, "fern", CubState::Thinking),
            cub(3, "ash", CubState::Fainted),
            cub(4, "wren", CubState::WaitingForUser),
            cub(5, "pike", CubState::Running),
        ],
        at(LATER * 2),
    );
    assert_eq!(den.needy(), vec![4, 3]);
    // Nobody needs the user: the selection stays where it is.
    let mut calm = den_of(&[cub(1, "moss", CubState::Editing)]);
    calm.select(Some(1));
    assert_eq!(calm.select_needy(), None);
    assert_eq!(calm.selected(), Some(1));
}

fn den_of(cubs: &[crate::model::Cub]) -> Den {
    den(cubs)
}

#[test]
fn the_keyboard_walks_the_roster_around_its_ends() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Idle),
        cub(3, "ash", CubState::Asleep),
    ]);
    assert_eq!(den.selected(), None);
    assert_eq!(den.select_by(1), Some(1));
    assert_eq!(den.select_by(1), Some(2));
    assert_eq!(den.select_by(1), Some(3));
    assert_eq!(den.select_by(1), Some(1), "around the end");
    assert_eq!(den.select_by(-1), Some(3), "and back");
    den.select(None);
    assert_eq!(den.select_by(-1), Some(3), "from nobody, back is the last");
    den.select(Some(77));
    assert_eq!(den.selected(), None, "nobody has that id");
    den.select(Some(2));
    assert!(actor(&den.frame(at(LATER)), 2).selected);
    assert!(roster_selected(&den, 2));
    // The selected lion goes home: nobody is selected.
    den.update(&[cub(1, "moss", CubState::Editing)], at(LATER));
    assert_eq!(den.selected(), None);
    assert_eq!(Den::new().select_by(1), None);
}

fn roster_selected(den: &Den, id: u64) -> bool {
    den.roster()
        .iter()
        .any(|entry| entry.id == id && entry.selected)
}

#[test]
fn the_pointer_finds_the_lion_it_is_on() {
    let mut den = den(&[
        cub(1, "moss", CubState::WaitingForUser),
        cub(2, "fern", CubState::Running),
    ]);
    let frame = den.frame(at(LATER));
    let moss = actor(&frame, 1);
    let (left, top, w, h) = moss.bounds();
    assert!(h > TILE, "a standing lion is taller than its tile");
    assert_eq!(den.cub_at(left + w / 2, top + h / 2, at(LATER)), Some(1));
    assert_eq!(
        den.cub_at(left + w / 2, top + 1, at(LATER)),
        Some(1),
        "its head"
    );
    assert_eq!(den.cub_at(left + w + 20, top, at(LATER)), None);
    assert_eq!(den.cub_at(-5, -5, at(LATER)), None);
    assert!(den.hover(Some(1)));
    assert!(!den.hover(Some(1)), "the same lion again changes nothing");
    assert!(actor(&den.frame(at(LATER)), 1).hovered);
    assert_eq!(den.hovered(), Some(1));
}

#[test]
fn the_truth_card_says_the_plain_fact() {
    let mut moss = cub(1, "moss", CubState::Running);
    moss.detail = Some("cargo test -p leon-den -- --nocapture".to_owned());
    moss.level = 12;
    let mut small = little(9, 1, CubState::Mystery);
    small.mystery = true;
    small.detail = Some("   ".to_owned());
    let den = den(&[moss, small]);
    let card = den.truth(1).unwrap();
    assert_eq!((card.name.as_str(), card.level), ("moss", 12));
    assert_eq!(card.label, "Running a command");
    assert_eq!(
        card.detail.as_deref(),
        Some("cargo test -p leon-den -- --nocapture")
    );
    assert_eq!((card.parent, card.mystery), (None, false));
    let card = den.truth(9).unwrap();
    assert_eq!(card.parent.as_deref(), Some("moss"));
    assert!(card.mystery);
    assert_eq!(card.detail, None, "a blank detail is no detail");
    assert!(card.status.is_some());
    assert!(den.truth(77).is_none());
}

#[test]
fn the_narrator_tells_what_happens_and_turns_its_templates() {
    let mut den = den(&[cub(1, "moss", CubState::Editing)]);
    let edit = Happening::new(
        1,
        "moss",
        Event::ToolStarted {
            kind: ToolKind::Edit,
            tool: "Edit".to_owned(),
            detail: Some("shell.rs".to_owned()),
        },
    );
    den.happen(&edit, at(10));
    let first = told(&den).pop().unwrap();
    den.happen(&edit, at(100));
    let second = told(&den).pop().unwrap();
    assert_eq!(den.feed().len(), 2);
    assert_ne!(
        first, second,
        "the same fact twice is not said the same way"
    );
    for said in [&first, &second] {
        assert!(
            said.starts_with("MOSS ") && said.contains("shell.rs"),
            "{said}"
        );
    }
    // A tool that ends without news says nothing.
    den.happen(
        &Happening::new(
            1,
            "moss",
            Event::ToolFinished {
                kind: ToolKind::Edit,
                ok: true,
            },
        ),
        at(200),
    );
    assert_eq!(told(&den), vec![first, second]);
    // The plain twin of a line is the fact with no joke.
    let entry = den.feed().entries().next().unwrap();
    assert_eq!(entry.item.plain, "Editing shell.rs");
    assert_eq!((entry.item.cub, entry.item.owner), (1, 1));
    assert_eq!(
        entry.item.tint,
        Some(cub(1, "moss", CubState::Idle).species.tint)
    );
}

#[test]
fn the_newest_line_of_the_narrator_is_typed_out_and_wakes_the_den_meanwhile() {
    let mut den = den(&[cub(1, "moss", CubState::Fainted)]);
    assert_eq!(den.frame(at(LATER)).wake, Wake::Never, "nothing moves");
    den.happen(&Happening::new(1, "moss", Event::FellAsleep), at(LATER));
    let total = "MOSS fell asleep.".chars().count();
    let frame = den.frame(at(LATER));
    let (entry, typed) = frame.typing.expect("it is being typed");
    assert_eq!(entry, den.feed().entries().next().unwrap().id);
    assert!(typed > 0 && typed < total);
    assert_eq!(
        frame.wake,
        Wake::At(at(LATER + 1)),
        "the next tick shows more"
    );
    let later = den.frame(at(LATER + 2));
    assert!(later.typing.unwrap().1 > typed);
    // Typed, it is whole, and nothing wakes the den for it.
    let done = den.frame(at(LATER + 20));
    assert_eq!((done.typing, done.wake), (None, Wake::Never));
    // What the agent says is not typed: it is there at once.
    den.speak(
        1,
        "moss",
        "A long answer, as it is in the transcript.",
        None,
        at(LATER + 30),
    );
    assert_eq!(den.frame(at(LATER + 30)).typing, None);
    // Nor is anything with reduced motion, or with the narrator off.
    den.happen(&Happening::new(1, "moss", Event::Joined), at(LATER + 40));
    assert!(den.frame(at(LATER + 40)).typing.is_some());
    den.set_reduced_motion(true, at(LATER + 40));
    assert_eq!(den.frame(at(LATER + 40)).typing, None);
    den.set_reduced_motion(false, at(LATER + 40));
    den.set_plain_status(true);
    assert_eq!(den.frame(at(LATER + 40)).typing, None);
}

#[test]
fn what_an_agent_says_goes_in_the_feed_as_it_said_it_under_its_lion() {
    let mut den = den(&[
        cub(1, "moss", CubState::WaitingForUser),
        little(9, 1, CubState::Reading),
    ]);
    den.set_wall_time(5_000);
    assert_eq!(den.wall_time(), Some(5_000));
    den.speak(1, "moss", "Done.\n\n* `a.rs`\n* `b.rs`", None, at(10));
    den.speak(9, "explore", "Nothing found.", Some(4_000), at(11));
    den.speak(1, "moss", "  \n ", None, at(12));
    let entries: Vec<_> = den.feed().entries().collect();
    assert_eq!(entries.len(), 2, "an empty message is none");
    assert_eq!(
        entries[0].item.text, "Done.\n\n* `a.rs`\n* `b.rs`",
        "nothing is rewritten"
    );
    assert_eq!(entries[0].item.kind, crate::feed::Kind::Speech);
    assert_eq!(
        entries[0].item.at,
        Some(5_000),
        "now, when the transcript says no time"
    );
    // A little one's words belong to the feed of the lion that sent it out.
    assert_eq!((entries[1].item.cub, entries[1].item.owner), (9, 1));
    assert_eq!(entries[1].item.at, Some(4_000));
    // The past of a session is put where its time says.
    den.remember(
        1,
        "moss",
        &[
            crate::feed::Past::Tool,
            crate::feed::Past::Speech("Earlier.".into(), Some(3_000)),
        ],
    );
    assert_eq!(
        told(&den),
        vec![
            "MOSS used a tool.",
            "Earlier.",
            "Done.  * `a.rs` * `b.rs`",
            "Nothing found."
        ]
    );
}

#[test]
fn a_command_that_ends_leaves_a_tick_or_a_cross_over_its_lion_for_a_moment() {
    let mut den = den(&[cub(1, "moss", CubState::Running)]);
    let ended = |ok| {
        Happening::new(
            1,
            "moss",
            Event::ToolFinished {
                kind: ToolKind::Run,
                ok,
            },
        )
    };
    den.happen(&ended(true), at(LATER));
    assert_eq!(
        actor(&den.frame(at(LATER + 3)), 1).look.bubble,
        Some(Bubble::Tick)
    );
    assert_eq!(actor(&den.frame(at(LATER + 40)), 1).look.bubble, None);
    den.happen(&ended(false), at(LATER + 50));
    assert_eq!(
        actor(&den.frame(at(LATER + 52)), 1).look.bubble,
        Some(Bubble::Cross)
    );
}

#[test]
fn when_nothing_new_is_said_the_box_speaks_of_the_den_as_a_whole() {
    // Busy: three at work.
    let mut busy = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Running),
        cub(3, "ash", CubState::Reading),
    ]);
    // What happens is told in the feed; the den's own line is another thing.
    busy.happen(&Happening::new(1, "moss", Event::Joined), at(0));
    let typed = at(20);
    assert_eq!(told(&busy), vec!["MOSS joined the pride."]);
    let quiet = typed + LINGER + Duration::from_secs(4);
    assert_eq!(
        busy.frame(quiet).said.rows,
        vec!["The pride is busy.", "You may go get coffee."]
    );
    // A lion that has been thinking for a long while.
    let thinking = den(&[cub(1, "moss", CubState::Thinking)]);
    assert!(thinking.frame(at(100)).said.rows.is_empty());
    let frame = thinking.frame(PONDER + Duration::from_secs(3));
    assert_eq!(
        frame.said.rows,
        vec!["MOSS is thinking very hard.", "Or napping."]
    );
    // The den wakes for it though nothing else moves.
    let wake = thinking.frame(at(100)).wake;
    assert!(matches!(wake, Wake::At(when) if when <= PONDER));
}

#[test]
fn with_the_narrator_off_the_box_says_the_state_of_the_den_in_plain_words() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::WaitingForUser),
        cub(3, "ash", CubState::NeedsPermission),
    ]);
    den.set_plain_status(true);
    den.happen(&Happening::new(3, "ash", Event::PermissionPrompt), at(5));
    let said = den.frame(at(50)).said;
    assert_eq!(
        said.rows,
        vec![
            "3 live sessions: 1 working, 1 waiting.",
            "1 needs permission."
        ]
    );
    assert!(
        said.urgent && said.complete,
        "what is urgent still carries its colour"
    );
    assert_eq!(said.next, None, "plain text is not typed");
    let mut empty = Den::new();
    empty.set_plain_status(true);
    assert_eq!(empty.frame(at(5)).said.rows, vec!["No live sessions."]);
    assert_eq!(empty.frame(at(5)).wake, Wake::Never);
    // The narrator back on: its own lines again.
    den.set_plain_status(false);
    den.happen(&Happening::new(1, "moss", Event::FellAsleep), at(60));
    assert_eq!(
        told(&den),
        vec![
            "A wild PERMISSION PROMPT appeared. ASH is staring at you.",
            "MOSS fell asleep."
        ],
        "what happened while it was off was kept, to be told plainly"
    );
    let plain: Vec<&str> = den
        .feed()
        .entries()
        .map(|entry| entry.item.plain.as_str())
        .collect();
    assert_eq!(
        plain,
        vec![
            "Needs your permission (inferred).",
            "Quiet for a long while."
        ]
    );
    assert!(den.frame(at(100)).said.rows != vec!["MOSS fell asleep."]);
}

/// Every lion of a den once everybody has arrived: its id, its state and
/// the tile it rests on.
fn resting(den: &Den) -> Vec<(u64, CubState, Tile)> {
    let frame = den.frame(at(LATER * 4));
    frame
        .actors
        .iter()
        .map(|actor| {
            let state = den
                .roster()
                .iter()
                .find(|row| row.id == actor.id)
                .map(|row| row.tag);
            let _ = state;
            (actor.id, actor.look.act, actor.tile)
        })
        .map(|(id, act, tile)| {
            let state = match act {
                Act::Sleep => CubState::Asleep,
                Act::Wait | Act::Stare => CubState::WaitingForUser,
                _ => CubState::Editing,
            };
            (id, state, tile)
        })
        .collect()
}

/// The mixes of states the placement is checked with: everybody the same,
/// and everybody different.
fn mixes() -> Vec<(&'static str, Vec<CubState>)> {
    let mut mixes: Vec<(&'static str, Vec<CubState>)> = vec![
        ("all asleep", vec![CubState::Asleep]),
        ("all waiting", vec![CubState::WaitingForUser]),
        ("all reading", vec![CubState::Reading]),
        ("all idle", vec![CubState::Idle]),
        ("all editing", vec![CubState::Editing]),
        ("all running", vec![CubState::Running]),
        ("all asking", vec![CubState::NeedsPermission]),
    ];
    let every: Vec<CubState> = CubState::ALL
        .into_iter()
        .filter(|state| *state != CubState::Gone)
        .collect();
    mixes.push(("mixed", every));
    mixes.push((
        "the owner's den",
        vec![
            CubState::Asleep,
            CubState::Asleep,
            CubState::Asleep,
            CubState::WaitingForUser,
            CubState::Asleep,
            CubState::Asleep,
            CubState::WaitingForUser,
            CubState::Asleep,
        ],
    ));
    mixes
}

fn rooms() -> Vec<(String, crate::layout::DenLayout)> {
    let mut rooms: Vec<(String, crate::layout::DenLayout)> = crate::prefabs::prefabs()
        .into_iter()
        .map(|prefab| (prefab.id.to_owned(), prefab.layout))
        .collect();
    // Rooms of the user's: nothing at all in one, no desk in another.
    rooms.push((
        "bare".to_owned(),
        crate::layout::DenLayout::empty("Bare", 12, 10),
    ));
    let mut no_desk = crate::prefabs::default_layout();
    no_desk
        .items
        .retain(|piece| !matches!(piece.id.as_str(), "desk" | "pc" | "bench" | "small_table"));
    rooms.push(("no desks".to_owned(), no_desk.repaired().layout));
    rooms.push((
        "smallest".to_owned(),
        crate::layout::DenLayout::empty("Small", 8, 8),
    ));
    rooms
}

fn crowd(count: usize, states: &[CubState]) -> Vec<crate::model::Cub> {
    let mut cubs = Vec::new();
    for index in 0..count {
        let state = states[index % states.len()];
        cubs.push(cub(index as u64 + 1, &format!("lion {index}"), state));
        // Every fifth has a little one.
        if index % 5 == 4 {
            cubs.push(little(
                1000 + index as u64,
                index as u64 + 1,
                states[(index + 3) % states.len()],
            ));
        }
    }
    cubs
}

#[test]
fn no_two_lions_rest_on_one_tile_whatever_the_room_the_number_and_the_states() {
    for (room, layout) in rooms() {
        let world = World::build(&layout);
        let floor = (0..world.rows)
            .flat_map(|y| (0..world.cols).map(move |x| Tile::new(x, y)))
            .filter(|tile| matches!(world.ground(*tile), Ground::Floor | Ground::Seat))
            .count();
        for (mix, states) in mixes() {
            for count in 1..=30usize {
                let mut den = Den::new();
                den.set_layout(&layout, false, at(0));
                let cubs = crowd(count, &states);
                den.update(&cubs, at(0));
                let lions = resting(&den);
                assert_eq!(
                    lions.len(),
                    cubs.len(),
                    "{room}, {mix}, {count}: everybody is there"
                );
                let what = format!("{room}, {mix}, {count} lions");
                let mut tiles = HashSet::new();
                for (id, _, tile) in &lions {
                    // In the room, on something a lion can be on.
                    assert!(
                        matches!(world.ground(*tile), Ground::Floor | Ground::Seat),
                        "{what}: lion {id} rests on {tile:?}"
                    );
                    assert!(
                        tile.x >= 1
                            && tile.x < world.cols - 1
                            && tile.y >= 2
                            && tile.y < world.rows
                    );
                    if cubs.len() < floor {
                        assert!(tiles.insert(*tile), "{what}: two lions on {tile:?}");
                    }
                }
                // No place holds more than it has spots.
                for place in Place::ALL {
                    let spots: HashSet<Tile> =
                        world.spots(place).iter().map(|spot| spot.tile).collect();
                    let there = lions
                        .iter()
                        .filter(|(_, _, tile)| spots.contains(tile))
                        .count();
                    assert!(there <= spots.len(), "{what}: {place:?}");
                }
                let entrance: HashSet<Tile> = world
                    .spots(Place::Entrance)
                    .iter()
                    .map(|spot| spot.tile)
                    .collect();
                let waiting = lions
                    .iter()
                    .filter(|(_, state, tile)| {
                        *state == CubState::WaitingForUser && entrance.contains(tile)
                    })
                    .count();
                // (In a room with more lions than bare floor, the last ones
                // have no other tile to call their own.)
                let open = (0..world.rows)
                    .flat_map(|y| (0..world.cols).map(move |x| Tile::new(x, y)))
                    .filter(|tile| {
                        world.walkable(*tile) && !world.spot_at(*tile) && *tile != world.door
                    })
                    .count();
                if cubs.len() <= open {
                    assert!(waiting <= ENTRANCE_LINE, "{what}: {waiting} in the line");
                }
                // The same again a moment later: nobody moved.
                den.update(&cubs, at(LATER * 5));
                let again: Vec<Tile> = {
                    let frame = den.frame(at(LATER * 9));
                    frame.actors.iter().map(|actor| actor.tile).collect()
                };
                let before: Vec<Tile> = {
                    let mut tiles: Vec<(u64, Tile)> =
                        lions.iter().map(|(id, _, tile)| (*id, *tile)).collect();
                    tiles.sort();
                    let frame = den.frame(at(LATER * 9));
                    frame
                        .actors
                        .iter()
                        .map(|actor| tiles.iter().find(|(id, _)| *id == actor.id).unwrap().1)
                        .collect()
                };
                assert_eq!(again, before, "{what}: a second update moves nobody");
            }
        }
    }
}

#[test]
fn lions_without_a_seat_stand_apart_on_bare_floor_near_the_desks() {
    for (room, layout) in rooms() {
        let world = World::build(&layout);
        let seats = world.spots(Place::Desks).len();
        // As many more lions than seats as the room can keep apart.
        for extra in [1usize, 3, 6] {
            let count = seats + extra;
            let mut den = Den::new();
            den.set_layout(&layout, false, at(0));
            let cubs: Vec<_> = (0..count)
                .map(|index| cub(index as u64 + 1, "lion", CubState::Asleep))
                .collect();
            den.update(&cubs, at(0));
            let lions = resting(&den);
            let standing: Vec<Tile> = lions
                .iter()
                .map(|(_, _, tile)| *tile)
                .filter(|tile| world.ground(*tile) == Ground::Floor)
                .collect();
            assert_eq!(standing.len(), extra, "{room}: the rest have a seat");
            for (index, a) in standing.iter().enumerate() {
                assert!(!world.spot_at(*a), "{room}: {a:?} is a place's spot");
                assert_ne!(*a, world.door, "{room}: the way in stays free");
                for b in &standing[index + 1..] {
                    let (dx, dy) = ((a.x - b.x).abs(), (a.y - b.y).abs());
                    assert!(
                        dx.max(dy) >= 2,
                        "{room} with {extra} standing: {a:?} and {b:?} touch"
                    );
                }
            }
        }
    }
}

#[test]
fn every_lion_has_a_desk_of_its_own_and_keeps_it() {
    let mut den = den(&[
        cub(1, "moss", CubState::Editing),
        cub(2, "fern", CubState::Reading),
        cub(3, "ash", CubState::Asleep),
        little(9, 2, CubState::Reading),
    ]);
    let world = den.world().clone();
    let tile = |den: &Den, id: u64| actor(&den.frame(at(LATER * 40)), id).tile;
    let desks: Vec<Tile> = world.spots(Place::Desks).iter().map(|s| s.tile).collect();
    // Each joined at the first free desk; the one that reads is at the shelf.
    assert_eq!(tile(&den, 1), desks[0]);
    assert_eq!(tile(&den, 2), world.spots(Place::Shelf)[0].tile);
    assert_eq!(
        tile(&den, 3),
        desks[2],
        "asleep at its own desk, not on the sofa"
    );
    assert!(world
        .spots(Place::Bench)
        .iter()
        .any(|s| s.tile == tile(&den, 9)));
    // Back from the shelf, fern sits at the desk that was kept for it, though
    // others came and went meanwhile.
    den.update(
        &[
            cub(2, "fern", CubState::Editing),
            cub(3, "ash", CubState::Asleep),
            cub(4, "wren", CubState::Editing),
        ],
        at(LATER * 5),
    );
    assert_eq!(tile(&den, 2), desks[1]);
    assert_eq!(tile(&den, 3), desks[2]);
    assert_eq!(
        tile(&den, 4),
        desks[0],
        "the desk moss left is the first free"
    );
    // Moving furniture that is not theirs moves nobody.
    let mut layout = den.layout().clone();
    layout.items.retain(|piece| piece.id != "plant");
    den.set_layout(&layout, false, at(LATER * 6));
    assert_eq!(
        (tile(&den, 2), tile(&den, 3), tile(&den, 4)),
        (desks[1], desks[2], desks[0])
    );
    // A seat that disappears sends only its lion to another.
    let gone = desks[2];
    layout
        .items
        .retain(|piece| !(piece.id == "bench" && Tile::new(piece.x, piece.y) == gone));
    den.set_layout(&layout, false, at(LATER * 7));
    assert_eq!((tile(&den, 2), tile(&den, 4)), (desks[1], desks[0]));
    assert_ne!(tile(&den, 3), gone);
    assert!(den
        .world()
        .spots(Place::Desks)
        .iter()
        .any(|s| s.tile == tile(&den, 3)));
}

#[test]
fn a_place_that_is_full_sends_nobody_more_and_the_rest_show_their_state_at_their_desk() {
    // Five wait: three in the line at the entrance, two at their desks,
    // each with its `!`.
    let waiting: Vec<_> = (1..=5)
        .map(|id| cub(id, "lion", CubState::WaitingForUser))
        .collect();
    let den = den(&waiting);
    let world = den.world();
    let frame = den.frame(at(LATER * 4));
    let entrance: HashSet<Tile> = world
        .spots(Place::Entrance)
        .iter()
        .map(|s| s.tile)
        .collect();
    let desks: HashSet<Tile> = world.spots(Place::Desks).iter().map(|s| s.tile).collect();
    let in_line: Vec<&crate::sim::Actor> = frame
        .actors
        .iter()
        .filter(|actor| entrance.contains(&actor.tile))
        .collect();
    assert_eq!(in_line.len(), ENTRANCE_LINE);
    // Nobody in the line stands straight behind another.
    for a in &in_line {
        for b in &in_line {
            assert!(
                a.id == b.id || a.tile.x != b.tile.x,
                "{:?} behind {:?}",
                a.tile,
                b.tile
            );
        }
    }
    for actor in &frame.actors {
        assert_eq!(
            actor.look.bubble,
            Some(Bubble::Bang),
            "everybody shows that it waits"
        );
        assert!(entrance.contains(&actor.tile) || desks.contains(&actor.tile));
    }
    // The office has one shelf spot... as many as it has: the readers beyond
    // them read at their desks, typing, and are still readers on the card.
    let readers: Vec<_> = (1..=6)
        .map(|id| cub(id, "lion", CubState::Reading))
        .collect();
    let den = self::den(&readers);
    let world = den.world();
    let shelf: HashSet<Tile> = world.spots(Place::Shelf).iter().map(|s| s.tile).collect();
    let frame = den.frame(at(LATER * 4));
    let at_shelf = frame
        .actors
        .iter()
        .filter(|a| shelf.contains(&a.tile))
        .count();
    assert_eq!(at_shelf, shelf.len());
    for actor in frame.actors.iter().filter(|a| !shelf.contains(&a.tile)) {
        assert_eq!(actor.look.act, Act::Type);
        assert!(actor.look.seat);
        assert_eq!(den.truth(actor.id).unwrap().label, "Reading a file");
    }
    // The lounge is a treat: as many idle lions as it has seats, the rest
    // idle at their desks. When a seat frees up nobody is moved for it.
    let idle: Vec<_> = (1..=6).map(|id| cub(id, "lion", CubState::Idle)).collect();
    let mut den = self::den(&idle);
    let sofa: HashSet<Tile> = den
        .world()
        .spots(Place::Sun)
        .iter()
        .map(|s| s.tile)
        .collect();
    let on_sofa = |den: &Den| {
        den.frame(at(LATER * 4))
            .actors
            .iter()
            .filter(|a| sofa.contains(&a.tile))
            .map(|a| a.id)
            .collect::<Vec<u64>>()
    };
    let before = on_sofa(&den);
    assert_eq!(before.len(), sofa.len().min(6));
    // One of them falls asleep where it is; one at a desk falls asleep there.
    let mut later = idle.clone();
    later[(before[0] - 1) as usize].state = CubState::Asleep;
    let at_desk = (1..=6).find(|id| !before.contains(id)).unwrap();
    later[(at_desk - 1) as usize].state = CubState::Asleep;
    den.update(&later, at(LATER * 5));
    let frame = den.frame(at(LATER * 9));
    assert!(
        sofa.contains(&actor(&frame, before[0]).tile),
        "it sleeps on its sofa"
    );
    assert!(
        !sofa.contains(&actor(&frame, at_desk).tile),
        "and that one at its desk"
    );
    assert_eq!(actor(&frame, at_desk).look.act, Act::Sleep);
}
