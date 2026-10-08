//! The colours of the Den's chrome, all of them handed in by the host.
//!
//! The room itself has its own colours: they are in the pictures. What is
//! around and over it follows the host's theme: the frame, the narrator's
//! box, the roster, the truth card, the mark of the selection, the name tags
//! and the status bubbles. A [`DenPalette`] names those roles and
//! [`DenPalette::from_tokens`] is the recipe the Leon themes fill them with,
//! written once here so that the app and the demo cannot drift. This crate
//! holds no colour of a theme.
//!
//! A glyph is drawn in [`Role`]s, never in colours.

use gpui_kit::{Hsla, Rgba};

/// The theme colours a den is made of. The names are the ones of the Leon
/// theme (`brand/tokens.md`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// The page: the floor of the den.
    pub background: Hsla,
    /// A raised surface: cards, the narrator's box.
    pub surface: Hsla,
    /// A second raised step.
    pub surface_2: Hsla,
    /// The hairline rule.
    pub border: Hsla,
    /// The faintest line of the blueprint.
    pub guide: Hsla,
    /// Crosshairs and corner ticks.
    pub grid_mark: Hsla,
    /// Body text.
    pub text: Hsla,
    /// Secondary text.
    pub text_muted: Hsla,
    /// The faintest text.
    pub text_faint: Hsla,
    /// The accent as a line or a text colour on the page.
    pub signal: Hsla,
    /// The accent as a fill, the same in light and dark.
    pub accent_fill: Hsla,
    /// What is written on the accent fill.
    pub on_accent_fill: Hsla,
    /// The four status colours.
    pub success: Hsla,
    /// See `success`.
    pub warning: Hsla,
    /// See `success`.
    pub error: Hsla,
    /// See `success`.
    pub info: Hsla,
}

/// Every colour of the chrome, by role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DenPalette {
    /// What is behind and around the room: the page.
    pub ground: Hsla,
    /// The outline of a bubble, and the plate under a name.
    pub ink: Hsla,
    /// The fill of a bubble, and a name on its plate.
    pub bubble: Hsla,
    /// What is drawn inside a bubble.
    pub bubble_ink: Hsla,
    /// The fill of the narrator's box, the roster and the truth card.
    pub paper: Hsla,
    /// The row of the selected lion in the roster.
    pub raised: Hsla,
    /// The 1 px rule around them, and around the room.
    pub rule: Hsla,
    /// Their corner ticks.
    pub tick: Hsla,
    /// Text.
    pub text: Hsla,
    /// Secondary text.
    pub text_muted: Hsla,
    /// The accent as a line: the selection.
    pub signal: Hsla,
    /// The accent as a fill: the `!` of a lion that waits.
    pub accent: Hsla,
    /// What is drawn on the accent fill.
    pub on_accent: Hsla,
    /// What is drawn on a status fill.
    pub on_status: Hsla,
    /// A command passed.
    pub success: Hsla,
    /// A permission prompt.
    pub warning: Hsla,
    /// A lion fainted, a command failed.
    pub error: Hsla,
    /// Nothing is known: the `?` of a mystery session.
    pub info: Hsla,
}

impl DenPalette {
    /// The palette of a Leon theme. Works for a dark and for a light theme:
    /// which one it is is read from the tokens themselves.
    pub fn from_tokens(t: &Tokens) -> Self {
        let dark = luminance(t.background) < luminance(t.text);
        Self {
            ground: t.background,
            ink: if dark { t.background } else { t.text },
            bubble: if dark { t.text } else { t.surface },
            bubble_ink: if dark { t.background } else { t.text },
            paper: t.surface,
            raised: t.surface_2,
            rule: t.border,
            tick: t.grid_mark,
            text: t.text,
            text_muted: t.text_muted,
            signal: t.signal,
            accent: t.accent_fill,
            on_accent: t.on_accent_fill,
            on_status: t.background,
            success: t.success,
            warning: t.warning,
            error: t.error,
            info: t.info,
        }
    }

    /// The colour of a role.
    pub fn color(&self, role: Role) -> Hsla {
        match role {
            Role::Ink => self.ink,
            Role::Bubble => self.bubble,
            Role::BubbleInk => self.bubble_ink,
            Role::Accent => self.accent,
            Role::OnAccent => self.on_accent,
            Role::Signal => self.signal,
            Role::OnStatus => self.on_status,
            Role::Success => self.success,
            Role::Warning => self.warning,
            Role::Error => self.error,
            Role::Info => self.info,
            Role::Text => self.text,
            Role::Muted => self.text_muted,
        }
    }
}

/// What a pixel of a glyph is, before it has a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The outline (`o`).
    Ink,
    /// The fill of a bubble (`w`).
    Bubble,
    /// What is written in a bubble (`b`).
    BubbleInk,
    /// The accent fill (`y`).
    Accent,
    /// What is drawn on the accent (`Y`).
    OnAccent,
    /// The accent as a line (`S`).
    Signal,
    /// What is drawn on a status fill (`O`).
    OnStatus,
    /// Status colours (`c`, `a`, `r`, `i`).
    Success,
    /// See `Success`.
    Warning,
    /// See `Success`.
    Error,
    /// See `Success`.
    Info,
    /// Text colours (`x` and `u`).
    Text,
    /// See `Text`.
    Muted,
}

impl Role {
    /// The role a character of a glyph's drawing stands for. `.` and a
    /// space are no pixel at all.
    pub fn of(letter: char) -> Option<Role> {
        Some(match letter {
            'o' => Role::Ink,
            'w' => Role::Bubble,
            'b' => Role::BubbleInk,
            'y' => Role::Accent,
            'Y' => Role::OnAccent,
            'S' => Role::Signal,
            'O' => Role::OnStatus,
            'c' => Role::Success,
            'a' => Role::Warning,
            'r' => Role::Error,
            'i' => Role::Info,
            'x' => Role::Text,
            'u' => Role::Muted,
            _ => return None,
        })
    }
}

/// A colour as four bytes: red, green, blue, alpha.
pub fn bytes(color: Hsla) -> [u8; 4] {
    let rgba = Rgba::from(color);
    let byte = |channel: f32| (channel * 255.).round().clamp(0., 255.) as u8;
    [byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a)]
}

/// `from` moved `amount` of the way to `to`, in sRGB: 0 is `from`, 1 is `to`.
pub fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let (a, b) = (Rgba::from(from), Rgba::from(to));
    let amount = amount.clamp(0., 1.);
    let lerp = |x: f32, y: f32| x + (y - x) * amount;
    Hsla::from(Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    })
}

/// The relative luminance of a colour (WCAG 2.x), 0 for black and 1 for
/// white.
pub fn luminance(color: Hsla) -> f32 {
    let rgba = Rgba::from(color);
    let linear = |channel: f32| {
        if channel <= 0.03928 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
}

/// The contrast ratio of two colours (WCAG 2.x), from 1 to 21.
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// A well mixed number from a seed (SplitMix64): the same seed always gives
/// the same number, and close seeds give unrelated ones.
pub fn scramble(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
