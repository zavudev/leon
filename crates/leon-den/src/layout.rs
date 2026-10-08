//! A den as data: the size of the room, its floor and its walls, and what is
//! put where.
//!
//! A [`DenLayout`] is what the editor changes, what a prefab is, and what is
//! kept in a file: JSON, with a version. It holds no picture and no rule of
//! its own: the pieces are named by their id in the [`crate::catalogue`] and
//! the floors and walls by theirs in [`crate::assets`].
//!
//! # The room
//!
//! `cols` by `rows` tiles, walls included: row 0 is the upper half of the
//! back wall, row 1 the back wall, the first and the last columns the side
//! walls; the bottom is open and the lions come in by the middle of it (the
//! [`door`](DenLayout::door)), which nothing may block.
//!
//! # Reading a file never fails
//!
//! [`DenLayout::load`] takes any text. What it does not understand it
//! leaves out and says so in a note: a piece whose id is unknown, a piece
//! outside the room (moved inside if it fits there, dropped otherwise), a
//! piece on another, a style that does not exist, a room too small or too
//! large. A text that is no den at all gives the default one, and a note.

use serde::{Deserialize, Serialize};

use crate::assets::{floor, wall, FLOORS, WALLS};
use crate::catalogue::{find, Entry, Placement, View};
use crate::world::Tile;

/// The version of the format written.
pub const VERSION: u32 = 1;
/// The narrowest room, walls included.
pub const MIN_COLS: i32 = 8;
/// The lowest room, the two rows of the back wall included.
pub const MIN_ROWS: i32 = 8;
/// The widest room.
pub const MAX_COLS: i32 = 30;
/// The highest room.
pub const MAX_ROWS: i32 = 19;

/// A piece of the catalogue, placed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    /// Its id in the catalogue.
    pub id: String,
    /// The column of the left tile of its footprint.
    pub x: i32,
    /// The row of the top tile of its footprint.
    pub y: i32,
    /// How many times it is turned: which of its views is used.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turn: u8,
}

fn is_zero(turn: &u8) -> bool {
    *turn == 0
}

/// A patch of another floor laid over the room's: a carpet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Carpet {
    /// Left column.
    pub x: i32,
    /// Top row.
    pub y: i32,
    /// Width in tiles.
    #[serde(default = "one")]
    pub w: i32,
    /// Height in tiles.
    #[serde(default = "one")]
    pub h: i32,
    /// The floor it is made of.
    pub style: String,
}

fn one() -> i32 {
    1
}

impl Carpet {
    fn covers(&self, tile: Tile) -> bool {
        tile.x >= self.x && tile.x < self.x + self.w && tile.y >= self.y && tile.y < self.y + self.h
    }
}

/// A den.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenLayout {
    /// The version of the format.
    #[serde(default)]
    pub version: u32,
    /// What the den is called.
    #[serde(default)]
    pub name: String,
    /// The id of the prefab it was made from, if it was: what "reset" goes
    /// back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<String>,
    /// The width of the room in tiles, walls included.
    pub cols: i32,
    /// The height of the room in tiles, the back wall's two rows included.
    pub rows: i32,
    /// The floor.
    #[serde(default)]
    pub floor: String,
    /// The walls.
    #[serde(default)]
    pub wall: String,
    /// Carpets, the lowest first.
    #[serde(default)]
    pub carpets: Vec<Carpet>,
    /// What is in the room.
    #[serde(default)]
    pub items: Vec<Placed>,
}

/// Why a piece cannot be where it is asked to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Why {
    /// No piece has this id.
    Unknown,
    /// It would stand outside the room, or in a wall.
    OutOfRoom,
    /// It goes on the back wall and this is not it.
    NotOnTheWall,
    /// Something is there already.
    InTheWay,
    /// It would block the way in.
    OnTheDoor,
}

impl Why {
    /// The reason in plain words.
    pub fn text(self) -> &'static str {
        match self {
            Why::Unknown => "No such piece.",
            Why::OutOfRoom => "It does not fit in the room there.",
            Why::NotOnTheWall => "It goes on the back wall.",
            Why::InTheWay => "Something is in the way.",
            Why::OnTheDoor => "It would block the way in.",
        }
    }
}

/// A den as it was read, and what was changed to make it one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// The den: always a valid one.
    pub layout: DenLayout,
    /// What was left out or put right, in plain words. Empty for a file
    /// that was right.
    pub notes: Vec<String>,
}

/// Pieces with their place in the file.
type Numbered = Vec<(usize, Placed)>;

fn tiles(x: i32, y: i32, w: i32, h: i32) -> impl Iterator<Item = Tile> {
    (y..y + h).flat_map(move |row| (x..x + w).map(move |col| Tile::new(col, row)))
}

impl Placed {
    /// A piece at a tile, not turned.
    pub fn new(id: &str, x: i32, y: i32) -> Self {
        Self {
            id: id.to_owned(),
            x,
            y,
            turn: 0,
        }
    }

    /// The piece turned `turn` times.
    pub fn turned(mut self, turn: u8) -> Self {
        self.turn = turn;
        self
    }

    /// Its entry in the catalogue and the view it is turned to.
    pub fn piece(&self) -> Option<(&'static Entry, &'static View)> {
        let entry = find(&self.id)?;
        Some((entry, entry.view(self.turn)))
    }

    /// Every tile of its footprint.
    pub fn footprint(&self) -> Vec<Tile> {
        match self.piece() {
            Some((_, view)) => tiles(self.x, self.y, view.w, view.h).collect(),
            None => Vec::new(),
        }
    }

    /// The tiles it really stands on: its footprint less the rows at the
    /// top that only stand up behind.
    pub fn base(&self) -> Vec<Tile> {
        match self.piece() {
            Some((entry, view)) => {
                let behind = entry.background.min(view.h - 1);
                tiles(self.x, self.y + behind, view.w, view.h - behind).collect()
            }
            None => Vec::new(),
        }
    }
}

impl DenLayout {
    /// An empty room of this size, with the default floor and walls.
    pub fn empty(name: &str, cols: i32, rows: i32) -> Self {
        Self {
            version: VERSION,
            name: name.to_owned(),
            based_on: None,
            cols: cols.clamp(MIN_COLS, MAX_COLS),
            rows: rows.clamp(MIN_ROWS, MAX_ROWS),
            floor: FLOORS[0].id.to_owned(),
            wall: WALLS[0].id.to_owned(),
            carpets: Vec::new(),
            items: Vec::new(),
        }
    }

    /// The tile of the bottom row the lions come in by.
    pub fn door(&self) -> Tile {
        Tile::new(self.cols / 2, self.rows - 1)
    }

    /// Whether a tile is floor: inside the walls, under the back wall.
    pub fn is_floor(&self, tile: Tile) -> bool {
        tile.x >= 1 && tile.x < self.cols - 1 && tile.y >= 2 && tile.y < self.rows
    }

    /// The index of the floor of a tile in [`FLOORS`]: the topmost carpet
    /// over it, or the room's.
    pub fn floor_at(&self, tile: Tile) -> usize {
        self.carpets
            .iter()
            .rev()
            .find(|carpet| carpet.covers(tile))
            .and_then(|carpet| floor(&carpet.style))
            .or_else(|| floor(&self.floor))
            .unwrap_or(0)
    }

    /// The index of the walls in [`WALLS`].
    pub fn wall_style(&self) -> usize {
        wall(&self.wall).unwrap_or(0)
    }

    /// Whether the piece at `index` is on a table rather than on the floor.
    pub fn on_surface(&self, index: usize) -> bool {
        self.surface_under(&self.items[index], Some(index))
            .is_some()
    }

    /// The table a piece stands on: the index of the piece that is a
    /// surface and has all of the piece's base on it.
    pub fn surface_under(&self, placed: &Placed, skip: Option<usize>) -> Option<usize> {
        let (entry, _) = placed.piece()?;
        if entry.placement != Placement::Surface {
            return None;
        }
        let base = placed.base();
        self.items.iter().enumerate().position(|(index, other)| {
            Some(index) != skip && other.piece().is_some_and(|(table, _)| table.surface) && {
                let top = other.footprint();
                base.iter().all(|tile| top.contains(tile))
            }
        })
    }

    /// The pieces that stand on the table at `index`.
    pub fn on_top_of(&self, index: usize) -> Vec<usize> {
        (0..self.items.len())
            .filter(|other| {
                *other != index
                    && self.surface_under(&self.items[*other], Some(*other)) == Some(index)
            })
            .collect()
    }

    /// Whether a piece can be where it says, among the others. `skip` is
    /// the index of the piece itself when it is already in the room (it is
    /// being moved or turned), and of whatever moves with it.
    pub fn check(&self, placed: &Placed, skip: &[usize]) -> Result<(), Why> {
        let (entry, view) = placed.piece().ok_or(Why::Unknown)?;
        let (x, y, w, h) = (placed.x, placed.y, view.w, view.h);
        let others = || {
            self.items
                .iter()
                .enumerate()
                .filter(|(index, _)| !skip.contains(index))
                .filter_map(|(index, other)| Some((index, other, other.piece()?.0)))
        };
        if entry.placement == Placement::Wall {
            if y < 0 || y + h > 2 {
                return Err(Why::NotOnTheWall);
            }
            if x < 1 || x + w > self.cols - 1 {
                return Err(Why::OutOfRoom);
            }
            let mine = placed.footprint();
            let taken = others().any(|(_, other, piece)| {
                piece.placement == Placement::Wall
                    && other.footprint().iter().any(|tile| mine.contains(tile))
            });
            return if taken { Err(Why::InTheWay) } else { Ok(()) };
        }
        let base = placed.base();
        let top = if entry.background > 0 { 1 } else { 2 };
        if x < 1 || x + w > self.cols - 1 || y < top || y + h > self.rows {
            return Err(Why::OutOfRoom);
        }
        if base.iter().any(|tile| !self.is_floor(*tile)) {
            return Err(Why::OutOfRoom);
        }
        if entry.solid && base.contains(&self.door()) {
            return Err(Why::OnTheDoor);
        }
        // On a table: only another thing on the same tile is in the way.
        let table = self
            .items
            .iter()
            .enumerate()
            .filter(|(index, _)| !skip.contains(index))
            .any(|(_, other)| {
                entry.placement == Placement::Surface
                    && other.piece().is_some_and(|(piece, _)| piece.surface)
                    && {
                        let top = other.footprint();
                        base.iter().all(|tile| top.contains(tile))
                    }
            });
        for (index, other, piece) in others() {
            if piece.placement == Placement::Wall {
                continue;
            }
            // A rug lies under everything; only two rugs collide.
            if entry.flat != piece.flat {
                continue;
            }
            let theirs_on_table = self.on_surface(index);
            if table != theirs_on_table {
                continue;
            }
            if other.base().iter().any(|tile| base.contains(tile)) {
                return Err(Why::InTheWay);
            }
        }
        Ok(())
    }

    /// The den as JSON, indented.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    /// Reads a den from the text of a file. It never fails: see the module
    /// documentation. `fallback` is the den to use when the text is none.
    pub fn load(text: &str, fallback: &DenLayout) -> Loaded {
        match serde_json::from_str::<DenLayout>(text) {
            Ok(layout) => layout.repaired(),
            Err(error) => Loaded {
                layout: fallback.clone(),
                notes: vec![format!(
                    "This is not a den ({error}): {} is used instead.",
                    fallback.name
                )],
            },
        }
    }

    /// The den made valid, with what had to change.
    pub fn repaired(mut self) -> Loaded {
        let mut notes = Vec::new();
        if self.version > VERSION {
            notes.push(format!(
                "The den was written by a newer Leon (format {}): what this one does not know is left out.",
                self.version
            ));
        }
        self.version = VERSION;
        if self.name.trim().is_empty() {
            self.name = "Den".to_owned();
        }
        let (cols, rows) = (
            self.cols.clamp(MIN_COLS, MAX_COLS),
            self.rows.clamp(MIN_ROWS, MAX_ROWS),
        );
        if (cols, rows) != (self.cols, self.rows) {
            notes.push(format!(
                "A room of {} by {} is out of bounds: it is {cols} by {rows}.",
                self.cols, self.rows
            ));
            (self.cols, self.rows) = (cols, rows);
        }
        if floor(&self.floor).is_none() {
            if !self.floor.is_empty() {
                notes.push(format!("There is no floor called `{}`.", self.floor));
            }
            self.floor = FLOORS[0].id.to_owned();
        }
        if wall(&self.wall).is_none() {
            if !self.wall.is_empty() {
                notes.push(format!("There is no wall called `{}`.", self.wall));
            }
            self.wall = WALLS[0].id.to_owned();
        }
        self.carpets.retain_mut(|carpet| {
            let known = floor(&carpet.style).is_some();
            if !known {
                notes.push(format!("There is no carpet called `{}`.", carpet.style));
            }
            // Cut to the room, so that no number in a file is too large.
            let (right, bottom) = (
                carpet.x.saturating_add(carpet.w).min(cols),
                carpet.y.saturating_add(carpet.h).min(rows),
            );
            carpet.x = carpet.x.max(0);
            carpet.y = carpet.y.max(0);
            carpet.w = right.saturating_sub(carpet.x);
            carpet.h = bottom.saturating_sub(carpet.y);
            known && carpet.w > 0 && carpet.h > 0
        });

        // The pieces go back in one by one, what stands on the floor before
        // what stands on it, each moved inside the room if it is outside.
        let items = std::mem::take(&mut self.items);
        let on_floor = |placed: &Placed| {
            placed
                .piece()
                .is_some_and(|(entry, _)| entry.placement != Placement::Surface)
        };
        let (first, second): (Numbered, Numbered) = items
            .into_iter()
            .filter(|placed| {
                let known = placed.piece().is_some();
                if !known {
                    notes.push(format!("There is no piece called `{}`.", placed.id));
                }
                known
            })
            .enumerate()
            .partition(|(_, placed)| on_floor(placed));
        // Kept in the order of the file, so that a den read and written
        // again is the same text.
        let mut order: Vec<usize> = Vec::new();
        for (index, mut placed) in first.into_iter().chain(second) {
            let Some((entry, view)) = placed.piece() else {
                continue;
            };
            placed.turn %= entry.views.len() as u8;
            let was = (placed.x, placed.y);
            let (low, high) = match entry.placement {
                Placement::Wall => (0, 2 - view.h),
                _ => (if entry.background > 0 { 1 } else { 2 }, self.rows - view.h),
            };
            placed.x = placed.x.clamp(1, (self.cols - 1 - view.w).max(1));
            placed.y = placed.y.clamp(low, high.max(low));
            match self.check(&placed, &[]) {
                Ok(()) => {
                    if was != (placed.x, placed.y) {
                        notes.push(format!(
                            "{} was outside the room: it is at {}, {} now.",
                            entry.name, placed.x, placed.y
                        ));
                    }
                    self.items.push(placed);
                    order.push(index);
                }
                Err(why) => notes.push(format!(
                    "{} at {}, {} is left out. {}",
                    entry.name,
                    was.0,
                    was.1,
                    why.text()
                )),
            }
        }
        let mut kept: Vec<(usize, Placed)> = order.into_iter().zip(self.items).collect();
        kept.sort_by_key(|(index, _)| *index);
        self.items = kept.into_iter().map(|(_, placed)| placed).collect();
        Loaded {
            layout: self,
            notes,
        }
    }
}
