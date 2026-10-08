//! The pictures of the Den, compiled into the binary.
//!
//! Nothing is read from disk at run time: every PNG under `assets/` that the
//! Den draws is included here and decoded once, the first time it is asked
//! for. The furniture, the floors and the walls are those of pixel-agents
//! (MIT, Copyright (c) 2026 Pablo De Lucca), unchanged; the lions are made
//! from its character sheets by [`crate::atelier`], and the props of
//! `assets/props/` are drawn there too. `crates/app/assets/ASSETS.md` has the
//! sources and the licences.
//!
//! The floors and the walls are grey tiles: a [`FloorStyle`] or a
//! [`WallStyle`] names a tile and the [`Tone`] that colours it. A room's
//! layout chooses its styles by id.

use std::sync::LazyLock;

use crate::bitmap::{Bitmap, Tone};

macro_rules! png {
    ($path:literal) => {
        Bitmap::from_png(include_bytes!(concat!("../assets/", $path)))
    };
}

/// A way to lay a floor, or a carpet: a grey tile and its colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloorStyle {
    /// The id a layout names it by.
    pub id: &'static str,
    /// What the editor calls it.
    pub name: &'static str,
    /// Which of the floor tiles: 0 plain, 1 and 2 large slabs, 3 and 4 small
    /// tiles, 5 and 6 planks, 7 and 8 checkers.
    pub tile: usize,
    /// Its colour; `None` leaves the tile as it is drawn.
    pub tone: Option<Tone>,
}

/// A way to paint the walls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallStyle {
    /// The id a layout names it by.
    pub id: &'static str,
    /// What the editor calls it.
    pub name: &'static str,
    /// Its colour.
    pub tone: Tone,
}

const fn tone(h: f32, s: f32, b: f32, c: f32) -> Tone {
    Tone { h, s, b, c }
}

/// Every floor, the default first. The plain ones in strong colours are
/// meant as carpets, but any of them can be the floor of a room.
pub const FLOORS: &[FloorStyle] = &[
    FloorStyle {
        id: "wood",
        name: "Wooden planks",
        tile: 6,
        tone: Some(tone(26., 46., -40., -86.)),
    },
    FloorStyle {
        id: "walnut",
        name: "Walnut planks",
        tile: 6,
        tone: Some(tone(18., 40., -64., -84.)),
    },
    FloorStyle {
        id: "pine",
        name: "Pine planks",
        tile: 5,
        tone: Some(tone(36., 44., -14., -84.)),
    },
    FloorStyle {
        id: "stone",
        name: "Stone slabs",
        tile: 2,
        tone: Some(tone(30., 10., -34., -78.)),
    },
    FloorStyle {
        id: "slate",
        name: "Slate tiles",
        tile: 3,
        tone: Some(tone(214., 16., -52., -76.)),
    },
    FloorStyle {
        id: "sand",
        name: "Sandstone tiles",
        tile: 4,
        tone: Some(tone(38., 34., -16., -82.)),
    },
    FloorStyle {
        id: "checker",
        name: "Checkerboard",
        tile: 8,
        tone: None,
    },
    FloorStyle {
        id: "red",
        name: "Red carpet",
        tile: 0,
        tone: Some(tone(14., 34., -52., -82.)),
    },
    FloorStyle {
        id: "green",
        name: "Green carpet",
        tile: 0,
        tone: Some(tone(140., 26., -50., -82.)),
    },
    FloorStyle {
        id: "blue",
        name: "Blue carpet",
        tile: 0,
        tone: Some(tone(212., 34., -44., -82.)),
    },
    FloorStyle {
        id: "plum",
        name: "Plum carpet",
        tile: 0,
        tone: Some(tone(300., 22., -54., -82.)),
    },
    FloorStyle {
        id: "ochre",
        name: "Ochre carpet",
        tile: 0,
        tone: Some(tone(42., 52., -30., -82.)),
    },
];

/// Every wall, the default first.
pub const WALLS: &[WallStyle] = &[
    WallStyle {
        id: "rock",
        name: "Den rock",
        tone: tone(24., 22., -96., -52.),
    },
    WallStyle {
        id: "navy",
        name: "Navy",
        tone: tone(214., 30., -100., -55.),
    },
    WallStyle {
        id: "plum",
        name: "Plum",
        tone: tone(318., 20., -96., -55.),
    },
    WallStyle {
        id: "moss",
        name: "Moss",
        tone: tone(130., 18., -94., -55.),
    },
    WallStyle {
        id: "clay",
        name: "Clay",
        tone: tone(12., 38., -78., -55.),
    },
    WallStyle {
        id: "ash",
        name: "Ash",
        tone: tone(30., 4., -70., -50.),
    },
];

/// The floor with this id, if there is one.
pub fn floor(id: &str) -> Option<usize> {
    FLOORS.iter().position(|style| style.id == id)
}

/// The wall with this id, if there is one.
pub fn wall(id: &str) -> Option<usize> {
    WALLS.iter().position(|style| style.id == id)
}

/// Every picture, decoded.
pub struct Art {
    /// The lion sheets, by body and then by style
    /// ([`crate::atelier::STYLES`]): ten frames of 16 by 32 in three rows
    /// each.
    pub lions: Vec<Vec<Bitmap>>,
    /// The cub sheet: eight frames of 16 by 16 in three rows.
    pub cub: Bitmap,
    /// The egg: three frames of 16 by 16.
    pub egg: Bitmap,
    /// One tile per floor of [`FLOORS`], coloured.
    pub floors: Vec<Bitmap>,
    /// Per wall of [`WALLS`], the sixteen pieces by the neighbours a wall
    /// tile has: north 1, east 2, south 4, west 8. Each is 16 by 32.
    pub walls: Vec<Vec<Bitmap>>,
    sprites: Vec<(&'static str, Bitmap)>,
}

impl Art {
    fn load() -> Art {
        let rack = png!("props/rack.png");
        let wall_set = png!("tiles/wall_0.png");
        let tiles = [
            png!("tiles/floor_0.png"),
            png!("tiles/floor_1.png"),
            png!("tiles/floor_2.png"),
            png!("tiles/floor_3.png"),
            png!("tiles/floor_4.png"),
            png!("tiles/floor_5.png"),
            png!("tiles/floor_6.png"),
            png!("tiles/floor_7.png"),
            png!("tiles/floor_8.png"),
        ];
        let floors = FLOORS
            .iter()
            .map(|style| match style.tone {
                Some(tone) => tone.colorize(&tiles[style.tile]),
                None => tiles[style.tile].clone(),
            })
            .collect();
        let walls = WALLS
            .iter()
            .map(|style| {
                (0..16)
                    .map(|mask| {
                        style.tone.colorize(&wall_set.part(
                            (mask % 4) * 16,
                            (mask / 4) * 32,
                            16,
                            32,
                        ))
                    })
                    .collect()
            })
            .collect();
        Art {
            lions: vec![
                vec![
                    png!("lions/lion_0_0.png"),
                    png!("lions/lion_0_1.png"),
                    png!("lions/lion_0_2.png"),
                    png!("lions/lion_0_3.png"),
                    png!("lions/lion_0_4.png"),
                    png!("lions/lion_0_5.png"),
                ],
                vec![
                    png!("lions/lion_1_0.png"),
                    png!("lions/lion_1_1.png"),
                    png!("lions/lion_1_2.png"),
                    png!("lions/lion_1_3.png"),
                    png!("lions/lion_1_4.png"),
                    png!("lions/lion_1_5.png"),
                ],
                vec![
                    png!("lions/lion_2_0.png"),
                    png!("lions/lion_2_1.png"),
                    png!("lions/lion_2_2.png"),
                    png!("lions/lion_2_3.png"),
                    png!("lions/lion_2_4.png"),
                    png!("lions/lion_2_5.png"),
                ],
                vec![
                    png!("lions/lion_3_0.png"),
                    png!("lions/lion_3_1.png"),
                    png!("lions/lion_3_2.png"),
                    png!("lions/lion_3_3.png"),
                    png!("lions/lion_3_4.png"),
                    png!("lions/lion_3_5.png"),
                ],
                vec![
                    png!("lions/lion_4_0.png"),
                    png!("lions/lion_4_1.png"),
                    png!("lions/lion_4_2.png"),
                    png!("lions/lion_4_3.png"),
                    png!("lions/lion_4_4.png"),
                    png!("lions/lion_4_5.png"),
                ],
            ],
            cub: png!("lions/cub.png"),
            egg: png!("props/egg.png"),
            floors,
            walls,
            sprites: vec![
                ("BIN", png!("furniture/BIN.png")),
                ("BOOKSHELF", png!("furniture/BOOKSHELF.png")),
                ("CACTUS", png!("furniture/CACTUS.png")),
                ("CLOCK", png!("furniture/CLOCK.png")),
                ("COFFEE", png!("furniture/COFFEE.png")),
                ("COFFEE_TABLE", png!("furniture/COFFEE_TABLE.png")),
                ("CUSHIONED_BENCH", png!("furniture/CUSHIONED_BENCH.png")),
                (
                    "CUSHIONED_CHAIR_BACK",
                    png!("furniture/CUSHIONED_CHAIR_BACK.png"),
                ),
                (
                    "CUSHIONED_CHAIR_FRONT",
                    png!("furniture/CUSHIONED_CHAIR_FRONT.png"),
                ),
                (
                    "CUSHIONED_CHAIR_SIDE",
                    png!("furniture/CUSHIONED_CHAIR_SIDE.png"),
                ),
                ("DESK_FRONT", png!("furniture/DESK_FRONT.png")),
                ("DESK_SIDE", png!("furniture/DESK_SIDE.png")),
                ("DOUBLE_BOOKSHELF", png!("furniture/DOUBLE_BOOKSHELF.png")),
                ("HANGING_PLANT", png!("furniture/HANGING_PLANT.png")),
                ("LARGE_PAINTING", png!("furniture/LARGE_PAINTING.png")),
                ("LARGE_PLANT", png!("furniture/LARGE_PLANT.png")),
                ("PC_BACK", png!("furniture/PC_BACK.png")),
                ("PC_FRONT_OFF", png!("furniture/PC_FRONT_OFF.png")),
                ("PC_FRONT_ON_1", png!("furniture/PC_FRONT_ON_1.png")),
                ("PC_FRONT_ON_2", png!("furniture/PC_FRONT_ON_2.png")),
                ("PC_FRONT_ON_3", png!("furniture/PC_FRONT_ON_3.png")),
                ("PC_SIDE", png!("furniture/PC_SIDE.png")),
                ("PLANT", png!("furniture/PLANT.png")),
                ("PLANT_2", png!("furniture/PLANT_2.png")),
                ("POT", png!("furniture/POT.png")),
                ("SMALL_PAINTING", png!("furniture/SMALL_PAINTING.png")),
                ("SMALL_PAINTING_2", png!("furniture/SMALL_PAINTING_2.png")),
                ("SMALL_TABLE_FRONT", png!("furniture/SMALL_TABLE_FRONT.png")),
                ("SMALL_TABLE_SIDE", png!("furniture/SMALL_TABLE_SIDE.png")),
                ("SOFA_BACK", png!("furniture/SOFA_BACK.png")),
                ("SOFA_FRONT", png!("furniture/SOFA_FRONT.png")),
                ("SOFA_SIDE", png!("furniture/SOFA_SIDE.png")),
                ("TABLE_FRONT", png!("furniture/TABLE_FRONT.png")),
                ("WHITEBOARD", png!("furniture/WHITEBOARD.png")),
                ("WOODEN_BENCH", png!("furniture/WOODEN_BENCH.png")),
                ("WOODEN_CHAIR_BACK", png!("furniture/WOODEN_CHAIR_BACK.png")),
                (
                    "WOODEN_CHAIR_FRONT",
                    png!("furniture/WOODEN_CHAIR_FRONT.png"),
                ),
                ("WOODEN_CHAIR_SIDE", png!("furniture/WOODEN_CHAIR_SIDE.png")),
                ("TELESCOPE", png!("props/telescope.png")),
                ("WINDOW", png!("props/window.png")),
                ("RUG", png!("props/rug.png")),
                ("NEST", png!("props/nest.png")),
                ("LION_PAINTING", png!("props/lion_painting.png")),
                ("RACK", rack.part(0, 0, 16, 32)),
                ("RACK_BUSY_1", rack.part(16, 0, 16, 32)),
                ("RACK_BUSY_2", rack.part(32, 0, 16, 32)),
            ],
        }
    }

    /// The picture called `name`: a furniture file's name without its
    /// extension, or a prop's in capitals. A name that is not bundled is a
    /// bug of the catalogue, and panics (a test asks for every one).
    pub fn sprite(&self, name: &str) -> &Bitmap {
        self.sprites
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, bitmap)| bitmap)
            .unwrap_or_else(|| panic!("no picture is called {name}"))
    }

    /// The names of every picture of furniture.
    pub fn sprite_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.sprites.iter().map(|(name, _)| *name)
    }
}

static ART: LazyLock<Art> = LazyLock::new(Art::load);

/// The pictures.
pub fn art() -> &'static Art {
    &ART
}
