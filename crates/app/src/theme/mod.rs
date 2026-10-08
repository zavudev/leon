//! Design tokens, and the themes that set them.
//!
//! Every colour, size, radius and font the UI uses is data of a *theme*, and a
//! theme is data of this module: one file per theme ([`zavu`], [`leon`]), one
//! registry ([`ThemeId`]). Views read what is active through [`palette`],
//! [`metrics`] and [`fonts`]; nothing outside this module contains a colour
//! literal or assumes a particular theme.
//!
//! A theme defines, for light and for dark, a full [`Palette`] (surfaces, text,
//! the accent, the four state colours, the logo, the agents' marks and the
//! terminal's colours), and, once for both, its [`Typography`] (families and
//! font features) and its [`Shape`] (the metrics that differ between brands).
//! The product's mark, the Leon glare, is the same in every theme and is tinted
//! with the palette's logo tokens. Appearance (system, light or dark) is a
//! separate choice, made in `settings.rs`.
//!
//! Whatever a theme's own rules are (Zavu spends one accent on focus, selection
//! and the active marker and keeps its logo monochrome; Leon fills its primary
//! button with the accent) is a property of that theme and is checked by the
//! tests of its file, not assumed here. What every theme must meet is checked
//! for all of them in `tests.rs`: contrast (WCAG 2.x relative luminance) for
//! every pair that carries text or a graphic, an accent that is not the warning
//! colour, and a complete set of tokens.
//!
//! Switching theme is cheap and live: [`apply`] installs the palette, the
//! typography and the shape, aligns the component library with them and
//! refreshes every window.

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};
use gpui_kit::{hsla, rgb, App, FontFeatures, Global, Hsla, Pixels};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::Cell;
use std::sync::Arc;

use crate::brand;
use leon_term::colors::mix;
use leon_term::TerminalTheme;

pub mod author;
mod check;
mod file;
mod leon;
pub mod registry;
pub mod tokens;
pub mod user;
pub mod watch;
mod zavu;

pub use file::{slug as file_slug, valid_id};
pub use registry::{intern, ThemeId};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod user_tests;

/// The interface sizes on offer, in percent. 100 is the design.
pub const SCALE_STEPS: [u16; 5] = [80, 90, 100, 110, 125];

thread_local! {
    /// The interface size in use, in percent. Per thread: the interface lives
    /// on one thread, and each test window on its own.
    static SCALE: Cell<u16> = const { Cell::new(100) };
    /// The sidebar: its width in design pixels, and whether it is showing.
    static SIDEBAR: Cell<(u16, bool)> = const { Cell::new((SIDEBAR_DEFAULT, true)) };
    /// Whether the file tree column is showing.
    static FILES: Cell<bool> = const { Cell::new(false) };
    /// The typography, shape and mark of the active theme. Per thread for the
    /// same reason: [`fonts`] and [`metrics`] are read from views that have no
    /// handle on the application.
    static LOOK: Cell<Option<Look>> = const { Cell::new(None) };
    /// Whether the settings turn the blueprint lines on or off.
    static LINES_MODE: Cell<LinesMode> = const { Cell::new(LinesMode::Theme) };
}

/// The sidebar's width at 100 %, by default.
pub const SIDEBAR_DEFAULT: u16 = 320;
/// The narrowest and the widest the sidebar can be, at 100 %.
pub const SIDEBAR_MIN: u16 = 220;
/// See [`SIDEBAR_MIN`].
pub const SIDEBAR_MAX: u16 = 560;
/// The sidebar width (at 100 %) from which a worktree row has room for the
/// state of its checkout in full; narrower, it keeps only the first part.
pub const SIDEBAR_ROOMY: u16 = 300;
/// How much of its colour the background of an added or removed line of a diff
/// (and of a hunk's header) takes: enough to read as a band, little enough to
/// leave the text its contrast in every theme.
pub const DIFF_TINT: f32 = 0.14;
/// How much one step of the keyboard widens or narrows the sidebar.
pub const SIDEBAR_STEP: u16 = 24;
/// How far below [`SIDEBAR_MIN`] (at 100 %) a drag has to go to close it.
pub const SIDEBAR_SNAP: u16 = 60;

/// A sidebar width kept between its limits.
pub fn clamp_sidebar(width: u16) -> u16 {
    width.clamp(SIDEBAR_MIN, SIDEBAR_MAX)
}

/// Sets the sidebar's width (at 100 %) and whether it is showing.
pub fn set_sidebar(width: u16, visible: bool) {
    SIDEBAR.with(|sidebar| sidebar.set((clamp_sidebar(width), visible)));
}

/// Sets whether the file tree column is showing.
pub fn set_files(visible: bool) {
    FILES.with(|files| files.set(visible));
}

/// Sets the interface size, in percent, to the nearest step on offer.
pub fn set_scale(percent: u16) {
    SCALE.with(|scale| scale.set(nearest_step(percent)));
}

/// The step on offer nearest to `percent`.
pub fn nearest_step(percent: u16) -> u16 {
    SCALE_STEPS
        .into_iter()
        .min_by_key(|step| step.abs_diff(percent))
        .unwrap_or(100)
}

/// The step after (`up`) or before the one in use; the same at the ends.
pub fn next_step(percent: u16, up: bool) -> u16 {
    let at = SCALE_STEPS
        .iter()
        .position(|step| *step == nearest_step(percent))
        .unwrap_or(2);
    let next = if up {
        (at + 1).min(SCALE_STEPS.len() - 1)
    } else {
        at.saturating_sub(1)
    };
    SCALE_STEPS[next]
}

/// The interface size in use, as a factor (1.0 is the design).
pub fn scale() -> f32 {
    f32::from(SCALE.with(Cell::get)) / 100.
}

/// Half pixels are as fine as a size gets: text and boxes stay crisp.
fn snap(value: f32) -> f32 {
    (value * 2.).round() / 2.
}

/// A design token at the interface size in use.
fn token(value: f32) -> Pixels {
    gpui_kit::px(snap(value * scale()))
}

/// A size written in a view, at the interface size in use. The views use this
/// `px`, not the toolkit's, which is how the whole interface scales as one.
pub fn px(value: f32) -> Pixels {
    token(value)
}

/// One device-independent pixel at every interface size: the rules between
/// panes and the arms of the grid marks stay hairlines.
pub fn hairline() -> Pixels {
    gpui_kit::px(1.)
}

/// Light or dark: how a theme is worn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Appearance {
    /// Paper, for dense reading.
    Light,
    /// Near-black, the default.
    Dark,
}

/// The colours of one theme in one appearance.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Which appearance this is.
    pub appearance: Appearance,

    // Surfaces
    /// The page: sidebar, main pane and their header bars.
    pub background: Hsla,
    /// Raised pieces on the page: cards, fields, chips.
    pub surface: Hsla,
    /// Recessed or hovered fill: the open row, a pressed control.
    pub surface_2: Hsla,
    /// Hairlines: pane rules, field outlines.
    pub border: Hsla,
    /// The crosshair where two rules meet, and the corner ticks of a framed
    /// surface: one step stronger than `border`.
    pub grid_mark: Hsla,
    /// The faintest line of the blueprint: the dimension rules and ticks of an
    /// empty state. Quieter than `grid_mark`, louder than nothing.
    pub guide: Hsla,
    /// The veil over the window behind an overlay.
    pub scrim: Hsla,
    /// The outline of something lifted over the window: stronger than a
    /// hairline.
    pub elevated_border: Hsla,

    // Text
    /// Names, message text.
    pub text: Hsla,
    /// Secondary text, labels, metadata.
    pub text_muted: Hsla,
    /// Glyphs that only decorate.
    pub text_faint: Hsla,

    // Signal
    /// The accent as text and as a line: focus ring, selection bar, caret,
    /// active marker.
    pub signal: Hsla,
    /// The accent as a fill, behind `on_accent_fill`: badges. It can differ
    /// from `signal` because a fill and a line are held to different contrasts.
    /// The interface draws no badge yet; the token is part of the brand's
    /// table and the tests hold it to its contrast.
    #[allow(dead_code)]
    pub accent_fill: Hsla,
    /// What is drawn on `accent_fill`.
    #[allow(dead_code)]
    pub on_accent_fill: Hsla,
    /// The fill of a primary button.
    pub primary_fill: Hsla,
    /// Text on a primary button.
    pub on_primary: Hsla,

    // State (never decoration)
    /// Operational, connected.
    pub success: Hsla,
    /// Attention, processing.
    pub warning: Hsla,
    /// Failure, interruption.
    pub error: Hsla,
    /// Activity, live signals.
    pub info: Hsla,
    /// A session that runs in another terminal: held by a process Leon did
    /// not start. Never the green of a session live in Leon.
    pub elsewhere: Hsla,

    // The brand mark
    /// The colour of the product's mark, the glare: one silhouette whose cuts
    /// show the surface behind it.
    pub logo: Hsla,

    // Agent marks. A deliberate exception to a theme's accent rule: each
    // coding agent's logo wears its own brand colour, so these are theme
    // tokens that a theme sets to its own choice (monochrome included). The
    // state of a session lives on the status dot, never on these.
    /// Claude Code's mark.
    pub agent_claude: Hsla,
    /// Codex's mark.
    pub agent_codex: Hsla,
    /// opencode's mark.
    pub agent_opencode: Hsla,

    /// What a terminal wears: its page, text, cursor, selection and the sixteen
    /// ANSI colours.
    pub terminal: TerminalTheme,
}

impl Palette {
    /// The colour of an agent's mark in this theme: its own token when the
    /// catalogue gives it one, else the theme's text colour.
    pub fn agent(&self, agent: leon_core::AgentId) -> Hsla {
        use leon_core::agent::Tint;
        match agent.spec().map_or(Tint::Neutral, |spec| spec.tint) {
            Tint::Claude => self.agent_claude,
            Tint::Codex => self.agent_codex,
            Tint::Opencode => self.agent_opencode,
            Tint::Neutral => self.text,
        }
    }

    /// The four colours that give a theme away at a glance: its page, a card,
    /// its text and its accent.
    pub fn swatch(&self) -> [Hsla; 4] {
        [self.background, self.surface, self.text, self.signal]
    }
}

/// Opaque colour from `0xRRGGBB`.
fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// How far a terminal's selection is the accent: a tint the text stays legible
/// on.
const SELECTION_SIGNAL: f32 = 0.38;

/// How far a find match's background is the accent: a tint the text stays
/// legible on, quieter than the current match, which is the accent itself.
const FIND_MATCH_SIGNAL: f32 = 0.20;

/// How opaque the accent is as the selection of a text field.
const FIELD_SELECTION: f32 = 0.3;

/// The terminal's colours from the interface's: the page and the text, the
/// accent for the cursor and a tint of it for the selection, and `ansi`.
fn terminal_colours(text: Hsla, background: Hsla, signal: Hsla, ansi: [u32; 16]) -> TerminalTheme {
    terminal_colours_tinted(text, background, signal, SELECTION_SIGNAL, ansi)
}

/// [`terminal_colours`] with the selection `tint` of the way from the page to
/// the accent, for a theme whose accent is too strong at the default.
fn terminal_colours_tinted(
    text: Hsla,
    background: Hsla,
    signal: Hsla,
    tint: f32,
    ansi: [u32; 16],
) -> TerminalTheme {
    TerminalTheme {
        foreground: text,
        background,
        cursor: signal,
        selection: mix(background, signal, tint),
        find_match: mix(background, signal, FIND_MATCH_SIGNAL),
        find_match_current: signal,
        ansi: ansi.map(hex),
    }
}

/// The fonts of a theme. Every family is bundled with the application (see
/// `assets/ASSETS.md`) and registered by [`install_fonts`], so switching theme
/// never loads anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Typography {
    /// Interface and message text.
    pub sans: &'static str,
    /// OpenType features of the interface font, as (tag, value).
    pub sans_features: &'static [(&'static str, u32)],
    /// Everything technical: labels, metadata, code.
    pub mono: &'static str,
    /// OpenType features of the mono font used in labels.
    pub mono_features: &'static [(&'static str, u32)],
}

fn features(list: &'static [(&'static str, u32)]) -> FontFeatures {
    FontFeatures(Arc::new(
        list.iter()
            .map(|(tag, on)| ((*tag).to_owned(), *on))
            .collect(),
    ))
}

/// The metrics that differ from one brand to the next, at 100 %.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// The corner radius of controls, chips, inputs and floating cards.
    pub radius: f32,
    /// The corner radius of grid cells and terminal panes.
    pub radius_cell: f32,
    /// The size of mono uppercase labels.
    pub label_size: f32,
    /// The length of a crosshair's arms, where two rules meet.
    pub crosshair: f32,
}

/// Which of the crosshairs a theme draws where pane rules meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crosshairs {
    /// None.
    Off,
    /// Only the one under the sidebar's header, where the first two rules
    /// meet: the application's original look.
    Header,
    /// At every intersection of the window's rules and of the split panes.
    All,
}

impl Crosshairs {
    /// The name a theme file gives it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Header => "header",
            Self::All => "all",
        }
    }

    /// Reads a name written by [`Crosshairs::name`].
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Off, Self::Header, Self::All]
            .into_iter()
            .find(|choice| choice.name() == text)
    }
}

/// The blueprint lines of a theme: which pieces of the line system it draws and
/// how heavy they are. The colours are palette tokens (`border`, `grid_mark`,
/// `guide`); the crosshair's arm is [`Shape::crosshair`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lines {
    /// Crosshairs where rules meet.
    pub crosshairs: Crosshairs,
    /// Corner ticks on framed surfaces (cards, menus, the focused pane).
    pub corner_ticks: bool,
    /// Whether the window's rules read as continuous lines: the sidebar's
    /// footer and the status strip share one rule across the window.
    pub guides: bool,
    /// Whether empty states are framed by corner ticks and a dimension line.
    pub empty_motif: bool,
    /// The length of a corner tick's arms, at 100 %.
    pub tick_length: f32,
    /// The thickness of crosshair arms and tick arms, in pixels at every
    /// interface size: 1 or 2.
    pub weight: f32,
}

/// One theme: everything that makes a look.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Its light palette.
    pub light: Palette,
    /// Its dark palette.
    pub dark: Palette,
    /// Its fonts.
    pub typography: Typography,
    /// Its shape.
    pub shape: Shape,
    /// Its blueprint lines.
    pub lines: Lines,
}

impl Theme {
    /// The palette of `appearance`.
    pub fn palette(&self, appearance: Appearance) -> Palette {
        match appearance {
            Appearance::Light => self.light,
            Appearance::Dark => self.dark,
        }
    }
}

impl Serialize for ThemeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.slug())
    }
}

/// An id this build does not know (a newer version's, a typo, the retired
/// `leon-lime` and `leon-bone`, a user theme whose file is gone) is the
/// default, not an error: an old or foreign `settings.json` must never keep the
/// application from starting. The missing id is noted so that the window can
/// say so (see [`registry::take_missing`]).
impl<'de> Deserialize<'de> for ThemeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Self::parse(&text).unwrap_or_else(|| {
            tracing::warn!(id = %text, "unknown theme in the settings; using the default");
            registry::note_missing(&text);
            Self::DEFAULT
        }))
    }
}

/// What views read without a handle on the application: the theme's typography
/// and shape.
#[derive(Clone, Copy)]
struct Look {
    typography: Typography,
    shape: Shape,
    lines: Lines,
}

impl From<&Theme> for Look {
    fn from(theme: &Theme) -> Self {
        Self {
            typography: theme.typography,
            shape: theme.shape,
            lines: theme.lines,
        }
    }
}

fn look() -> Look {
    LOOK.with(Cell::get)
        .unwrap_or_else(|| Look::from(&ThemeId::DEFAULT.theme()))
}

/// Whether the blueprint lines follow the theme, are always drawn, or never.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinesMode {
    /// What the theme says.
    #[default]
    Theme,
    /// The whole line system, whatever the theme says.
    On,
    /// No crosshairs, ticks or frames.
    Off,
}

/// Chooses whether the blueprint lines follow the theme (a setting).
pub fn set_lines_mode(mode: LinesMode) {
    LINES_MODE.with(|cell| cell.set(mode));
}

/// The blueprint lines in use: the active theme's, unless the settings turn
/// them on or off for every theme.
pub fn lines() -> Lines {
    let lines = look().lines;
    match LINES_MODE.with(Cell::get) {
        LinesMode::Theme => lines,
        LinesMode::On => Lines {
            crosshairs: Crosshairs::All,
            corner_ticks: true,
            empty_motif: true,
            ..lines
        },
        LinesMode::Off => Lines {
            crosshairs: Crosshairs::Off,
            corner_ticks: false,
            empty_motif: false,
            ..lines
        },
    }
}

/// Font families and features of the active theme.
pub mod fonts {
    use super::{features, look, FontFeatures};

    /// Interface and message text.
    pub fn sans() -> &'static str {
        look().typography.sans
    }
    /// Everything technical: labels, metadata, code.
    pub fn mono() -> &'static str {
        look().typography.mono
    }
    /// The features the interface font is drawn with.
    pub fn sans_features() -> FontFeatures {
        features(look().typography.sans_features)
    }
    /// The features the mono font of the labels is drawn with.
    pub fn mono_features() -> FontFeatures {
        features(look().typography.mono_features)
    }
}

/// Sizes and radii, at the interface size in use.
///
/// The numbers are the design at 100 %; every one is multiplied by the
/// interface scale when it is read, so they are functions, named like the
/// constants they stand for. The few that a theme sets for itself read the
/// active theme's [`Shape`].
#[allow(non_snake_case)]
pub mod metrics {
    use super::{look, token, Pixels};

    /// Width of the sidebar tree: the width it was given, scaled with the
    /// interface; nothing while it is hidden.
    pub fn SIDEBAR_WIDTH() -> Pixels {
        let (width, visible) = super::SIDEBAR.with(std::cell::Cell::get);
        if visible {
            token(f32::from(width))
        } else {
            gpui_kit::px(0.)
        }
    }
    /// Width of the file tree column, between the sidebar and the main pane;
    /// nothing while it is hidden.
    pub fn FILES_WIDTH() -> Pixels {
        if super::FILES.with(std::cell::Cell::get) {
            token(260.0)
        } else {
            gpui_kit::px(0.)
        }
    }
    /// How far each level of the tree is indented.
    pub fn INDENT() -> Pixels {
        token(12.0)
    }
    /// Height of the top strip of every pane. Their bottom rules line up into
    /// one line across the window.
    pub fn HEADER_HEIGHT() -> Pixels {
        HEADER_PAD() * 2.
            + HEADER_TITLE_LINE()
            + HEADER_GAP()
            + HEADER_META_LINE()
            + super::hairline()
    }
    /// The line of a header's title: the room for the toggle button, the logo
    /// and the 13 px text.
    pub fn HEADER_TITLE_LINE() -> Pixels {
        token(20.0)
    }
    /// The line of a header's mono metadata.
    pub fn HEADER_META_LINE() -> Pixels {
        token(15.0)
    }
    /// The space between the two lines of a header.
    pub fn HEADER_GAP() -> Pixels {
        token(2.0)
    }
    /// The space above and below a header's two lines.
    pub fn HEADER_PAD() -> Pixels {
        token(5.0)
    }
    /// Height of one row of the sidebar tree, whatever it shows.
    pub fn ROW_HEIGHT() -> Pixels {
        token(38.0)
    }
    /// The column at the right of the sidebar tree that its scrollbar has to
    /// itself, so the rows' marks at their end are never under the thumb.
    pub fn SCROLLBAR_GUTTER() -> Pixels {
        token(16.0)
    }
    /// Height of the status strip under the main pane.
    pub fn STATUS_HEIGHT() -> Pixels {
        token(28.0)
    }
    /// The side of a square control.
    pub fn CONTROL() -> Pixels {
        token(32.0)
    }
    /// Width of the palette.
    pub fn PALETTE_WIDTH() -> Pixels {
        token(640.0)
    }
    /// Width of the Settings card.
    pub fn SETTINGS_WIDTH() -> Pixels {
        token(960.0)
    }
    /// Width of the Settings card's list of sections.
    pub fn SETTINGS_NAV() -> Pixels {
        token(208.0)
    }
    /// The least height of an option of the Settings card.
    pub fn SETTINGS_ROW() -> Pixels {
        token(56.0)
    }
    /// The space above and below the content of an option of the Settings
    /// card.
    pub fn SETTINGS_ROW_PAD() -> Pixels {
        token(10.0)
    }
    /// The width of the column of controls at the right of an option of the
    /// Settings card: the text never runs under it.
    pub fn SETTINGS_CONTROL() -> Pixels {
        token(240.0)
    }
    /// The narrowest column of text an option of the Settings card keeps beside
    /// its controls; narrower than this, the controls go under the text.
    pub fn SETTINGS_TEXT_MIN() -> Pixels {
        token(300.0)
    }
    /// The corner radius of controls, chips, inputs and floating cards: the
    /// active theme's.
    pub fn RADIUS() -> Pixels {
        token(look().shape.radius)
    }
    /// The corner radius of grid cells and terminal panes: the active
    /// theme's.
    pub fn RADIUS_CELL() -> Pixels {
        token(look().shape.radius_cell)
    }
    /// Body text.
    pub fn TEXT_BODY() -> Pixels {
        token(13.0)
    }
    /// Secondary text.
    pub fn TEXT_SMALL() -> Pixels {
        token(12.0)
    }
    /// Height of the strip of terminal tabs.
    pub fn TAB_BAR_HEIGHT() -> Pixels {
        token(30.0)
    }
    /// The space between the terminal's grid and the edge of its pane.
    pub fn TERMINAL_PADDING() -> Pixels {
        token(8.0)
    }
    /// The side of a project's logo in the tree and the palette.
    pub fn PROJECT_ICON() -> Pixels {
        token(16.0)
    }
    /// Mono uppercase labels: the active theme's size. Small caps in mono are
    /// set semibold.
    pub fn TEXT_LABEL() -> Pixels {
        token(look().shape.label_size)
    }
    /// The length of a crosshair's arms: the active theme's.
    pub fn CROSSHAIR() -> Pixels {
        token(look().shape.crosshair)
    }
    /// The length of a corner tick's arms: the active theme's.
    pub fn TICK() -> Pixels {
        token(look().lines.tick_length)
    }
    /// The height of the sidebar's tools: a button, the room around it and its
    /// rule.
    pub fn TOOLS_HEIGHT() -> Pixels {
        CONTROL() + token(8.0) + super::hairline()
    }
    /// The height of the strip under the main pane. Where the theme draws
    /// continuous rules it is as tall as the sidebar's tools, so their rules
    /// are one line across the window.
    pub fn FOOTER_HEIGHT() -> Pixels {
        if look().lines.guides {
            TOOLS_HEIGHT()
        } else {
            STATUS_HEIGHT()
        }
    }
}

/// The theme in use, with its palette in the appearance in use.
struct ActiveTheme {
    #[cfg(test)]
    id: ThemeId,
    palette: Palette,
}

impl Global for ActiveTheme {}

/// The active palette.
pub fn palette(cx: &App) -> Palette {
    cx.global::<ActiveTheme>().palette
}

/// The active theme: the one on screen, a preview included.
#[cfg(test)]
pub fn current(cx: &App) -> ThemeId {
    cx.global::<ActiveTheme>().id
}

/// Registers every bundled font, of every theme. Call once, before the first
/// window opens.
pub fn install_fonts(cx: &App) {
    if let Err(error) = cx.text_system().add_fonts(brand::fonts()) {
        // The system's default faces take over; nothing else changes.
        tracing::warn!(%error, "could not load the bundled fonts");
    }
}

/// Puts a theme on screen: installs its palette in `appearance`, its
/// typography, shape and mark, and aligns the component library (text fields,
/// scrollbars) with them. Every window is redrawn, terminals included.
pub fn apply(id: ThemeId, appearance: Appearance, cx: &mut App) {
    let theme = id.theme();
    let palette = theme.palette(appearance);
    LOOK.with(|look| look.set(Some(Look::from(&theme))));
    cx.set_global(ActiveTheme {
        #[cfg(test)]
        id,
        palette,
    });

    let mode = match appearance {
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
    };
    ComponentTheme::change(mode, None, cx);
    ComponentTheme::update(cx, |component| {
        component.colors.background = palette.background;
        component.colors.foreground = palette.text;
        component.colors.muted_foreground = palette.text_muted;
        component.colors.caret = palette.signal;
        component.colors.primary = palette.primary_fill;
        component.colors.primary_foreground = palette.on_primary;
        component.colors.ring = palette.signal;
        component.colors.selection = palette.signal.opacity(FIELD_SELECTION);
        component.colors.border = palette.border;
        component.colors.scrollbar = hsla(0., 0., 0., 0.);
        component.colors.scrollbar_thumb = palette.text_faint.opacity(0.45);
        component.colors.scrollbar_thumb_hover = palette.text_faint.opacity(0.7);
        component.font_family = theme.typography.sans.into();
        component.mono_font_family = theme.typography.mono.into();
        component.font_size = metrics::TEXT_BODY();
        component.radius = metrics::RADIUS();
        component.radius_lg = metrics::RADIUS();
    });
    cx.refresh_windows();
}
