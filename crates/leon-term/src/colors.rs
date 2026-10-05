//! Colour resolution: from what a program asked for to a colour on screen.
//!
//! A program names colours three ways: one of the 16 ANSI colours, an index
//! into the 256-colour palette, or an RGB triple. The application supplies
//! the 16 ANSI colours, the foreground, the background, the cursor and the
//! selection in a [`TerminalTheme`]; indices 16 to 255 are the standard
//! xterm cube and grey ramp, computed here; truecolor passes through. A
//! program may also redefine colours at run time (OSC 4, 10, 11); those
//! overrides, kept by the emulator, win over the theme.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::{Hsla, Rgba};

/// The colours of a terminal, supplied by the application.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalTheme {
    /// Text with no colour of its own.
    pub foreground: Hsla,
    /// The page. Cells of this colour are not painted at all.
    pub background: Hsla,
    /// The cursor.
    pub cursor: Hsla,
    /// The background of selected cells (opaque: text keeps its colour).
    pub selection: Hsla,
    /// The background of a match of the find bar (opaque: text keeps its
    /// colour).
    pub find_match: Hsla,
    /// The background of the match the find bar is on; the text on it is drawn
    /// in the page's colour.
    pub find_match_current: Hsla,
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan,
    /// white, then the same eight bright.
    pub ansi: [Hsla; 16],
}

/// The six levels of one channel of the xterm colour cube.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// How far a dim cell's text moves towards the background.
const DIM_MIX: f32 = 0.4;

/// The RGB of xterm palette index `index` for the indices the theme does not
/// define (16 to 255); `None` for 0 to 15.
pub fn xterm_rgb(index: u8) -> Option<(u8, u8, u8)> {
    match index {
        0..=15 => None,
        16..=231 => {
            let i = usize::from(index - 16);
            Some((
                CUBE_LEVELS[i / 36],
                CUBE_LEVELS[(i / 6) % 6],
                CUBE_LEVELS[i % 6],
            ))
        }
        232..=255 => {
            let level = 8 + 10 * (index - 232);
            Some((level, level, level))
        }
    }
}

/// An opaque colour from 8-bit channels.
pub fn from_rgb8(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
    .into()
}

/// A colour as 8-bit channels (alpha is ignored).
pub fn to_rgb8(colour: Hsla) -> (u8, u8, u8) {
    let rgba = Rgba::from(colour);
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    (channel(rgba.r), channel(rgba.g), channel(rgba.b))
}

/// `a` moved `amount` (0 to 1) of the way towards `b`, in RGB.
pub fn mix(a: Hsla, b: Hsla, amount: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let lerp = |x: f32, y: f32| x + (y - x) * amount;
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: 1.0,
    }
    .into()
}

impl TerminalTheme {
    /// The colour of palette index `index`: the theme's own for 0 to 15, the
    /// xterm cube and ramp above.
    pub fn indexed(&self, index: u8) -> Hsla {
        match xterm_rgb(index) {
            Some((r, g, b)) => from_rgb8(r, g, b),
            None => self.ansi[usize::from(index)],
        }
    }

    /// The colour a program asked for. `overrides` are the colours the
    /// program redefined; they win over the theme.
    pub fn resolve(&self, colour: Color, overrides: &Colors) -> Hsla {
        match colour {
            Color::Spec(Rgb { r, g, b }) => from_rgb8(r, g, b),
            Color::Indexed(index) => match overrides[usize::from(index)] {
                Some(Rgb { r, g, b }) => from_rgb8(r, g, b),
                None => self.indexed(index),
            },
            Color::Named(named) => {
                if let Some(Rgb { r, g, b }) = overrides[named] {
                    return from_rgb8(r, g, b);
                }
                self.named(named)
            }
        }
    }

    fn named(&self, named: NamedColor) -> Hsla {
        use NamedColor as N;
        let ansi = |i: usize| self.ansi[i];
        match named {
            N::Black => ansi(0),
            N::Red => ansi(1),
            N::Green => ansi(2),
            N::Yellow => ansi(3),
            N::Blue => ansi(4),
            N::Magenta => ansi(5),
            N::Cyan => ansi(6),
            N::White => ansi(7),
            N::BrightBlack => ansi(8),
            N::BrightRed => ansi(9),
            N::BrightGreen => ansi(10),
            N::BrightYellow => ansi(11),
            N::BrightBlue => ansi(12),
            N::BrightMagenta => ansi(13),
            N::BrightCyan => ansi(14),
            N::BrightWhite => ansi(15),
            N::Foreground | N::BrightForeground => self.foreground,
            N::Background => self.background,
            N::Cursor => self.cursor,
            N::DimForeground => self.dim(self.foreground),
            N::DimBlack => self.dim(ansi(0)),
            N::DimRed => self.dim(ansi(1)),
            N::DimGreen => self.dim(ansi(2)),
            N::DimYellow => self.dim(ansi(3)),
            N::DimBlue => self.dim(ansi(4)),
            N::DimMagenta => self.dim(ansi(5)),
            N::DimCyan => self.dim(ansi(6)),
            N::DimWhite => self.dim(ansi(7)),
        }
    }

    /// `colour` as dim text: moved towards the background.
    pub fn dim(&self, colour: Hsla) -> Hsla {
        mix(colour, self.background, DIM_MIX)
    }

    /// The colour for the emulator's colour index space: 0 to 15 ANSI, 16 to
    /// 255 xterm, 256 foreground, 257 background, 258 cursor. Used to answer
    /// a program that asks what a colour is.
    pub fn by_slot(&self, slot: usize) -> Hsla {
        match slot {
            0..=255 => self.indexed(slot as u8),
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            _ => self.foreground,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::hsla;

    /// A theme whose 16 colours are distinguishable by their red channel.
    fn theme() -> TerminalTheme {
        let ansi = std::array::from_fn(|i| from_rgb8(i as u8 * 10, 0, 0));
        TerminalTheme {
            foreground: from_rgb8(200, 200, 200),
            background: from_rgb8(0, 0, 0),
            cursor: from_rgb8(1, 2, 3),
            selection: from_rgb8(30, 30, 90),
            find_match: from_rgb8(60, 60, 20),
            find_match_current: from_rgb8(250, 200, 0),
            ansi,
        }
    }

    #[test]
    fn the_cube_starts_at_black_and_ends_at_white() {
        assert_eq!(xterm_rgb(16), Some((0, 0, 0)));
        assert_eq!(xterm_rgb(231), Some((255, 255, 255)));
    }

    #[test]
    fn the_cube_uses_the_xterm_levels() {
        // 16 + 36*r + 6*g + b
        assert_eq!(xterm_rgb(16 + 36 + 6 * 2 + 3), Some((95, 135, 175)));
        assert_eq!(xterm_rgb(16 + 36 * 5), Some((255, 0, 0)));
    }

    #[test]
    fn the_grey_ramp_runs_from_8_to_238() {
        assert_eq!(xterm_rgb(232), Some((8, 8, 8)));
        assert_eq!(xterm_rgb(255), Some((238, 238, 238)));
    }

    #[test]
    fn the_theme_owns_the_first_sixteen_indices() {
        assert_eq!(xterm_rgb(0), None);
        assert_eq!(xterm_rgb(15), None);
        let t = theme();
        assert_eq!(t.indexed(3), t.ansi[3]);
        assert_eq!(t.indexed(15), t.ansi[15]);
    }

    #[test]
    fn named_colours_come_from_the_theme() {
        let t = theme();
        let none = Colors::default();
        assert_eq!(t.resolve(Color::Named(NamedColor::Red), &none), t.ansi[1]);
        assert_eq!(
            t.resolve(Color::Named(NamedColor::BrightCyan), &none),
            t.ansi[14]
        );
        assert_eq!(
            t.resolve(Color::Named(NamedColor::Foreground), &none),
            t.foreground
        );
        assert_eq!(
            t.resolve(Color::Named(NamedColor::Background), &none),
            t.background
        );
        assert_eq!(t.resolve(Color::Named(NamedColor::Cursor), &none), t.cursor);
    }

    #[test]
    fn truecolor_passes_through_exactly() {
        let t = theme();
        let got = t.resolve(
            Color::Spec(Rgb {
                r: 18,
                g: 52,
                b: 86,
            }),
            &Colors::default(),
        );
        assert_eq!(to_rgb8(got), (18, 52, 86));
    }

    #[test]
    fn indexed_colours_use_the_cube_beyond_the_sixteen() {
        let t = theme();
        let got = t.resolve(Color::Indexed(196), &Colors::default());
        assert_eq!(to_rgb8(got), (255, 0, 0));
    }

    #[test]
    fn a_colour_the_program_redefined_wins_over_the_theme() {
        let t = theme();
        let mut overrides = Colors::default();
        overrides[NamedColor::Red] = Some(Rgb { r: 9, g: 8, b: 7 });
        overrides[200usize] = Some(Rgb { r: 1, g: 1, b: 1 });
        assert_eq!(
            to_rgb8(t.resolve(Color::Named(NamedColor::Red), &overrides)),
            (9, 8, 7)
        );
        assert_eq!(
            to_rgb8(t.resolve(Color::Indexed(200), &overrides)),
            (1, 1, 1)
        );
    }

    #[test]
    fn dim_text_moves_towards_the_background() {
        let t = theme();
        let dimmed = t.dim(t.foreground);
        let (r, ..) = to_rgb8(dimmed);
        assert!(r < 200 && r > 0, "{r}");
        assert_eq!(
            to_rgb8(t.resolve(Color::Named(NamedColor::DimForeground), &Colors::default())).0,
            r
        );
    }

    #[test]
    fn colour_round_trips_through_eight_bit_channels() {
        for (r, g, b) in [(0, 0, 0), (255, 255, 255), (97, 95, 255), (3, 200, 41)] {
            assert_eq!(to_rgb8(from_rgb8(r, g, b)), (r, g, b));
        }
    }

    #[test]
    fn slots_follow_the_emulators_index_space() {
        let t = theme();
        assert_eq!(t.by_slot(2), t.ansi[2]);
        assert_eq!(t.by_slot(256), t.foreground);
        assert_eq!(t.by_slot(257), t.background);
        assert_eq!(t.by_slot(258), t.cursor);
        let _ = hsla(0., 0., 0., 1.);
    }
}
