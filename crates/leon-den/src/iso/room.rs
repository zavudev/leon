//! The room at a moment in 2.5D: its floor and walls, every piece of its
//! layout, and the lions of a frame.
//!
//! [`furnish`] draws a layout with nobody in it: the floor inside the
//! walls, ruled as its style says, the carpets as zones of another tone,
//! the back wall and the wall at its left (the two at the right and at the
//! front are left open, so the eye sees in), and each placed piece where
//! the layout puts it: on the floor, on the table under it, or on the back
//! wall. [`build`] adds the lions of a [`Frame`], each where the simulation
//! has it, sitting on its real seat and doing what its look says, and
//! answers with where each one stands ([`Stand`]): what the name plates,
//! the bubbles and the pointer are placed by. Lions that share a tile are
//! put side by side on it.
//!
//! While the room is edited, [`hit`] says what is under the pointer (a
//! piece, the back wall, a tile of floor) and [`marks`] draws what the
//! editor has to show: the piece in hand where it would go, the selected
//! one, the tile under the pointer.

use std::collections::HashMap;

use crate::catalogue::{Placement, Role};
use crate::editor::Marks;
use crate::layout::{DenLayout, Placed};
use crate::palette::bytes;
use crate::pose::{Act, Bubble, TILE};
use crate::sim::{Den, Frame};
use crate::world::Tile;

use super::camera::{Camera, WALL_HEIGHT};
use super::lion::{self, Pose, Traits, LITTLE_SCALE, SCALE};
use super::mesh::{blend, Finish, Mesh, Part, Rgb};
use super::pieces::{self, angle, Live, Slot};
use super::theme::{Ruling, Theme};

/// Where a lion stands in the room, for what is written over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stand {
    /// The lion.
    pub id: u64,
    /// Its name.
    pub name: String,
    /// The floor under it.
    pub feet: [f32; 3],
    /// How high over its feet its bubble hangs.
    pub height: f32,
    /// Half of its width.
    pub half: f32,
    /// What floats over it.
    pub bubble: Option<Bubble>,
    /// It is the selected one.
    pub selected: bool,
    /// The pointer is on it.
    pub hovered: bool,
    /// Still in its egg: it has no plate.
    pub egg: bool,
}

/// A room, drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// Its triangles and hairlines.
    pub mesh: Mesh,
    /// Where each lion stands, the farthest from the eye first.
    pub stands: Vec<Stand>,
    /// The ids of the pieces that had no drawing of their own.
    pub plain: Vec<String>,
}

fn floor(mesh: &mut Mesh, t: &Theme, layout: &DenLayout) {
    let (cols, rows) = (layout.cols as f32, layout.rows as f32);
    let own = crate::assets::floor(&layout.floor).unwrap_or(0);
    mesh.piece(
        Part::ROOT,
        [cols - 2. + 0.2, 0.3, rows - 2. + 0.1],
        t.floor,
        [cols / 2. - 0.1, -0.3, (rows + 2.) / 2. - 0.05],
    );
    // The carpets: zones of another tone, the later over the earlier.
    for (index, carpet) in layout.carpets.iter().enumerate() {
        let (x0, x1) = (carpet.x.max(1), (carpet.x + carpet.w).min(layout.cols - 1));
        let (z0, z1) = (carpet.y.max(2), (carpet.y + carpet.h).min(layout.rows));
        if x1 <= x0 || z1 <= z0 {
            continue;
        }
        let style = crate::assets::floor(&carpet.style).unwrap_or(own);
        let high = (0.004 + 0.002 * index as f32).min(0.012);
        mesh.bit(
            Part::ROOT,
            [(x1 - x0) as f32, high, (z1 - z0) as f32],
            t.floor_of(style, own),
            [(x0 + x1) as f32 / 2., 0., (z0 + z1) as f32 / 2.],
        );
    }
    if !mesh.hairlines {
        return;
    }
    // The ruling: each tile draws its right and its front edge, as its own
    // floor is ruled.
    let y = 0.013;
    for row in 2..layout.rows {
        for col in 1..layout.cols - 1 {
            let style = layout.floor_at(Tile::new(col, row));
            let (x, z) = (col as f32, row as f32);
            let (right, front) = match Theme::ruling(style) {
                Ruling::Plain => (false, false),
                Ruling::Tiles => (true, true),
                Ruling::Slabs => (col % 2 == 0, row % 2 == 0),
                Ruling::Planks => ((col + row * 2) % 4 == 0, true),
                Ruling::Checker => {
                    if (col + row) % 2 == 0 {
                        mesh.bit(
                            Part::ROOT,
                            [1., 0.003, 1.],
                            blend(t.floor_of(style, own), t.text, 0.07),
                            [x + 0.5, 0.012, z + 0.5],
                        );
                    }
                    (true, true)
                }
            };
            if right && col + 1 < layout.cols - 1 {
                mesh.line([x + 1., y, z], [x + 1., y, z + 1.], t.grid);
            }
            if front && row + 1 < layout.rows {
                mesh.line([x, y, z + 1.], [x + 1., y, z + 1.], t.grid);
            }
        }
    }
}

fn walls(mesh: &mut Mesh, t: &Theme, layout: &DenLayout) {
    let (cols, rows) = (layout.cols as f32, layout.rows as f32);
    let wall = t.wall_of(layout.wall_style());
    mesh.piece(
        Part::ROOT,
        [cols - 2. + 0.2, WALL_HEIGHT, 0.2],
        wall,
        [cols / 2. - 0.1, 0., 1.9],
    );
    mesh.piece(
        Part::ROOT,
        [0.2, WALL_HEIGHT, rows - 2.],
        wall,
        [0.9, 0., (rows + 2.) / 2.],
    );
    // The wall at the right is only its foot: the eye looks in over it.
    mesh.piece(
        Part::ROOT,
        [0.1, 0.1, rows - 2.],
        wall,
        [cols - 0.95, 0., (rows + 2.) / 2.],
    );
}

/// Where a placed piece goes in the room, if its id is in the catalogue.
pub fn slot(layout: &DenLayout, index: usize) -> Option<Slot> {
    slot_of(layout, layout.items.get(index)?, Some(index))
}

/// Where a piece would go in the room: one of the layout (`index` is then
/// its own) or one that is not in it yet.
pub fn slot_of(layout: &DenLayout, placed: &Placed, index: Option<usize>) -> Option<Slot> {
    let (entry, view) = placed.piece()?;
    if entry.placement == Placement::Wall {
        return Some(Slot {
            x: placed.x as f32,
            z: 1.,
            w: view.w as f32,
            d: 1.,
            y: 0.,
            facing: None,
            row: placed.y.clamp(0, 1),
            rows: view.h.clamp(1, 2 - placed.y.clamp(0, 1)),
        });
    }
    let behind = entry.background.min(view.h - 1);
    let table = layout
        .surface_under(placed, index)
        .map(|table| pieces::surface_height(&layout.items[table].id));
    Some(Slot {
        x: placed.x as f32,
        z: (placed.y + behind) as f32,
        w: view.w as f32,
        d: (view.h - behind) as f32,
        y: table.unwrap_or(0.),
        facing: view.facing,
        row: 0,
        rows: 0,
    })
}

/// Draws a layout with nobody in it. `live` says what each piece shows of
/// the life of the room, by its index in the layout. It answers with the
/// ids that had no drawing of their own.
pub fn furnish(
    mesh: &mut Mesh,
    t: &Theme,
    layout: &DenLayout,
    live: impl Fn(usize) -> Live,
) -> Vec<String> {
    floor(mesh, t, layout);
    walls(mesh, t, layout);
    let mut plain = Vec::new();
    for (index, placed) in layout.items.iter().enumerate() {
        let Some(slot) = slot(layout, index) else {
            continue;
        };
        if !pieces::piece(mesh, t, &placed.id, &slot, &live(index)) {
            plain.push(placed.id.clone());
        }
    }
    plain
}

/// What the pointer of the editor is after.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aim {
    /// Whatever shows under it: a piece, the wall, the floor.
    Any,
    /// The plane at this height: the floor, with nothing. What a piece in
    /// hand for the floor, a carpet, and a piece being dragged follow.
    Level(f32),
    /// The top of a table, or the floor where there is none: what a piece
    /// that stands on a table follows.
    Surface,
    /// The back wall, or the floor where the pointer is off it: what a
    /// piece that hangs follows.
    Wall,
}

/// What the pointer found in the room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The tile of the plan: of the floor, or, in its two first rows, of
    /// the back wall.
    pub tile: Tile,
    /// How high the point is.
    pub height: f32,
    /// The piece it is on: its index in the layout.
    pub item: Option<usize>,
}

/// The face of the back wall.
const WALL_FACE: f32 = 2.;

/// What is under a pixel of a picture of the room.
///
/// A pixel shows a whole line of points, one at each height, that goes
/// toward the eye as it goes up. With [`Aim::Any`] the line is followed
/// from the eye: the first box of a piece it enters wins (a piece is the
/// box of the tiles it stands on, as high as [`pieces::height`] says), then
/// the back wall, then the floor. So a piece is taken by any part of it
/// that shows, and one hidden behind another is taken by the part of it
/// that still shows, or by its tiles of floor when another aim is used. A
/// tile off the room is given all the same: the editor refuses what it must.
pub fn hit(layout: &DenLayout, camera: &Camera, x: f32, y: f32, aim: Aim) -> Hit {
    let (x0, z0) = camera.floor(x, y, 0.);
    let slope = camera.floor(x, y, 1.).0 - x0;
    let at = |height: f32| (x0 + slope * height, z0 + slope * height);
    let level = |height: f32, item: Option<usize>| {
        let (px, pz) = at(height);
        Hit {
            tile: Tile::new(px.floor() as i32, pz.floor() as i32),
            height,
            item,
        }
    };
    // Where the line meets the back wall, if it does on its face.
    let on_wall = || {
        let height = (WALL_FACE - z0) / slope;
        let (px, _) = at(height);
        let inside =
            (0. ..=WALL_HEIGHT).contains(&height) && px >= 1. && px < (layout.cols - 1) as f32;
        inside.then(|| Hit {
            tile: Tile::new(px.floor() as i32, i32::from(height < pieces::wall_row(0).0)),
            height,
            item: None,
        })
    };
    // The highest point of the line in a box: where the eye enters it.
    let enters = |slot: &Slot, high: f32| {
        let span =
            |from: f32, to: f32, origin: f32| ((from - origin) / slope, (to - origin) / slope);
        let (xa, xb) = span(slot.x, slot.x + slot.w, x0);
        let (za, zb) = span(slot.z, slot.z + slot.d, z0);
        let (low, top) = (slot.y.max(xa).max(za), (slot.y + high).min(xb).min(zb));
        (low <= top).then_some(top)
    };
    let boxes = |only_surfaces: bool| {
        let mut best: Option<Hit> = None;
        for (index, placed) in layout.items.iter().enumerate() {
            let Some((entry, _)) = placed.piece() else {
                continue;
            };
            if entry.placement == Placement::Wall || (only_surfaces && !entry.surface) {
                continue;
            }
            let Some(slot) = slot(layout, index) else {
                continue;
            };
            let Some(height) = enters(&slot, pieces::height(&placed.id)) else {
                continue;
            };
            if best.is_some_and(|best| best.height >= height) {
                continue;
            }
            // The tile is one the piece stands on, whatever the rounding.
            let (px, pz) = at(height - 1e-3);
            let within = |value: f32, from: f32, span: f32| {
                (value.floor() as i32).clamp(from as i32, (from + span) as i32 - 1)
            };
            best = Some(Hit {
                tile: Tile::new(within(px, slot.x, slot.w), within(pz, slot.z, slot.d)),
                height,
                item: Some(index),
            });
        }
        best
    };
    match aim {
        Aim::Level(height) => level(height, None),
        Aim::Wall => on_wall().unwrap_or_else(|| level(0., None)),
        Aim::Surface => boxes(true).unwrap_or_else(|| level(0., None)),
        Aim::Any => {
            // What hangs on the wall is found by its tile.
            [boxes(false), on_wall()]
                .into_iter()
                .flatten()
                .max_by(|a, b| a.height.total_cmp(&b.height))
                .unwrap_or_else(|| level(0., None))
        }
    }
}

/// A tile of the plan marked in a colour: a wash on the floor with a line
/// about it, or the same on the back wall for its two first rows.
fn mark(mesh: &mut Mesh, t: &Theme, tile: Tile, color: Rgb, by: f32) {
    let (x, z) = (tile.x as f32, tile.y as f32);
    if tile.y < 2 {
        let (low, top) = pieces::wall_row(tile.y.max(0));
        let face = WALL_FACE + 0.012;
        mesh.solid(
            Part::ROOT,
            [0.94, top - low - 0.06, 0.006],
            blend(t.wall, color, by),
            [x + 0.5, low + 0.03, face],
            Finish::Glow,
        );
        return;
    }
    let y = 0.016;
    mesh.solid(
        Part::ROOT,
        [0.94, 0.004, 0.94],
        blend(t.floor, color, by),
        [x + 0.5, y, z + 0.5],
        Finish::Glow,
    );
    let (a, b) = (0.03, 0.97);
    let top = y + 0.006;
    for (from, to) in [
        ((a, a), (b, a)),
        ((b, a), (b, b)),
        ((b, b), (a, b)),
        ((a, b), (a, a)),
    ] {
        mesh.line(
            [x + from.0, top, z + from.1],
            [x + to.0, top, z + to.1],
            color,
        );
    }
}

/// The edges of the box a piece fills, a little outside it, in a colour.
fn outline(mesh: &mut Mesh, slot: &Slot, high: f32, color: Rgb) {
    let grow = 0.05;
    let (x0, x1) = (slot.x - grow, slot.x + slot.w + grow);
    let (z0, z1) = (slot.z - grow, slot.z + slot.d + grow);
    let (y0, y1) = (slot.y + 0.02, slot.y + high + grow);
    let corner = |index: usize| {
        [
            if index & 1 == 0 { x0 } else { x1 },
            if index & 2 == 0 { y0 } else { y1 },
            if index & 4 == 0 { z0 } else { z1 },
        ]
    };
    for (from, to) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        mesh.line(corner(from), corner(to), color);
    }
}

/// What the editor draws over the room: the selected piece in the accent,
/// the tile under an empty hand, the tile a carpet would take, and the
/// piece in hand where it would go, green where it may and red where it
/// may not.
pub fn marks(mesh: &mut Mesh, t: &Theme, layout: &DenLayout, marks: &Marks) {
    for tile in &marks.selected {
        mark(mesh, t, *tile, t.accent, 0.3);
    }
    if let Some(slot) = marks.chosen.and_then(|index| slot(layout, index)) {
        let placed = &layout.items[marks.chosen.unwrap_or(0)];
        let hung = placed
            .piece()
            .is_some_and(|(entry, _)| entry.placement == Placement::Wall);
        if !hung {
            outline(mesh, &slot, pieces::height(&placed.id), t.accent);
        }
    }
    if let Some(tile) = marks.cursor {
        mark(mesh, t, tile, t.accent, 0.14);
    }
    if let Some((tile, fits)) = marks.tile {
        mark(mesh, t, tile, if fits { t.success } else { t.error }, 0.45);
    }
    let Some((placed, fits)) = &marks.ghost else {
        return;
    };
    let color = if *fits { t.success } else { t.error };
    for tile in placed.footprint() {
        mark(mesh, t, tile, color, 0.3);
    }
    let Some(slot) = slot_of(layout, placed, None) else {
        return;
    };
    // The piece itself, washed in the colour, with its edges in it.
    let ghost = t.washed(color, 0.55);
    let hairlines = std::mem::replace(&mut mesh.hairlines, true);
    let edge = std::mem::replace(&mut mesh.edge, color);
    pieces::piece(mesh, &ghost, &placed.id, &slot, &Live::default());
    mesh.hairlines = hairlines;
    mesh.edge = edge;
}

fn rgb(color: gpui_kit::Hsla) -> Rgb {
    let [r, g, b, _] = bytes(color);
    [
        f32::from(r) / 255.,
        f32::from(g) / 255.,
        f32::from(b) / 255.,
    ]
}

/// A square drawn on the floor about a point: the mark of the selection.
fn ring(mesh: &mut Mesh, at: [f32; 3], side: f32, thick: f32, color: Rgb) {
    let part = Part::at(at[0], 0.014, at[2]);
    let half = side / 2.;
    for (x, z, w, d) in [
        (0., -half, side + thick, thick),
        (0., half, side + thick, thick),
        (-half, 0., thick, side - thick),
        (half, 0., thick, side - thick),
    ] {
        mesh.solid(part, [w, 0.006, d], color, [x, 0., z], Finish::Glow);
    }
}

/// How high whoever is on a tile sits: the seats, and the nests.
fn seats(layout: &DenLayout) -> HashMap<Tile, f32> {
    let mut seats = HashMap::new();
    for placed in &layout.items {
        let Some((entry, _)) = placed.piece() else {
            continue;
        };
        let high = match entry.role {
            Some(Role::Seat) => pieces::seat_height(entry.id),
            Some(Role::Nest) => 0.16,
            _ => continue,
        };
        for tile in placed.base() {
            seats.insert(tile, high);
        }
    }
    seats
}

/// The middle of the entrance of a room, its rug: where those that wait
/// for the user line up.
fn entrance(layout: &DenLayout) -> Option<[f32; 2]> {
    let tiles: Vec<Tile> = layout
        .items
        .iter()
        .filter(|placed| {
            placed
                .piece()
                .is_some_and(|(entry, _)| entry.role == Some(Role::Entrance))
        })
        .flat_map(|placed| placed.footprint())
        .collect();
    let count = tiles.len() as f32;
    (count > 0.).then(|| {
        [
            tiles.iter().map(|tile| tile.x as f32 + 0.5).sum::<f32>() / count,
            tiles.iter().map(|tile| tile.y as f32 + 0.5).sum::<f32>() / count,
        ]
    })
}

/// A lion of a frame, placed: where it stands and how it holds itself.
struct Stood {
    stand: Stand,
    scale: f32,
    /// How high it sits, or the nest it lies in.
    high: Option<f32>,
    pose: Option<Pose>,
    /// It stands at the entrance and waits for the user.
    waits: bool,
}

fn place(
    frame: &Frame,
    glide: &[(u64, f32, f32)],
    seconds: f32,
    seats: &HashMap<Tile, f32>,
    entrance: Option<[f32; 2]>,
) -> Vec<Stood> {
    frame
        .actors
        .iter()
        .map(|actor| {
            let (col, row) = glide.iter().find(|(id, _, _)| *id == actor.id).map_or(
                (actor.x as f32 / TILE as f32, actor.y as f32 / TILE as f32),
                |(_, x, y)| (*x, *y),
            );
            let scale = if actor.look.little {
                LITTLE_SCALE
            } else {
                SCALE
            };
            let egg = matches!(actor.look.act, Act::Egg | Act::Hatch);
            let high = seats.get(&actor.tile).copied();
            let pose = (!egg).then(|| Pose::of(&actor.look, seconds * 9. + (actor.id % 7) as f32));
            let height = match &pose {
                Some(pose) => lion::height(pose, high.unwrap_or(pieces::SEAT), scale),
                None => 1.,
            };
            Stood {
                stand: Stand {
                    id: actor.id,
                    name: actor.name.clone(),
                    feet: [col + 0.5, 0., row + 0.5],
                    height,
                    half: 0.36 * scale,
                    bubble: actor.look.bubble,
                    selected: actor.selected,
                    hovered: actor.hovered,
                    egg,
                },
                scale,
                high,
                pose,
                waits: matches!(actor.look.act, Act::Wait | Act::Stare),
            }
        })
        .collect::<Vec<_>>()
        .lined_up(entrance)
        .apart()
}

/// How far apart those that wait stand in their line, and how far behind
/// a line the next one is, in tiles.
const LINE_APART: f32 = 1.05;
const LINES_APART: f32 = 1.7;
/// How many stand in one line.
const IN_LINE: usize = 3;

trait LinedUp {
    fn lined_up(self, entrance: Option<[f32; 2]>) -> Self;
}

impl LinedUp for Vec<Stood> {
    /// Those that wait for the user are the ones to tell apart at a glance,
    /// and on neighbouring tiles of the rug one stands in front of the
    /// other for the eye. They are put in a line across the eye's line,
    /// about the middle of the entrance, each wholly in view; more than
    /// [`IN_LINE`] make a second line behind the first, and so on, past the
    /// rug if they are many. The order is that of their ids, so nobody
    /// changes place when another comes. One alone stays on its tile.
    fn lined_up(mut self, entrance: Option<[f32; 2]>) -> Self {
        let mut waiting: Vec<usize> = (0..self.len())
            .filter(|index| {
                let stood = &self[*index];
                stood.waits
                    && stood.high.is_none()
                    && stood.stand.feet[0].fract() == 0.5
                    && stood.stand.feet[2].fract() == 0.5
            })
            .collect();
        if waiting.len() < 2 {
            return self;
        }
        waiting.sort_by_key(|index| self[*index].stand.id);
        let count = waiting.len() as f32;
        let [mid_x, mid_z] = entrance.unwrap_or_else(|| {
            let sum = waiting.iter().fold([0., 0.], |sum: [f32; 2], index| {
                let feet = self[*index].stand.feet;
                [sum[0] + feet[0], sum[1] + feet[2]]
            });
            [sum[0] / count, sum[1] / count]
        });
        let across = std::f32::consts::FRAC_1_SQRT_2;
        let lines = waiting.len().div_ceil(IN_LINE);
        for (place, index) in waiting.iter().enumerate() {
            let (line, slot) = (place / IN_LINE, place % IN_LINE);
            let in_this = if line + 1 == lines {
                waiting.len() - line * IN_LINE
            } else {
                IN_LINE
            };
            let along = (slot as f32 - (in_this as f32 - 1.) / 2.) * LINE_APART;
            // The first line is nearest the eye; the others stand behind.
            let back = (line as f32 - (lines as f32 - 1.) / 2.) * LINES_APART;
            let feet = &mut self[*index].stand.feet;
            feet[0] = mid_x + (along - back) * across;
            feet[2] = mid_z + (-along - back) * across;
        }
        self
    }
}

/// How far from the middle of a tile each of several lions on it stands.
const APART: f32 = 0.3;

trait Apart {
    fn apart(self) -> Self;
}

impl Apart for Vec<Stood> {
    /// Lions that stand on the same spot are put about it, so that each
    /// shows: the pixel art draws them one over the other, and a room seen
    /// from above has the place to do better. Whoever sits, lies in a nest
    /// or is on its way is left where it is.
    fn apart(mut self) -> Self {
        let free = |stood: &Stood| {
            stood.high.is_none()
                && stood.pose.as_ref().is_some_and(|pose| !pose.seated)
                && stood.stand.feet[0].fract() == 0.5
                && stood.stand.feet[2].fract() == 0.5
        };
        let mut spots: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, stood) in self.iter().enumerate() {
            if free(stood) {
                let [x, _, z] = stood.stand.feet;
                spots
                    .entry((x.floor() as i32, z.floor() as i32))
                    .or_default()
                    .push(index);
            }
        }
        for sharing in spots.values().filter(|sharing| sharing.len() > 1) {
            let count = sharing.len() as f32;
            for (place, index) in sharing.iter().enumerate() {
                // Side by side across the eye's line, the first at the left.
                let along =
                    (place as f32 - (count - 1.) / 2.) * (2. * APART / (count - 1.)).min(0.62);
                let feet = &mut self[*index].stand.feet;
                feet[0] += along * std::f32::consts::FRAC_1_SQRT_2;
                feet[2] -= along * std::f32::consts::FRAC_1_SQRT_2;
            }
        }
        self
    }
}

/// Where the lions of a frame stand, without drawing anything: what the
/// name plates, the bubbles and the pointer need. `glide` is where each is
/// between two tiles ([`Den::glide`]) and `seconds` the time.
pub fn stands(den: &Den, frame: &Frame, glide: &[(u64, f32, f32)], seconds: f32) -> Vec<Stand> {
    place(
        frame,
        glide,
        seconds,
        &seats(den.layout()),
        entrance(den.layout()),
    )
    .into_iter()
    .map(|placed| placed.stand)
    .collect()
}

/// The room of a den at a frame, with its lions. `glide` is where each
/// lion is between two tiles ([`Den::glide`]) and `seconds` the time, for
/// the stride of those that walk.
pub fn build(
    den: &Den,
    frame: &Frame,
    glide: &[(u64, f32, f32)],
    seconds: f32,
    t: &Theme,
    hairlines: bool,
    edited: Option<&Marks>,
) -> Built {
    let layout = den.layout();
    let world = den.world();
    let mut mesh = Mesh::new(t.edge, hairlines);

    // What the pieces show: a screen for the lion at its seat, the lights
    // of the racks while a command runs.
    let mut screens: HashMap<usize, Rgb> = HashMap::new();
    for actor in &frame.actors {
        let Some(computer) = world.computer_of(actor.tile) else {
            continue;
        };
        let lit = match actor.look.act {
            Act::Type => t.success,
            Act::Think => t.info,
            Act::Wonder => t.faint,
            Act::Faint => t.error,
            _ => continue,
        };
        screens.insert(computer, lit);
    }
    let running = frame
        .actors
        .iter()
        .any(|actor| actor.look.act == Act::Operate);
    let plain = furnish(&mut mesh, t, layout, |index| Live {
        tick: frame.tick,
        screen: screens.get(&index).copied(),
        running,
    });

    let placed = place(frame, glide, seconds, &seats(layout), entrance(layout));
    let mut stands = Vec::with_capacity(placed.len());
    for (actor, placed) in frame.actors.iter().zip(placed) {
        let at = placed.stand.feet;
        let mane = rgb(actor.tint);
        match &placed.pose {
            None => {
                let nest = Part::at(at[0], placed.high.unwrap_or(0.), at[2]);
                lion::egg(
                    &mut mesh,
                    t,
                    nest,
                    mane,
                    actor.look.beat,
                    actor.look.act == Act::Hatch,
                );
            }
            Some(pose) => {
                let feet = Part::at(at[0], 0., at[2]).turned(angle(actor.look.facing));
                lion::lion(
                    &mut mesh,
                    t,
                    feet,
                    &Traits::of(actor.seed),
                    mane,
                    pose,
                    lion::Fit {
                        seat: placed.high.unwrap_or(pieces::SEAT),
                        scale: placed.scale,
                    },
                );
            }
        }
        if actor.look.act == Act::Rummage {
            papers(&mut mesh, t, at, placed.stand.height, frame.tick + actor.id);
        }
        if actor.selected {
            ring(&mut mesh, at, 0.92, 0.07, t.accent);
        } else if actor.hovered {
            ring(&mut mesh, at, 0.92, 0.04, t.muted);
        }
        stands.push(placed.stand);
    }
    if let Some(edited) = edited {
        marks(&mut mesh, t, layout, edited);
    }
    Built {
        mesh,
        stands,
        plain,
    }
}

/// The sheets a lion throws over its shoulders as it empties a shelf: three
/// of them, each on its own arc, turning as it flies.
fn papers(mesh: &mut Mesh, t: &Theme, at: [f32; 3], head: f32, tick: u64) {
    for sheet in 0..3u64 {
        let phase = ((tick + sheet * 5) % 15) as f32 / 15.;
        let side = [-1., 1., -0.3][sheet as usize];
        let part = Part::at(
            at[0] + side * (0.25 + phase * 0.55),
            head * 0.8 + (phase * std::f32::consts::PI).sin() * 0.75,
            at[2] + 0.2 + phase * 0.5,
        )
        .turned(phase * 4. + sheet as f32)
        .rolled((phase * 6.).sin() * 0.7);
        mesh.bit(part, [0.24, 0.012, 0.3], t.paper, [0., 0., 0.]);
    }
}
