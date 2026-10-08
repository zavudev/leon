//! The room as the simulation sees it: where a lion may be, what each place
//! is for, how it walks.
//!
//! A [`World`] is built from a [`DenLayout`] and nothing else, so the same
//! layout always gives the same den. It finds the places of the simulation
//! by the roles of what is in the room ([`crate::catalogue::Role`]):
//!
//! | [`Place`] | Its spots | Who goes |
//! | --- | --- | --- |
//! | `Desks` | the seats that face a computer | a lion that edits, thinks, or does nobody knows what |
//! | `Bench` | the little seats that face a computer | little ones at work |
//! | `Shelf` | the floor in front of a shelf | a lion that reads or searches |
//! | `Board` | the floor in front of a board | a lion that plans |
//! | `Lookout` | the floor in front of a window or a telescope | a lion on the web |
//! | `Rack` | the floor in front of a rack | a lion that runs a command |
//! | `Sun` | the seats that face no computer | idle and sleeping lions |
//! | `Entrance` | the tiles of an entrance rug, the nearest to the door first | lions that wait for the user |
//! | `Nest` | the nests | eggs |
//! | `Watch` | the floor beside a nest | a lion whose little one is out |
//!
//! A seat looks the way it is turned; a seat with no front (a bench) looks
//! at the table next to it. A seat *faces a computer* when one stands within
//! three tiles straight ahead of it; that computer is the seat's own and is
//! on while its lion works ([`World::computer_of`]).
//!
//! # No layout breaks the den
//!
//! A room may have no shelf, no seat, nothing at all. A place without spots,
//! or with all of them taken, overflows onto the nearest free floor
//! ([`World::claim`]): the lion does the same thing there, standing.
//! [`World::missing`] lists the places with no spot, for the editor to say
//! so. A seat nobody can reach is still a seat: a lion jumps to it rather
//! than not arrive.
//!
//! What never comes in front of a lion, the floor, the carpets, the walls,
//! what hangs on them and the rugs, is painted once ([`World::backdrop`]);
//! the rest is a list of [`Item`]s the painter sorts with the lions.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::assets::art;
use crate::bitmap::Bitmap;
use crate::catalogue::{Placement, Role};
use crate::layout::DenLayout;
use crate::pose::{Facing, TILE};

/// How far ahead of a seat its computer may be, in tiles.
pub const COMPUTER_REACH: i32 = 3;

/// A tile of the map, column and row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tile {
    /// Column, from the left.
    pub x: i32,
    /// Row, from the top.
    pub y: i32,
}

impl Tile {
    /// The tile at a column and a row.
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// How many steps apart two tiles are, walking along the grid.
    pub fn distance(self, other: Tile) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }

    /// The way from this tile to another, along the longer axis.
    pub fn facing_to(self, other: Tile) -> Facing {
        let (dx, dy) = (other.x - self.x, other.y - self.y);
        if dx.abs() > dy.abs() {
            if dx < 0 {
                Facing::Left
            } else {
                Facing::Right
            }
        } else if dy < 0 {
            Facing::Up
        } else {
            Facing::Down
        }
    }
}

/// A place of the den.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// The desks.
    Desks,
    /// The small table of the little ones.
    Bench,
    /// The bookshelf.
    Shelf,
    /// The window and the telescope.
    Lookout,
    /// The whiteboard.
    Board,
    /// The rack.
    Rack,
    /// The sofas: the sunny corner.
    Sun,
    /// The way in and out.
    Entrance,
    /// The nest, where eggs lie.
    Nest,
    /// Beside the nest.
    Watch,
}

impl Place {
    /// Every place.
    pub const ALL: [Place; 10] = [
        Place::Desks,
        Place::Bench,
        Place::Shelf,
        Place::Lookout,
        Place::Board,
        Place::Rack,
        Place::Sun,
        Place::Entrance,
        Place::Nest,
        Place::Watch,
    ];

    fn index(self) -> usize {
        Place::ALL
            .iter()
            .position(|place| *place == self)
            .unwrap_or(0)
    }
}

/// Where a lion is and which way it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spot {
    /// The tile.
    pub tile: Tile,
    /// The way it faces there.
    pub facing: Facing,
}

/// What a tile is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ground {
    /// Outside the room.
    Void,
    /// A wall.
    Wall,
    /// Open floor.
    Floor,
    /// A seat, or a nest: a walk ends there, none goes through.
    Seat,
    /// Furniture: no way through.
    Solid,
}

/// A piece of furniture in the room, as it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    /// Which piece of the layout it is.
    pub index: usize,
    /// The name of its picture.
    pub sprite: &'static str,
    /// The left edge of its picture, in pixels of the art.
    pub x: i32,
    /// The top edge of its picture.
    pub y: i32,
    /// How far down the room it stands, doubled: what is lower is drawn
    /// later, and a lion on a tile comes between its seat and its table.
    pub z: i32,
    /// Its picture is mirrored.
    pub mirrored: bool,
    /// What it is for.
    pub role: Option<Role>,
}

/// The den, laid out.
#[derive(Clone, Debug)]
pub struct World {
    /// Width in tiles, walls included.
    pub cols: i32,
    /// Height in tiles, the row over the back wall included.
    pub rows: i32,
    ground: Vec<Ground>,
    spots: [Vec<Spot>; 10],
    anchors: [Tile; 10],
    /// The tile of the bottom row lions come in and go out by.
    pub door: Tile,
    items: Vec<Item>,
    /// The computer of each work seat: the index of the piece.
    computers: HashMap<Tile, usize>,
    backdrop: Bitmap,
}

fn ahead(tile: Tile, facing: Facing, steps: i32) -> Tile {
    match facing {
        Facing::Up => Tile::new(tile.x, tile.y - steps),
        Facing::Down => Tile::new(tile.x, tile.y + steps),
        Facing::Left => Tile::new(tile.x - steps, tile.y),
        Facing::Right => Tile::new(tile.x + steps, tile.y),
    }
}

impl World {
    /// The den of a layout. The layout is taken as it is: one read from a
    /// file goes through [`DenLayout::load`] first.
    pub fn build(layout: &DenLayout) -> World {
        let (cols, rows) = (layout.cols, layout.rows);
        let mut world = World {
            cols,
            rows,
            ground: vec![Ground::Floor; (cols * rows).max(0) as usize],
            spots: Default::default(),
            anchors: [layout.door(); 10],
            door: layout.door(),
            items: Vec::new(),
            computers: HashMap::new(),
            backdrop: Bitmap::new(0, 0),
        };
        for x in 0..cols {
            world.set(Tile::new(x, 0), Ground::Void);
            world.set(Tile::new(x, 1), Ground::Wall);
        }
        for y in 1..rows {
            world.set(Tile::new(0, y), Ground::Wall);
            world.set(Tile::new(cols - 1, y), Ground::Wall);
        }

        // What stands where: the ground under each piece, and the pieces
        // that are drawn with the lions.
        let mut surfaces: HashSet<Tile> = HashSet::new();
        let mut computers: Vec<(usize, Vec<Tile>)> = Vec::new();
        for (index, placed) in layout.items.iter().enumerate() {
            let Some((entry, view)) = placed.piece() else {
                continue;
            };
            let on_table = layout.on_surface(index);
            if entry.surface {
                surfaces.extend(placed.footprint());
            }
            if entry.role == Some(Role::Computer) {
                computers.push((index, placed.footprint()));
            }
            if entry.placement != Placement::Wall && !on_table {
                let kind = if entry.solid {
                    Some(Ground::Solid)
                } else if matches!(entry.role, Some(Role::Seat | Role::Nest)) {
                    Some(Ground::Seat)
                } else {
                    None
                };
                for tile in placed.base() {
                    if let Some(kind) = kind {
                        world.set(tile, kind);
                    }
                }
            }
            if entry.placement == Placement::Wall || entry.flat {
                continue;
            }
            let picture = art().sprite(view.sprite);
            let bottom = (placed.y + view.h) * TILE;
            let z = match (entry.role, view.facing) {
                // A chair seen from behind hides the lower half of whoever
                // sits on it; any other seat is under its lion.
                (Some(Role::Seat), Some(Facing::Up)) => 2 * bottom + 2,
                (Some(Role::Seat), _) => {
                    2 * (placed.y + entry.background.min(view.h - 1) + 1) * TILE
                }
                // What stands on a table is drawn just after the table.
                _ if on_table => {
                    let table = layout
                        .surface_under(placed, Some(index))
                        .and_then(|table| {
                            let table = &layout.items[table];
                            Some((table.y + table.piece()?.1.h) * TILE)
                        })
                        .unwrap_or(bottom);
                    2 * table.max(bottom) + 1
                }
                // A nest is under the egg that lies in it.
                (Some(Role::Nest), _) => 2 * placed.y * TILE + 1,
                _ => 2 * bottom,
            };
            world.items.push(Item {
                index,
                sprite: view.sprite,
                x: placed.x * TILE,
                y: (placed.y + view.h) * TILE - picture.h,
                z,
                mirrored: view.mirrored,
                role: entry.role,
            });
        }
        world.items.sort_by_key(|item| (item.z, item.x, item.index));

        // The places, by the roles of the pieces.
        let mut desks = Vec::new();
        let mut entrance = Vec::new();
        for placed in &layout.items {
            let Some((entry, view)) = placed.piece() else {
                continue;
            };
            let below = |world: &World| -> Vec<Tile> {
                // Straight under the piece: the first row of floor there.
                (placed.x..placed.x + view.w)
                    .map(|x| Tile::new(x, (placed.y + view.h).max(2)))
                    .filter(|tile| world.ground(*tile) == Ground::Floor)
                    .collect()
            };
            match entry.role {
                Some(Role::Seat) => {
                    for tile in placed.base() {
                        let facing = view.facing.unwrap_or_else(|| {
                            [Facing::Up, Facing::Down, Facing::Left, Facing::Right]
                                .into_iter()
                                .find(|facing| surfaces.contains(&ahead(tile, *facing, 1)))
                                .unwrap_or(Facing::Down)
                        });
                        let computer = (1..=COMPUTER_REACH).find_map(|steps| {
                            let looked = ahead(tile, facing, steps);
                            computers
                                .iter()
                                .find(|(_, tiles)| tiles.contains(&looked))
                                .map(|(index, _)| *index)
                        });
                        match computer {
                            Some(computer) => {
                                world.computers.insert(tile, computer);
                                desks.push((entry.little, Spot { tile, facing }));
                            }
                            None => world.spots[Place::Sun.index()].push(Spot { tile, facing }),
                        }
                    }
                }
                Some(Role::Shelf) => world.front(Place::Shelf, below(&world)),
                Some(Role::Board) => world.front(Place::Board, below(&world)),
                Some(Role::Lookout) => world.front(Place::Lookout, below(&world)),
                Some(Role::Rack) => world.front(Place::Rack, below(&world)),
                Some(Role::Entrance) => entrance.extend(placed.footprint()),
                Some(Role::Nest) => {
                    for tile in placed.base() {
                        world.spots[Place::Nest.index()].push(Spot {
                            tile,
                            facing: Facing::Down,
                        });
                    }
                }
                Some(Role::Computer) | None => {}
            }
        }
        // A little seat is a little one's; with none of them, the little
        // ones take what the grown lions leave.
        for (little, spot) in desks {
            let place = if little { Place::Bench } else { Place::Desks };
            world.spots[place.index()].push(spot);
        }
        // On the rug: the nearest to the door first, and no two side by
        // side while there is room, so that their names can be read.
        let door = world.door;
        entrance.retain(|tile| world.ground(*tile) == Ground::Floor && *tile != door);
        entrance.sort_by_key(|tile| {
            let apart = (tile.x - door.x).abs() % 2 + (tile.y - door.y + 1).abs() % 2;
            (
                apart % 2,
                tile.distance(Tile::new(door.x, door.y - 1)),
                tile.y,
                tile.x,
            )
        });
        world.front_facing(Place::Entrance, entrance, Facing::Down);
        // Beside a nest: left, right, then over it.
        let nests: Vec<Tile> = world.spots[Place::Nest.index()]
            .iter()
            .map(|spot| spot.tile)
            .collect();
        for (dx, dy, facing) in [
            (-1, 0, Facing::Right),
            (1, 0, Facing::Left),
            (0, -1, Facing::Down),
        ] {
            let beside: Vec<Tile> = nests
                .iter()
                .map(|nest| Tile::new(nest.x + dx, nest.y + dy))
                .filter(|tile| world.ground(*tile) == Ground::Floor && *tile != door)
                .collect();
            world.front_facing(Place::Watch, beside, facing);
        }

        // Where a place overflows from: its first spot, or the middle of
        // the room for one that has none.
        let middle = Tile::new(cols / 2, (rows / 2).max(2));
        for place in Place::ALL {
            world.anchors[place.index()] = world.spots[place.index()]
                .first()
                .map_or(middle, |spot| spot.tile);
        }
        if world.spots[Place::Entrance.index()].is_empty() {
            world.anchors[Place::Entrance.index()] = Tile::new(door.x, door.y - 1);
        }
        world.backdrop = world.paint(layout);
        world
    }

    fn set(&mut self, tile: Tile, kind: Ground) {
        if tile.x >= 0 && tile.y >= 0 && tile.x < self.cols && tile.y < self.rows {
            self.ground[(tile.y * self.cols + tile.x) as usize] = kind;
        }
    }

    /// Adds spots that look up at a piece.
    fn front(&mut self, place: Place, tiles: Vec<Tile>) {
        self.front_facing(place, tiles, Facing::Up);
    }

    fn front_facing(&mut self, place: Place, tiles: Vec<Tile>, facing: Facing) {
        for tile in tiles {
            let taken = self
                .spots
                .iter()
                .any(|spots| spots.iter().any(|spot| spot.tile == tile));
            if !taken && tile != self.door {
                self.spots[place.index()].push(Spot { tile, facing });
            }
        }
    }

    /// The places that have no spot in this room: what a lion sent there
    /// will do standing on free floor. `Watch` is not listed: it follows
    /// the nest.
    pub fn missing(&self) -> Vec<Place> {
        Place::ALL
            .into_iter()
            .filter(|place| *place != Place::Watch && self.spots(*place).is_empty())
            .collect()
    }

    /// The computer a lion at this seat works at: the index of the piece in
    /// the layout.
    pub fn computer_of(&self, seat: Tile) -> Option<usize> {
        self.computers.get(&seat).copied()
    }

    /// What a tile is made of. Outside the map there is nothing.
    pub fn ground(&self, tile: Tile) -> Ground {
        if tile.x < 0 || tile.y < 0 || tile.x >= self.cols || tile.y >= self.rows {
            return Ground::Void;
        }
        self.ground[(tile.y * self.cols + tile.x) as usize]
    }

    /// Whether a lion can walk over a tile on its way somewhere else.
    pub fn walkable(&self, tile: Tile) -> bool {
        self.ground(tile) == Ground::Floor
    }

    /// The tile just outside the open side of the room: where a lion that
    /// comes in is first seen, and where one that leaves is last.
    pub fn outside(&self) -> Tile {
        Tile::new(self.door.x, self.rows)
    }

    /// The spots of a place, in the order they are taken.
    pub fn spots(&self, place: Place) -> &[Spot] {
        &self.spots[place.index()]
    }

    /// The tile a place is counted from when it overflows.
    pub fn anchor(&self, place: Place) -> Tile {
        self.anchors[place.index()]
    }

    /// The furniture, the farthest first.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    fn tiles(&self) -> impl Iterator<Item = Tile> {
        let cols = self.cols;
        (0..self.rows).flat_map(move |y| (0..cols).map(move |x| Tile::new(x, y)))
    }

    fn is_spot(&self, tile: Tile) -> bool {
        self.spots
            .iter()
            .any(|spots| spots.iter().any(|spot| spot.tile == tile))
    }

    /// Where a lion that goes to `place` is to be, given the tiles that are
    /// `taken`: the first free spot of the place, or else the nearest free
    /// tile of open floor, first one that is no other place's spot and has
    /// no neighbour, then any. With the whole floor taken it answers the
    /// place's first spot: two lions share a tile.
    pub fn claim(&self, place: Place, taken: &HashSet<Tile>) -> Spot {
        // A little one with no little seat takes a grown lion's.
        let also: &[Spot] = if place == Place::Bench {
            self.spots(Place::Desks)
        } else {
            &[]
        };
        if let Some(spot) = self
            .spots(place)
            .iter()
            .chain(also)
            .find(|spot| !taken.contains(&spot.tile))
        {
            return *spot;
        }
        let anchor = self.anchor(place);
        let order = self.reach(anchor);
        let free =
            |tile: &Tile| self.walkable(*tile) && *tile != self.door && !taken.contains(tile);
        let alone = |tile: &Tile| {
            neighbours(*tile)
                .iter()
                .all(|next| !taken.contains(next) && !self.is_spot(*next))
        };
        let found = order
            .iter()
            .find(|tile| free(tile) && !self.is_spot(**tile) && alone(tile))
            .or_else(|| {
                order
                    .iter()
                    .find(|tile| free(tile) && !self.is_spot(**tile))
            })
            .or_else(|| order.iter().find(|tile| free(tile)));
        match found {
            Some(tile) => Spot {
                tile: *tile,
                facing: match place {
                    Place::Shelf | Place::Lookout | Place::Board | Place::Rack => Facing::Up,
                    Place::Watch => Facing::Right,
                    _ => Facing::Down,
                },
            },
            None => self.spots(place).first().copied().unwrap_or(Spot {
                tile: anchor,
                facing: Facing::Down,
            }),
        }
    }

    /// The first spot of a place that nobody has, or none: a place holds as
    /// many as it has spots, and nobody is sent to one that is full.
    pub fn free_spot(&self, place: Place, taken: &HashSet<Tile>) -> Option<Spot> {
        self.spots(place)
            .iter()
            .find(|spot| !taken.contains(&spot.tile))
            .copied()
    }

    /// Whether a tile is the spot of a place.
    pub fn spot_at(&self, tile: Tile) -> bool {
        self.is_spot(tile)
    }

    /// A tile of bare floor for a lion that has no seat, near `near`: one
    /// that is nobody's and no place's spot, not the way in, and as far
    /// from the others as the room allows. First a tile with nobody on any
    /// of the eight around it (and, while there is one, no spot of a place
    /// there either), then one with nobody beside, above or below it, then
    /// any free one; in a room with no floor left, `near` itself.
    pub fn standing(&self, near: Tile, taken: &HashSet<Tile>) -> Spot {
        let order = self.reach(near);
        let free =
            |tile: &Tile| self.walkable(*tile) && *tile != self.door && !taken.contains(tile);
        let around = |tile: Tile, diagonal: bool| {
            (-1..=1)
                .flat_map(move |dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter(move |(dx, dy)| (*dx, *dy) != (0, 0) && (diagonal || *dx == 0 || *dy == 0))
                .map(move |(dx, dy)| Tile::new(tile.x + dx, tile.y + dy))
        };
        let apart = |tile: &Tile, diagonal: bool| {
            around(*tile, diagonal).all(|next| !taken.contains(&next))
        };
        // Best of all, also away from where others come to stand: the
        // spots of the places.
        let clear = |tile: &Tile| around(*tile, true).all(|next| !self.is_spot(next));
        let found = order
            .iter()
            .find(|tile| free(tile) && !self.is_spot(**tile) && apart(tile, true) && clear(tile))
            .or_else(|| {
                order
                    .iter()
                    .find(|tile| free(tile) && !self.is_spot(**tile) && apart(tile, true))
            })
            .or_else(|| {
                order
                    .iter()
                    .find(|tile| free(tile) && !self.is_spot(**tile) && apart(tile, false))
            })
            .or_else(|| {
                order
                    .iter()
                    .find(|tile| free(tile) && !self.is_spot(**tile))
            })
            .or_else(|| order.iter().find(|tile| free(tile)));
        Spot {
            tile: found.copied().unwrap_or(near),
            facing: Facing::Down,
        }
    }

    /// Every tile of open floor, nearest to `from` first.
    fn reach(&self, from: Tile) -> Vec<Tile> {
        let mut seen: HashSet<Tile> = HashSet::new();
        let mut queue: VecDeque<Tile> = VecDeque::new();
        let mut order = Vec::new();
        // The anchor may be furniture, or a corner shut in by it: start
        // from the nearest floor that the door leads to.
        let open = self.open_floor();
        let start = self
            .tiles()
            .filter(|tile| open.contains(tile))
            .min_by_key(|tile| (tile.distance(from), tile.y, tile.x))
            .unwrap_or(from);
        seen.insert(start);
        queue.push_back(start);
        while let Some(tile) = queue.pop_front() {
            order.push(tile);
            for next in neighbours(tile) {
                if self.walkable(next) && seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        order
    }

    /// The floor that can be walked to from the door.
    fn open_floor(&self) -> HashSet<Tile> {
        let mut seen: HashSet<Tile> = HashSet::new();
        let mut queue: VecDeque<Tile> = VecDeque::new();
        if self.walkable(self.door) {
            seen.insert(self.door);
            queue.push_back(self.door);
        }
        while let Some(tile) = queue.pop_front() {
            for next in neighbours(tile) {
                if self.walkable(next) && seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        seen
    }

    /// The tiles a lion walks over from one tile to another, both included.
    /// It goes around the furniture, and around the seats of others unless
    /// that is the only way; a walk from or to [`World::outside`] goes by the door. With no
    /// way through it jumps.
    pub fn path(&self, from: Tile, to: Tile) -> Vec<Tile> {
        if from == to {
            return vec![from];
        }
        let outside = self.outside();
        if from == outside {
            let mut path = vec![from];
            path.extend(self.path(self.door, to));
            return path;
        }
        if to == outside {
            let mut path = self.path(from, self.door);
            path.push(to);
            return path;
        }
        let inside =
            |tile: Tile| tile.x >= 0 && tile.y >= 0 && tile.x < self.cols && tile.y < self.rows;
        if !inside(from) || !inside(to) {
            return vec![from, to];
        }
        // First around every seat; if a seat is walled in by others, then
        // over them: a lion squeezes past a bench rather than not arrive.
        self.search(from, to, false)
            .or_else(|| self.search(from, to, true))
            .unwrap_or_else(|| vec![from, to])
    }

    fn search(&self, from: Tile, to: Tile, over_seats: bool) -> Option<Vec<Tile>> {
        let index = |tile: Tile| (tile.y * self.cols + tile.x) as usize;
        let open = |tile: Tile| match self.ground(tile) {
            Ground::Floor => true,
            Ground::Seat => over_seats || tile == to,
            _ => false,
        };
        let mut came: Vec<Option<Tile>> = vec![None; (self.cols * self.rows) as usize];
        let mut queue: VecDeque<Tile> = VecDeque::new();
        came[index(from)] = Some(from);
        queue.push_back(from);
        while let Some(tile) = queue.pop_front() {
            if tile == to {
                break;
            }
            for next in neighbours(tile) {
                if open(next) && came[index(next)].is_none() {
                    came[index(next)] = Some(tile);
                    queue.push_back(next);
                }
            }
        }
        came[index(to)]?;
        let mut path = vec![to];
        let mut at = to;
        while at != from {
            at = came[index(at)].unwrap_or(from);
            path.push(at);
        }
        path.reverse();
        Some(path)
    }

    /// Everything of the den that never comes in front of a lion, as one
    /// picture the size of the map: the floor and its carpets, the rugs, the
    /// walls and what hangs on them.
    pub fn backdrop(&self) -> &Bitmap {
        &self.backdrop
    }

    fn paint(&self, layout: &DenLayout) -> Bitmap {
        let art = art();
        let mut out = Bitmap::new(self.cols * TILE, self.rows * TILE);
        for tile in self.tiles() {
            if matches!(self.ground(tile), Ground::Void | Ground::Wall) {
                continue;
            }
            out.draw(
                &art.floors[layout.floor_at(tile)],
                tile.x * TILE,
                tile.y * TILE,
            );
        }
        let flat = |wanted: bool| {
            layout.items.iter().filter_map(move |placed| {
                let (entry, view) = placed.piece()?;
                (entry.flat == wanted && (entry.flat || entry.placement == Placement::Wall))
                    .then_some((placed, view))
            })
        };
        for (placed, view) in flat(true) {
            let picture = art.sprite(view.sprite);
            out.draw_part(
                picture,
                0,
                0,
                picture.w,
                picture.h,
                placed.x * TILE,
                placed.y * TILE,
                view.mirrored,
            );
        }
        // The walls: each piece by the walls around it, standing up from
        // its tile.
        let walls = &art.walls[layout.wall_style()];
        let wall = |x: i32, y: i32| self.ground(Tile::new(x, y)) == Ground::Wall;
        for tile in self.tiles().filter(|tile| wall(tile.x, tile.y)) {
            let mask = i32::from(wall(tile.x, tile.y - 1))
                | i32::from(wall(tile.x + 1, tile.y)) << 1
                | i32::from(wall(tile.x, tile.y + 1)) << 2
                | i32::from(wall(tile.x - 1, tile.y)) << 3;
            out.draw(&walls[mask as usize], tile.x * TILE, tile.y * TILE - TILE);
        }
        for (placed, view) in flat(false) {
            let picture = art.sprite(view.sprite);
            out.draw_part(
                picture,
                0,
                0,
                picture.w,
                picture.h,
                placed.x * TILE,
                (placed.y + view.h) * TILE - picture.h,
                view.mirrored,
            );
        }
        out
    }
}

/// The four tiles next to a tile, in the order a search tries them.
fn neighbours(tile: Tile) -> [Tile; 4] {
    [
        Tile::new(tile.x, tile.y - 1),
        Tile::new(tile.x - 1, tile.y),
        Tile::new(tile.x + 1, tile.y),
        Tile::new(tile.x, tile.y + 1),
    ]
}
