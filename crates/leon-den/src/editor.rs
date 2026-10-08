//! Editing a den: every change a user can make to a layout, as pure
//! operations with undo.
//!
//! An [`Editor`] holds the layout being edited and what the user has in
//! hand ([`Brush`]). The view translates the pointer and the keys into its
//! operations and draws what [`Editor::marks`] says; nothing here knows a
//! window. Every operation either changes the layout to another valid one,
//! and can be undone, or leaves it as it was and says why not ([`Why`]).
//!
//! What moves together: a table takes what stands on it along, and takes it
//! away when it is deleted.

use crate::assets::{floor, wall, FLOORS, WALLS};
use crate::catalogue::{find, Placement};
use crate::layout::{Carpet, DenLayout, Placed, Why, MAX_COLS, MAX_ROWS, MIN_COLS, MIN_ROWS};
use crate::world::{Place, Tile, World};

/// How many steps back an editor remembers.
pub const UNDO_DEPTH: usize = 200;

/// What the user has in hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Brush {
    /// Nothing: a click selects a piece, a drag moves it.
    Hand,
    /// A piece of the catalogue, turned so many times: a click puts it down.
    Piece {
        /// Its id.
        id: &'static str,
        /// How it is turned.
        turn: u8,
    },
    /// A carpet: a click or a drag lays it on a tile.
    Carpet(&'static str),
    /// The eraser of carpets.
    BareFloor,
}

/// A side of the room that can be moved to resize it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    /// The right wall: the room gets wider or narrower.
    Right,
    /// The open side: the room gets deeper or shallower.
    Bottom,
}

/// What the view draws over the room while it is edited.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Marks {
    /// The piece in hand where the pointer is, and whether it may go there.
    pub ghost: Option<(Placed, bool)>,
    /// The tiles of the selected piece.
    pub selected: Vec<Tile>,
    /// The selected piece: its index in the layout.
    pub chosen: Option<usize>,
    /// The tile under the pointer, for a carpet brush.
    pub tile: Option<(Tile, bool)>,
    /// The tile under the pointer of an empty hand: what a click would take.
    pub cursor: Option<Tile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Drag {
    index: usize,
    /// From the piece's corner to the tile it was taken by.
    grip: (i32, i32),
    /// The layout before the drag: what undo goes back to.
    before: DenLayout,
    moved: bool,
}

/// A den being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    layout: DenLayout,
    undo: Vec<DenLayout>,
    redo: Vec<DenLayout>,
    brush: Brush,
    selected: Option<usize>,
    pointer: Option<Tile>,
    drag: Option<Drag>,
}

impl Editor {
    /// An editor of a layout, with nothing in hand and nothing to undo.
    pub fn new(layout: DenLayout) -> Self {
        Self {
            layout,
            undo: Vec::new(),
            redo: Vec::new(),
            brush: Brush::Hand,
            selected: None,
            pointer: None,
            drag: None,
        }
    }

    /// The layout as it is now.
    pub fn layout(&self) -> &DenLayout {
        &self.layout
    }

    /// What is in hand.
    pub fn brush(&self) -> &Brush {
        &self.brush
    }

    /// The selected piece: its index in the layout.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Where the pointer is, in tiles.
    pub fn pointer(&self) -> Option<Tile> {
        self.pointer
    }

    /// The piece the pointer is dragging: its index in the layout.
    pub fn dragging(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.index)
    }

    /// Lets go of the selected piece: nothing is selected.
    pub fn deselect(&mut self) {
        self.selected = None;
        self.drag = None;
    }

    /// Whether there is something to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is something to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Whether the layout differs from the one the editor was opened on.
    pub fn changed(&self) -> bool {
        self.undo.first().is_some_and(|first| *first != self.layout)
    }

    fn commit(&mut self, before: DenLayout) {
        if before == self.layout {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Takes the last change back. `false` when there is none.
    pub fn undo(&mut self) -> bool {
        self.drag = None;
        match self.undo.pop() {
            Some(before) => {
                self.redo.push(std::mem::replace(&mut self.layout, before));
                self.selected = None;
                true
            }
            None => false,
        }
    }

    /// Makes the change taken back again. `false` when there is none.
    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(after) => {
                self.undo.push(std::mem::replace(&mut self.layout, after));
                self.selected = None;
                true
            }
            None => false,
        }
    }

    /// Takes a piece of the catalogue in hand. An unknown id is no piece:
    /// the hand stays empty.
    pub fn pick(&mut self, id: &str) {
        self.selected = None;
        self.brush = match find(id) {
            Some(entry) => Brush::Piece {
                id: entry.id,
                turn: 0,
            },
            None => Brush::Hand,
        };
    }

    /// Takes a carpet in hand, or the eraser of carpets with `None`.
    pub fn pick_carpet(&mut self, style: Option<&str>) {
        self.selected = None;
        self.brush = match style.and_then(floor) {
            Some(index) => Brush::Carpet(FLOORS[index].id),
            None => Brush::BareFloor,
        };
    }

    /// Empties the hand.
    pub fn drop_brush(&mut self) {
        self.brush = Brush::Hand;
    }

    /// Tells the editor where the pointer is, in tiles; `None` off the room.
    /// While a piece is dragged it follows, as far as it may go.
    pub fn point(&mut self, tile: Option<Tile>) {
        self.pointer = tile;
        let (Some(tile), Some(drag)) = (tile, self.drag.clone()) else {
            return;
        };
        let to = (tile.x - drag.grip.0, tile.y - drag.grip.1);
        if self.shift(drag.index, to).is_ok() {
            if let Some(drag) = &mut self.drag {
                drag.moved = true;
            }
        }
    }

    /// The topmost piece on a tile: what stands on a table before the
    /// table, the furniture before a rug, and on the wall what hangs there.
    pub fn piece_at(&self, tile: Tile) -> Option<usize> {
        let rank = |index: usize| {
            let (entry, _) = self.layout.items[index].piece()?;
            Some(if self.layout.on_surface(index) {
                3
            } else if entry.flat {
                1
            } else {
                2
            })
        };
        (0..self.layout.items.len())
            .filter(|index| self.layout.items[*index].footprint().contains(&tile))
            .filter_map(|index| Some((rank(index)?, index)))
            .max()
            .map(|(_, index)| index)
    }

    /// What a press of the pointer on a tile does with what is in hand: a
    /// piece is put down, a carpet laid, and with an empty hand the piece
    /// there is selected and taken to be dragged.
    pub fn press(&mut self, tile: Tile) -> Result<(), Why> {
        self.pointer = Some(tile);
        match self.brush.clone() {
            Brush::Piece { id, turn } => {
                let placed = Placed::new(id, tile.x, tile.y).turned(turn);
                self.place(self.anchored(placed))
            }
            Brush::Carpet(style) => {
                self.paint(tile, Some(style));
                Ok(())
            }
            Brush::BareFloor => {
                self.paint(tile, None);
                Ok(())
            }
            Brush::Hand => {
                self.selected = self.piece_at(tile);
                if let Some(index) = self.selected {
                    let piece = &self.layout.items[index];
                    self.drag = Some(Drag {
                        index,
                        grip: (tile.x - piece.x, tile.y - piece.y),
                        before: self.layout.clone(),
                        moved: false,
                    });
                }
                Ok(())
            }
        }
    }

    /// The pointer was let go: a drag ends, and is one step to undo.
    pub fn release(&mut self) {
        if let Some(drag) = self.drag.take() {
            if drag.moved {
                self.commit(drag.before);
            }
        }
    }

    /// A piece in hand is held by its bottom row, so that what is tall
    /// stands where the pointer is and not under it.
    fn anchored(&self, mut placed: Placed) -> Placed {
        if let Some((entry, view)) = placed.piece() {
            if entry.placement == Placement::Wall {
                placed.y = placed.y.clamp(0, (2 - view.h).max(0));
            } else {
                placed.y -= view.h - 1;
            }
        }
        placed
    }

    /// Puts a piece in the room.
    pub fn place(&mut self, placed: Placed) -> Result<(), Why> {
        self.layout.check(&placed, &[])?;
        let before = self.layout.clone();
        self.layout.items.push(placed);
        self.commit(before);
        Ok(())
    }

    /// Moves the piece at `index` so that its corner is at `to`, with what
    /// stands on it. Not a step of undo by itself: [`Self::move_to`] is.
    fn shift(&mut self, index: usize, to: (i32, i32)) -> Result<(), Why> {
        let piece = self.layout.items.get(index).ok_or(Why::Unknown)?.clone();
        let (dx, dy) = (to.0 - piece.x, to.1 - piece.y);
        if (dx, dy) == (0, 0) {
            return Ok(());
        }
        let riders = self.layout.on_top_of(index);
        let mut moving = vec![index];
        moving.extend(&riders);
        let moved = Placed {
            x: to.0,
            y: to.1,
            ..piece
        };
        self.layout.check(&moved, &moving)?;
        // What rides must still be on the table once both have moved, which
        // it is: they move by the same step. It must only stay in the room.
        for rider in &riders {
            let rider = &self.layout.items[*rider];
            let there = Placed {
                x: rider.x + dx,
                y: rider.y + dy,
                ..rider.clone()
            };
            let (_, view) = there.piece().ok_or(Why::Unknown)?;
            if there.x < 1 || there.x + view.w > self.layout.cols - 1 || there.y < 0 {
                return Err(Why::OutOfRoom);
            }
        }
        for index in moving {
            self.layout.items[index].x += dx;
            self.layout.items[index].y += dy;
        }
        Ok(())
    }

    /// Moves a piece, and what stands on it, to another tile.
    pub fn move_to(&mut self, index: usize, x: i32, y: i32) -> Result<(), Why> {
        let before = self.layout.clone();
        self.shift(index, (x, y))?;
        self.commit(before);
        Ok(())
    }

    /// Moves the selected piece by a step.
    pub fn nudge(&mut self, dx: i32, dy: i32) -> Result<(), Why> {
        let index = self.selected.ok_or(Why::Unknown)?;
        let piece = &self.layout.items[index];
        self.move_to(index, piece.x + dx, piece.y + dy)
    }

    /// Turns what is in hand, or else the selected piece, to its next view.
    /// A piece that has one view only does not turn.
    pub fn rotate(&mut self) -> Result<(), Why> {
        if let Brush::Piece { id, turn } = &mut self.brush {
            let views = find(id).map_or(1, |entry| entry.views.len()) as u8;
            *turn = (*turn + 1) % views;
            return Ok(());
        }
        let index = self.selected.ok_or(Why::Unknown)?;
        let piece = self.layout.items[index].clone();
        let (entry, _) = piece.piece().ok_or(Why::Unknown)?;
        if !entry.turns() {
            return Ok(());
        }
        // The first of its other views that fits where it stands. What
        // stands on a table would not turn with it: a table is turned bare.
        if !self.layout.on_top_of(index).is_empty() {
            return Err(Why::InTheWay);
        }
        let views = entry.views.len() as u8;
        let turned = (1..views)
            .map(|step| piece.clone().turned((piece.turn + step) % views))
            .find(|turned| self.layout.check(turned, &[index]).is_ok())
            .ok_or(Why::InTheWay)?;
        let before = self.layout.clone();
        self.layout.items[index] = turned;
        self.commit(before);
        Ok(())
    }

    /// Takes the selected piece out of the room, with what stands on it.
    pub fn delete(&mut self) -> bool {
        let Some(index) = self.selected.take() else {
            return false;
        };
        let before = self.layout.clone();
        let mut gone = self.layout.on_top_of(index);
        gone.push(index);
        gone.sort_unstable();
        for index in gone.into_iter().rev() {
            self.layout.items.remove(index);
        }
        self.commit(before);
        true
    }

    /// Lays a carpet on a tile, or takes what carpet there is off it.
    pub fn paint(&mut self, tile: Tile, style: Option<&str>) {
        if !self.layout.is_floor(tile) {
            return;
        }
        let now = self.layout.floor_at(tile);
        let wanted = style
            .and_then(floor)
            .or_else(|| floor(&self.layout.floor))
            .unwrap_or(0);
        if now == wanted {
            return;
        }
        let before = self.layout.clone();
        // The carpets over this tile are cut around it.
        let mut cut = Vec::new();
        for carpet in std::mem::take(&mut self.layout.carpets) {
            let covers = tile.x >= carpet.x
                && tile.x < carpet.x + carpet.w
                && tile.y >= carpet.y
                && tile.y < carpet.y + carpet.h;
            if !covers {
                cut.push(carpet);
                continue;
            }
            let piece = |x: i32, y: i32, w: i32, h: i32| Carpet {
                x,
                y,
                w,
                h,
                style: carpet.style.clone(),
            };
            let (right, bottom) = (carpet.x + carpet.w, carpet.y + carpet.h);
            for part in [
                piece(carpet.x, carpet.y, carpet.w, tile.y - carpet.y),
                piece(carpet.x, tile.y, tile.x - carpet.x, 1),
                piece(tile.x + 1, tile.y, right - tile.x - 1, 1),
                piece(carpet.x, tile.y + 1, carpet.w, bottom - tile.y - 1),
            ] {
                if part.w > 0 && part.h > 0 {
                    cut.push(part);
                }
            }
        }
        self.layout.carpets = cut;
        if let Some(style) = style.filter(|style| floor(style).is_some()) {
            if Some(style) != Some(self.layout.floor.as_str()) {
                self.layout.carpets.push(Carpet {
                    x: tile.x,
                    y: tile.y,
                    w: 1,
                    h: 1,
                    style: style.to_owned(),
                });
            }
        }
        self.commit(before);
    }

    /// Lays the next floor of the list in the whole room (the previous one
    /// with `back`). Carpets stay.
    pub fn cycle_floor(&mut self, back: bool) {
        let now = floor(&self.layout.floor).unwrap_or(0);
        let next = step(now, FLOORS.len(), back);
        let before = self.layout.clone();
        self.layout.floor = FLOORS[next].id.to_owned();
        self.commit(before);
    }

    /// Paints the walls in the next colour of the list.
    pub fn cycle_wall(&mut self, back: bool) {
        let now = wall(&self.layout.wall).unwrap_or(0);
        let next = step(now, WALLS.len(), back);
        let before = self.layout.clone();
        self.layout.wall = WALLS[next].id.to_owned();
        self.commit(before);
    }

    /// Moves an edge of the room by `by` tiles: wider or narrower, deeper or
    /// shallower. A room does not shrink onto a piece, nor past its limits.
    pub fn resize(&mut self, edge: Edge, by: i32) -> Result<(), Why> {
        let (cols, rows) = match edge {
            Edge::Right => (self.layout.cols + by, self.layout.rows),
            Edge::Bottom => (self.layout.cols, self.layout.rows + by),
        };
        if !(MIN_COLS..=MAX_COLS).contains(&cols) || !(MIN_ROWS..=MAX_ROWS).contains(&rows) {
            return Err(Why::OutOfRoom);
        }
        let mut resized = self.layout.clone();
        (resized.cols, resized.rows) = (cols, rows);
        // Every piece must still be where it may be, the door included: it
        // moves with the room.
        for index in 0..resized.items.len() {
            let piece = resized.items[index].clone();
            let mut skip = vec![index];
            skip.extend(resized.on_top_of(index));
            if resized.on_surface(index) {
                continue;
            }
            resized.check(&piece, &skip)?;
        }
        let before = std::mem::replace(&mut self.layout, resized);
        self.commit(before);
        Ok(())
    }

    /// Replaces the whole layout: another den was chosen, or this one is
    /// reset. One step to undo.
    pub fn replace(&mut self, layout: DenLayout) {
        let before = std::mem::replace(&mut self.layout, layout);
        self.selected = None;
        self.drag = None;
        self.commit(before);
    }

    /// What to draw over the room.
    pub fn marks(&self) -> Marks {
        let mut marks = Marks::default();
        if let Some(index) = self.selected {
            if let Some(piece) = self.layout.items.get(index) {
                marks.selected = piece.footprint();
                marks.chosen = Some(index);
            }
        }
        let Some(tile) = self.pointer else {
            return marks;
        };
        match &self.brush {
            Brush::Piece { id, turn } => {
                let placed = self.anchored(Placed::new(id, tile.x, tile.y).turned(*turn));
                let fits = self.layout.check(&placed, &[]).is_ok();
                marks.ghost = Some((placed, fits));
            }
            Brush::Carpet(_) | Brush::BareFloor => {
                marks.tile = Some((tile, self.layout.is_floor(tile)));
            }
            Brush::Hand => {
                if self.drag.is_none() {
                    marks.cursor = Some(tile);
                }
            }
        }
        marks
    }
}

fn step(now: usize, len: usize, back: bool) -> usize {
    if back {
        (now + len - 1) % len
    } else {
        (now + 1) % len
    }
}

/// What a room lacks, said twice: in the narrator's voice, and plainly.
///
/// The den works without any of it (a lion with nowhere to go stands on
/// free floor and does the same thing there), so this is advice, not an
/// error.
pub fn advice(place: Place) -> Option<(&'static str, &'static str)> {
    Some(match place {
        Place::Desks => (
            "No desk. The pride will type standing.",
            "No seat faces a computer: lions that edit stand with a tablet.",
        ),
        Place::Bench => (
            "No little seat. The little ones borrow a desk.",
            "No wooden bench faces a computer: sub-agents use the lions' seats.",
        ),
        Place::Shelf => (
            "No shelf. Readers will stand around.",
            "No bookshelf: lions that read or search stand on free floor.",
        ),
        Place::Board => (
            "No board. Plans will be made in the air.",
            "No whiteboard: lions that plan stand on free floor.",
        ),
        Place::Rack => (
            "No rack. Commands will run somewhere.",
            "No rack: lions that run a command stand on free floor.",
        ),
        Place::Lookout => (
            "No lookout. The web is out there, unseen.",
            "No window or telescope: lions on the web stand on free floor.",
        ),
        Place::Sun => (
            "Nowhere to rest. The idle will loiter.",
            "No seat without a computer: idle and sleeping lions stand on free floor.",
        ),
        Place::Entrance => (
            "No rug. They will wait by the door anyway.",
            "No entrance rug: lions that wait for you stand near the way in.",
        ),
        Place::Nest => (
            "No nest. Eggs will hatch on the floor.",
            "No nest: sub-agents hatch on free floor.",
        ),
        Place::Watch => return None,
    })
}

/// Everything a layout lacks: [`advice`] for each place without a spot.
pub fn advice_for(layout: &DenLayout) -> Vec<(&'static str, &'static str)> {
    World::build(layout)
        .missing()
        .into_iter()
        .filter_map(advice)
        .collect()
}
