//! The dens that come with Leon.
//!
//! Each prefab is a whole [`DenLayout`], written here piece by piece, that
//! serves every role: a lion has somewhere to type, read, plan, run, look
//! out, rest, wait and hatch in any of them. They differ in character and in
//! how many lions have a desk. A test builds every one and checks that it is
//! valid as written and that no place is missing.
//!
//! The first, [`OFFICE`], is the default: what a user who never chose a den
//! sees.

use crate::layout::{Carpet, DenLayout, Placed};

/// The id of the default den.
pub const OFFICE: &str = "office";

/// A prefab: its id, and the den.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefab {
    /// The id it is chosen by. It never changes.
    pub id: &'static str,
    /// What it is like, in a line.
    pub about: &'static str,
    /// The den.
    pub layout: DenLayout,
}

struct Room(DenLayout);

impl Room {
    fn new(name: &str, cols: i32, rows: i32, floor: &str, wall: &str) -> Self {
        let mut layout = DenLayout::empty(name, cols, rows);
        layout.floor = floor.to_owned();
        layout.wall = wall.to_owned();
        Self(layout)
    }

    fn put(&mut self, id: &str, x: i32, y: i32) -> &mut Self {
        self.0.items.push(Placed::new(id, x, y));
        self
    }

    fn turned(&mut self, id: &str, x: i32, y: i32, turn: u8) -> &mut Self {
        self.0.items.push(Placed::new(id, x, y).turned(turn));
        self
    }

    fn carpet(&mut self, style: &str, x: i32, y: i32, w: i32, h: i32) -> &mut Self {
        self.0.carpets.push(Carpet {
            x,
            y,
            w,
            h,
            style: style.to_owned(),
        });
        self
    }

    /// A desk for two, seen from the front, its top row at `top`: a
    /// computer and a bench at each end.
    fn desk(&mut self, x: i32, top: i32) -> &mut Self {
        self.put("desk", x, top);
        for seat in [x, x + 2] {
            self.put("pc", seat, top).put("bench", seat, top + 2);
        }
        self
    }

    /// The little ones' corner: a small table with two computers, their
    /// benches under it and two nests under those.
    fn nursery(&mut self, x: i32, top: i32) -> &mut Self {
        self.put("small_table", x, top);
        for seat in [x, x + 1] {
            self.put("pc", seat, top)
                .put("wooden_bench", seat, top + 2)
                .put("nest", seat, top + 3);
        }
        self
    }

    /// A study desk for two: a desk with two computers seen from the front
    /// and a wooden chair in front of each, so that the reader looks at a
    /// screen one can see.
    fn study(&mut self, x: i32, top: i32) -> &mut Self {
        self.put("desk", x, top);
        for seat in [x, x + 2] {
            self.put("pc", seat, top)
                .turned("wooden_chair", seat, top + 1, 2);
        }
        self
    }

    /// A sofa over a coffee table with a cup on it, and a sofa at its side.
    fn lounge(&mut self, x: i32, top: i32) -> &mut Self {
        self.put("sofa", x + 1, top)
            .put("coffee_table", x + 1, top + 1)
            .put("coffee", x + 1, top + 1)
            .turned("sofa", x, top + 1, 1)
    }

    fn done(&mut self, id: &'static str, about: &'static str) -> Prefab {
        let mut layout = self.0.clone();
        layout.based_on = Some(id.to_owned());
        Prefab { id, about, layout }
    }
}

fn office() -> Prefab {
    let mut room = Room::new("The office", 20, 15, "wood", "rock");
    room.put("double_bookshelf", 1, 0)
        .put("double_bookshelf", 3, 0)
        .put("clock", 5, 0)
        .put("whiteboard", 6, 0)
        .put("painting", 8, 0)
        .put("window", 10, 0)
        .put("telescope", 11, 1)
        .put("hanging_plant", 12, 0)
        .put("lion_painting", 13, 0)
        .put("plant", 15, 1)
        .put("rack", 16, 1)
        .put("rack", 17, 1)
        .put("rack", 18, 1);
    // Two rows of three desks, two tiles of aisle between any two.
    for top in [4, 8] {
        for x in [2, 7, 12] {
            room.desk(x, top);
        }
    }
    room.nursery(16, 4)
        .carpet("red", 1, 12, 5, 3)
        .lounge(1, 12)
        .put("plant", 6, 11)
        .put("rug", 9, 13)
        .put("bin", 13, 12)
        .put("large_plant", 16, 11);
    room.done(
        OFFICE,
        "Six desks for two in two rows, a sofa and a nursery. Twelve at work.",
    )
}

/// The office as it was before it grew: 14 by 11, three desks. The tests
/// that put a piece on a tile they name are written against this room, so
/// that the built-in one can change without them.
#[cfg(test)]
pub(crate) fn small_office() -> DenLayout {
    let mut room = Room::new("The office", 14, 11, "wood", "rock");
    room.put("double_bookshelf", 1, 0)
        .put("clock", 3, 0)
        .put("whiteboard", 4, 0)
        .put("painting", 6, 0)
        .put("window", 7, 0)
        .put("hanging_plant", 9, 0)
        .put("telescope", 8, 1)
        .put("rack", 11, 1)
        .put("rack", 12, 1)
        .desk(2, 4)
        .desk(6, 4)
        .desk(10, 4)
        .carpet("red", 1, 8, 4, 3)
        .lounge(1, 8)
        .put("plant", 4, 7)
        .put("rug", 6, 9)
        .put("bin", 10, 8)
        .nursery(11, 7);
    room.done(OFFICE, "Three desks for two.").layout
}

fn open_plan() -> Prefab {
    let mut room = Room::new("The open plan", 26, 15, "pine", "ash");
    room.put("double_bookshelf", 1, 0)
        .put("double_bookshelf", 3, 0)
        .put("clock", 5, 0)
        .put("whiteboard", 7, 0)
        .put("whiteboard", 9, 0)
        .put("lion_painting", 11, 0)
        .put("window", 13, 0)
        .put("telescope", 15, 1)
        .put("large_painting", 17, 0)
        .put("hanging_plant", 20, 0)
        .put("rack", 22, 1)
        .put("rack", 23, 1)
        .put("rack", 24, 1);
    for x in [2, 6, 10, 14, 18, 22] {
        room.desk(x, 4);
    }
    for x in [2, 6, 10, 14, 18] {
        room.desk(x, 8);
    }
    room.carpet("green", 1, 12, 5, 3)
        .lounge(1, 12)
        .put("plant_2", 5, 11)
        .carpet("ochre", 11, 12, 5, 3)
        .put("rug", 12, 13)
        .put("cactus", 17, 12)
        .put("bin", 21, 13)
        .nursery(23, 11);
    room.done(
        "open-plan",
        "Rows of desks under a long wall. Twenty-two at work.",
    )
}

fn library() -> Prefab {
    let mut room = Room::new("The library", 18, 13, "walnut", "moss");
    for x in [1, 3, 5, 7] {
        room.put("double_bookshelf", x, 0);
    }
    room.put("clock", 9, 0)
        .put("whiteboard", 10, 0)
        .put("window", 13, 0)
        .put("telescope", 15, 1)
        .put("rack", 16, 1)
        .carpet("green", 1, 3, 6, 7)
        .study(2, 3)
        .study(2, 6)
        .carpet("green", 8, 3, 6, 7)
        .study(9, 3)
        .study(9, 6)
        .put("large_plant", 15, 3)
        .carpet("plum", 1, 10, 5, 3)
        .lounge(1, 10)
        .put("plant", 5, 9)
        .put("rug", 8, 11)
        .put("pot", 12, 12)
        .nursery(15, 9);
    room.done(
        "library",
        "Shelves along the wall and four study desks on green carpets. Eight at work.",
    )
}

fn server_room() -> Prefab {
    let mut room = Room::new("The server room", 18, 11, "slate", "navy");
    room.put("bookshelf", 1, 0)
        .put("whiteboard", 3, 0)
        .put("window", 6, 0)
        .put("telescope", 8, 1)
        .put("lion_painting", 9, 0);
    for x in 10..=16 {
        room.put("rack", x, 1);
    }
    room.carpet("blue", 1, 3, 9, 4)
        .desk(2, 4)
        .desk(6, 4)
        .put("small_table", 11, 5)
        .put("pc", 11, 5)
        .put("pc", 12, 5)
        .turned("chair", 11, 7, 2)
        .turned("chair", 12, 7, 2)
        .put("bin", 14, 6)
        .turned("sofa", 1, 9, 1)
        .put("sofa", 2, 8)
        .put("cactus", 4, 7)
        .put("rug", 8, 9)
        .put("small_table", 15, 7)
        .put("pc", 15, 7)
        .put("wooden_bench", 15, 9)
        .put("nest", 15, 10)
        .put("nest", 16, 10);
    room.done(
        "server-room",
        "A wall of racks and a cold floor. Six at work, and room to watch commands run.",
    )
}

fn lounge() -> Prefab {
    let mut room = Room::new("The lounge", 16, 11, "walnut", "plum");
    room.put("large_painting", 1, 0)
        .put("bookshelf", 3, 1)
        .put("lion_painting", 5, 0)
        .put("whiteboard", 6, 0)
        .put("window", 9, 0)
        .put("telescope", 11, 1)
        .put("hanging_plant", 12, 0)
        .put("rack", 14, 1)
        .carpet("ochre", 1, 3, 6, 5)
        .put("sofa", 2, 3)
        .put("coffee_table", 2, 4)
        .put("coffee", 3, 4)
        .turned("sofa", 1, 4, 1)
        .turned("sofa", 4, 4, 3)
        .turned("sofa", 2, 6, 2)
        .put("large_plant", 5, 5)
        .carpet("plum", 8, 3, 7, 4)
        .put("small_table", 8, 3)
        .put("pc", 8, 3)
        .put("pc", 9, 3)
        .turned("chair", 8, 5, 2)
        .turned("chair", 9, 5, 2)
        .put("small_table", 12, 3)
        .put("pc", 12, 3)
        .put("pc", 13, 3)
        .turned("chair", 12, 5, 2)
        .turned("chair", 13, 5, 2)
        .put("plant", 11, 4)
        .put("rug", 7, 9)
        .put("pot", 1, 10)
        .put("chair", 3, 9)
        .nursery(13, 7);
    room.done(
        "lounge",
        "Sofas round a coffee table, a few armchairs with a screen. Four at work, many at rest.",
    )
}

fn nook() -> Prefab {
    let mut room = Room::new("The nook", 10, 9, "sand", "clay");
    room.put("bookshelf", 1, 1)
        .put("whiteboard", 3, 0)
        .put("window", 5, 0)
        .put("lion_painting", 7, 0)
        .put("rack", 8, 1)
        .desk(2, 3)
        .put("plant", 1, 3)
        .turned("sofa", 1, 6, 1)
        .put("rug", 4, 7)
        .put("nest", 8, 8)
        .put("cactus", 8, 4);
    room.done("nook", "One desk, one sofa, one nest. For one or two.")
}

/// Every prefab, the default first.
pub fn prefabs() -> Vec<Prefab> {
    vec![
        office(),
        open_plan(),
        library(),
        server_room(),
        lounge(),
        nook(),
    ]
}

/// The prefab with this id.
pub fn prefab(id: &str) -> Option<Prefab> {
    prefabs().into_iter().find(|prefab| prefab.id == id)
}

/// The default den: the office.
pub fn default_layout() -> DenLayout {
    office().layout
}
