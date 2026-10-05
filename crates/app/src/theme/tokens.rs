//! The tokens of a theme, as data: the one list that the theme files, the
//! export, the new-theme template and the reference (`docs/THEMES.md`) are all
//! generated from, so they cannot disagree with each other or with the code.
//!
//! A token has a stable public *key* (what a theme file calls it), the table of
//! the file it lives in, a kind (colour, number, switch, text, choice or list
//! of colours), a line of documentation and how it is read from and written to a
//! [`Theme`]. The keys are public API: they are never renamed once released.
//!
//! * Colour tokens of the palette are in `[dark]` and `[light]`.
//! * The terminal's colours are in `[dark.terminal]` and `[light.terminal]`
//!   (a `[terminal]` table is read as the same colours for both).
//! * `[fonts]`, `[shape]` and `[lines]` are one for both appearances.
//!
//! A token that the built-in themes *derive* (the terminal's cursor and
//! selection from the accent, for instance) says which in [`Token::derived`]:
//! a theme that extends another and sets only the accent gets the derived
//! tokens recomputed.

use super::{intern, Appearance, Crosshairs, Palette, Theme};
use gpui_kit::{Hsla, Rgba};
use leon_term::colors::{from_rgb8, to_rgb8};

/// A value of a token.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A colour, with its opacity.
    Colour(Hsla),
    /// A list of colours (the sixteen ANSI ones).
    Colours(Vec<Hsla>),
    /// A number.
    Number(f32),
    /// A switch.
    Bool(bool),
    /// Text, or the name of a choice.
    Text(String),
}

/// What a token holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// `#RRGGBB`; with `alpha`, `#RRGGBBAA` is allowed too (only the veil).
    Colour {
        /// Whether a translucent colour is allowed.
        alpha: bool,
    },
    /// Sixteen `#RRGGBB` colours.
    Colours,
    /// A number between two limits.
    Number {
        /// Smallest.
        min: f32,
        /// Largest.
        max: f32,
    },
    /// `true` or `false`.
    Bool,
    /// A font family name.
    Text,
    /// One of these words.
    Choice(&'static [&'static str]),
}

/// The table of a theme file a token is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// `[dark]` and `[light]`.
    Palette,
    /// `[dark.terminal]` and `[light.terminal]`.
    Terminal,
    /// `[fonts]`.
    Fonts,
    /// `[shape]`.
    Shape,
    /// `[lines]`.
    Lines,
}

impl Section {
    /// Whether the section has one value per appearance.
    pub fn per_appearance(self) -> bool {
        matches!(self, Self::Palette | Self::Terminal)
    }
}

/// One token.
pub struct Token {
    /// What a theme file calls it.
    pub key: &'static str,
    /// Where it is in the file.
    pub section: Section,
    /// What it holds.
    pub kind: Kind,
    /// What it is for.
    pub doc: &'static str,
    /// What it follows when a theme does not set it: `""` when it does not.
    /// Read by the generated reference (`docs/THEMES.md`), which a test checks.
    #[cfg_attr(not(test), allow(dead_code))]
    pub derived: &'static str,
    /// Reads it.
    pub get: fn(&Theme, Appearance) -> Value,
    /// Writes it.
    pub set: fn(&mut Theme, Appearance, Value),
}

impl Theme {
    /// The palette of `appearance`, to change.
    pub fn palette_mut(&mut self, appearance: Appearance) -> &mut Palette {
        match appearance {
            Appearance::Light => &mut self.light,
            Appearance::Dark => &mut self.dark,
        }
    }
}

macro_rules! colour {
    ($key:literal, $doc:literal, $derived:literal, $field:ident) => {
        Token {
            key: $key,
            section: Section::Palette,
            kind: Kind::Colour { alpha: false },
            doc: $doc,
            derived: $derived,
            get: |theme, appearance| Value::Colour(theme.palette(appearance).$field),
            set: |theme, appearance, value| {
                if let Value::Colour(colour) = value {
                    theme.palette_mut(appearance).$field = colour;
                }
            },
        }
    };
}

macro_rules! terminal_colour {
    ($key:literal, $doc:literal, $derived:literal, $field:ident) => {
        Token {
            key: $key,
            section: Section::Terminal,
            kind: Kind::Colour { alpha: false },
            doc: $doc,
            derived: $derived,
            get: |theme, appearance| Value::Colour(theme.palette(appearance).terminal.$field),
            set: |theme, appearance, value| {
                if let Value::Colour(colour) = value {
                    theme.palette_mut(appearance).terminal.$field = colour;
                }
            },
        }
    };
}

macro_rules! number {
    ($key:literal, $section:ident, $min:literal, $max:literal, $doc:literal, $read:expr, $write:expr) => {
        Token {
            key: $key,
            section: Section::$section,
            kind: Kind::Number {
                min: $min,
                max: $max,
            },
            doc: $doc,
            derived: "",
            get: |theme, _| Value::Number($read(theme)),
            set: |theme, _, value| {
                if let Value::Number(number) = value {
                    $write(theme, number);
                }
            },
        }
    };
}

macro_rules! switch {
    ($key:literal, $doc:literal, $field:ident) => {
        Token {
            key: $key,
            section: Section::Lines,
            kind: Kind::Bool,
            doc: $doc,
            derived: "",
            get: |theme, _| Value::Bool(theme.lines.$field),
            set: |theme, _, value| {
                if let Value::Bool(on) = value {
                    theme.lines.$field = on;
                }
            },
        }
    };
}

/// Every token, in the order the reference and the templates list them.
pub static TOKENS: &[Token] = &[
    colour!("background", "The page: the sidebar, the main pane and their headers.", "", background),
    colour!("surface", "Raised pieces on the page: cards, fields, chips.", "", surface),
    colour!("surface_2", "Recessed or hovered fill: the open row, a pressed control.", "", surface_2),
    colour!("border", "Hairline rules between panes and around fields.", "", border),
    colour!("grid_mark", "Crosshairs where rules meet and corner ticks: one step stronger than `border`.", "", grid_mark),
    colour!("guide", "The faintest blueprint line: dimension lines and ticks of an empty state.", "", guide),
    Token {
        key: "scrim",
        section: Section::Palette,
        kind: Kind::Colour { alpha: true },
        doc: "The veil over the window behind an overlay. Translucent: `#RRGGBBAA`.",
        derived: "",
        get: |theme, appearance| Value::Colour(theme.palette(appearance).scrim),
        set: |theme, appearance, value| {
            if let Value::Colour(colour) = value {
                theme.palette_mut(appearance).scrim = colour;
            }
        },
    },
    colour!("elevated_border", "The outline of something lifted over the window: a card, a menu.", "", elevated_border),
    colour!("text", "Names and message text.", "", text),
    colour!("text_muted", "Secondary text, labels, metadata.", "", text_muted),
    colour!("text_faint", "Glyphs that only decorate, and quiet captions.", "", text_faint),
    colour!("accent", "The accent as text and as a line: focus ring, selection bar, caret, active marker.", "", signal),
    colour!("accent_fill", "The accent as a fill, behind `on_accent_fill`.", "accent when the parent's did", accent_fill),
    colour!("on_accent_fill", "What is drawn on `accent_fill`.", "the most legible of the parent's, the page and the text", on_accent_fill),
    colour!("primary_fill", "The fill of a primary button.", "accent when the parent's did", primary_fill),
    colour!("on_primary", "Text on a primary button.", "the most legible of the parent's, the page and the text", on_primary),
    colour!("success", "Operational, connected. A state colour: never decoration.", "", success),
    colour!("warning", "Attention, processing. A state colour.", "", warning),
    colour!("error", "Failure, interruption. A state colour.", "", error),
    colour!("info", "Activity, live signals. A state colour.", "", info),
    colour!("elsewhere", "A session running in another terminal, not in Leon. A state colour, apart from `success`.", "", elsewhere),
    colour!("logo", "The colour of the Leon mark in the header. The mark itself is not themeable.", "accent when the parent's did", logo),
    colour!("agent_claude", "Claude Code's mark.", "", agent_claude),
    colour!("agent_codex", "Codex's mark.", "", agent_codex),
    colour!("agent_opencode", "opencode's mark.", "", agent_opencode),
    terminal_colour!("foreground", "Terminal text with no colour of its own.", "text", foreground),
    terminal_colour!("background", "The terminal's page.", "background", background),
    terminal_colour!("cursor", "The terminal's cursor.", "accent", cursor),
    terminal_colour!("selection", "The background of selected cells (opaque).", "a tint of the accent over the page", selection),
    terminal_colour!("find_match", "The background of a find match (opaque).", "a lighter tint of the accent over the page", find_match),
    terminal_colour!("find_match_current", "The background of the match the find bar is on; its text is drawn in the page's colour.", "accent", find_match_current),
    Token {
        key: "ansi",
        section: Section::Terminal,
        kind: Kind::Colours,
        doc: "The sixteen ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then the eight bright ones.",
        derived: "",
        get: |theme, appearance| Value::Colours(theme.palette(appearance).terminal.ansi.to_vec()),
        set: |theme, appearance, value| {
            if let Value::Colours(list) = value {
                if let Ok(ansi) = <[Hsla; 16]>::try_from(list) {
                    theme.palette_mut(appearance).terminal.ansi = ansi;
                }
            }
        },
    },
    Token {
        key: "sans",
        section: Section::Fonts,
        kind: Kind::Text,
        doc: "Interface and message text. A family bundled with Leon (Inter, Space Grotesk) or installed on the system; a missing one falls back to the parent's.",
        derived: "",
        get: |theme, _| Value::Text(theme.typography.sans.to_owned()),
        set: |theme, _, value| {
            if let Value::Text(family) = value {
                theme.typography.sans = intern(&family);
            }
        },
    },
    Token {
        key: "mono",
        section: Section::Fonts,
        kind: Kind::Text,
        doc: "Everything technical: labels, metadata, code, terminals. A family bundled with Leon (JetBrains Mono, Geist Mono) or installed on the system.",
        derived: "",
        get: |theme, _| Value::Text(theme.typography.mono.to_owned()),
        set: |theme, _, value| {
            if let Value::Text(family) = value {
                theme.typography.mono = intern(&family);
            }
        },
    },
    number!("radius", Shape, 0.0, 24.0, "The corner radius of controls, chips, inputs and floating cards, in pixels at 100 %.", |t: &Theme| t.shape.radius, |t: &mut Theme, n| t.shape.radius = n),
    number!("radius_cell", Shape, 0.0, 24.0, "The corner radius of grid cells and terminal panes.", |t: &Theme| t.shape.radius_cell, |t: &mut Theme, n| t.shape.radius_cell = n),
    number!("label_size", Shape, 8.0, 16.0, "The size of mono uppercase labels such as `[ STATUS ]`.", |t: &Theme| t.shape.label_size, |t: &mut Theme, n| t.shape.label_size = n),
    number!("crosshair", Shape, 5.0, 31.0, "The length of a crosshair's arms where two rules meet.", |t: &Theme| t.shape.crosshair, |t: &mut Theme, n| t.shape.crosshair = n),
    Token {
        key: "crosshairs",
        section: Section::Lines,
        kind: Kind::Choice(&["off", "header", "all"]),
        doc: "Which crosshairs are drawn: `off`, `header` (only under the sidebar's header) or `all` (every intersection of the window's rules and of split panes).",
        derived: "",
        get: |theme, _| Value::Text(theme.lines.crosshairs.name().to_owned()),
        set: |theme, _, value| {
            if let Value::Text(name) = value {
                if let Some(choice) = Crosshairs::parse(&name) {
                    theme.lines.crosshairs = choice;
                }
            }
        },
    },
    switch!("corner_ticks", "Corner ticks on framed surfaces: cards, menus, the focused pane.", corner_ticks),
    switch!("guides", "The sidebar's footer and the status strip share one rule across the window.", guides),
    switch!("empty_motif", "A frame of corner ticks and a dimension line around empty states.", empty_motif),
    number!("tick_length", Lines, 3.0, 24.0, "The length of a corner tick's arms, in pixels at 100 %.", |t: &Theme| t.lines.tick_length, |t: &mut Theme, n| t.lines.tick_length = n),
    number!("weight", Lines, 1.0, 2.0, "The thickness of crosshair and tick arms: 1 or 2 pixels at every interface size.", |t: &Theme| t.lines.weight, |t: &mut Theme, n| t.lines.weight = n),
];

/// The token with this key in this section.
pub fn find(section: Section, key: &str) -> Option<&'static Token> {
    TOKENS
        .iter()
        .find(|token| token.section == section && token.key == key)
}

/// A colour as the text of a file: `#RRGGBB`, with the opacity as `AA` after
/// it when it is not full.
pub fn colour_text(colour: Hsla) -> String {
    let (r, g, b) = to_rgb8(colour);
    let alpha = (Rgba::from(colour).a.clamp(0.0, 1.0) * 255.0).round() as u8;
    if alpha == 255 {
        format!("#{r:02X}{g:02X}{b:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}{alpha:02X}")
    }
}

/// Reads `#RRGGBB` or `#RRGGBBAA`.
pub fn parse_colour(text: &str) -> Result<Hsla, String> {
    let digits = text
        .strip_prefix('#')
        .ok_or_else(|| format!("{text:?} is not a colour: it starts with `#`"))?;
    if !(digits.len() == 6 || digits.len() == 8) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "{text:?} is not a colour: write `#RRGGBB` or `#RRGGBBAA`"
        ));
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
    let mut colour = from_rgb8(byte(0), byte(2), byte(4));
    if digits.len() == 8 {
        colour.a = f32::from(byte(6)) / 255.0;
    }
    Ok(colour)
}

/// How a value is written in a file.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Colour(colour) => format!("\"{}\"", colour_text(*colour)),
        Value::Colours(list) => {
            let items: Vec<String> = list
                .iter()
                .map(|colour| format!("\"{}\"", colour_text(*colour)))
                .collect();
            format!("[{}]", items.join(", "))
        }
        Value::Number(number) => {
            if number.fract() == 0.0 {
                format!("{number:.1}")
            } else {
                format!("{number}")
            }
        }
        Value::Bool(on) => on.to_string(),
        Value::Text(text) => format!("\"{text}\""),
    }
}

/// The reference of every token as Markdown: what `docs/THEMES.md` embeds. A
/// test fails when the file's copy is not this text.
#[cfg_attr(not(test), allow(dead_code))]
pub fn reference() -> String {
    let mut out = String::new();
    let sections = [
        (Section::Palette, "Colours: `[dark]` and `[light]`"),
        (
            Section::Terminal,
            "The terminal: `[dark.terminal]` and `[light.terminal]`",
        ),
        (Section::Fonts, "`[fonts]`"),
        (Section::Shape, "`[shape]`"),
        (Section::Lines, "`[lines]`"),
    ];
    for (section, heading) in sections {
        out.push_str(&format!("#### {heading}\n\n"));
        out.push_str(
            "| Key | Holds | Follows when not set | What it is |\n| --- | --- | --- | --- |\n",
        );
        for token in TOKENS.iter().filter(|token| token.section == section) {
            let holds = match token.kind {
                Kind::Colour { alpha: false } => "`#RRGGBB`".to_owned(),
                Kind::Colour { alpha: true } => "`#RRGGBBAA`".to_owned(),
                Kind::Colours => "16 x `#RRGGBB`".to_owned(),
                Kind::Number { min, max } => format!("{min} to {max}"),
                Kind::Bool => "`true` or `false`".to_owned(),
                Kind::Text => "a family name".to_owned(),
                Kind::Choice(words) => words
                    .iter()
                    .map(|word| format!("`{word}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let follows = if token.derived.is_empty() {
                "its parent's".to_owned()
            } else {
                token.derived.to_owned()
            };
            out.push_str(&format!(
                "| `{}` | {holds} | {follows} | {} |\n",
                token.key, token.doc
            ));
        }
        out.push('\n');
    }
    out.trim_end().to_owned() + "\n"
}
