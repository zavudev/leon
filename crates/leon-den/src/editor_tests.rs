//! Tests of the editor: every change gives a valid den or none, and every
//! change is one step back.

use crate::assets::{floor, FLOORS, WALLS};
use crate::editor::{advice, advice_for, Brush, Edge, Editor, UNDO_DEPTH};
use crate::layout::{DenLayout, Placed, Why, MAX_COLS, MAX_ROWS};
use crate::prefabs::{prefab, small_office};
use crate::world::{Place, Tile};

fn office() -> Editor {
    Editor::new(small_office())
}

fn index_of(editor: &Editor, id: &str) -> usize {
    editor
        .layout()
        .items
        .iter()
        .position(|piece| piece.id == id)
        .unwrap()
}

/// Every den an editor has shown must be one that needs no repair.
fn valid(editor: &Editor) {
    let again = editor.layout().clone().repaired();
    assert_eq!(again.notes, Vec::<String>::new());
}

#[test]
fn a_piece_in_hand_is_put_down_where_it_fits_and_nowhere_else() {
    let mut editor = office();
    let pieces = editor.layout().items.len();
    assert_eq!(editor.brush(), &Brush::Hand);
    assert!(!editor.can_undo() && !editor.changed());
    editor.pick("pot");
    assert_eq!(editor.brush(), &Brush::Piece { id: "pot", turn: 0 });
    assert_eq!(editor.press(Tile::new(5, 8)), Ok(()));
    assert_eq!(editor.layout().items.len(), pieces + 1);
    assert_eq!(editor.layout().items[pieces], Placed::new("pot", 5, 8));
    assert!(editor.can_undo() && editor.changed());
    // It stays in hand: the next click puts down another, where it fits.
    assert_eq!(editor.press(Tile::new(5, 8)), Err(Why::InTheWay));
    assert_eq!(editor.press(Tile::new(0, 8)), Err(Why::OutOfRoom));
    assert_eq!(editor.press(Tile::new(7, 10)), Err(Why::OnTheDoor));
    assert_eq!(
        editor.layout().items.len(),
        pieces + 1,
        "a refusal changes nothing"
    );
    assert_eq!(editor.press(Tile::new(5, 7)), Ok(()));
    valid(&editor);
    // Each is a step back.
    assert!(editor.undo() && editor.undo());
    assert_eq!(editor.layout(), &small_office());
    assert!(!editor.undo() && !editor.changed());
    assert!(editor.redo());
    assert_eq!(editor.layout().items.len(), pieces + 1);
    // A change after an undo forgets what was undone.
    assert!(editor.can_redo());
    editor.cycle_wall(false);
    assert!(!editor.can_redo() && !editor.redo());
    // An unknown piece is no piece.
    editor.pick("throne");
    assert_eq!(editor.brush(), &Brush::Hand);
}

#[test]
fn a_tall_piece_in_hand_stands_on_the_tile_under_the_pointer() {
    let mut editor = office();
    editor.pick("rack");
    // A rack is two tiles tall: its foot is where the pointer is.
    editor.point(Some(Tile::new(5, 8)));
    let (ghost, fits) = editor.marks().ghost.unwrap();
    assert_eq!((ghost, fits), (Placed::new("rack", 5, 7), true));
    assert_eq!(editor.press(Tile::new(5, 8)), Ok(()));
    assert_eq!(
        editor.layout().items.last(),
        Some(&Placed::new("rack", 5, 7))
    );
    // What hangs on the wall goes on the wall wherever in it the pointer is.
    editor.pick("clock");
    editor.point(Some(Tile::new(10, 1)));
    let (ghost, fits) = editor.marks().ghost.unwrap();
    assert!(fits && ghost.y <= 1, "{ghost:?}");
    editor.point(Some(Tile::new(10, 6)));
    let (ghost, fits) = editor.marks().ghost.unwrap();
    assert!(
        fits && ghost.y <= 1,
        "it is shown on the wall above: {ghost:?}"
    );
    editor.point(Some(Tile::new(4, 6)));
    assert!(!editor.marks().ghost.unwrap().1, "the board hangs there");
    editor.point(None);
    assert_eq!(editor.marks().ghost, None);
}

#[test]
fn a_table_moves_with_what_stands_on_it_in_one_step() {
    let mut editor = office();
    let desk = index_of(&editor, "desk");
    let riders = editor.layout().on_top_of(desk);
    assert_eq!(editor.move_to(desk, 2, 3), Ok(()));
    assert_eq!(
        (editor.layout().items[desk].x, editor.layout().items[desk].y),
        (2, 3)
    );
    for rider in &riders {
        assert_eq!(editor.layout().items[*rider].y, 3);
        assert!(editor.layout().on_surface(*rider));
    }
    valid(&editor);
    // Not onto the next desk, nor into the wall.
    assert_eq!(editor.move_to(desk, 5, 4), Err(Why::InTheWay));
    assert_eq!(editor.move_to(desk, 0, 3), Err(Why::OutOfRoom));
    assert_eq!(editor.move_to(99, 2, 3), Err(Why::Unknown));
    assert!(editor.undo());
    assert_eq!(editor.layout(), &small_office());
    // To where it is already is no move, and nothing to undo.
    assert_eq!(editor.move_to(desk, 2, 4), Ok(()));
    assert!(!editor.can_undo());
}

#[test]
fn a_drag_follows_the_pointer_as_far_as_the_piece_may_go_and_is_one_step() {
    let mut editor = office();
    let desk = index_of(&editor, "desk");
    // Taken by its middle tile.
    assert_eq!(editor.press(Tile::new(3, 5)), Ok(()));
    assert_eq!(editor.selected(), Some(desk));
    assert_eq!(
        editor.marks().selected,
        editor.layout().items[desk].footprint()
    );
    editor.point(Some(Tile::new(3, 4)));
    assert_eq!(editor.layout().items[desk].y, 3);
    // Over the wall it stays where it last could be.
    editor.point(Some(Tile::new(3, 1)));
    assert_eq!(editor.layout().items[desk].y, 3);
    editor.point(Some(Tile::new(4, 4)));
    editor.point(Some(Tile::new(3, 5)));
    editor.point(Some(Tile::new(3, 4)));
    editor.release();
    assert_eq!(editor.selected(), Some(desk), "it stays selected");
    assert!(editor.undo());
    assert_eq!(
        editor.layout(),
        &small_office(),
        "the whole drag is one step"
    );
    assert!(!editor.can_undo());
    // A click that moves nothing is nothing to undo; after it the pointer
    // moves no piece.
    editor.press(Tile::new(3, 5)).unwrap();
    editor.release();
    editor.point(Some(Tile::new(3, 4)));
    assert!(!editor.can_undo());
    assert_eq!(editor.layout(), &small_office());
    // A click on bare floor selects nothing.
    editor.press(Tile::new(5, 8)).unwrap();
    assert_eq!(editor.selected(), None);
    assert!(editor.marks().selected.is_empty());
    // An empty hand shows the tile it is over, but not while it drags.
    assert_eq!(editor.marks().cursor, Some(Tile::new(5, 8)));
    assert_eq!(editor.pointer(), Some(Tile::new(5, 8)));
    editor.press(Tile::new(3, 5)).unwrap();
    assert_eq!(editor.marks().cursor, None);
    editor.deselect();
    assert_eq!(editor.selected(), None);
    editor.point(Some(Tile::new(3, 4)));
    assert_eq!(
        editor.layout(),
        &small_office(),
        "let go, it is not dragged"
    );
}

#[test]
fn a_click_takes_what_is_on_top() {
    let editor = office();
    let desk = index_of(&editor, "desk");
    let computer = editor.piece_at(Tile::new(2, 4)).unwrap();
    assert_eq!(editor.layout().items[computer].id, "pc");
    assert_eq!(editor.piece_at(Tile::new(3, 4)), Some(desk));
    // Furniture before the rug it stands on; the rug where it is bare.
    let mut editor = office();
    editor.place(Placed::new("pot", 6, 9)).unwrap();
    let on_rug = editor.piece_at(Tile::new(6, 9)).unwrap();
    assert_eq!(editor.layout().items[on_rug].id, "pot");
    let rug = editor.piece_at(Tile::new(8, 9)).unwrap();
    assert_eq!(editor.layout().items[rug].id, "rug");
    let board = editor.piece_at(Tile::new(4, 0)).unwrap();
    assert_eq!(editor.layout().items[board].id, "whiteboard");
    assert_eq!(editor.piece_at(Tile::new(5, 8)), None);
}

#[test]
fn the_selected_piece_is_nudged_by_a_tile_until_something_is_in_the_way() {
    let mut editor = office();
    assert_eq!(editor.nudge(1, 0), Err(Why::Unknown), "nothing is selected");
    editor.place(Placed::new("pot", 5, 8)).unwrap();
    editor.press(Tile::new(5, 8)).unwrap();
    editor.release();
    assert_eq!(editor.nudge(0, -1), Ok(()));
    let pot = editor.selected().unwrap();
    assert_eq!(editor.layout().items[pot], Placed::new("pot", 5, 7));
    assert_eq!(editor.nudge(0, 1), Ok(()));
    // The plant of the office stands on 4, 8.
    assert_eq!(editor.nudge(-1, 0), Err(Why::InTheWay));
    assert_eq!(editor.layout().items[pot], Placed::new("pot", 5, 8));
    assert_eq!(
        editor.selected(),
        Some(pot),
        "a refusal keeps the selection"
    );
    valid(&editor);
}

#[test]
fn a_piece_turns_in_hand_and_in_the_room() {
    let mut editor = office();
    assert_eq!(editor.rotate(), Err(Why::Unknown), "nothing to turn");
    editor.pick("chair");
    for turn in [1, 2, 3, 0] {
        assert_eq!(editor.rotate(), Ok(()));
        assert_eq!(editor.brush(), &Brush::Piece { id: "chair", turn });
    }
    assert!(!editor.can_undo(), "turning what is in hand changes no den");
    editor.rotate().unwrap();
    editor.press(Tile::new(5, 8)).unwrap();
    assert_eq!(editor.layout().items.last().unwrap().turn, 1);
    // In the room: the selected one.
    editor.drop_brush();
    editor.press(Tile::new(5, 8)).unwrap();
    editor.release();
    let chair = editor.selected().unwrap();
    assert_eq!(editor.rotate(), Ok(()));
    assert_eq!(editor.layout().items[chair].turn, 2);
    assert!(editor.undo());
    assert_eq!(editor.layout().items[chair].turn, 1);
    // A piece with one view stays as it is, and that is no change.
    let before = editor.layout().clone();
    editor.press(Tile::new(11, 2)).unwrap();
    editor.release();
    let rack = editor.selected().unwrap();
    assert_eq!(editor.layout().items[rack].id, "rack");
    assert_eq!(editor.rotate(), Ok(()));
    assert_eq!(editor.layout(), &before);
    valid(&editor);
}

#[test]
fn deleting_a_table_takes_what_stands_on_it_too() {
    let mut editor = office();
    assert!(!editor.delete(), "nothing is selected");
    let pieces = editor.layout().items.len();
    editor.press(Tile::new(3, 5)).unwrap();
    editor.release();
    assert!(editor.delete());
    assert_eq!(
        editor.layout().items.len(),
        pieces - 3,
        "the desk and two computers"
    );
    assert_eq!(editor.selected(), None);
    assert_eq!(editor.piece_at(Tile::new(2, 4)), None);
    valid(&editor);
    assert!(editor.undo());
    assert_eq!(editor.layout(), &small_office());
    // A computer alone leaves the desk.
    editor.press(Tile::new(2, 4)).unwrap();
    editor.release();
    assert!(editor.delete());
    assert_eq!(editor.layout().items.len(), pieces - 1);
    assert_eq!(
        editor.layout().items[editor.piece_at(Tile::new(2, 4)).unwrap()].id,
        "desk"
    );
}

#[test]
fn a_carpet_is_laid_tile_by_tile_and_taken_off_the_same_way() {
    let mut editor = office();
    let (wood, red, blue) = (
        floor("wood").unwrap(),
        floor("red").unwrap(),
        floor("blue").unwrap(),
    );
    let tile = Tile::new(6, 7);
    editor.pick_carpet(Some("blue"));
    assert_eq!(editor.brush(), &Brush::Carpet("blue"));
    editor.point(Some(tile));
    assert_eq!(editor.marks().tile, Some((tile, true)));
    editor.press(tile).unwrap();
    assert_eq!(editor.layout().floor_at(tile), blue);
    assert_eq!(editor.layout().floor_at(Tile::new(7, 7)), wood);
    // Again on the same tile is no change.
    let steps = |editor: &Editor| {
        let mut editor = editor.clone();
        std::iter::from_fn(|| editor.undo().then_some(())).count()
    };
    assert_eq!(steps(&editor), 1);
    editor.press(tile).unwrap();
    assert_eq!(steps(&editor), 1);
    // The eraser takes a tile out of the middle of the office's red carpet
    // and leaves the rest of it.
    editor.pick_carpet(None);
    assert_eq!(editor.brush(), &Brush::BareFloor);
    editor.press(Tile::new(2, 9)).unwrap();
    for y in 8..11 {
        for x in 1..5 {
            let wanted = if (x, y) == (2, 9) { wood } else { red };
            assert_eq!(
                editor.layout().floor_at(Tile::new(x, y)),
                wanted,
                "{x}, {y}"
            );
        }
    }
    // Over another carpet, the new one replaces it there.
    editor.paint(Tile::new(1, 8), Some("blue"));
    assert_eq!(editor.layout().floor_at(Tile::new(1, 8)), blue);
    assert_eq!(editor.layout().floor_at(Tile::new(2, 8)), red);
    // The room's own floor as a carpet is no carpet.
    let carpets = editor.layout().carpets.len();
    editor.paint(Tile::new(8, 7), Some("wood"));
    assert_eq!(editor.layout().carpets.len(), carpets);
    // Not on a wall, not with a carpet that does not exist.
    let before = editor.layout().clone();
    editor.paint(Tile::new(0, 5), Some("blue"));
    editor.paint(Tile::new(5, 1), Some("blue"));
    editor.paint(Tile::new(8, 7), Some("lava"));
    assert_eq!(editor.layout(), &before);
    editor.point(Some(Tile::new(0, 5)));
    assert_eq!(editor.marks().tile, Some((Tile::new(0, 5), false)));
    valid(&editor);
}

#[test]
fn the_floor_and_the_walls_go_through_every_style_and_round() {
    let mut editor = office();
    let mut seen = vec![editor.layout().floor.clone()];
    for _ in 1..FLOORS.len() {
        editor.cycle_floor(false);
        assert!(!seen.contains(&editor.layout().floor));
        seen.push(editor.layout().floor.clone());
    }
    editor.cycle_floor(false);
    assert_eq!(editor.layout().floor, "wood");
    editor.cycle_floor(true);
    assert_eq!(editor.layout().floor, FLOORS[FLOORS.len() - 1].id);
    for _ in 0..WALLS.len() {
        editor.cycle_wall(false);
    }
    assert_eq!(editor.layout().wall, "rock");
    editor.cycle_wall(true);
    assert_eq!(editor.layout().wall, WALLS[WALLS.len() - 1].id);
    // The carpets stayed.
    assert_eq!(editor.layout().carpets, small_office().carpets);
    valid(&editor);
}

#[test]
fn the_room_grows_and_shrinks_by_its_right_and_its_open_side_within_limits() {
    let mut editor = office();
    assert_eq!(editor.resize(Edge::Right, 1), Ok(()));
    assert_eq!(editor.resize(Edge::Bottom, 2), Ok(()));
    assert_eq!((editor.layout().cols, editor.layout().rows), (15, 13));
    assert_eq!(
        editor.layout().door(),
        Tile::new(7, 12),
        "the way in moves with it"
    );
    valid(&editor);
    assert!(editor.undo() && editor.undo());
    assert_eq!(editor.layout(), &small_office());
    // Not onto the furniture: the racks stand by the right wall, the
    // nests on the last row.
    assert_eq!(editor.resize(Edge::Right, -1), Err(Why::OutOfRoom));
    assert_eq!(editor.resize(Edge::Bottom, -1), Err(Why::OutOfRoom));
    assert_eq!(editor.layout(), &small_office());
    assert!(!editor.can_undo());
    // An empty room goes down to the least and up to the most.
    let mut bare = Editor::new(DenLayout::empty("Bare", 9, 9));
    assert_eq!(bare.resize(Edge::Right, -1), Ok(()));
    assert_eq!(bare.resize(Edge::Right, -1), Err(Why::OutOfRoom));
    assert_eq!(bare.resize(Edge::Bottom, -1), Ok(()));
    assert_eq!(bare.resize(Edge::Bottom, -1), Err(Why::OutOfRoom));
    assert_eq!(bare.resize(Edge::Right, MAX_COLS - 8), Ok(()));
    assert_eq!(bare.resize(Edge::Bottom, MAX_ROWS - 8), Ok(()));
    assert_eq!(bare.resize(Edge::Right, 1), Err(Why::OutOfRoom));
    assert_eq!(bare.resize(Edge::Bottom, 1), Err(Why::OutOfRoom));
    assert_eq!(
        (bare.layout().cols, bare.layout().rows),
        (MAX_COLS, MAX_ROWS)
    );
    // A piece that would come to stand on the way in keeps the room as it is.
    let mut blocked = Editor::new(DenLayout::empty("Blocked", 10, 9));
    blocked.place(Placed::new("pot", 4, 8)).unwrap();
    assert_eq!(blocked.layout().door(), Tile::new(5, 8));
    assert_eq!(blocked.resize(Edge::Right, -2), Err(Why::OnTheDoor));
}

#[test]
fn another_den_replaces_this_one_in_one_step() {
    let mut editor = office();
    editor.press(Tile::new(3, 5)).unwrap();
    let nook = prefab("nook").unwrap().layout;
    editor.replace(nook.clone());
    assert_eq!(editor.layout(), &nook);
    assert_eq!(editor.selected(), None);
    assert!(editor.changed());
    // The pointer no longer drags the piece of the den that is gone.
    editor.point(Some(Tile::new(3, 4)));
    assert_eq!(editor.layout(), &nook);
    assert!(editor.undo());
    assert_eq!(editor.layout(), &small_office());
    assert!(!editor.changed());
    // The same den again is nothing to undo.
    editor.replace(small_office());
    assert!(!editor.can_undo());
}

#[test]
fn the_editor_remembers_a_limited_number_of_steps() {
    let mut editor = office();
    for _ in 0..UNDO_DEPTH + 50 {
        editor.cycle_floor(false);
    }
    let mut steps = 0;
    while editor.undo() {
        steps += 1;
    }
    assert_eq!(steps, UNDO_DEPTH);
}

#[test]
fn a_room_is_told_what_it_lacks_in_the_narrators_voice_and_plainly() {
    assert_eq!(advice_for(&small_office()), Vec::new());
    let bare = advice_for(&DenLayout::empty("Bare", 10, 9));
    assert_eq!(bare.len(), 9);
    assert_eq!(advice(Place::Watch), None);
    for place in Place::ALL {
        let Some((voiced, plain)) = advice(place) else {
            continue;
        };
        assert!(
            voiced.starts_with("No") && voiced.ends_with('.'),
            "{voiced}"
        );
        assert!(plain.ends_with('.') && plain.contains(':'), "{plain}");
        assert!(!voiced.contains('!') && !plain.contains('!'));
        assert!(
            voiced.chars().count() <= 48,
            "{voiced}: short enough for a line"
        );
        assert_ne!(voiced, plain);
    }
    // Take the shelf out of the office and it is told so, and only that.
    let mut editor = office();
    editor.press(Tile::new(1, 0)).unwrap();
    editor.release();
    assert!(editor.delete());
    assert_eq!(
        advice_for(editor.layout()),
        vec![(
            "No shelf. Readers will stand around.",
            "No bookshelf: lions that read or search stand on free floor."
        )]
    );
}
