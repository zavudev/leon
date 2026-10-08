//! Tests of the room: its places come from what is in it, nobody shares a
//! tile while there is floor left, and every spot can be walked to.

use std::collections::HashSet;

use crate::layout::{DenLayout, Placed};
use crate::pose::{Facing, TILE};
use crate::prefabs::{prefabs, small_office};
use crate::world::{Ground, Place, Tile, World};

fn office() -> World {
    World::build(&small_office())
}

fn bare(cols: i32, rows: i32, pieces: &[Placed]) -> DenLayout {
    let mut room = DenLayout::empty("Test", cols, rows);
    room.items = pieces.to_vec();
    let loaded = room.repaired();
    assert_eq!(
        loaded.notes,
        Vec::<String>::new(),
        "the test's room is valid"
    );
    loaded.layout
}

#[test]
fn the_room_has_a_back_wall_two_side_walls_and_an_open_side() {
    for prefab in prefabs() {
        let world = World::build(&prefab.layout);
        let (cols, rows) = (world.cols, world.rows);
        assert!((0..cols).all(|x| world.ground(Tile::new(x, 0)) == Ground::Void));
        assert!((0..cols).all(|x| world.ground(Tile::new(x, 1)) == Ground::Wall));
        for y in 1..rows {
            assert_eq!(world.ground(Tile::new(0, y)), Ground::Wall);
            assert_eq!(world.ground(Tile::new(cols - 1, y)), Ground::Wall);
        }
        assert_eq!(world.door, Tile::new(cols / 2, rows - 1));
        assert_eq!(world.ground(world.door), Ground::Floor, "{}", prefab.id);
        assert_eq!(world.outside(), Tile::new(world.door.x, rows));
    }
}

#[test]
fn no_tile_is_the_spot_of_two_places_and_every_spot_can_be_stood_on() {
    for prefab in prefabs() {
        let world = World::build(&prefab.layout);
        let mut seen: HashSet<Tile> = HashSet::new();
        for place in Place::ALL {
            for spot in world.spots(place) {
                assert!(
                    seen.insert(spot.tile),
                    "{}: {:?} twice",
                    prefab.id,
                    spot.tile
                );
                assert!(
                    matches!(world.ground(spot.tile), Ground::Floor | Ground::Seat),
                    "{}: {place:?} at {:?}",
                    prefab.id,
                    spot.tile
                );
                assert_ne!(spot.tile, world.door, "the door stays free");
            }
        }
    }
}

#[test]
fn a_seat_that_faces_a_computer_is_a_work_seat_and_any_other_is_for_rest() {
    let room = bare(
        12,
        10,
        &[
            Placed::new("desk", 2, 3),
            Placed::new("pc", 2, 3),
            // A bench has no front: it looks at the table next to it.
            Placed::new("bench", 2, 5),
            // No computer over this one.
            Placed::new("bench", 4, 5),
            // A chair turned to the right, with a screen two tiles ahead.
            Placed::new("chair", 6, 7).turned(1),
            Placed::new("small_table", 7, 6),
            Placed::new("pc", 8, 6).turned(3),
            // A sofa looks at nothing.
            Placed::new("sofa", 2, 8),
        ],
    );
    let world = World::build(&room);
    let desks: Vec<(Tile, Facing)> = world
        .spots(Place::Desks)
        .iter()
        .map(|spot| (spot.tile, spot.facing))
        .collect();
    assert_eq!(
        desks,
        vec![
            (Tile::new(2, 5), Facing::Up),
            (Tile::new(6, 7), Facing::Right)
        ]
    );
    // Each has its own computer: the piece of the layout it faces.
    assert_eq!(world.computer_of(Tile::new(2, 5)), Some(1));
    assert_eq!(world.computer_of(Tile::new(6, 7)), Some(6));
    assert_eq!(world.computer_of(Tile::new(4, 5)), None);
    let rest: Vec<Tile> = world
        .spots(Place::Sun)
        .iter()
        .map(|spot| spot.tile)
        .collect();
    assert_eq!(
        rest,
        vec![Tile::new(4, 5), Tile::new(2, 8), Tile::new(3, 8)],
        "the other bench, and both seats of the sofa"
    );
    assert!(world
        .spots(Place::Sun)
        .iter()
        .all(|spot| world.ground(spot.tile) == Ground::Seat));
}

#[test]
fn a_little_seat_is_a_little_ones_and_without_one_they_borrow_a_desk() {
    let with = bare(
        12,
        10,
        &[
            Placed::new("desk", 2, 3),
            Placed::new("pc", 2, 3),
            Placed::new("pc", 4, 3),
            Placed::new("bench", 2, 5),
            Placed::new("wooden_bench", 4, 5),
        ],
    );
    let world = World::build(&with);
    assert_eq!(world.spots(Place::Desks).len(), 1);
    assert_eq!(world.spots(Place::Bench)[0].tile, Tile::new(4, 5));
    assert_eq!(
        world.claim(Place::Bench, &HashSet::new()).tile,
        Tile::new(4, 5)
    );

    let without = World::build(&small_office_without("wooden_bench"));
    assert!(without.spots(Place::Bench).is_empty());
    assert!(without.missing().contains(&Place::Bench));
    let borrowed = without.claim(Place::Bench, &HashSet::new());
    assert_eq!(borrowed, without.spots(Place::Desks)[0]);
}

fn small_office_without(id: &str) -> DenLayout {
    let mut room = small_office();
    room.items.retain(|piece| piece.id != id);
    room
}

#[test]
fn a_lion_stands_in_front_of_what_it_uses_looking_at_it() {
    let world = office();
    for (place, piece) in [
        (Place::Shelf, "double_bookshelf"),
        (Place::Board, "whiteboard"),
        (Place::Rack, "rack"),
    ] {
        let room = small_office();
        let piece = room.items.iter().find(|item| item.id == piece).unwrap();
        let spot = world.spots(place)[0];
        assert_eq!(spot.facing, Facing::Up, "{place:?}");
        assert_eq!(spot.tile.x, piece.x, "{place:?}: under its left tile");
        assert_eq!(world.ground(spot.tile), Ground::Floor);
        let (_, view) = piece.piece().unwrap();
        assert_eq!(
            spot.tile.y,
            (piece.y + view.h).max(2),
            "{place:?}: just under it"
        );
    }
    // A window and a telescope are both lookouts; the telescope stands
    // under the window here, so its tile is no spot.
    let lookout: Vec<Tile> = world.spots(Place::Lookout).iter().map(|s| s.tile).collect();
    assert_eq!(lookout, vec![Tile::new(7, 2), Tile::new(8, 3)]);
}

#[test]
fn the_rug_is_the_entrance_its_middle_first_and_no_two_side_by_side_first() {
    let world = office();
    let rug: Vec<Tile> = world
        .spots(Place::Entrance)
        .iter()
        .map(|s| s.tile)
        .collect();
    assert_eq!(
        rug,
        vec![
            Tile::new(7, 9),
            Tile::new(6, 10),
            Tile::new(8, 10),
            Tile::new(6, 9),
            Tile::new(8, 9)
        ],
        "the door's own tile is left free"
    );
    assert!(world
        .spots(Place::Entrance)
        .iter()
        .all(|spot| spot.facing == Facing::Down));
    // Beside the nests, looking at them.
    let watch = world.spots(Place::Watch)[0];
    assert_eq!(
        (watch.tile, watch.facing),
        (Tile::new(10, 10), Facing::Right)
    );
    assert_eq!(world.spots(Place::Nest).len(), 2);
}

#[test]
fn a_room_says_what_it_lacks_and_works_without_it() {
    let empty = World::build(&DenLayout::empty("Bare", 9, 8));
    assert_eq!(
        empty.missing(),
        vec![
            Place::Desks,
            Place::Bench,
            Place::Shelf,
            Place::Lookout,
            Place::Board,
            Place::Rack,
            Place::Sun,
            Place::Entrance,
            Place::Nest
        ]
    );
    // Sent to a place that is not there, a lion stands on free floor, and
    // the next one elsewhere.
    let mut taken = HashSet::new();
    for place in Place::ALL {
        let spot = empty.claim(place, &taken);
        assert_eq!(empty.ground(spot.tile), Ground::Floor, "{place:?}");
        assert!(taken.insert(spot.tile), "{place:?} shares a tile");
        assert_ne!(spot.tile, empty.door);
    }
    assert_eq!(office().missing(), Vec::<Place>::new());
    let no_shelf = World::build(&small_office_without("double_bookshelf"));
    assert_eq!(no_shelf.missing(), vec![Place::Shelf]);
}

#[test]
fn every_spot_is_walked_to_from_outside_step_by_step_and_never_over_furniture() {
    for prefab in prefabs() {
        let world = World::build(&prefab.layout);
        for place in Place::ALL {
            for spot in world.spots(place) {
                let path = world.path(world.outside(), spot.tile);
                assert_eq!(path[0], world.outside());
                assert_eq!(path[1], world.door, "in by the door");
                assert_eq!(path.last(), Some(&spot.tile));
                for pair in path.windows(2) {
                    assert_eq!(
                        pair[0].distance(pair[1]),
                        1,
                        "{}: no way to {place:?} at {:?}",
                        prefab.id,
                        spot.tile
                    );
                }
                for tile in &path[1..path.len() - 1] {
                    assert!(
                        matches!(world.ground(*tile), Ground::Floor | Ground::Seat),
                        "{}: over {tile:?}",
                        prefab.id
                    );
                }
                let back = world.path(spot.tile, world.outside());
                assert_eq!(back.len(), path.len(), "{} {place:?}", prefab.id);
            }
        }
    }
}

#[test]
fn a_seat_nobody_can_reach_is_jumped_to_rather_than_lost() {
    // A bench walled in by plants on every side.
    let room = bare(
        10,
        10,
        &[
            Placed::new("bench", 4, 5),
            Placed::new("pot", 3, 5),
            Placed::new("pot", 5, 5),
            Placed::new("pot", 4, 4),
            Placed::new("pot", 4, 6),
        ],
    );
    let world = World::build(&room);
    let path = world.path(world.door, Tile::new(4, 5));
    assert_eq!(path, vec![world.door, Tile::new(4, 5)]);
}

#[test]
fn the_same_walk_is_always_the_same_and_a_walk_to_here_is_no_walk() {
    let world = office();
    let to = world.spots(Place::Rack)[0].tile;
    assert_eq!(world.path(world.door, to), world.path(world.door, to));
    assert_eq!(world.path(to, to), vec![to]);
    // From one seat to another: neither is walked over, both are ends.
    let (a, b) = (
        world.spots(Place::Desks)[0].tile,
        world.spots(Place::Sun)[0].tile,
    );
    let path = world.path(a, b);
    assert_eq!((path[0], *path.last().unwrap()), (a, b));
    assert!(path[1..path.len() - 1]
        .iter()
        .all(|tile| world.walkable(*tile)));
}

#[test]
fn a_full_place_overflows_onto_free_floor_and_nobody_shares_a_tile() {
    for (prefab, lions) in [
        ("office", 16),
        ("server-room", 22),
        ("open-plan", 40),
        ("nook", 8),
    ] {
        let world = World::build(&crate::prefabs::prefab(prefab).unwrap().layout);
        for place in [
            Place::Desks,
            Place::Rack,
            Place::Entrance,
            Place::Sun,
            Place::Bench,
        ] {
            let mut taken: HashSet<Tile> = HashSet::new();
            for lion in 0..lions {
                let spot = world.claim(place, &taken);
                assert!(
                    taken.insert(spot.tile),
                    "{prefab}: lion {lion} at {place:?} shares {:?}",
                    spot.tile
                );
                assert!(matches!(
                    world.ground(spot.tile),
                    Ground::Floor | Ground::Seat
                ));
                assert_ne!(spot.tile, world.door);
            }
        }
    }
}

#[test]
fn with_the_whole_floor_taken_two_lions_share_a_tile_rather_than_vanish() {
    let world = office();
    let mut taken = HashSet::new();
    for x in 0..world.cols {
        for y in 0..world.rows {
            taken.insert(Tile::new(x, y));
        }
    }
    assert_eq!(
        world.claim(Place::Desks, &taken),
        world.spots(Place::Desks)[0]
    );
}

#[test]
fn the_furniture_is_sorted_back_to_front_and_what_is_on_a_table_comes_after_it() {
    let room = small_office();
    let world = World::build(&room);
    let items = world.items();
    assert!(items.windows(2).all(|pair| pair[0].z <= pair[1].z));
    let at = |id: &str| {
        items
            .iter()
            .position(|item| room.items[item.index].id == id)
            .unwrap()
    };
    assert!(at("desk") < at("pc"), "a computer is drawn on its desk");
    assert!(at("coffee_table") < at("coffee"));
    // What hangs on the wall and what lies flat are not in the list: they
    // are in the backdrop.
    for item in items {
        let id = room.items[item.index].id.as_str();
        assert!(
            !matches!(id, "rug" | "whiteboard" | "window" | "clock"),
            "{id}"
        );
    }
    // A chair seen from behind is drawn over the lion that sits on it, any
    // other seat under it: the lion of a tile is at twice its bottom plus 1.
    let seats = World::build(&bare(
        10,
        10,
        &[
            Placed::new("chair", 3, 5).turned(2),
            Placed::new("chair", 5, 5),
        ],
    ));
    let lion = 2 * 6 * TILE + 1;
    assert!(seats.items()[1].z > lion && seats.items()[0].z < lion);
}

#[test]
fn the_backdrop_is_the_size_of_the_map_with_a_floor_under_every_floor_tile() {
    let world = office();
    let backdrop = world.backdrop();
    assert_eq!((backdrop.w, backdrop.h), (14 * TILE, 11 * TILE));
    assert_eq!(backdrop, office().backdrop());
    for y in 0..world.rows {
        for x in 0..world.cols {
            let tile = Tile::new(x, y);
            let pixel = backdrop.get(x * TILE + 8, y * TILE + 12);
            if world.ground(tile) != Ground::Void {
                assert_eq!(pixel[3], 255, "{tile:?} is painted");
            }
        }
    }
    // A carpet is another floor on its tiles, and only there.
    let mut plain = small_office();
    plain.carpets.clear();
    let bare = World::build(&plain);
    let differs = |tile: Tile| {
        bare.backdrop().get(tile.x * TILE + 3, tile.y * TILE + 3)
            != backdrop.get(tile.x * TILE + 3, tile.y * TILE + 3)
    };
    assert!(differs(Tile::new(4, 9)) && !differs(Tile::new(9, 3)));
}
