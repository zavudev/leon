//! The colours of the room in 2.5D, all of them from the host's theme.
//!
//! The pixel-art room has colours of its own, in its pictures. This one has
//! none: a [`Theme`] is made from the same [`Tokens`] as the chrome
//! ([`Theme::from_tokens`]), so the room is dark in a dark theme and light
//! in a light one, its planes are the theme's surfaces and its hairlines
//! the theme's rules. The accent only signals: the rug of the entrance, who
//! waits for the user, the selection.
//!
//! A layout still chooses a floor, walls and carpets. Here they are tones
//! of the same surfaces ([`Theme::floor_of`], [`Theme::wall_of`]) and the
//! way the floor is ruled ([`Ruling`]): planks, tiles, slabs, a checker.

use crate::assets::{FLOORS, WALLS};
use crate::palette::{bytes, luminance, Tokens};

use super::mesh::{blend, Rgb};

/// The colours of the room.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// A dark theme: light things on dark planes.
    pub dark: bool,
    /// What is behind the room: the page.
    pub background: Rgb,
    /// The floor.
    pub floor: Rgb,
    /// The walls.
    pub wall: Rgb,
    /// The top of a piece of furniture.
    pub piece: Rgb,
    /// Its sides, its legs: a step lower.
    pub piece_low: Rgb,
    /// Frames, stands, handles.
    pub metal: Rgb,
    /// The hairline on an edge.
    pub edge: Rgb,
    /// The ruling of the floor.
    pub grid: Rgb,
    /// Text: the lines on a board, a white sheet.
    pub text: Rgb,
    /// Secondary text.
    pub muted: Rgb,
    /// The faintest text.
    pub faint: Rgb,
    /// The accent fill: the rug, the selection.
    pub accent: Rgb,
    /// What is drawn on the accent.
    pub on_accent: Rgb,
    /// A screen at work, a light that is on.
    pub success: Rgb,
    /// A screen that asks.
    pub warning: Rgb,
    /// A screen of a session that failed.
    pub error: Rgb,
    /// A screen of a session that thinks; the sky in a window.
    pub info: Rgb,
    /// Leaves.
    pub plant: Rgb,
    /// A sheet of paper, the face of a board.
    pub paper: Rgb,
    /// The black of a screen that is off, of an eye.
    pub ink: Rgb,
    /// The clothes: four quiet tones.
    pub shirts: [Rgb; 4],
    /// The light on a face in shade, the sun's part, how dark a shadow is.
    pub light: [f32; 3],
}

/// How a floor is ruled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ruling {
    /// Not at all: a carpet.
    Plain,
    /// A line every tile, both ways.
    Tiles,
    /// A line every other tile, both ways.
    Slabs,
    /// A line every tile along the room, and joints that alternate.
    Planks,
    /// Every other tile a tone lower.
    Checker,
}

fn rgb(color: gpui_kit::Hsla) -> Rgb {
    let [r, g, b, _] = bytes(color);
    [
        f32::from(r) / 255.,
        f32::from(g) / 255.,
        f32::from(b) / 255.,
    ]
}

impl Theme {
    /// The room of a theme. Works for a dark and for a light one: which it
    /// is is read from the tokens themselves.
    pub fn from_tokens(t: &Tokens) -> Theme {
        let dark = luminance(t.background) < luminance(t.text);
        let (background, surface, surface_2) =
            (rgb(t.background), rgb(t.surface), rgb(t.surface_2));
        let (border, guide, text) = (rgb(t.border), rgb(t.guide), rgb(t.text));
        let (muted, faint, success) = (rgb(t.text_muted), rgb(t.text_faint), rgb(t.success));
        Theme {
            dark,
            background,
            floor: surface,
            wall: surface_2,
            piece: if dark {
                blend(border, text, 0.05)
            } else {
                border
            },
            piece_low: if dark {
                blend(surface_2, border, 0.8)
            } else {
                surface_2
            },
            metal: if dark {
                blend(border, text, 0.13)
            } else {
                guide
            },
            edge: blend(border, text, if dark { 0.25 } else { 0.3 }),
            grid: border,
            text,
            muted,
            faint,
            accent: rgb(t.accent_fill),
            on_accent: rgb(t.on_accent_fill),
            success,
            warning: rgb(t.warning),
            error: rgb(t.error),
            info: rgb(t.info),
            plant: if dark {
                blend(success, background, 0.58)
            } else {
                blend(success, surface, 0.36)
            },
            paper: if dark {
                blend(surface, text, 0.5)
            } else {
                surface
            },
            ink: if dark { background } else { text },
            shirts: if dark {
                [
                    blend(border, text, 0.3),
                    blend(border, text, 0.12),
                    muted,
                    text,
                ]
            } else {
                [muted, blend(muted, surface, 0.5), text, guide]
            },
            light: if dark {
                [0.62, 0.48, 0.5]
            } else {
                [0.84, 0.2, 0.3]
            },
        }
    }

    /// The same room washed in a colour: what a piece that is not there
    /// yet is drawn in, green where it may go and red where it may not.
    pub fn washed(&self, with: Rgb, by: f32) -> Theme {
        let wash = |color: Rgb| blend(color, with, by);
        Theme {
            wall: wash(self.wall),
            piece: wash(self.piece),
            piece_low: wash(self.piece_low),
            metal: wash(self.metal),
            edge: with,
            text: wash(self.text),
            muted: wash(self.muted),
            faint: wash(self.faint),
            accent: wash(self.accent),
            on_accent: wash(self.on_accent),
            success: wash(self.success),
            info: wash(self.info),
            plant: wash(self.plant),
            paper: wash(self.paper),
            ink: wash(self.ink),
            ..*self
        }
    }

    /// The tone of a floor of [`FLOORS`]: the room's own floor is the
    /// surface, and every other style a step of its own away from it, so
    /// that a carpet reads as a zone of the room.
    pub fn floor_of(&self, style: usize, room: usize) -> Rgb {
        if style == room {
            return self.floor;
        }
        // Steps that no two styles share, a little stronger in the dark.
        let step = 1 + style % 4;
        let by = if self.dark { 0.035 } else { 0.028 } * step as f32 + 0.02;
        blend(self.floor, self.text, by)
    }

    /// The tone of the walls of [`WALLS`]: the surface, a step away for
    /// every style after the first.
    pub fn wall_of(&self, style: usize) -> Rgb {
        let step = (style % WALLS.len().max(1)) as f32;
        blend(
            self.wall,
            self.text,
            step * if self.dark { 0.012 } else { 0.01 },
        )
    }

    /// How a floor of [`FLOORS`] is ruled.
    pub fn ruling(style: usize) -> Ruling {
        match FLOORS.get(style).map(|floor| floor.tile) {
            Some(1 | 2) => Ruling::Slabs,
            Some(3 | 4) => Ruling::Tiles,
            Some(5 | 6) => Ruling::Planks,
            Some(7 | 8) => Ruling::Checker,
            _ => Ruling::Plain,
        }
    }
}
