//! The lions in 2.5D: a few boxes and a faceted mane, in every pose the
//! simulation knows.
//!
//! What an individual looks like follows from its seed ([`Traits::of`]):
//! its coat, the cut of its mane, the shade of it, its build, whether it
//! wears glasses, what else it wears, the tone of its shirt. The mane is the
//! colour of its agent, as in the pixel art, in one of the shades the pixel
//! lions have ([`crate::paint::SHADES`]), and a lioness, who has none, wears
//! a scarf in it. A den of one agent's sessions is still a den of
//! individuals, and most of what tells them apart shows from behind too:
//! the shade and the cut, the build, the cap, the headset, the shirt.
//!
//! What it does follows from its [`Look`] ([`Pose::of`]): every
//! [`Act`] has a pose of its own, counted on the look's beat, so a lion in
//! 2.5D moves at the pace of the pixel one. Only a walk is smoother: its
//! stride is given as a time.

use std::f32::consts::PI;

use crate::paint::{shaded, SHADES};
use crate::palette::scramble;
use crate::pose::{Act, Look};

use super::mesh::{hex, shade, Mesh, Part, Rgb};
use super::theme::Theme;

/// How much larger than its drawing a lion is in the room.
pub const SCALE: f32 = 0.92;
/// The same for a little one.
pub const LITTLE_SCALE: f32 = 0.6;

/// The coats: golden, sand, tawny, ash, dusky brown, white.
pub const COATS: [u32; 6] = [0xE2A658, 0xF2D6A2, 0xB98248, 0xCFC8BD, 0x8A5E3C, 0xF6EEDD];
const CREAM: u32 = 0xFCECCC;

/// The cut of a mane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// Full and combed.
    Mane,
    /// Full and rough.
    Wild,
    /// Cropped.
    Crop,
    /// Full, with a crest.
    Crest,
    /// None: a lioness, with a scarf in her agent's colour.
    Lioness,
}

impl Style {
    /// Every cut.
    pub const ALL: [Style; 5] = [
        Style::Mane,
        Style::Wild,
        Style::Crop,
        Style::Crest,
        Style::Lioness,
    ];
}

/// How a lion is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Build {
    /// Neither.
    Even,
    /// Narrow, and a little taller.
    Slim,
    /// Broad, and a little shorter.
    Broad,
}

impl Build {
    /// Every build.
    pub const ALL: [Build; 3] = [Build::Even, Build::Slim, Build::Broad];

    /// How much wider and how much taller than the drawing.
    fn stretch(self) -> (f32, f32) {
        match self {
            Build::Even => (1., 1.),
            Build::Slim => (0.88, 1.07),
            Build::Broad => (1.14, 0.94),
        }
    }
}

/// What a lion wears besides its shirt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wear {
    /// Nothing.
    Nothing,
    /// A cap, its peak forward.
    Cap,
    /// A headset: a band over the head and a cup on each ear.
    Headset,
    /// A bow on one ear.
    Bow,
    /// A tie.
    Tie,
}

impl Wear {
    /// Everything worn, nothing first and twice: half the lions wear
    /// nothing more.
    pub const ALL: [Wear; 6] = [
        Wear::Nothing,
        Wear::Cap,
        Wear::Nothing,
        Wear::Headset,
        Wear::Bow,
        Wear::Tie,
    ];
}

/// What makes a lion an individual.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Traits {
    /// Which of the [`COATS`].
    pub coat: usize,
    /// The cut of its mane.
    pub style: Style,
    /// It wears glasses.
    pub glasses: bool,
    /// Which of the theme's shirts.
    pub shirt: usize,
    /// Which shade of its agent's colour: an index into
    /// [`crate::paint::SHADES`].
    pub shade: usize,
    /// How it is built.
    pub build: Build,
    /// What else it wears.
    pub wear: Wear,
}

impl Traits {
    /// The traits of a seed: the same seed always gives the same lion, and
    /// each trait is read from bits of its own, so they vary apart.
    pub fn of(seed: u64) -> Traits {
        let mixed = scramble(seed);
        Traits {
            coat: (mixed % COATS.len() as u64) as usize,
            style: Style::ALL[((mixed >> 16) % Style::ALL.len() as u64) as usize],
            glasses: (mixed >> 32) % 4 == 0,
            shirt: ((mixed >> 40) % 4) as usize,
            shade: ((mixed >> 8) % SHADES.len() as u64) as usize,
            build: Build::ALL[((mixed >> 24) % Build::ALL.len() as u64) as usize],
            wear: Wear::ALL[((mixed >> 48) % Wear::ALL.len() as u64) as usize],
        }
    }
}

/// How the eyes are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eyes {
    /// Open.
    Open,
    /// Shut: asleep, or out cold.
    Shut,
    /// Wide: the stare.
    Wide,
}

/// How a lion holds itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// On a seat: its legs forward.
    pub seated: bool,
    /// On its back on the floor.
    pub lying: bool,
    /// How far each arm is raised forward, left and right: 0 hangs, a
    /// quarter turn points ahead, more points up.
    pub arms: (f32, f32),
    /// How far the body leans forward.
    pub lean: f32,
    /// How far the head nods down.
    pub nod: f32,
    /// How far the head leans to a side.
    pub tilt: f32,
    /// The eyes.
    pub eyes: Eyes,
    /// How high the whole lion is lifted: a bob.
    pub bob: f32,
    /// The stride of a walk, from -1 to 1.
    pub stride: f32,
    /// It holds a book.
    pub book: bool,
}

impl Pose {
    const STILL: Pose = Pose {
        seated: false,
        lying: false,
        arms: (0., 0.),
        lean: 0.,
        nod: 0.,
        tilt: 0.,
        eyes: Eyes::Open,
        bob: 0.,
        stride: 0.,
        book: false,
    };

    /// The pose of a look. `stride` is where a walk is in its step, as an
    /// angle; nothing else reads it.
    pub fn of(look: &Look, stride: f32) -> Pose {
        let odd = look.beat % 2 == 1;
        let side = if odd { 1. } else { -1. };
        let seated = look.seat && look.act.seated();
        let still = Pose {
            seated,
            ..Pose::STILL
        };
        match look.act {
            // An egg has no pose: it is drawn as an egg.
            Act::Egg | Act::Hatch | Act::Stand => still,
            Act::Walk => Pose {
                stride: stride.sin(),
                bob: stride.sin().abs() * 0.03,
                arms: (-stride.sin() * 0.5, stride.sin() * 0.5),
                ..still
            },
            Act::Type => Pose {
                arms: if odd { (1.25, 1.05) } else { (1.05, 1.25) },
                nod: if odd { 0.12 } else { 0.06 },
                ..still
            },
            Act::Think => Pose {
                arms: (0., 2.5),
                tilt: side * 0.1,
                nod: -0.05,
                ..still
            },
            Act::Wonder => Pose {
                tilt: side * 0.2,
                arms: (0.3, 0.3),
                ..still
            },
            Act::Browse => Pose {
                arms: (1.15, 1.15),
                nod: if odd { 0.34 } else { 0.28 },
                book: true,
                ..still
            },
            Act::Rummage => Pose {
                arms: if odd { (2.7, 1.6) } else { (1.6, 2.7) },
                nod: -0.22,
                ..still
            },
            Act::Operate => Pose {
                arms: (0., if odd { 1.55 } else { 1.25 }),
                nod: 0.08,
                ..still
            },
            Act::Peer => Pose {
                lean: 0.22,
                nod: -0.3,
                arms: (1.3, 1.3),
                bob: if odd { 0.01 } else { 0. },
                ..still
            },
            Act::Plan => Pose {
                arms: (0.2, if odd { 2.3 } else { 2.0 }),
                nod: -0.12,
                ..still
            },
            Act::Guard => Pose {
                nod: 0.36,
                tilt: side * 0.06,
                ..still
            },
            Act::Wait => Pose {
                bob: if odd { 0.02 } else { 0. },
                arms: if look.beat % 4 == 0 {
                    (2.8, 0.)
                } else {
                    (0.1, 0.1)
                },
                ..still
            },
            Act::Stare => Pose {
                eyes: Eyes::Wide,
                lean: 0.1,
                arms: (0.35, 0.35),
                bob: if odd { 0.012 } else { 0. },
                ..still
            },
            Act::Lounge => Pose {
                lean: -0.14,
                tilt: side * 0.04,
                ..still
            },
            Act::Sleep => Pose {
                nod: 0.5,
                eyes: Eyes::Shut,
                bob: if odd { 0.012 } else { 0. },
                ..still
            },
            Act::Faint => Pose {
                lying: true,
                eyes: Eyes::Shut,
                seated: false,
                arms: (0.5, 0.5),
                ..still
            },
        }
    }
}

/// How high a lion's bubble hangs over its feet, in the room.
pub fn height(pose: &Pose, seat: f32, scale: f32) -> f32 {
    if pose.lying {
        return 0.9 * scale;
    }
    let body = if pose.seated { seat / scale } else { 0.34 };
    (body + pose.bob + 1.36) * scale
}

/// How a lion fits the room: how high it sits when it does, and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The top of its seat.
    pub seat: f32,
    /// How much larger than its drawing it is: [`SCALE`], or
    /// [`LITTLE_SCALE`] for a little one.
    pub scale: f32,
}

/// Draws a lion at `feet`, a part whose origin is the floor under it and
/// whose front is the way it looks, at the size of the room.
pub fn lion(
    mesh: &mut Mesh,
    t: &Theme,
    feet: Part,
    traits: &Traits,
    mane: Rgb,
    pose: &Pose,
    fit: Fit,
) {
    let Fit { seat, scale } = fit;
    let mane = shade_of(mane, traits.shade);
    let (wide, tall) = traits.build.stretch();
    let coat = hex(COATS[traits.coat % COATS.len()]);
    let dark = shade(coat, 0.74);
    let cream = hex(CREAM);
    let shirt = t.shirts[traits.shirt % t.shirts.len()];
    let feet = feet.scaled(scale);
    let hip = if pose.seated { seat / scale } else { 0.34 } + pose.bob;
    let body = if pose.lying {
        // On its back: its feet where it looked, its face to the ceiling.
        feet.moved([0., 0.42, 0.45]).pitched(-PI / 2.)
    } else {
        feet.moved([0., hip, 0.])
    };
    if pose.seated {
        for x in [-0.13, 0.13] {
            mesh.bit(body, [0.17, 0.15, 0.4], dark, [x, 0., 0.2]);
            mesh.bit(body, [0.15, 0.36, 0.15], dark, [x, -0.36, 0.34]);
        }
    } else {
        let swing = pose.stride * 0.1;
        mesh.bit(body, [0.17, 0.34, 0.2], dark, [-0.13, -0.34, swing]);
        mesh.bit(body, [0.17, 0.34, 0.2], dark, [0.13, -0.34, -swing]);
    }
    // The tail and its tuft hang from the hips, whatever the back does. A
    // lion on its back lies on them.
    let lioness = traits.style == Style::Lioness;
    if !pose.lying {
        mesh.bit(body, [0.07, 0.07, 0.3], coat, [0., 0.06, -0.3]);
        mesh.bit(
            body,
            [0.14, 0.14, 0.14],
            if lioness { dark } else { mane },
            [0., 0.04, -0.5],
        );
    }

    let chest = body.moved([0., 0.1, 0.]).pitched(pose.lean);
    let high = 0.46 * tall;
    mesh.bit(chest, [0.5 * wide, high, 0.32], shirt, [0., 0., 0.]);
    mesh.bit(
        chest,
        [0.2 * wide, 0.36 * tall, 0.02],
        cream,
        [0., 0.06, 0.165],
    );
    if traits.wear == Wear::Tie {
        mesh.bit(chest, [0.07, 0.3 * tall, 0.02], mane, [0., 0.1, 0.176]);
        mesh.bit(chest, [0.1, 0.06, 0.025], mane, [0., 0.38 * tall, 0.176]);
    }
    // A yoke across the shoulders: what shows of the shirt from behind.
    mesh.bit(
        chest,
        [0.56 * wide, 0.1, 0.1],
        shade(shirt, 0.82),
        [0., high - 0.08, -0.14],
    );
    for (x, raised) in [(-0.315 * wide, pose.arms.0), (0.315 * wide, pose.arms.1)] {
        let arm = chest.moved([x, high - 0.04, 0.]).pitched(-raised);
        mesh.bit(arm, [0.13, 0.32, 0.15], shirt, [0., -0.32, 0.]);
        mesh.bit(arm, [0.12, 0.1, 0.12], coat, [0., -0.42, 0.]);
    }
    if pose.book {
        mesh.bit(chest, [0.4, 0.04, 0.26], t.paper, [0., 0.12, 0.42]);
        mesh.bit(chest, [0.02, 0.05, 0.26], t.muted, [0., 0.12, 0.42]);
    }

    let head = chest
        .moved([0., high + 0.02, 0.])
        .pitched(pose.nod)
        .rolled(pose.tilt);
    if lioness {
        // No mane: a scarf in the colour of its agent, one end down.
        mesh.bit(head, [0.56, 0.1, 0.36], mane, [0., -0.04, 0.]);
        mesh.bit(head, [0.12, 0.2, 0.05], mane, [0.16, -0.2, 0.17]);
        mesh.bit(head, [0.12, 0.16, 0.05], mane, [-0.1, -0.18, -0.17]);
    } else {
        let radius = if traits.style == Style::Crop {
            0.34
        } else {
            0.41
        };
        mesh.ball(
            head,
            [radius, radius * 0.98, radius * 0.76],
            mane,
            [0., 0.25, -0.06],
            traits.style != Style::Wild,
        );
        // From behind a mane is not a ball: it falls in locks over the
        // neck, a tone darker, and parts over the shoulders.
        let lock = shade(mane, 0.84);
        for (x, y, r) in [(-0.17, 0.04, 0.2), (0.17, 0.04, 0.2), (0., -0.06, 0.17)] {
            mesh.ball(head, [r, r * 1.1, r * 0.7], lock, [x, y, -0.26], false);
        }
        if traits.style == Style::Crest {
            for step in 0..4 {
                let step = step as f32;
                mesh.bit(
                    head,
                    [0.1, 0.16 - step * 0.02, 0.12],
                    mane,
                    [0., 0.62, 0.14 - step * 0.13],
                );
            }
        }
    }
    mesh.bit(head, [0.5, 0.44, 0.42], coat, [0., 0.04, 0.02]);
    mesh.bit(head, [0.26, 0.17, 0.12], cream, [0., 0.05, 0.27]);
    mesh.bit(head, [0.09, 0.05, 0.03], t.ink, [0., 0.17, 0.33]);
    for x in [-0.14, 0.14] {
        match pose.eyes {
            Eyes::Open => mesh.bit(head, [0.07, 0.07, 0.02], t.ink, [x, 0.28, 0.232]),
            Eyes::Shut => mesh.bit(head, [0.08, 0.02, 0.02], t.ink, [x, 0.3, 0.232]),
            Eyes::Wide => {
                mesh.bit(head, [0.12, 0.12, 0.02], hex(0xFFFFFF), [x, 0.25, 0.232]);
                mesh.bit(head, [0.05, 0.05, 0.02], t.ink, [x, 0.285, 0.24]);
            }
        }
        if traits.glasses {
            mesh.bit(head, [0.17, 0.14, 0.015], t.ink, [x, 0.245, 0.236]);
            mesh.bit(head, [0.11, 0.09, 0.02], hex(0xE8F4F5), [x, 0.27, 0.24]);
        }
    }
    if traits.glasses {
        // The arms of the glasses show from behind, where the face does not.
        for x in [-0.255, 0.255] {
            mesh.bit(head, [0.015, 0.03, 0.3], t.ink, [x, 0.3, 0.08]);
        }
    }
    // The ears stand clear of the mane, so that a lion seen from behind is
    // still a lion.
    let ear = if lioness { 0.48 } else { 0.6 };
    for x in [-0.23, 0.23] {
        mesh.bit(head, [0.17, 0.17, 0.1], coat, [x, ear, -0.02]);
        mesh.bit(
            head,
            [0.09, 0.09, 0.02],
            shade(coat, 0.6),
            [x, ear + 0.03, 0.04],
        );
    }
    let crown = if lioness { 0.48 } else { 0.62 };
    match traits.wear {
        Wear::Cap => {
            // Between the ears, in the tone of the shirt, its peak forward.
            let cap = shade(shirt, 0.9);
            mesh.bit(head, [0.3, 0.12, 0.4], cap, [0., crown, 0.]);
            mesh.bit(head, [0.3, 0.03, 0.16], shade(cap, 0.8), [0., crown, 0.26]);
        }
        Wear::Headset => {
            mesh.bit(head, [0.62, 0.05, 0.07], t.ink, [0., crown + 0.02, 0.]);
            for x in [-0.3, 0.3] {
                mesh.bit(head, [0.06, 0.34, 0.07], t.ink, [x, crown - 0.3, 0.]);
                mesh.bit(head, [0.08, 0.18, 0.18], t.muted, [x, 0.18, 0.02]);
            }
            mesh.bit(head, [0.03, 0.03, 0.26], t.ink, [0.3, 0.14, 0.2]);
        }
        Wear::Bow => {
            let bow = if lioness { mane } else { hex(CREAM) };
            for x in [0.15, 0.31] {
                mesh.bit(head, [0.12, 0.14, 0.08], bow, [x, ear + 0.12, 0.]);
            }
            mesh.bit(
                head,
                [0.06, 0.08, 0.1],
                shade(bow, 0.75),
                [0.23, ear + 0.15, 0.],
            );
        }
        Wear::Nothing | Wear::Tie => {}
    }
}

/// The colour of a mane in one of the shades of its agent's colour: what a
/// lion of that shade shows of its agent.
pub fn shade_of(mane: Rgb, shade: usize) -> Rgb {
    let bytes = mane.map(|part| (part.clamp(0., 1.) * 255.).round() as u8);
    shaded(bytes, shade).map(|part| f32::from(part) / 255.)
}

/// Draws an egg at `feet`: whole and wobbling on its beat, or, with
/// `hatching`, open, the little one's head out of the shell.
pub fn egg(mesh: &mut Mesh, t: &Theme, feet: Part, mane: Rgb, beat: u64, hatching: bool) {
    let shell = hex(0xF4EEDF);
    if hatching {
        mesh.ball(feet, [0.27, 0.18, 0.27], shell, [0., 0.2, 0.], false);
        let rise = (beat.min(6) as f32) * 0.03;
        let head = feet.moved([0., 0.24 + rise, 0.]).scaled(0.62);
        let coat = hex(COATS[0]);
        mesh.bit(head, [0.5, 0.44, 0.42], coat, [0., 0., 0.]);
        mesh.bit(head, [0.26, 0.17, 0.12], hex(CREAM), [0., 0.02, 0.25]);
        for x in [-0.14, 0.14] {
            mesh.bit(head, [0.07, 0.07, 0.02], t.ink, [x, 0.24, 0.212]);
            mesh.bit(head, [0.15, 0.14, 0.1], coat, [x * 1.5, 0.44, 0.]);
        }
        mesh.bit(head, [0.3, 0.08, 0.3], mane, [0., 0.44, -0.04]);
        return;
    }
    let lean = [0., -0.1, 0., 0.1][(beat % 4) as usize];
    let egg = feet.rolled(lean);
    mesh.ball(egg, [0.27, 0.36, 0.27], shell, [0., 0.38, 0.], true);
    // Speckled in the colour of the mane to be.
    for (x, y, z) in [(0.1, 0.5, 0.24), (-0.12, 0.32, 0.23), (0.02, 0.66, 0.16)] {
        mesh.bit(egg, [0.06, 0.06, 0.03], mane, [x, y, z]);
    }
    if beat % 8 >= 6 {
        mesh.bit(egg, [0.2, 0.02, 0.03], t.ink, [0., 0.44, 0.262]);
    }
}
