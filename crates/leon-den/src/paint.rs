//! The painter: a frame of the den as one picture.
//!
//! [`compose`] lays the room out in a [`Bitmap`] at the size of its art, a
//! pixel of the picture for a pixel of the sheets: the backdrop, then the
//! furniture and the lions together, the farthest first, so that a lion
//! walks behind a desk and sits in front of its seat, and last what floats:
//! the names, the marks of the selection, the bubbles. The view scales the
//! result by a whole number and hands it to GPUI as a single image; the
//! example `den_png` writes the same picture to a file.
//!
//! The room has its own colours. Only what is chrome is painted in the
//! host's palette: the bubbles, the name plates, the selection.
//!
//! # The look of a lion
//!
//! Its seed makes a lion an individual ([`Look`]): which body and clothes,
//! which head ([`crate::atelier::STYLES`]), which fur ([`COATS`]) and which
//! shade of its agent's colour ([`SHADES`]). The agent's colour is on every
//! lion, on its mane or on what it wears, so the agents are told apart at
//! a glance and the sessions of one agent are not one lion many times.
//!
//! A [`Wardrobe`] keeps the sheets already dyed, so that a lion is dyed
//! once and not on every frame.

use std::collections::HashMap;

use gpui_kit::Hsla;

use crate::assets::art;
use crate::atelier::{COAT, CUB_FRAME, FRAME_H, FRAME_W, MANE, STYLES};
use crate::bitmap::{hsl, Bitmap, Rgba};
use crate::catalogue::Role;
use crate::editor::Marks;
use crate::glyphs::{self, GLYPH_ADVANCE, GLYPH_H, GLYPH_W};
use crate::palette::{bytes, scramble, DenPalette};
use crate::pose::{bubble, cel, Act, Sheet, TILE};
use crate::sim::{Actor, Den, Frame};
use crate::sprite::Stamp;
use crate::world::Item;

/// The lightness a mane's middle colour is kept within: neither a fleece
/// nor a head of black hair.
pub const MANE_LIGHTNESS: (f32, f32) = (0.26, 0.74);

fn to_hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.,
        f32::from(g) / 255.,
        f32::from(b) / 255.,
    );
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.;
    if max - min < 1e-4 {
        return (0., 0., l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2. - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.)
    } else if max == g {
        (b - r) / d + 2.
    } else {
        (r - g) / d + 4.
    };
    (h * 60., s, l)
}

/// The three colours of a mane of this tint: light, middle and dark.
///
/// Whatever the tint, the result is a mane. A tint with no colour of its
/// own, the white or the black of a monochrome agent, is warmed a little:
/// white gives a pale silver-cream mane and black a charcoal-brown one. The
/// middle colour is kept within [`MANE_LIGHTNESS`], and the light and the
/// dark steps stand a fixed way off it, so the strands always show.
pub fn mane(tint: [u8; 3]) -> [[u8; 3]; 3] {
    let (hue, saturation, lightness) = to_hsl(tint);
    // A grey has no hue: give it the warm one of fur, faintly.
    let (hue, saturation) = if saturation < 0.12 {
        (32., 0.16)
    } else {
        (hue, saturation.min(0.85))
    };
    let middle = lightness.clamp(MANE_LIGHTNESS.0, MANE_LIGHTNESS.1);
    [
        hsl(hue, saturation * 0.9, middle + 0.13),
        hsl(hue, saturation, middle),
        hsl(hue, (saturation * 1.1).min(1.), middle - 0.15),
    ]
}

/// A coat: the light, the middle and the dark of the fur, the cream of the
/// muzzle and the inside of the ear, in the order of [`COAT`].
pub type Coat = [[u8; 3]; 5];

/// The coats a lion can have, the golden one of the sheets first. They are
/// all coats of a lion, the white one too.
pub const COATS: [Coat; 6] = [
    // Golden: the sheets as they are drawn.
    [
        [244, 200, 124],
        [226, 166, 88],
        [178, 114, 56],
        [252, 236, 204],
        [204, 126, 104],
    ],
    // Tawny.
    [
        [226, 160, 96],
        [198, 124, 62],
        [140, 80, 38],
        [246, 222, 186],
        [176, 98, 84],
    ],
    // Sand.
    [
        [246, 226, 180],
        [228, 200, 146],
        [186, 152, 100],
        [255, 246, 226],
        [214, 150, 130],
    ],
    // White.
    [
        [250, 248, 240],
        [228, 224, 212],
        [180, 174, 164],
        [255, 252, 246],
        [232, 170, 170],
    ],
    // Dusky brown.
    [
        [168, 120, 88],
        [136, 92, 64],
        [92, 58, 40],
        [226, 196, 164],
        [150, 90, 84],
    ],
    // Ash grey.
    [
        [190, 186, 180],
        [152, 148, 144],
        [104, 100, 100],
        [232, 228, 220],
        [176, 128, 128],
    ],
];

/// The shades of a mane: how far its hue (in degrees) and its lightness
/// stand from the agent's colour. The first is the colour itself. They are
/// few and small: two lions of one agent differ, and both are still plainly
/// that agent's.
pub const SHADES: [(f32, f32); 9] = [
    (0., 0.),
    (-10., 0.),
    (10., 0.),
    (0., 0.08),
    (0., -0.08),
    (-10., 0.08),
    (10., 0.08),
    (-10., -0.08),
    (10., -0.08),
];

/// The tint of a mane in one of the [`SHADES`].
pub fn shaded(tint: [u8; 3], shade: usize) -> [u8; 3] {
    let (turn, lift) = SHADES[shade % SHADES.len()];
    if turn == 0. && lift == 0. {
        return tint;
    }
    let (hue, saturation, lightness) = to_hsl(tint);
    hsl(
        (hue + turn).rem_euclid(360.),
        saturation,
        (lightness + lift).clamp(0.04, 0.96),
    )
}

/// What an individual looks like: all of it follows from its seed, so a
/// session keeps its lion for as long as it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    /// Which body, and so which clothes: an index into the sheets.
    pub body: usize,
    /// Which head: an index into [`STYLES`].
    pub style: usize,
    /// Which fur: an index into [`COATS`].
    pub coat: usize,
    /// Which shade of its agent's colour: an index into [`SHADES`].
    pub shade: usize,
}

impl Look {
    /// How many looks there are.
    pub fn count() -> usize {
        art().lions.len() * STYLES.len() * COATS.len() * SHADES.len()
    }

    /// The look of the individual with this seed. Each choice is a digit of
    /// its own of the scrambled seed, so none follows from another.
    pub fn of(seed: u64) -> Self {
        let mut rest = scramble(seed);
        let mut pick = |choices: usize| {
            let chosen = (rest % choices as u64) as usize;
            rest /= choices as u64;
            chosen
        };
        Self {
            body: pick(art().lions.len()),
            style: pick(STYLES.len()),
            coat: pick(COATS.len()),
            shade: pick(SHADES.len()),
        }
    }
}

/// How far apart a mane and a fur must be to be told apart: the distance
/// between their colours, each channel counted.
const MANE_FROM_FUR: i32 = 40;

/// The coat a lion wears under a mane of this tint: the one asked for,
/// unless the mane would be lost on it (a silver mane on a white or an ash
/// coat); then the next coat that shows it.
pub fn coat_under(coat: usize, tint: [u8; 3]) -> usize {
    let middle = mane(tint)[1];
    let apart = |fur: [u8; 3]| -> i32 {
        (0..3)
            .map(|channel| (i32::from(fur[channel]) - i32::from(middle[channel])).abs())
            .sum()
    };
    (0..COATS.len())
        .map(|step| (coat + step) % COATS.len())
        .find(|coat| {
            COATS[*coat][..2]
                .iter()
                .all(|fur| apart(*fur) >= MANE_FROM_FUR)
        })
        .unwrap_or(coat % COATS.len())
}

/// A sheet with its mane in a tint and its fur in a coat: the key colours
/// of the workshop are replaced by the ramp of [`mane`] and by the colours
/// of the coat.
pub fn dyed(sheet: &Bitmap, tint: [u8; 3], coat: usize) -> Bitmap {
    let ramp = mane(tint);
    let coat = COATS[coat % COATS.len()];
    sheet.mapped(|pixel| {
        if let Some(step) = MANE.iter().position(|key| *key == pixel) {
            [ramp[step][0], ramp[step][1], ramp[step][2], pixel[3]]
        } else if let Some(part) = COAT.iter().position(|key| *key == pixel) {
            [coat[part][0], coat[part][1], coat[part][2], pixel[3]]
        } else {
            pixel
        }
    })
}

/// How many dyed sheets the wardrobe keeps before it starts again: far more
/// than a den can hold, so a full room never dyes a sheet twice.
const WARDROBE: usize = 192;

/// The sheets, dyed: one per look and agent colour.
#[derive(Default)]
pub struct Wardrobe {
    sheets: HashMap<(Sheet, Look, [u8; 3]), Bitmap>,
}

impl Wardrobe {
    /// An empty wardrobe.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many sheets have been dyed.
    pub fn len(&self) -> usize {
        self.sheets.len()
    }

    /// Whether nothing has been dyed yet.
    pub fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    /// The sheet of a lion: the body and the head its seed picks, in its
    /// coat and in its shade of its agent's tint. A little one has one body
    /// and one head, and an egg has no fur: what does not show is not told
    /// apart.
    fn sheet(&mut self, kind: Sheet, seed: u64, tint: Hsla) -> &Bitmap {
        let art = art();
        let look = Look::of(seed);
        let look = match kind {
            Sheet::Lion => look,
            Sheet::Cub => Look {
                body: 0,
                style: 0,
                ..look
            },
            Sheet::Egg => Look {
                body: 0,
                style: 0,
                coat: 0,
                ..look
            },
        };
        let color = bytes(tint);
        let tint = [color[0], color[1], color[2]];
        let look = Look {
            coat: coat_under(look.coat, shaded(tint, look.shade)),
            ..look
        };
        // A wardrobe that grew past everything a den can hold is one of a
        // theme that changed, or of many sessions long gone: start again.
        if self.sheets.len() > WARDROBE {
            self.sheets.clear();
        }
        self.sheets.entry((kind, look, tint)).or_insert_with(|| {
            let source = match kind {
                Sheet::Lion => &art.lions[look.body][look.style],
                Sheet::Cub => &art.cub,
                Sheet::Egg => &art.egg,
            };
            dyed(source, shaded(tint, look.shade), look.coat)
        })
    }
}

/// Draws a glyph on the picture in the palette's colours.
fn stamp(out: &mut Bitmap, stamp: Stamp<'_>, palette: &DenPalette) {
    for run in stamp.runs() {
        out.fill(run.x, run.y, run.w, run.h, bytes(palette.color(run.role)));
    }
}

/// Writes a text in the little font, its top left at a pixel.
fn write(out: &mut Bitmap, x: i32, y: i32, text: &str, color: Rgba) {
    for (index, letter) in text.chars().enumerate() {
        let left = x + index as i32 * GLYPH_ADVANCE;
        for (row, bits) in glyphs::glyph(letter).iter().enumerate() {
            for col in 0..GLYPH_W {
                if bits >> (GLYPH_W - 1 - col) & 1 == 1 {
                    out.blend(left + col, y + row as i32, color);
                }
            }
        }
    }
}

/// How many letters a name plate holds at most.
pub const PLATE_CHARS: usize = 12;
/// How many it holds where the room is crowded.
pub const PLATE_CHARS_SHORT: usize = 6;

/// Words that say nothing of a session by themselves: a plate skips them.
const SMALL_WORDS: [&str; 30] = [
    "A", "AN", "THE", "OF", "TO", "FOR", "AND", "OR", "IN", "ON", "WITH", "DE", "DEL", "LA", "EL",
    "LOS", "LAS", "UN", "UNA", "Y", "O", "EN", "CON", "PARA", "POR", "AL", "LE", "LES", "DES",
    "ET",
];

/// The name of a lion as its plate shows it: in capitals and in the letters
/// of the little font (accents off), its first words that mean something,
/// whole, as many as fit in `most` letters. A first word longer than that
/// is cut and ends in an ellipsis. The whole name stays in the roster, on
/// the truth card and in the feed.
pub fn plate_name(name: &str, most: usize) -> String {
    let most = most.max(2);
    let text: String = glyphs::plain(name)
        .to_uppercase()
        .chars()
        .map(|c| {
            if (c.is_alphanumeric() || matches!(c, '-' | '_' | '.')) && glyphs::writes(c) {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = text.split_whitespace().collect();
    let meaning: Vec<&str> = words
        .iter()
        .copied()
        .filter(|word| !SMALL_WORDS.contains(word))
        .collect();
    let words = if meaning.is_empty() { words } else { meaning };
    let Some(first) = words.first() else {
        return "?".to_owned();
    };
    if first.chars().count() > most {
        let mut cut: String = first.chars().take(most - 1).collect();
        cut.push('\u{2026}');
        return cut;
    }
    let mut out = (*first).to_owned();
    for word in &words[1..] {
        if out.chars().count() + 1 + word.chars().count() > most {
            break;
        }
        out.push(' ');
        out.push_str(word);
    }
    out
}

/// The name of a lion as its tag shows it: [`plate_name`] at its full
/// length.
pub fn tag_name(name: &str) -> String {
    plate_name(name, PLATE_CHARS)
}

/// A rectangle of the picture: left, top, width, height.
pub type Area = (i32, i32, i32, i32);

fn meet(a: Area, b: Area) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// The head of a lion: what a plate or a bubble of another must not cover.
pub fn head(actor: &Actor) -> Area {
    let (left, top, w, h) = actor.bounds();
    (left + 2, top, (w - 4).max(1), h.min(15))
}

/// A name plate: whose, where its box is, and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plate {
    /// The lion.
    pub id: u64,
    /// Its box in the picture.
    pub area: Area,
    /// The name as written.
    pub text: String,
    /// It is the selected lion's.
    pub selected: bool,
}

/// Where the name plates of a frame go in a picture of `w` by `h`.
///
/// Each plate is tried under its lion, a line lower, over its head, over
/// its bubble, and up to a tile to either side of each, first with the name at its full length
/// and then shortened; it takes the first place that is whole inside the
/// picture and covers no plate already placed and no other lion's head.
/// The selected lion and the one under the pointer go first. A lion with
/// no such place has no plate: the roster, the pointer and the selection
/// still name it. Only the selected lion and the one under the pointer are
/// always named, where they stand, whatever that covers.
pub fn plates(frame: &Frame, w: i32, h: i32) -> Vec<Plate> {
    let named: Vec<&Actor> = frame
        .actors
        .iter()
        .filter(|actor| !matches!(actor.look.act, Act::Egg | Act::Hatch))
        .collect();
    let mut heads: Vec<(u64, Area)> = named.iter().map(|actor| (actor.id, head(actor))).collect();
    // The bubbles are in the way too, everybody's: a plate under a `!`
    // would hide it.
    let shifts = bubble_shifts(frame);
    for actor in &frame.actors {
        let shift = shifts
            .iter()
            .find(|(id, _)| *id == actor.id)
            .map_or(0, |(_, shift)| *shift);
        for stamp in bubble(&actor.look) {
            heads.push((
                u64::MAX,
                (
                    actor.x + stamp.x + shift - 1,
                    actor.y + stamp.y - 1,
                    stamp.sprite.w + 2,
                    stamp.sprite.h + 2,
                ),
            ));
        }
    }
    let mut order: Vec<&Actor> = named.clone();
    order.sort_by_key(|actor| (!actor.selected, !actor.hovered));
    let mut placed: Vec<Plate> = Vec::new();
    for actor in order {
        let (_, top, _, height) = actor.bounds();
        let must = actor.selected || actor.hovered;
        let mut found: Option<(Area, String)> = None;
        let mut first: Option<(Area, String)> = None;
        'search: for most in [PLATE_CHARS, PLATE_CHARS_SHORT] {
            let text = plate_name(&actor.name, most);
            let width = glyphs::text_width(&text);
            let (pw, ph) = (width + 4, GLYPH_H + 2);
            let centre = actor.x + (TILE - pw) / 2;
            let under = top + height + 1;
            for y in [
                under,
                under + ph + 1,
                top - ph - 1,
                // Over its own bubble.
                top - ph - 12,
                under + 2 * (ph + 1),
            ] {
                for dx in [0, -6, 6, -12, 12, -18, 18] {
                    let area = (centre + dx, y, pw, ph);
                    if first.is_none() {
                        // Where it goes when it must be shown: under the
                        // lion, moved only as far as the picture needs.
                        let x = centre.clamp(0, (w - pw).max(0));
                        let y = under.clamp(0, (h - ph).max(0));
                        first = Some(((x, y, pw, ph), text.clone()));
                    }
                    let inside = area.0 >= 0 && area.1 >= 0 && area.0 + pw <= w && area.1 + ph <= h;
                    let clear = inside
                        && !placed.iter().any(|plate| meet(plate.area, area))
                        && !heads
                            .iter()
                            .any(|(id, head)| *id != actor.id && meet(*head, area));
                    if clear {
                        found = Some((area, text));
                        break 'search;
                    }
                }
            }
        }
        if let Some((area, text)) = found.or(first.filter(|_| must)) {
            placed.push(Plate {
                id: actor.id,
                area,
                text,
                selected: actor.selected,
            });
        }
    }
    placed
}

/// How far to the side the bubble of each lion is drawn so that it covers
/// no other lion's head: by its lion's id, in pixels. A bubble stays over
/// its own lion: it moves at most half a tile.
pub fn bubble_shifts(frame: &Frame) -> Vec<(u64, i32)> {
    let heads: Vec<(u64, Area)> = frame
        .actors
        .iter()
        .filter(|actor| !matches!(actor.look.act, Act::Egg | Act::Hatch))
        .map(|actor| (actor.id, head(actor)))
        .collect();
    let mut shifts = Vec::new();
    for actor in &frame.actors {
        let stamps = bubble(&actor.look);
        if stamps.is_empty() {
            continue;
        }
        let covers = |dx: i32| {
            stamps.iter().any(|stamp| {
                let area = (
                    actor.x + stamp.x + dx,
                    actor.y + stamp.y,
                    stamp.sprite.w,
                    stamp.sprite.h,
                );
                heads
                    .iter()
                    .any(|(id, head)| *id != actor.id && meet(*head, area))
            })
        };
        let shift = [0, 5, -5, 8, -8].into_iter().find(|dx| !covers(*dx));
        if let Some(shift) = shift.filter(|shift| *shift != 0) {
            shifts.push((actor.id, shift));
        }
    }
    shifts
}

fn draw_actor(out: &mut Bitmap, actor: &Actor, wardrobe: &mut Wardrobe) {
    let drawn = cel(&actor.look);
    let sheet = wardrobe.sheet(drawn.sheet, actor.seed, actor.tint);
    let (w, h) = match drawn.sheet {
        Sheet::Lion => (FRAME_W, FRAME_H),
        Sheet::Cub | Sheet::Egg => (CUB_FRAME, CUB_FRAME),
    };
    let (sx, sy) = (drawn.frame * w, drawn.row * h);
    let (x, y) = (actor.x + drawn.dx, actor.y + drawn.dy);
    if drawn.lying {
        out.draw(&sheet.part(sx, sy, w, h).turned(), x, y);
    } else {
        out.draw_part(sheet, sx, sy, w, h, x, y, drawn.mirrored);
    }
}

/// The picture of a piece of furniture at this moment: a computer is on
/// while a lion works at the seat that faces it, the rack runs while a
/// command does.
fn sprite_now(item: &Item, den: &Den, frame: &Frame, still: bool) -> &'static str {
    let tick = if still { 0 } else { frame.tick };
    match (item.role, item.sprite) {
        (Some(Role::Computer), "PC_FRONT_OFF") => {
            let user = frame.actors.iter().find(|actor| {
                actor.look.seat && den.world().computer_of(actor.tile) == Some(item.index)
            });
            match user.map(|actor| actor.look.act) {
                Some(Act::Type) => match (tick / 2) % 3 {
                    0 => "PC_FRONT_ON_1",
                    1 => "PC_FRONT_ON_2",
                    _ => "PC_FRONT_ON_3",
                },
                Some(Act::Think | Act::Wonder) => "PC_FRONT_ON_1",
                _ => item.sprite,
            }
        }
        (Some(Role::Rack), _) => {
            let running = frame
                .actors
                .iter()
                .any(|actor| actor.look.act == Act::Operate);
            match (running, (tick / 3 + item.index as u64) % 2) {
                (false, _) => item.sprite,
                (true, 0) => "RACK_BUSY_1",
                (true, _) => "RACK_BUSY_2",
            }
        }
        _ => item.sprite,
    }
}

/// Whether the picture of a frame depends on its tick beyond what its
/// actors say: a screen that scrolls, a rack that blinks, paper that flies.
/// When it does not, two frames with the same actors are the same picture.
pub fn animated(frame: &Frame) -> bool {
    frame
        .actors
        .iter()
        .any(|actor| matches!(actor.look.act, Act::Type | Act::Operate | Act::Rummage))
}

/// A translucent colour of the palette, for the marks of the editor.
fn wash(color: Hsla, alpha: u8) -> Rgba {
    let [r, g, b, _] = bytes(color);
    [r, g, b, alpha]
}

/// Draws what the editor shows over the room: the footprint of the selected
/// piece, the piece in hand where it would go, green where it may and red
/// where it may not.
fn draw_marks(out: &mut Bitmap, marks: &Marks, palette: &DenPalette) {
    let art = art();
    for tile in &marks.selected {
        out.fill(
            tile.x * TILE,
            tile.y * TILE,
            TILE,
            TILE,
            wash(palette.accent, 70),
        );
    }
    if let Some(tile) = marks.cursor {
        out.fill(
            tile.x * TILE,
            tile.y * TILE,
            TILE,
            TILE,
            wash(palette.accent, 40),
        );
    }
    if let Some((tile, fits)) = marks.tile {
        let color = if fits { palette.success } else { palette.error };
        out.fill(tile.x * TILE, tile.y * TILE, TILE, TILE, wash(color, 110));
    }
    let Some((placed, fits)) = &marks.ghost else {
        return;
    };
    let Some((_, view)) = placed.piece() else {
        return;
    };
    let color = if *fits {
        palette.success
    } else {
        palette.error
    };
    for tile in placed.footprint() {
        out.fill(tile.x * TILE, tile.y * TILE, TILE, TILE, wash(color, 80));
    }
    let picture = art
        .sprite(view.sprite)
        .mapped(|[r, g, b, a]| [r, g, b, a / 4 * 3]);
    out.draw_part(
        &picture,
        0,
        0,
        picture.w,
        picture.h,
        placed.x * TILE,
        (placed.y + view.h) * TILE - picture.h,
        view.mirrored,
    );
}

/// The den at a moment, as one picture: `den.world().cols` by `rows` tiles
/// of 16 pixels. With `marks`, the editor's marks are drawn over it.
pub fn compose(
    den: &Den,
    frame: &Frame,
    palette: &DenPalette,
    wardrobe: &mut Wardrobe,
    marks: Option<&Marks>,
) -> Bitmap {
    let world = den.world();
    let art = art();
    let still = den.reduced_motion();
    let tick = if still { 0 } else { frame.tick };
    let mut out = world.backdrop().clone();

    // The furniture and the lions, the farthest first.
    enum Thing<'a> {
        Item(&'a Item),
        Actor(&'a Actor),
    }
    let mut things: Vec<(i32, u8, Thing)> = world
        .items()
        .iter()
        .map(|item| (item.z, 0, Thing::Item(item)))
        .collect();
    for actor in &frame.actors {
        // Between its seat and what stands in front of it.
        things.push((2 * (actor.y + TILE) + 1, 1, Thing::Actor(actor)));
    }
    things.sort_by_key(|(z, kind, _)| (*z, *kind));
    for (_, _, thing) in things {
        match thing {
            Thing::Item(item) => {
                let picture = art.sprite(sprite_now(item, den, frame, still));
                out.draw_part(
                    picture,
                    0,
                    0,
                    picture.w,
                    picture.h,
                    item.x,
                    item.y,
                    item.mirrored,
                );
            }
            Thing::Actor(actor) => draw_actor(&mut out, actor, wardrobe),
        }
    }

    if !still {
        // Paper flies around a lion that empties the shelf.
        for actor in frame
            .actors
            .iter()
            .filter(|actor| actor.look.act == Act::Rummage)
        {
            for sheet in 0..3u64 {
                let age = (tick + sheet * 3) % 9;
                let life = (tick + sheet * 3) / 9;
                let roll = scramble(life * 17 + sheet + actor.id);
                let dx = (roll % 27) as i32 - 13;
                stamp(
                    &mut out,
                    Stamp::at(
                        &glyphs::PAPER,
                        actor.x + 6 + dx,
                        actor.y - 20 + age as i32 * 3,
                    ),
                    palette,
                );
            }
        }
    }

    if let Some(marks) = marks {
        draw_marks(&mut out, marks, palette);
    }

    // What floats over the room: the marks of the pointer and of the
    // selection, the names, the bubbles.
    for actor in &frame.actors {
        if !(actor.selected || actor.hovered) {
            continue;
        }
        let (left, top, w, h) = actor.bounds();
        let (corner, reach) = if actor.selected {
            (&*glyphs::CORNER, 3)
        } else {
            (&*glyphs::CORNER_HOVER, 1)
        };
        let (x0, y0) = (left - reach, top - reach);
        let (x1, y1) = (left + w + reach - corner.w, top + h + reach - corner.h);
        for (x, y, flip_h, flip_v) in [
            (x0, y0, false, false),
            (x1, y0, true, false),
            (x0, y1, false, true),
            (x1, y1, true, true),
        ] {
            stamp(
                &mut out,
                Stamp::at(corner, x, y).mirrored(flip_h).upside_down(flip_v),
                palette,
            );
        }
    }
    // The names: each on a plate where it covers nobody (see `plates`).
    for plate in plates(frame, out.w, out.h) {
        let (plate_color, ink) = if plate.selected {
            (bytes(palette.accent), bytes(palette.on_accent))
        } else {
            let [r, g, b, _] = bytes(palette.ink);
            ([r, g, b, 200], bytes(palette.bubble))
        };
        let (x, y, w, h) = plate.area;
        out.fill(x, y, w, h, plate_color);
        write(&mut out, x + 2, y + 1, &plate.text, ink);
    }
    let shifts = bubble_shifts(frame);
    for actor in &frame.actors {
        let shift = shifts
            .iter()
            .find(|(id, _)| *id == actor.id)
            .map_or(0, |(_, shift)| *shift);
        for mut glyph in bubble(&actor.look) {
            glyph.x += actor.x + shift;
            glyph.y += actor.y;
            stamp(&mut out, glyph, palette);
        }
    }
    out
}

/// A den with nobody in it, as one picture: what a host shows of a room to
/// choose it by.
pub fn thumbnail(layout: &crate::layout::DenLayout, palette: &DenPalette) -> Bitmap {
    let mut den = Den::new();
    den.set_layout(layout, false, std::time::Duration::ZERO);
    den.set_reduced_motion(true, std::time::Duration::ZERO);
    let frame = den.frame(std::time::Duration::ZERO);
    compose(&den, &frame, palette, &mut Wardrobe::new(), None)
}

/// A piece of the catalogue as it is drawn when it is turned so many times:
/// what a host shows of it in a list. `None` for an id that is no piece.
pub fn piece(id: &str, turn: u8) -> Option<Bitmap> {
    let view = crate::catalogue::find(id)?.view(turn);
    let picture = art().sprite(view.sprite);
    Some(if view.mirrored {
        picture.mirrored()
    } else {
        picture.clone()
    })
}

/// A tile of a floor, by its id: what a host shows of a carpet.
pub fn floor_tile(id: &str) -> Option<Bitmap> {
    Some(art().floors[crate::assets::floor(id)?].clone())
}
