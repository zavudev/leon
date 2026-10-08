//! Tests of the den as data: what is written is read back the same, any
//! text at all gives a valid den, and a piece goes only where it may.

use crate::layout::{Carpet, DenLayout, Placed, Why, MAX_COLS, MAX_ROWS, VERSION};
use crate::prefabs::{default_layout, prefabs};
use crate::world::Tile;

fn read(text: &str) -> (DenLayout, Vec<String>) {
    let loaded = DenLayout::load(text, &default_layout());
    (loaded.layout, loaded.notes)
}

fn index_of(room: &DenLayout, id: &str) -> usize {
    room.items.iter().position(|piece| piece.id == id).unwrap()
}

#[test]
fn a_den_is_written_as_json_with_its_version_and_read_back_the_same() {
    for prefab in prefabs() {
        let text = prefab.layout.to_json();
        let (room, notes) = read(&text);
        assert_eq!(room, prefab.layout);
        assert_eq!(notes, Vec::<String>::new());
    }
    let text = default_layout().to_json();
    assert!(text.contains(&format!("\"version\": {VERSION}")));
    assert!(text.contains("\"name\": \"The office\""));
    assert!(text.contains("\"based_on\": \"office\""));
    assert!(text.contains("\"cols\": 14") && text.contains("\"rows\": 11"));
    // A piece not turned does not say so.
    let mut small = DenLayout::empty("Small", 9, 9);
    small.items.push(Placed::new("chair", 3, 4));
    assert!(!small.to_json().contains("turn"));
    small.items[0].turn = 2;
    assert!(small.to_json().contains("\"turn\": 2"));
}

#[test]
fn the_least_a_file_must_say_is_the_size_of_the_room() {
    let (room, notes) = read(r#"{"cols": 10, "rows": 9}"#);
    assert_eq!(notes, Vec::<String>::new());
    assert_eq!((room.cols, room.rows, room.version), (10, 9, VERSION));
    assert_eq!(
        (room.name.as_str(), room.floor.as_str(), room.wall.as_str()),
        ("Den", "wood", "rock")
    );
    assert!(room.items.is_empty() && room.carpets.is_empty());
    // What a later Leon may add is ignored, not an error.
    let (room, notes) = read(r#"{"cols": 10, "rows": 9, "weather": "rain"}"#);
    assert_eq!((room.cols, notes.len()), (10, 0));
}

#[test]
fn a_text_that_is_no_den_gives_the_default_one_and_says_so() {
    for text in [
        "",
        "not json",
        "[]",
        "{}",
        "{\"cols\": \"wide\"}",
        "null",
        "{\"cols\": 3",
    ] {
        let (room, notes) = read(text);
        assert_eq!(room, default_layout(), "{text:?}");
        assert_eq!(notes.len(), 1, "{text:?}");
        assert!(notes[0].starts_with("This is not a den"), "{}", notes[0]);
        assert!(
            notes[0].ends_with("The office is used instead."),
            "{}",
            notes[0]
        );
    }
}

#[test]
fn what_a_file_gets_wrong_is_left_out_or_put_right_and_said() {
    let (room, notes) = read(
        r#"{
            "version": 7, "name": "  ", "cols": 99, "rows": 2,
            "floor": "lava", "wall": "glass",
            "carpets": [
                {"x": 2, "y": 3, "w": 2, "h": 2, "style": "red"},
                {"x": 2, "y": 3, "w": 2, "h": 2, "style": "moss"},
                {"x": 2, "y": 3, "w": 0, "h": 2, "style": "red"},
                {"x": 28, "y": 6, "w": 2000000000, "h": 2000000000, "style": "blue"}
            ],
            "items": [
                {"id": "throne", "x": 3, "y": 3},
                {"id": "plant", "x": 500, "y": -40},
                {"id": "desk", "x": 4, "y": 4},
                {"id": "desk", "x": 5, "y": 4},
                {"id": "chair", "x": 10, "y": 6, "turn": 9},
                {"id": "whiteboard", "x": 3, "y": 6}
            ]
        }"#,
    );
    assert_eq!((room.cols, room.rows), (MAX_COLS, 8));
    assert_eq!((room.version, room.name.as_str()), (VERSION, "Den"));
    assert_eq!((room.floor.as_str(), room.wall.as_str()), ("wood", "rock"));
    assert_eq!(
        room.carpets,
        vec![
            Carpet {
                x: 2,
                y: 3,
                w: 2,
                h: 2,
                style: "red".into()
            },
            // Cut to the room.
            Carpet {
                x: 28,
                y: 6,
                w: 2,
                h: 2,
                style: "blue".into()
            },
        ]
    );
    let ids: Vec<&str> = room.items.iter().map(|piece| piece.id.as_str()).collect();
    assert_eq!(ids, ["plant", "desk", "chair", "whiteboard"]);
    // The plant was far outside: it is in the nearest corner.
    assert_eq!((room.items[0].x, room.items[0].y), (28, 1));
    // The chair's turn is one of its four.
    assert_eq!(room.items[2].turn, 1);
    // The board was on the floor: it is on the wall.
    assert_eq!((room.items[3].x, room.items[3].y), (3, 0));
    let said = notes.join("\n");
    for part in [
        "newer Leon (format 7)",
        "A room of 99 by 2 is out of bounds: it is 30 by 8.",
        "no floor called `lava`",
        "no wall called `glass`",
        "no carpet called `moss`",
        "no piece called `throne`",
        "Plant was outside the room: it is at 28, 1 now.",
        "Desk at 5, 4 is left out. Something is in the way.",
        "Whiteboard was outside the room: it is at 3, 0 now.",
    ] {
        assert!(said.contains(part), "{part:?} in:\n{said}");
    }
    // What was read is a den like any other: nothing more to repair.
    assert_eq!(room.clone().repaired().notes, Vec::<String>::new());
}

#[test]
fn no_text_makes_reading_panic() {
    let numbers = [
        "0",
        "-1",
        "1",
        "7",
        "31",
        "2147483647",
        "-2147483648",
        "1e99",
        "\"x\"",
        "null",
    ];
    let mut read_some = 0;
    for a in numbers {
        for b in numbers {
            let text = format!(
                r#"{{"cols": {a}, "rows": {b}, "version": {a},
                    "carpets": [{{"x": {a}, "y": {b}, "w": {b}, "h": {a}, "style": "red"}}],
                    "items": [{{"id": "desk", "x": {a}, "y": {b}, "turn": 3}},
                              {{"id": "pc", "x": {b}, "y": {a}}},
                              {{"id": "window", "x": {a}, "y": {a}}},
                              {{"id": "rug", "x": {b}, "y": {b}}}]}}"#
            );
            let (room, _) = read(&text);
            // Whatever came out is valid and can be built and painted.
            assert_eq!(
                room.clone().repaired().notes,
                Vec::<String>::new(),
                "{text}"
            );
            let world = crate::world::World::build(&room);
            assert_eq!(world.ground(room.door()), crate::world::Ground::Floor);
            let _ = world.backdrop();
            read_some += usize::from(room != default_layout());
        }
    }
    assert!(read_some > 20, "the numbers that are numbers were read");
    // Cut anywhere, a file is no den or a smaller one, never a panic.
    let whole = default_layout().to_json();
    for end in (0..whole.len()).step_by(7) {
        if whole.is_char_boundary(end) {
            let _ = read(&whole[..end]);
        }
    }
}

#[test]
fn a_piece_goes_only_where_it_may() {
    let room = default_layout();
    let check = |piece: Placed| room.check(&piece, &[]);
    assert_eq!(check(Placed::new("throne", 5, 8)), Err(Why::Unknown));
    // On the floor: inside the walls, on nothing else, off the way in.
    assert_eq!(check(Placed::new("pot", 5, 8)), Ok(()));
    assert_eq!(check(Placed::new("pot", 0, 8)), Err(Why::OutOfRoom));
    assert_eq!(check(Placed::new("pot", 13, 8)), Err(Why::OutOfRoom));
    assert_eq!(check(Placed::new("pot", 5, 11)), Err(Why::OutOfRoom));
    assert_eq!(check(Placed::new("pot", 5, 1)), Err(Why::OutOfRoom));
    assert_eq!(
        check(Placed::new("pot", 2, 6)),
        Err(Why::InTheWay),
        "a bench"
    );
    assert_eq!(check(Placed::new("pot", 7, 10)), Err(Why::OnTheDoor));
    // What a lion walks over may lie on the way in: the rug does.
    assert_eq!(room.door(), Tile::new(7, 10));
    let rug = index_of(&room, "rug");
    assert!(room.items[rug].footprint().contains(&room.door()));
    // And a thing may stand on a rug, though not a rug on a rug.
    assert_eq!(check(Placed::new("pot", 6, 9)), Ok(()));
    assert_eq!(check(Placed::new("rug", 5, 9)), Err(Why::InTheWay));
    // On the wall: the back wall only, and not over what hangs there.
    assert_eq!(check(Placed::new("clock", 10, 0)), Ok(()));
    assert_eq!(check(Placed::new("clock", 10, 4)), Err(Why::NotOnTheWall));
    assert_eq!(check(Placed::new("clock", 0, 0)), Err(Why::OutOfRoom));
    assert_eq!(
        check(Placed::new("clock", 4, 0)),
        Err(Why::InTheWay),
        "the board"
    );
    // A piece that is moved is not in its own way.
    let desk = index_of(&room, "desk");
    let moved = Placed {
        x: 4,
        ..room.items[desk].clone()
    };
    assert_eq!(room.check(&moved, &[]), Err(Why::InTheWay));
    let mut with = vec![desk];
    with.extend(room.on_top_of(desk));
    assert_eq!(
        room.check(&moved, &with),
        Err(Why::InTheWay),
        "the next desk"
    );
    let up = Placed {
        y: 3,
        ..room.items[desk].clone()
    };
    assert_eq!(room.check(&up, &with), Ok(()));
    for why in [
        Why::Unknown,
        Why::OutOfRoom,
        Why::NotOnTheWall,
        Why::InTheWay,
        Why::OnTheDoor,
    ] {
        assert!(why.text().ends_with('.'));
    }
}

#[test]
fn what_is_put_on_a_table_stands_on_it_and_beside_what_is_there() {
    let room = default_layout();
    let desk = index_of(&room, "desk");
    let riders = room.on_top_of(desk);
    assert_eq!(riders.len(), 2, "two computers");
    for rider in &riders {
        assert_eq!(room.items[*rider].id, "pc");
        assert!(room.on_surface(*rider));
        assert_eq!(
            room.surface_under(&room.items[*rider], Some(*rider)),
            Some(desk)
        );
    }
    assert!(!room.on_surface(desk));
    // Between the two there is room for a third, not on one of them.
    assert_eq!(room.check(&Placed::new("pc", 3, 4), &[]), Ok(()));
    assert_eq!(
        room.check(&Placed::new("pc", 2, 4), &[]),
        Err(Why::InTheWay)
    );
    // A computer may stand on the floor too.
    assert_eq!(room.check(&Placed::new("pc", 5, 7), &[]), Ok(()));
}

#[test]
fn the_floor_of_a_tile_is_the_topmost_carpet_over_it() {
    let mut room = DenLayout::empty("Test", 10, 9);
    let wood = room.floor_at(Tile::new(3, 3));
    room.carpets.push(Carpet {
        x: 2,
        y: 3,
        w: 3,
        h: 2,
        style: "red".into(),
    });
    room.carpets.push(Carpet {
        x: 3,
        y: 3,
        w: 1,
        h: 1,
        style: "blue".into(),
    });
    let (red, blue) = (
        room.floor_at(Tile::new(2, 3)),
        room.floor_at(Tile::new(3, 3)),
    );
    assert!(wood != red && red != blue && blue != wood);
    assert_eq!(room.floor_at(Tile::new(4, 4)), red);
    assert_eq!(room.floor_at(Tile::new(5, 4)), wood);
    // A room is never smaller or larger than a room can be.
    let huge = DenLayout::empty("Huge", 500, 500);
    assert_eq!((huge.cols, huge.rows), (MAX_COLS, MAX_ROWS));
    assert!(!huge.is_floor(Tile::new(0, 5)) && !huge.is_floor(Tile::new(5, 1)));
    assert!(huge.is_floor(Tile::new(1, 2)) && huge.is_floor(huge.door()));
}
