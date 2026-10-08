//! From what a lion is doing to the frame of its sheet that shows it.
//!
//! A [`Look`] says what a lion does ([`Act`]), which way it faces, whether it
//! is a little one, whether it sits, and the beat of its animation. [`cel`]
//! answers with the frame to draw and where: which row and column of the
//! sheet, mirrored or lying down, and how far from the lion's tile. The
//! sheets are those of [`crate::atelier`]. Nothing here knows the time: the
//! beat is a number the simulation counts.

use crate::atelier::{CUB_FRAME, FRAME_ASLEEP, FRAME_H, FRAME_OUT, FRAME_STARE};
use crate::glyphs;
use crate::sprite::{Sprite, Stamp};

/// The side of a tile, in pixels of the art.
pub const TILE: i32 = 16;
/// How far down a lion is drawn when it sits: into its seat.
pub const SITTING: i32 = 6;

/// Which way a lion looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Facing {
    /// Towards the viewer.
    Down,
    /// Away from the viewer.
    Up,
    /// To the left.
    Left,
    /// To the right.
    Right,
}

/// What a lion is seen doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Act {
    /// Still in its egg, which wobbles.
    Egg,
    /// The egg cracks and the little one comes out.
    Hatch,
    /// Walking to its place.
    Walk,
    /// Standing.
    Stand,
    /// Typing at its computer: an edit. The picture of work.
    Type,
    /// At its desk, not typing: thinking.
    Think,
    /// At its desk, doing nobody knows what.
    Wonder,
    /// Reading what it took off the shelf.
    Browse,
    /// Emptying the shelf: searching.
    Rummage,
    /// Minding the rack while a command runs.
    Operate,
    /// At the telescope: the web.
    Peer,
    /// Staring at the board: a plan.
    Plan,
    /// Standing by the nest, watching its egg.
    Guard,
    /// Standing at the entrance, done, waiting for an order.
    Wait,
    /// Standing at the entrance and staring at the user: a permission prompt.
    Stare,
    /// On the sofa.
    Lounge,
    /// Asleep: on the sofa, or at its desk.
    Sleep,
    /// On the floor, out cold.
    Faint,
}

impl Act {
    /// How many ticks one beat of this act lasts; 0 for an act that does not
    /// move at all.
    pub fn beat_ticks(self) -> u64 {
        match self {
            Act::Walk | Act::Hatch => 1,
            Act::Rummage | Act::Egg => 2,
            Act::Type | Act::Operate | Act::Stare => 3,
            Act::Think | Act::Browse => 5,
            Act::Peer | Act::Guard | Act::Plan => 8,
            Act::Wait | Act::Wonder => 7,
            Act::Lounge | Act::Sleep => 10,
            Act::Stand | Act::Faint => 0,
        }
    }

    /// Whether a lion does this sitting, when it has a seat.
    pub fn seated(self) -> bool {
        matches!(
            self,
            Act::Type | Act::Think | Act::Wonder | Act::Sleep | Act::Lounge
        )
    }
}

/// What floats over a lion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bubble {
    /// `!`: waiting for an order.
    Bang,
    /// `!` in the warning colour: a permission prompt.
    Urgent,
    /// Dots in a bubble: thinking.
    Thought,
    /// `?`: nothing is known.
    Question,
    /// A cross in the error colour: fainted, or the command failed.
    Cross,
    /// A tick in the success colour: the command passed.
    Tick,
    /// `Zzz`.
    Zzz,
}

/// Everything that decides how a lion is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    /// What it does.
    pub act: Act,
    /// Which way it faces.
    pub facing: Facing,
    /// A little one: a cub.
    pub little: bool,
    /// It is on a seat.
    pub seat: bool,
    /// The frame of its animation: it goes up by one every
    /// [`Act::beat_ticks`].
    pub beat: u64,
    /// What floats over it.
    pub bubble: Option<Bubble>,
}

/// Which sheet a cel is cut from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sheet {
    /// A lion sheet: frames of 16 by 32.
    Lion,
    /// The cub sheet: frames of 16 by 16.
    Cub,
    /// The egg: frames of 16 by 16.
    Egg,
}

/// One frame of a sheet, and where it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cel {
    /// The sheet.
    pub sheet: Sheet,
    /// The row of the sheet: 0 facing down, 1 up, 2 right.
    pub row: i32,
    /// The frame in the row.
    pub frame: i32,
    /// Mirrored: a lion that looks left is one that looks right, mirrored.
    pub mirrored: bool,
    /// Turned a quarter: lying on the floor.
    pub lying: bool,
    /// From the left of the lion's tile to the left of the picture.
    pub dx: i32,
    /// From the top of the lion's tile to the top of the picture.
    pub dy: i32,
    /// How much farther down the room it is drawn than its tile says: a
    /// seated lion is in front of its seat.
    pub depth: i32,
}

fn cycle<T: Copy, const N: usize>(values: [T; N], beat: u64) -> T {
    values[(beat % N as u64) as usize]
}

/// The frame that shows a lion with this look.
pub fn cel(look: &Look) -> Cel {
    let beat = look.beat;
    let (row, mirrored) = match look.facing {
        Facing::Down => (0, false),
        Facing::Up => (1, false),
        Facing::Right => (2, false),
        Facing::Left => (2, true),
    };
    if matches!(look.act, Act::Egg | Act::Hatch) {
        let (frame, dx) = match look.act {
            Act::Hatch => (2, 0),
            // It wobbles, and cracks as its time comes.
            _ => (i32::from(beat % 8 >= 6), cycle([0, -1, 0, 1], beat)),
        };
        return Cel {
            sheet: Sheet::Egg,
            row: 0,
            frame,
            mirrored: false,
            lying: false,
            dx,
            dy: 0,
            depth: 2,
        };
    }
    let seated = look.seat && look.act.seated();
    let down = |frame: i32| (0, false, frame);
    if look.little {
        let sit = if seated { 3 } else { 0 };
        let (row, mirrored, frame) = match look.act {
            Act::Walk => (row, mirrored, cycle([1, 0, 2, 0], beat)),
            Act::Type if seated => (row, mirrored, cycle([3, 4], beat)),
            Act::Type | Act::Browse => down(cycle([3, 4], beat)),
            Act::Rummage | Act::Operate => (row, mirrored, cycle([0, 4], beat)),
            Act::Sleep if seated && row == 1 => (1, false, 4),
            Act::Sleep | Act::Lounge => down(5),
            Act::Stare => down(6),
            Act::Faint => down(7),
            Act::Wait | Act::Wonder | Act::Think if !seated => down(0),
            _ => (row, mirrored, 0),
        };
        return Cel {
            sheet: Sheet::Cub,
            row,
            frame,
            mirrored,
            lying: false,
            dx: 0,
            dy: sit + i32::from(look.act == Act::Faint) * 2,
            depth: sit,
        };
    }
    let sit = if seated { SITTING } else { 0 };
    let (row, mirrored, frame, bob) = match look.act {
        Act::Walk => (row, mirrored, cycle([0, 1, 2, 1], beat), 0),
        // At its computer, seen from behind; with no desk, on its feet with
        // a tablet.
        Act::Type if seated => (row, mirrored, cycle([3, 4], beat), 0),
        Act::Type | Act::Browse => (0, false, cycle([5, 6], beat), 0),
        Act::Think | Act::Wonder if seated => (row, mirrored, 3, 0),
        Act::Think | Act::Wonder | Act::Wait => (0, false, 1, 0),
        Act::Stare => (0, false, FRAME_STARE, 0),
        Act::Rummage => (1, false, cycle([0, 1, 2, 1], beat), 0),
        // A small hop when the machine spits a spark.
        Act::Operate => (1, false, 1, cycle([0, 0, -1, 0], beat)),
        Act::Peer | Act::Plan | Act::Guard | Act::Stand => (row, mirrored, 1, 0),
        Act::Lounge if row == 0 => (0, false, 3, 0),
        Act::Lounge => (row, mirrored, 3, -2),
        Act::Sleep if row == 0 || !seated => (0, false, FRAME_ASLEEP, (beat % 2) as i32),
        Act::Sleep => (row, mirrored, 3, 1 + (beat % 2) as i32),
        Act::Faint => (0, false, FRAME_OUT, 0),
        Act::Egg | Act::Hatch => (0, false, 0, 0),
    };
    if look.act == Act::Faint {
        return Cel {
            sheet: Sheet::Lion,
            row,
            frame,
            mirrored: false,
            lying: true,
            dx: -8,
            dy: 0,
            depth: 0,
        };
    }
    Cel {
        sheet: Sheet::Lion,
        row,
        frame,
        mirrored,
        lying: false,
        dx: 0,
        dy: TILE - FRAME_H + sit + bob,
        depth: sit,
    }
}

/// How far below the top of its tile the head of a lion with this look
/// starts: where its bubble sits, and the top of the box the pointer hits.
pub fn head_top(look: &Look) -> i32 {
    let drawn = cel(look);
    match drawn.sheet {
        Sheet::Egg => 3,
        Sheet::Cub => drawn.dy,
        Sheet::Lion if drawn.lying => 0,
        // The head starts three pixels into a standing frame, and eight
        // into one that is seated and seen from the front.
        Sheet::Lion => {
            let seated_front = drawn.row == 0 && (3..=7).contains(&drawn.frame);
            drawn.dy + if seated_front { 8 } else { 3 }
        }
    }
}

/// The box a lion with this look is drawn in, relative to its tile: left,
/// top, width, height.
pub fn bounds(look: &Look) -> (i32, i32, i32, i32) {
    let drawn = cel(look);
    match drawn.sheet {
        Sheet::Egg | Sheet::Cub => (0, drawn.dy, CUB_FRAME, CUB_FRAME),
        Sheet::Lion if drawn.lying => (drawn.dx, 0, FRAME_H, TILE),
        Sheet::Lion => {
            let top = head_top(look);
            (0, top, TILE, TILE + SITTING - top)
        }
    }
}

/// The stamps of the bubble of a lion with this look, relative to the top
/// left of its tile.
pub fn bubble(look: &Look) -> Vec<Stamp<'static>> {
    let Some(bubble) = look.bubble else {
        return Vec::new();
    };
    let (beat, top) = (look.beat, head_top(look));
    let over = |sprite: &'static Sprite, bob: i32| {
        Stamp::at(sprite, (TILE - sprite.w) / 2, top - sprite.h + bob)
    };
    match bubble {
        Bubble::Bang => vec![over(&glyphs::BANG, cycle([0, 0, -1, -1], beat))],
        Bubble::Urgent => vec![over(
            cycle([&*glyphs::BANG_URGENT, &*glyphs::BANG_URGENT_ALT], beat),
            cycle([0, -1], beat),
        )],
        Bubble::Question => vec![over(&glyphs::QUESTION, cycle([0, -1, 0, 0], beat))],
        Bubble::Cross => vec![over(&glyphs::CROSS, 0)],
        Bubble::Tick => vec![over(&glyphs::TICK, 0)],
        Bubble::Thought => {
            let base = over(&glyphs::THOUGHT, 0);
            let mut stamps = vec![base];
            for dot in 0..(beat % 4) as i32 {
                stamps.push(Stamp::at(&glyphs::DOT, base.x + 3 + dot * 2, base.y + 2));
            }
            stamps
        }
        Bubble::Zzz => {
            let (x, y) = (TILE - 4, top + 2);
            match beat % 4 {
                0 => vec![Stamp::at(&glyphs::ZED_SMALL, x, y - 3)],
                1 => vec![
                    Stamp::at(&glyphs::ZED_SMALL, x + 1, y - 5),
                    Stamp::at(&glyphs::ZED_SMALL, x - 3, y - 1),
                ],
                2 => vec![
                    Stamp::at(&glyphs::ZED, x + 2, y - 10),
                    Stamp::at(&glyphs::ZED_SMALL, x - 2, y - 3),
                ],
                _ => vec![
                    Stamp::at(&glyphs::ZED, x + 3, y - 12),
                    Stamp::at(&glyphs::ZED_SMALL, x - 1, y - 5),
                ],
            }
        }
    }
}
