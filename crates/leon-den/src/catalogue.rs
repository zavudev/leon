//! The catalogue: everything that can be put in a den.
//!
//! An [`Entry`] is a piece of furniture or a prop: its picture in each
//! direction it can be turned to ([`View`]), how many tiles it stands on,
//! where it may go ([`Placement`]), whether a lion can walk through it, and
//! what it is *for* ([`Role`]). The rules are those of pixel-agents'
//! furniture manifests (footprint, rows at the top that stand up behind
//! whoever passes, what goes on a wall, what goes on a table), written here
//! as data; the pictures are in [`crate::assets`].
//!
//! # Roles
//!
//! The simulation finds its places in a room by the roles of what is in it
//! ([`crate::world`]):
//!
//! | [`Role`] | What a lion does there |
//! | --- | --- |
//! | `Seat` | sits: a seat that faces a computer is a **work seat** (edits, thinks), any other is a **lounge seat** (idle, asleep). A seat marked `little` is a little one's |
//! | `Computer` | lights up for the lion at the seat that faces it |
//! | `Shelf` | reads and searches, standing in front of it |
//! | `Board` | plans |
//! | `Rack` | runs commands |
//! | `Lookout` | goes to the web |
//! | `Entrance` | waits for the user, on it |
//! | `Nest` | eggs lie in it; who sent a little one out waits beside it |

use crate::pose::Facing;

/// Where a piece may be put.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Placement {
    /// On the floor.
    Floor,
    /// On the back wall.
    Wall,
    /// On a table or a desk; also on the floor.
    Surface,
}

/// The group a piece is listed under in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    /// Desks and tables.
    Tables,
    /// Chairs, benches and sofas.
    Seats,
    /// Computers and the rack.
    Machines,
    /// What hangs on the wall.
    Wall,
    /// Plants and small things.
    Decor,
    /// The den's own: the rug, the nest, the telescope.
    Den,
}

impl Category {
    /// Every category, in the order the editor lists them.
    pub const ALL: [Category; 6] = [
        Category::Tables,
        Category::Seats,
        Category::Machines,
        Category::Wall,
        Category::Decor,
        Category::Den,
    ];

    /// What the editor calls it.
    pub fn name(self) -> &'static str {
        match self {
            Category::Tables => "Tables",
            Category::Seats => "Seats",
            Category::Machines => "Machines",
            Category::Wall => "Wall",
            Category::Decor => "Decor",
            Category::Den => "Den",
        }
    }
}

/// What a piece is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// A lion sits on it.
    Seat,
    /// A computer: on while a lion works at the seat that faces it.
    Computer,
    /// Reading and searching.
    Shelf,
    /// Planning.
    Board,
    /// Running commands.
    Rack,
    /// The web.
    Lookout,
    /// Waiting for the user.
    Entrance,
    /// Eggs.
    Nest,
}

impl Role {
    /// Every role.
    pub const ALL: [Role; 8] = [
        Role::Seat,
        Role::Computer,
        Role::Shelf,
        Role::Board,
        Role::Rack,
        Role::Lookout,
        Role::Entrance,
        Role::Nest,
    ];
}

/// A piece seen from one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    /// The name of its picture ([`crate::assets::Art::sprite`]).
    pub sprite: &'static str,
    /// The picture is drawn mirrored.
    pub mirrored: bool,
    /// How many tiles wide it stands.
    pub w: i32,
    /// How many tiles deep it stands.
    pub h: i32,
    /// Which way a lion on it looks (a seat), or which way it shows its
    /// screen (a computer). `None` for a piece with no front.
    pub facing: Option<Facing>,
}

/// A piece of the catalogue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    /// The id a layout names it by. It never changes.
    pub id: &'static str,
    /// What the editor calls it.
    pub name: &'static str,
    /// The group it is listed under.
    pub category: Category,
    /// Where it may go.
    pub placement: Placement,
    /// The piece from each side it can be turned to, the default first.
    pub views: &'static [View],
    /// How many rows at the top of its footprint only stand up behind: a
    /// lion walks through them and other pieces may stand there.
    pub background: i32,
    /// A lion cannot walk through the rest of its footprint.
    pub solid: bool,
    /// Things can be put on it.
    pub surface: bool,
    /// It lies flat on the floor, under everything: a rug.
    pub flat: bool,
    /// What it is for.
    pub role: Option<Role>,
    /// A seat for a little one.
    pub little: bool,
}

const fn view(sprite: &'static str, w: i32, h: i32) -> View {
    View {
        sprite,
        mirrored: false,
        w,
        h,
        facing: None,
    }
}

const fn facing(sprite: &'static str, w: i32, h: i32, facing: Facing) -> View {
    View {
        sprite,
        mirrored: false,
        w,
        h,
        facing: Some(facing),
    }
}

const fn mirrored(sprite: &'static str, w: i32, h: i32, facing: Facing) -> View {
    View {
        sprite,
        mirrored: true,
        w,
        h,
        facing: Some(facing),
    }
}

const fn entry(
    id: &'static str,
    name: &'static str,
    category: Category,
    placement: Placement,
    views: &'static [View],
) -> Entry {
    Entry {
        id,
        name,
        category,
        placement,
        views,
        background: 0,
        solid: true,
        surface: false,
        flat: false,
        role: None,
        little: false,
    }
}

const fn seat(id: &'static str, name: &'static str, views: &'static [View]) -> Entry {
    Entry {
        solid: false,
        role: Some(Role::Seat),
        ..entry(id, name, Category::Seats, Placement::Floor, views)
    }
}

use Category as C;
use Facing as F;
use Placement as P;

/// Everything there is, in the order the editor lists it.
pub const CATALOGUE: &[Entry] = &[
    // ----- tables
    Entry {
        background: 1,
        surface: true,
        ..entry(
            "desk",
            "Desk",
            C::Tables,
            P::Floor,
            &[view("DESK_FRONT", 3, 2), view("DESK_SIDE", 1, 4)],
        )
    },
    Entry {
        background: 1,
        surface: true,
        ..entry(
            "small_table",
            "Small table",
            C::Tables,
            P::Floor,
            &[
                view("SMALL_TABLE_FRONT", 2, 2),
                view("SMALL_TABLE_SIDE", 1, 3),
            ],
        )
    },
    Entry {
        background: 1,
        surface: true,
        ..entry(
            "table",
            "Large table",
            C::Tables,
            P::Floor,
            &[view("TABLE_FRONT", 3, 4)],
        )
    },
    Entry {
        surface: true,
        ..entry(
            "coffee_table",
            "Coffee table",
            C::Tables,
            P::Floor,
            &[view("COFFEE_TABLE", 2, 2)],
        )
    },
    // ----- seats
    seat("bench", "Cushioned bench", &[view("CUSHIONED_BENCH", 1, 1)]),
    Entry {
        little: true,
        ..seat(
            "wooden_bench",
            "Wooden bench (for a little one)",
            &[view("WOODEN_BENCH", 1, 1)],
        )
    },
    seat(
        "chair",
        "Cushioned chair",
        &[
            facing("CUSHIONED_CHAIR_FRONT", 1, 1, F::Down),
            facing("CUSHIONED_CHAIR_SIDE", 1, 1, F::Right),
            facing("CUSHIONED_CHAIR_BACK", 1, 1, F::Up),
            mirrored("CUSHIONED_CHAIR_SIDE", 1, 1, F::Left),
        ],
    ),
    Entry {
        background: 1,
        ..seat(
            "wooden_chair",
            "Wooden chair",
            &[
                facing("WOODEN_CHAIR_FRONT", 1, 2, F::Down),
                facing("WOODEN_CHAIR_SIDE", 1, 2, F::Right),
                facing("WOODEN_CHAIR_BACK", 1, 2, F::Up),
                mirrored("WOODEN_CHAIR_SIDE", 1, 2, F::Left),
            ],
        )
    },
    seat(
        "sofa",
        "Sofa",
        &[
            facing("SOFA_FRONT", 2, 1, F::Down),
            facing("SOFA_SIDE", 1, 2, F::Right),
            facing("SOFA_BACK", 2, 1, F::Up),
            mirrored("SOFA_SIDE", 1, 2, F::Left),
        ],
    ),
    // ----- machines
    Entry {
        background: 1,
        role: Some(Role::Computer),
        ..entry(
            "pc",
            "Computer",
            C::Machines,
            P::Surface,
            &[
                facing("PC_FRONT_OFF", 1, 2, F::Down),
                facing("PC_SIDE", 1, 2, F::Right),
                facing("PC_BACK", 1, 2, F::Up),
                mirrored("PC_SIDE", 1, 2, F::Left),
            ],
        )
    },
    Entry {
        background: 1,
        role: Some(Role::Rack),
        ..entry("rack", "Rack", C::Machines, P::Floor, &[view("RACK", 1, 2)])
    },
    // ----- on the wall
    Entry {
        role: Some(Role::Shelf),
        ..entry(
            "double_bookshelf",
            "Bookshelf",
            C::Wall,
            P::Wall,
            &[view("DOUBLE_BOOKSHELF", 2, 2)],
        )
    },
    Entry {
        role: Some(Role::Shelf),
        ..entry(
            "bookshelf",
            "Small shelf",
            C::Wall,
            P::Wall,
            &[view("BOOKSHELF", 2, 1)],
        )
    },
    Entry {
        role: Some(Role::Board),
        ..entry(
            "whiteboard",
            "Whiteboard",
            C::Wall,
            P::Wall,
            &[view("WHITEBOARD", 2, 2)],
        )
    },
    Entry {
        role: Some(Role::Lookout),
        ..entry(
            "window",
            "Window",
            C::Wall,
            P::Wall,
            &[view("WINDOW", 2, 2)],
        )
    },
    entry("clock", "Clock", C::Wall, P::Wall, &[view("CLOCK", 1, 2)]),
    entry(
        "painting",
        "Painting",
        C::Wall,
        P::Wall,
        &[view("SMALL_PAINTING", 1, 2)],
    ),
    entry(
        "painting_2",
        "Portrait",
        C::Wall,
        P::Wall,
        &[view("SMALL_PAINTING_2", 1, 2)],
    ),
    entry(
        "large_painting",
        "Large painting",
        C::Wall,
        P::Wall,
        &[view("LARGE_PAINTING", 2, 2)],
    ),
    entry(
        "lion_painting",
        "The glare, framed",
        C::Wall,
        P::Wall,
        &[view("LION_PAINTING", 1, 2)],
    ),
    entry(
        "hanging_plant",
        "Hanging plant",
        C::Wall,
        P::Wall,
        &[view("HANGING_PLANT", 1, 2)],
    ),
    // ----- decor
    Entry {
        background: 1,
        ..entry("plant", "Plant", C::Decor, P::Floor, &[view("PLANT", 1, 2)])
    },
    Entry {
        background: 1,
        ..entry(
            "plant_2",
            "Fern",
            C::Decor,
            P::Floor,
            &[view("PLANT_2", 1, 2)],
        )
    },
    Entry {
        background: 1,
        ..entry(
            "cactus",
            "Cactus",
            C::Decor,
            P::Floor,
            &[view("CACTUS", 1, 2)],
        )
    },
    Entry {
        background: 2,
        ..entry(
            "large_plant",
            "Large plant",
            C::Decor,
            P::Floor,
            &[view("LARGE_PLANT", 2, 3)],
        )
    },
    entry("pot", "Pot", C::Decor, P::Floor, &[view("POT", 1, 1)]),
    entry("bin", "Bin", C::Decor, P::Floor, &[view("BIN", 1, 1)]),
    Entry {
        solid: false,
        ..entry(
            "coffee",
            "Coffee",
            C::Decor,
            P::Surface,
            &[view("COFFEE", 1, 1)],
        )
    },
    // ----- the den's own
    Entry {
        solid: false,
        flat: true,
        role: Some(Role::Entrance),
        ..entry(
            "rug",
            "Entrance rug",
            C::Den,
            P::Floor,
            &[view("RUG", 3, 2)],
        )
    },
    Entry {
        solid: false,
        role: Some(Role::Nest),
        ..entry("nest", "Nest", C::Den, P::Floor, &[view("NEST", 1, 1)])
    },
    Entry {
        background: 1,
        role: Some(Role::Lookout),
        ..entry(
            "telescope",
            "Telescope",
            C::Den,
            P::Floor,
            &[view("TELESCOPE", 1, 2)],
        )
    },
];

/// The piece with this id.
pub fn find(id: &str) -> Option<&'static Entry> {
    CATALOGUE.iter().find(|entry| entry.id == id)
}

impl Entry {
    /// The piece turned `turn` times: its views go round.
    pub fn view(&self, turn: u8) -> &'static View {
        &self.views[usize::from(turn) % self.views.len()]
    }

    /// Whether it can be turned at all.
    pub fn turns(&self) -> bool {
        self.views.len() > 1
    }
}
