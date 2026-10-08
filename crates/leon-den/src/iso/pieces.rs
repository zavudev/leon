//! The furniture of the catalogue in 2.5D: every piece as a few boxes.
//!
//! [`piece`] draws the piece with an id of the [`crate::catalogue`] in the
//! place a layout gives it ([`Slot`]): the tiles it really stands on, the
//! height it stands at (the floor, or the table under it), the way it is
//! turned. A piece is drawn about the middle of its base with its front to
//! `+z`, and turned to where it looks. An id with no drawing of its own
//! gets a plain box of its footprint ([`MODELLED`] lists those that have
//! one), so a room never has a hole in it.
//!
//! The pieces have no colour of their own: all of it is the [`Theme`]'s.

use std::f32::consts::PI;

use crate::pose::Facing;

use super::camera::WALL_HEIGHT;
use super::mesh::{blend, Mesh, Part, Rgb};
use super::theme::Theme;

/// How high the top of a seat is.
pub const SEAT: f32 = 0.46;
/// How high the top of a little one's seat is.
pub const LITTLE_SEAT: f32 = 0.3;
/// How high the top of a desk or a table is.
pub const TABLE: f32 = 0.7;
/// How high the top of a coffee table is.
pub const LOW_TABLE: f32 = 0.36;
/// How far up the wall the top of what hangs in its upper row is.
const WALL_TOP: f32 = WALL_HEIGHT - 0.15;

/// Every id that has a drawing of its own.
pub const MODELLED: &[&str] = &[
    "desk",
    "small_table",
    "table",
    "coffee_table",
    "bench",
    "wooden_bench",
    "chair",
    "wooden_chair",
    "sofa",
    "pc",
    "rack",
    "double_bookshelf",
    "bookshelf",
    "whiteboard",
    "window",
    "clock",
    "painting",
    "painting_2",
    "large_painting",
    "lion_painting",
    "hanging_plant",
    "plant",
    "plant_2",
    "cactus",
    "large_plant",
    "pot",
    "bin",
    "coffee",
    "rug",
    "nest",
    "telescope",
];

/// Where a piece goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// The left of the tiles it stands on.
    pub x: f32,
    /// Their back: the edge nearest the back wall.
    pub z: f32,
    /// How many tiles wide they are.
    pub w: f32,
    /// How many tiles deep.
    pub d: f32,
    /// How high it stands: 0 on the floor, the top of the table under it.
    pub y: f32,
    /// Which way it looks, when it has a front.
    pub facing: Option<Facing>,
    /// For what hangs on the wall: the row its top is in, 0 or 1.
    pub row: i32,
    /// And how many rows high it is.
    pub rows: i32,
}

/// What a piece shows of the life of the room.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Live {
    /// The tick of the moment: lights that run, run on it.
    pub tick: u64,
    /// What a computer's screen shows: the colour of its lines, or nothing
    /// when it is off.
    pub screen: Option<Rgb>,
    /// A lion minds the racks: their lights run.
    pub running: bool,
}

/// The angle a piece that looks this way is turned by.
pub fn angle(facing: Facing) -> f32 {
    match facing {
        Facing::Down => 0.,
        Facing::Right => PI / 2.,
        Facing::Up => PI,
        Facing::Left => -PI / 2.,
    }
}

/// How high what stands on a piece stands: the top of a table.
pub fn surface_height(id: &str) -> f32 {
    match id {
        "coffee_table" => LOW_TABLE,
        _ => TABLE,
    }
}

/// How high a piece stands over what it stands on, roughly: the box the
/// pointer finds it by.
pub fn height(id: &str) -> f32 {
    match id {
        "coffee_table" => LOW_TABLE,
        "desk" | "small_table" | "table" => TABLE,
        "bench" | "chair" | "wooden_chair" => 0.95,
        "wooden_bench" => LITTLE_SEAT,
        "sofa" => 0.85,
        "pc" => 0.6,
        "rack" => 2.1,
        "plant" | "plant_2" | "cactus" => 1.25,
        "large_plant" => 2.,
        "pot" => 0.46,
        "bin" => 0.4,
        "coffee" => 0.2,
        "rug" => 0.03,
        "nest" => 0.26,
        "telescope" => 1.5,
        _ => 0.8,
    }
}

/// How a floor is ruled is the theme's; how high the rows of the wall are
/// is here: the bottom of the row a piece hangs in, 0 the upper one.
pub fn wall_row(row: i32) -> (f32, f32) {
    let top = WALL_TOP - row as f32;
    (top - 1., top)
}

/// How high a lion sits on a piece.
pub fn seat_height(id: &str) -> f32 {
    match id {
        "wooden_bench" => LITTLE_SEAT,
        _ => SEAT,
    }
}

/// The face of a screen that looks to `+z`: dark, with a few lines of a
/// terminal in a colour, or none when it is off.
fn screen(
    mesh: &mut Mesh,
    t: &Theme,
    part: Part,
    (w, h): (f32, f32),
    base: [f32; 3],
    lit: Option<Rgb>,
) {
    mesh.glow(part, [w, h, 0.004], t.ink_screen(), base);
    let Some(color) = lit else {
        return;
    };
    let z = base[2] + 0.006;
    for (row, (start, length)) in [
        (0.06, 0.46),
        (0.06, 0.7),
        (0.16, 0.4),
        (0.16, 0.6),
        (0.06, 0.3),
    ]
    .into_iter()
    .enumerate()
    {
        let y = base[1] + h * (0.82 - row as f32 * 0.16);
        // Seen from its front, the lines start at the left.
        let x = w * (start + length / 2.) - w / 2.;
        mesh.glow(
            part,
            [w * length, h * 0.05, 0.004],
            color,
            [base[0] + x, y, z],
        );
    }
    let x = w * 0.44 - w / 2.;
    mesh.glow(
        part,
        [w * 0.06, h * 0.07, 0.004],
        t.accent,
        [base[0] + x, base[1] + h * 0.16, z],
    );
}

impl Theme {
    /// The black of a screen: dark in every theme.
    pub fn ink_screen(&self) -> Rgb {
        if self.dark {
            self.background
        } else {
            self.text
        }
    }
}

fn table(mesh: &mut Mesh, t: &Theme, part: Part, w: f32, d: f32, top: f32, legs: bool) {
    let thick = 0.06;
    mesh.piece(
        part,
        [w - 0.08, thick, d - 0.1],
        t.piece,
        [0., top - thick, 0.],
    );
    if legs {
        for (x, z) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
            mesh.piece(
                part,
                [0.08, top - thick, 0.08],
                t.piece_low,
                [x * (w / 2. - 0.14), 0., z * (d / 2. - 0.14)],
            );
        }
    } else if w >= d {
        for x in [-1., 1.] {
            mesh.piece(
                part,
                [0.05, top - thick, d - 0.24],
                t.piece_low,
                [x * (w / 2. - 0.1), 0., 0.],
            );
        }
    } else {
        for z in [-1., 1.] {
            mesh.piece(
                part,
                [w - 0.24, top - thick, 0.05],
                t.piece_low,
                [0., 0., z * (d / 2. - 0.1)],
            );
        }
    }
}

fn leaves(mesh: &mut Mesh, t: &Theme, part: Part, balls: &[(f32, f32, f32, f32)]) {
    for (x, y, z, r) in balls {
        mesh.ball(part, [*r; 3], t.plant, [*x, *y, *z], false);
    }
}

fn pot(mesh: &mut Mesh, t: &Theme, part: Part, side: f32, high: f32) {
    mesh.piece(part, [side, high, side], t.piece, [0., 0., 0.]);
    mesh.bit(
        part,
        [side - 0.08, 0.02, side - 0.08],
        t.ink_screen(),
        [0., high - 0.01, 0.],
    );
}

/// The quiet colours of the spines of books.
fn spines(t: &Theme) -> [Rgb; 8] {
    [
        t.muted,
        t.faint,
        t.metal,
        t.piece,
        t.faint,
        blend(t.info, t.wall, 0.72),
        t.muted,
        blend(t.error, t.wall, 0.74),
    ]
}

/// A row of books on a board `w` wide whose top is at `y`, their backs at
/// `z`.
fn books(mesh: &mut Mesh, t: &Theme, part: Part, w: f32, y: f32, z: f32, seed: usize) {
    let colors = spines(t);
    let mut x = -w / 2. + 0.06;
    let mut index = seed;
    while x < w / 2. - 0.12 {
        let thick = 0.06 + (index * 7 % 4) as f32 * 0.015;
        let high = 0.26 + (index * 5 % 5) as f32 * 0.03;
        // A gap now and then: a shelf is never full.
        if index % 7 != 3 {
            mesh.bit(
                part,
                [thick, high, 0.2],
                colors[index * 3 % colors.len()],
                [x + thick / 2., y, z],
            );
        }
        x += thick + 0.012;
        index += 1;
    }
}

fn frame(mesh: &mut Mesh, t: &Theme, part: Part, w: f32, h: f32, y: f32, canvas: Rgb) {
    mesh.piece(part, [w, h, 0.05], t.metal, [0., y, 0.025]);
    mesh.bit(
        part,
        [w - 0.1, h - 0.1, 0.01],
        canvas,
        [0., y + 0.05, 0.055],
    );
}

/// A point of the mark (`brand/logo/final/mark.svg`, 64 by 64) in a square
/// of `size` about the middle of a part: the mark's top to `-z`.
fn of_mark(size: f32, [x, y]: [f32; 2]) -> [f32; 2] {
    [(x - 32.) / 64. * size, (y - 32.5) / 64. * size]
}

/// The face of Leon's mark, flat, in a colour: the two slanted eyes and
/// the nose with its stem, as the mark has them.
fn face(mesh: &mut Mesh, part: Part, size: f32, y: f32, color: Rgb) {
    const SHAPES: [&[[f32; 2]]; 6] = [
        &[[8., 19.], [28., 30.], [28., 33.], [13., 28.]],
        &[[56., 19.], [51., 28.], [36., 33.], [36., 30.]],
        &[[23., 43.], [32., 50.], [30.75, 52.6], [23., 46.5]],
        &[[41., 43.], [41., 46.5], [33.25, 52.6], [32., 50.]],
        &[[32., 50.], [33.25, 52.6], [30.75, 52.6]],
        &[[30.75, 52.6], [33.25, 52.6], [33.25, 61.], [30.75, 61.]],
    ];
    for shape in SHAPES {
        let outline: Vec<[f32; 2]> = shape.iter().map(|point| of_mark(size, *point)).collect();
        mesh.shape(part, &outline, y, color);
    }
}

/// The head of Leon's mark, flat, in a colour: its outline, ears and all.
fn head(mesh: &mut Mesh, part: Part, size: f32, y: f32, color: Rgb) {
    const OUTLINE: [[f32; 2]; 14] = [
        [32., 11.],
        [25., 6.],
        [5., 4.],
        [2., 16.],
        [11., 38.],
        [18., 54.],
        [27., 61.],
        [37., 61.],
        [46., 54.],
        [53., 38.],
        [62., 16.],
        [59., 4.],
        [39., 6.],
        [32., 11.],
    ];
    // Every corner shows from the middle of the head: a fan from there.
    let middle = of_mark(size, [32., 34.]);
    for pair in OUTLINE.windows(2) {
        let outline = [middle, of_mark(size, pair[0]), of_mark(size, pair[1])];
        mesh.shape(part, &outline, y, color);
    }
}

/// Draws a piece. `false` when the id had no drawing of its own and a plain
/// box stands for it.
pub fn piece(mesh: &mut Mesh, t: &Theme, id: &str, slot: &Slot, live: &Live) -> bool {
    let turn = slot.facing.map_or(0., angle);
    let sideways = matches!(slot.facing, Some(Facing::Left | Facing::Right));
    let (w, d) = if sideways {
        (slot.d, slot.w)
    } else {
        (slot.w, slot.d)
    };
    let part = Part::at(slot.x + slot.w / 2., slot.y, slot.z + slot.d / 2.).turned(turn);
    // What hangs on the wall is drawn from the wall, its front to the room.
    let hung = Part::at(slot.x + slot.w / 2., 0., slot.z + slot.d);
    let top = WALL_TOP - slot.row as f32;
    let low = top - slot.rows as f32;
    match id {
        "desk" | "small_table" => table(mesh, t, part, w, d, TABLE, false),
        "table" => table(mesh, t, part, w, d, TABLE, true),
        "coffee_table" => {
            mesh.piece(
                part,
                [w - 0.5, LOW_TABLE - 0.06, d - 0.5],
                t.piece_low,
                [0., 0., 0.],
            );
            mesh.piece(
                part,
                [w - 0.3, 0.06, d - 0.3],
                t.piece,
                [0., LOW_TABLE - 0.06, 0.],
            );
        }
        "bench" => {
            mesh.piece(part, [0.62, SEAT - 0.12, 0.62], t.piece_low, [0., 0., 0.]);
            mesh.piece(part, [0.7, 0.12, 0.7], t.piece, [0., SEAT - 0.12, 0.]);
        }
        "wooden_bench" => {
            mesh.piece(
                part,
                [0.44, LITTLE_SEAT - 0.06, 0.44],
                t.piece_low,
                [0., 0., 0.],
            );
            mesh.piece(
                part,
                [0.54, 0.06, 0.54],
                t.piece,
                [0., LITTLE_SEAT - 0.06, 0.],
            );
        }
        "chair" => {
            mesh.bit(part, [0.5, 0.04, 0.5], t.metal, [0., 0., 0.]);
            mesh.bit(part, [0.07, SEAT - 0.14, 0.07], t.metal, [0., 0.04, 0.]);
            mesh.piece(part, [0.58, 0.1, 0.58], t.piece, [0., SEAT - 0.1, 0.]);
            mesh.piece(part, [0.58, 0.62, 0.07], t.piece_low, [0., SEAT, -0.3]);
        }
        "wooden_chair" => {
            for (x, z) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
                mesh.bit(
                    part,
                    [0.06, SEAT - 0.06, 0.06],
                    t.piece_low,
                    [x * 0.23, 0., z * 0.23],
                );
            }
            mesh.piece(part, [0.56, 0.06, 0.56], t.piece, [0., SEAT - 0.06, 0.]);
            for x in [-0.23, 0.23] {
                mesh.bit(part, [0.06, 0.6, 0.06], t.piece_low, [x, SEAT, -0.25]);
            }
            mesh.piece(part, [0.52, 0.16, 0.05], t.piece, [0., SEAT + 0.42, -0.25]);
        }
        "sofa" => {
            mesh.piece(
                part,
                [w - 0.1, SEAT - 0.14, d - 0.14],
                t.piece_low,
                [0., 0., 0.],
            );
            for x in [-0.5, 0.5] {
                mesh.piece(
                    part,
                    [w / 2. - 0.14, 0.14, d - 0.36],
                    t.piece,
                    [x * (w / 2. - 0.08), SEAT - 0.14, 0.08],
                );
            }
            mesh.piece(
                part,
                [w - 0.1, 0.66, 0.2],
                t.piece,
                [0., SEAT - 0.14, -d / 2. + 0.17],
            );
            for x in [-1., 1.] {
                mesh.piece(
                    part,
                    [0.16, 0.3, d - 0.14],
                    t.piece,
                    [x * (w / 2. - 0.13), SEAT - 0.14, 0.],
                );
            }
        }
        "pc" => {
            mesh.bit(part, [0.24, 0.03, 0.16], t.metal, [0., 0., -0.22]);
            mesh.bit(part, [0.05, 0.16, 0.05], t.metal, [0., 0.03, -0.24]);
            mesh.piece(part, [0.68, 0.44, 0.05], t.metal, [0., 0.13, -0.22]);
            screen(mesh, t, part, (0.6, 0.36), [0., 0.17, -0.192], live.screen);
            mesh.piece(part, [0.42, 0.025, 0.15], t.piece_low, [0., 0., 0.16]);
            mesh.bit(part, [0.2, 0.012, 0.28], t.paper, [-0.3, 0., -0.02]);
        }
        "rack" => {
            mesh.piece(part, [0.86, 2.0, 0.76], t.piece_low, [0., 0., 0.]);
            for unit in 0..6u64 {
                let y = 0.2 + unit as f32 * 0.29;
                mesh.bit(part, [0.7, 0.2, 0.02], t.metal, [0., y, 0.38]);
                let on = if live.running {
                    (live.tick / 3 + unit * 2 + slot.x as u64) % 3 != 0
                } else {
                    unit == 0
                };
                if on {
                    mesh.glow(
                        part,
                        [0.07, 0.05, 0.004],
                        t.success,
                        [0.24, y + 0.075, 0.394],
                    );
                }
                mesh.glow(
                    part,
                    [0.28, 0.03, 0.004],
                    t.faint,
                    [-0.14, y + 0.085, 0.394],
                );
            }
        }
        "double_bookshelf" => {
            // Open at the front: a back, two sides, a top, and the boards.
            let high = (top - 0.25).max(1.2);
            mesh.piece(hung, [w - 0.1, high, 0.04], t.piece_low, [0., 0., 0.02]);
            for x in [-1., 1.] {
                mesh.piece(
                    hung,
                    [0.05, high, 0.34],
                    t.piece_low,
                    [x * (w / 2. - 0.075), 0., 0.17],
                );
            }
            mesh.piece(hung, [w - 0.1, 0.05, 0.34], t.piece, [0., high, 0.17]);
            let shelves = 4;
            for shelf in 0..shelves {
                let y = 0.08 + shelf as f32 * (high - 0.1) / shelves as f32;
                mesh.bit(hung, [w - 0.2, 0.03, 0.3], t.piece, [0., y, 0.19]);
                books(
                    mesh,
                    t,
                    hung,
                    w - 0.24,
                    y + 0.03,
                    0.18,
                    shelf * 11 + slot.x as usize,
                );
            }
        }
        "bookshelf" => {
            mesh.piece(
                hung,
                [w - 0.14, top - low - 0.1, 0.05],
                t.piece_low,
                [0., low + 0.05, 0.025],
            );
            for (shelf, y) in [low + 0.08, low + 0.5].into_iter().enumerate() {
                mesh.piece(hung, [w - 0.14, 0.04, 0.28], t.piece, [0., y, 0.16]);
                books(
                    mesh,
                    t,
                    hung,
                    w - 0.2,
                    y + 0.04,
                    0.17,
                    shelf * 5 + slot.x as usize,
                );
            }
        }
        "whiteboard" => {
            let (bw, bh, y) = (w - 0.2, (top - low) * 0.62, low + 0.5);
            mesh.piece(hung, [bw, bh, 0.05], t.metal, [0., y, 0.025]);
            mesh.bit(
                hung,
                [bw - 0.1, bh - 0.1, 0.01],
                t.paper,
                [0., y + 0.05, 0.055],
            );
            for (row, (start, length, color)) in [
                (0.08, 0.5, t.ink_line()),
                (0.08, 0.34, t.ink_line()),
                (0.2, 0.44, t.info),
                (0.2, 0.26, t.ink_line()),
            ]
            .into_iter()
            .enumerate()
            {
                let x = (bw - 0.1) * (start + length / 2.) - (bw - 0.1) / 2.;
                mesh.glow(
                    hung,
                    [(bw - 0.1) * length, 0.03, 0.004],
                    color,
                    [x, y + bh - 0.22 - row as f32 * 0.16, 0.064],
                );
            }
            mesh.bit(hung, [bw * 0.5, 0.03, 0.08], t.metal, [0., y - 0.03, 0.07]);
        }
        "window" => {
            let (ww, wh, y) = (w - 0.3, (top - low) * 0.6, low + 0.62);
            mesh.piece(hung, [ww, wh, 0.06], t.metal, [0., y, 0.03]);
            let sky = if t.dark {
                blend(t.background, t.info, 0.16)
            } else {
                blend(t.floor, t.info, 0.22)
            };
            mesh.glow(
                hung,
                [ww - 0.12, wh - 0.12, 0.004],
                sky,
                [0., y + 0.06, 0.064],
            );
            mesh.bit(hung, [0.04, wh - 0.12, 0.02], t.metal, [0., y + 0.06, 0.07]);
            mesh.bit(
                hung,
                [ww - 0.12, 0.04, 0.02],
                t.metal,
                [0., y + wh / 2. - 0.02, 0.07],
            );
            if t.dark {
                for (x, dy) in [
                    (-0.5, 0.3),
                    (-0.22, 0.62),
                    (0.3, 0.5),
                    (0.52, 0.78),
                    (0.4, 0.22),
                ] {
                    mesh.glow(
                        hung,
                        [0.03, 0.03, 0.004],
                        t.text,
                        [x * ww * 0.8, y + wh * dy, 0.07],
                    );
                }
            }
            mesh.piece(hung, [ww + 0.1, 0.04, 0.14], t.piece, [0., y - 0.04, 0.07]);
        }
        "clock" => {
            let y = low + (top - low) * 0.55;
            mesh.piece(hung, [0.46, 0.46, 0.05], t.metal, [0., y, 0.025]);
            mesh.bit(hung, [0.38, 0.38, 0.01], t.paper, [0., y + 0.04, 0.055]);
            mesh.glow(
                hung,
                [0.03, 0.15, 0.004],
                t.ink_line(),
                [0., y + 0.23, 0.064],
            );
            mesh.glow(
                hung,
                [0.11, 0.03, 0.004],
                t.ink_line(),
                [0.05, y + 0.215, 0.064],
            );
        }
        "painting" | "painting_2" | "large_painting" | "lion_painting" => {
            let (pw, ph) = if id == "large_painting" {
                (w - 0.3, 1.1)
            } else {
                (0.72, 0.9)
            };
            let y = low + (top - low - ph) * 0.6;
            let canvas = match id {
                "lion_painting" => t.ink_screen(),
                "painting_2" => blend(t.wall, t.text, 0.16),
                _ => blend(t.wall, t.text, 0.08),
            };
            frame(mesh, t, hung, pw, ph, y, canvas);
            match id {
                "lion_painting" => {
                    // The mark stands up here: drawn flat, then raised.
                    let up = hung.moved([0., y + ph / 2., 0.062]).pitched(PI / 2.);
                    head(mesh, up, pw * 0.86, 0., t.accent);
                    face(mesh, up, pw * 0.86, 0.004, t.ink_screen());
                }
                "painting_2" => {
                    mesh.bit(
                        hung,
                        [pw * 0.34, ph * 0.3, 0.01],
                        t.muted,
                        [0., y + ph * 0.4, 0.06],
                    );
                    mesh.bit(
                        hung,
                        [pw * 0.5, ph * 0.22, 0.01],
                        t.faint,
                        [0., y + ph * 0.16, 0.06],
                    );
                }
                _ => {
                    mesh.bit(
                        hung,
                        [pw * 0.6, ph * 0.2, 0.01],
                        t.faint,
                        [-pw * 0.08, y + ph * 0.18, 0.06],
                    );
                    mesh.bit(
                        hung,
                        [pw * 0.22, ph * 0.22, 0.01],
                        t.muted,
                        [pw * 0.2, y + ph * 0.56, 0.06],
                    );
                }
            }
        }
        "hanging_plant" => {
            let y = low + (top - low) * 0.66;
            mesh.bit(hung, [0.05, 0.05, 0.3], t.metal, [0., y + 0.3, 0.15]);
            mesh.piece(hung, [0.3, 0.24, 0.3], t.piece, [0., y, 0.28]);
            leaves(
                mesh,
                t,
                hung,
                &[
                    (0., y + 0.3, 0.28, 0.22),
                    (-0.14, y + 0.08, 0.36, 0.15),
                    (0.13, y - 0.1, 0.34, 0.13),
                    (-0.05, y - 0.3, 0.38, 0.1),
                ],
            );
        }
        "plant" => {
            pot(mesh, t, part, 0.42, 0.4);
            leaves(
                mesh,
                t,
                part,
                &[
                    (0., 0.84, 0., 0.38),
                    (0.16, 1.14, -0.08, 0.3),
                    (-0.14, 1.32, 0.1, 0.24),
                    (0.02, 1.52, 0., 0.16),
                ],
            );
        }
        "plant_2" => {
            pot(mesh, t, part, 0.46, 0.34);
            leaves(
                mesh,
                t,
                part,
                &[
                    (-0.2, 0.62, 0.06, 0.26),
                    (0.2, 0.66, -0.06, 0.28),
                    (0., 0.86, 0.14, 0.26),
                    (0.04, 0.9, -0.18, 0.22),
                    (0., 1.1, 0., 0.2),
                ],
            );
        }
        "cactus" => {
            pot(mesh, t, part, 0.4, 0.32);
            mesh.bit(part, [0.2, 0.9, 0.2], t.plant, [0., 0.32, 0.]);
            mesh.bit(part, [0.26, 0.1, 0.12], t.plant, [0.18, 0.74, 0.]);
            mesh.bit(part, [0.1, 0.3, 0.12], t.plant, [0.28, 0.74, 0.]);
            mesh.bit(part, [0.24, 0.1, 0.12], t.plant, [-0.17, 0.56, 0.]);
            mesh.bit(part, [0.1, 0.24, 0.12], t.plant, [-0.26, 0.56, 0.]);
        }
        "large_plant" => {
            pot(mesh, t, part, 0.7, 0.5);
            leaves(
                mesh,
                t,
                part,
                &[
                    (0., 1.0, 0., 0.52),
                    (0.36, 1.3, -0.1, 0.42),
                    (-0.34, 1.42, 0.12, 0.4),
                    (0.06, 1.74, 0.02, 0.36),
                    (-0.1, 2.06, -0.04, 0.24),
                ],
            );
        }
        "pot" => pot(mesh, t, part, 0.5, 0.46),
        "bin" => {
            mesh.piece(part, [0.42, 0.5, 0.42], t.metal, [0., 0., 0.]);
            mesh.bit(part, [0.34, 0.02, 0.34], t.ink_screen(), [0., 0.49, 0.]);
        }
        "coffee" => {
            mesh.bit(part, [0.14, 0.15, 0.14], t.paper, [0., 0., 0.]);
            mesh.bit(part, [0.1, 0.012, 0.1], t.ink_screen(), [0., 0.15, 0.]);
            mesh.bit(part, [0.05, 0.08, 0.03], t.paper, [0.09, 0.04, 0.]);
        }
        "rug" => {
            mesh.glow(part, [w - 0.16, 0.03, d - 0.16], t.accent, [0., 0., 0.]);
            face(
                mesh,
                part,
                (d - 0.16).min(w - 0.16) * 1.02,
                0.034,
                t.on_accent,
            );
        }
        "nest" => {
            mesh.piece(part, [0.84, 0.14, 0.84], t.piece_low, [0., 0., 0.]);
            for (x, z, bw, bd) in [
                (0., -0.36, 0.84, 0.12),
                (0., 0.36, 0.84, 0.12),
                (-0.36, 0., 0.12, 0.6),
                (0.36, 0., 0.12, 0.6),
            ] {
                mesh.piece(part, [bw, 0.12, bd], t.piece, [x, 0.14, z]);
            }
            mesh.bit(part, [0.58, 0.04, 0.58], t.muted, [0., 0.14, 0.]);
        }
        "telescope" => {
            // A tripod: three legs from one head, their feet apart.
            let apex = 1.02;
            for turn in [0., 2.1, -2.1] {
                let leg = part.moved([0., apex, 0.]).turned(turn).pitched(0.3);
                mesh.bit(
                    leg,
                    [0.05, apex + 0.05, 0.05],
                    t.metal,
                    [0., -apex - 0.04, 0.],
                );
            }
            mesh.piece(part, [0.16, 0.12, 0.16], t.piece_low, [0., apex - 0.02, 0.]);
            // The tube looks up and out over the back wall: wide at the
            // glass, narrow at the eye, with a ring and a finder on it.
            let tube = part.moved([0., apex + 0.18, 0.]).pitched(0.55);
            mesh.piece(tube, [0.2, 0.2, 0.95], t.piece, [0., -0.1, -0.12]);
            mesh.piece(tube, [0.27, 0.27, 0.16], t.metal, [0., -0.135, -0.66]);
            mesh.glow(tube, [0.19, 0.19, 0.01], t.info, [0., -0.095, -0.745]);
            mesh.bit(tube, [0.22, 0.22, 0.05], t.text, [0., -0.11, -0.3]);
            mesh.bit(tube, [0.1, 0.1, 0.2], t.metal, [0., -0.05, 0.44]);
            mesh.bit(tube, [0.06, 0.06, 0.34], t.metal, [0., 0.1, -0.2]);
        }
        _ => {
            mesh.piece(part, [w - 0.12, 0.8, d - 0.12], t.piece, [0., 0., 0.]);
            return false;
        }
    }
    true
}

/// A piece on its own, turned so many times: its mesh, and the box of the
/// room it fills, from its lowest corner to its highest. What a picture of
/// it in a list is taken of. `None` for an id that is no piece.
pub fn alone(t: &Theme, id: &str, turn: u8) -> Option<(Mesh, [f32; 3], [f32; 3])> {
    use crate::catalogue::Placement;
    let entry = crate::catalogue::find(id)?;
    let view = entry.view(turn);
    let mut mesh = Mesh::new(t.edge, true);
    let live = Live::default();
    if entry.placement == Placement::Wall {
        let rows = view.h.clamp(1, 2);
        let slot = Slot {
            x: 0.,
            z: 1.,
            w: view.w as f32,
            d: 1.,
            y: 0.,
            facing: None,
            row: 0,
            rows,
        };
        // A piece of the wall it hangs on, behind it.
        let (low, top) = (wall_row(rows - 1).0, wall_row(0).1);
        mesh.bit(
            Part::ROOT,
            [slot.w + 0.2, top - low + 0.2, 0.06],
            t.wall,
            [slot.w / 2., low - 0.1, 1.97],
        );
        piece(&mut mesh, t, id, &slot, &live);
        return Some((
            mesh,
            [-0.1, low - 0.1, 1.94],
            [slot.w + 0.1, top + 0.1, 2.3],
        ));
    }
    let behind = entry.background.min(view.h - 1);
    let slot = Slot {
        x: 0.,
        z: 0.,
        w: view.w as f32,
        d: (view.h - behind) as f32,
        y: 0.,
        facing: view.facing,
        row: 0,
        rows: 0,
    };
    piece(&mut mesh, t, id, &slot, &live);
    Some((mesh, [0., 0., 0.], [slot.w, height(id).max(0.2), slot.d]))
}

impl Theme {
    /// A line drawn on paper: dark in every theme.
    pub fn ink_line(&self) -> Rgb {
        if self.dark {
            self.background
        } else {
            self.text
        }
    }
}
