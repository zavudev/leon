//! The Leon theme: the brand system (`brand/tokens.md`).
//!
//! Stone neutrals on paper or near-black, hairlines instead of shadows, Inter
//! for the interface and JetBrains Mono for everything technical, 6 px corners
//! on controls and square grid cells. One accent, acid yellow, that is a signal,
//! not decoration, and that also fills the primary button, with ink on it.
//!
//! Acid yellow is the hue of a warning, so the warning colour is orange, and
//! the five colours that carry meaning (the accent and the four states) are
//! held at least 20 apart in CIE76 for every pair (see `theme/tests.rs`).

use super::{
    hex, terminal_colours_tinted, Appearance, Crosshairs, Lines, Palette, Shape, Theme, Typography,
};

const INK: u32 = 0x0A_0A_0A;
const PAPER_DARK_TEXT: u32 = 0xFA_FA_F9;

// Neutrals of wuapi: stone, warm.
const BACKGROUND_DARK: u32 = 0x0A_0A_0A;
const SURFACE_DARK: u32 = 0x14_14_14;
const SURFACE_2_DARK: u32 = 0x1C_1C_1C;
const BORDER_DARK: u32 = 0x26_26_26;
const GRID_MARK_DARK: u32 = 0x55_55_55;
const GUIDE_DARK: u32 = 0x33_33_33;
const ELEVATED_DARK: u32 = 0x5A_5A_5A;
const TEXT_MUTED_DARK: u32 = 0xA8_A2_9E;
const TEXT_FAINT_DARK: u32 = 0x8C_85_80;

const BACKGROUND_LIGHT: u32 = 0xFA_FA_F9;
const SURFACE_LIGHT: u32 = 0xFF_FF_FF;
const SURFACE_2_LIGHT: u32 = 0xF5_F5_F4;
const BORDER_LIGHT: u32 = 0xE7_E5_E4;
const GRID_MARK_LIGHT: u32 = 0xAA_A8_A7;
const GUIDE_LIGHT: u32 = 0xD3_D1_D0;
const ELEVATED_LIGHT: u32 = 0xA8_A2_9E;
const TEXT_LIGHT: u32 = 0x0C_0A_09;
const TEXT_MUTED_LIGHT: u32 = 0x57_53_4E;
const TEXT_FAINT_LIGHT: u32 = 0x78_71_6C;

// The accent: acid yellow. On the dark page the signal and the fill are the
// same yellow (16.05:1 on ink); on paper the signal is a dark olive-yellow
// (5.50:1) and the fill stays yellow, carrying ink.
const SIGNAL_DARK: u32 = 0xFF_EA_00;
const SIGNAL_LIGHT: u32 = 0x75_66_00;
const ACCENT_FILL: u32 = 0xFF_EA_00;

// State colours. Warning is orange, away from the accent's yellow.
const SUCCESS_DARK: u32 = 0x4D_F6_88;
const WARNING_DARK: u32 = 0xFF_95_00;
const ERROR_DARK: u32 = 0xFF_5E_5E;
const INFO_DARK: u32 = 0x6E_FA_FF;
const SUCCESS_LIGHT: u32 = 0x04_78_57;
const WARNING_LIGHT: u32 = 0xA0_60_00;
const ERROR_LIGHT: u32 = 0xB9_1C_1C;
const INFO_LIGHT: u32 = 0x0E_74_90;
// A session held by a process in another terminal: violet, away from the
// green of a live session, the cyan of activity and the yellow accent.
const ELSEWHERE_DARK: u32 = 0xC7_92_FF;
const ELSEWHERE_LIGHT: u32 = 0x7E_22_CE;

// Agent marks: Claude's clay (a darker one on paper), Codex monochrome,
// opencode's own two monochromes.
const CLAUDE_DARK: u32 = 0xD9_77_57;
const CLAUDE_LIGHT: u32 = 0xB8_58_3A;
const OPENCODE_DARK: u32 = 0xF1_EC_EC;
const OPENCODE_LIGHT: u32 = 0x21_1E_1E;

/// How far the terminal's selection is the accent. Lighter than the yellow
/// would allow at Zavu's 0.38: coloured text (orange, red) must stay legible on
/// the selected cells.
const SELECTION_SIGNAL: f32 = 0.24;

/// The terminal's ANSI colours. Red, green, yellow and cyan stand for the four
/// state colours so that a state means the same in a terminal and in the
/// interface; blue and magenta stay cool and violet. The yellow is the warning
/// orange, so yellow text is never mistaken for the acid-yellow cursor.
const ANSI_DARK: [u32; 16] = [
    0x3F_3F_46,
    0xFF_5E_5E,
    0x4D_F6_88,
    WARNING_DARK,
    0x7A_A2_FF,
    0xC7_92_FF,
    0x6E_FA_FF,
    0xD4_D4_D8,
    0x71_71_7A,
    0xFF_8A_8A,
    0x86_FF_AD,
    0xFF_B3_40,
    0xA5_C0_FF,
    0xDD_B8_FF,
    0xA5_FC_FF,
    0xFA_FA_FA,
];

fn ansi_light() -> [u32; 16] {
    let mut ansi = [
        0x27_27_2A, 0xB9_1C_1C, 0x04_78_57, 0x00_00_00, 0x1D_4E_D8, 0x7E_22_CE, 0x0E_74_90,
        0x52_52_5B, 0x6B_6B_74, 0x9F_12_39, 0x15_6B_34, 0x00_00_00, 0x1E_40_AF, 0x6B_21_A8,
        0x0B_5E_75, 0x3F_3F_46,
    ];
    ansi[3] = WARNING_LIGHT;
    ansi[11] = 0x8A_52_00;
    ansi
}

fn dark() -> Palette {
    let (background, text, signal) = (hex(BACKGROUND_DARK), hex(PAPER_DARK_TEXT), hex(SIGNAL_DARK));
    Palette {
        appearance: Appearance::Dark,
        background,
        surface: hex(SURFACE_DARK),
        surface_2: hex(SURFACE_2_DARK),
        border: hex(BORDER_DARK),
        grid_mark: hex(GRID_MARK_DARK),
        guide: hex(GUIDE_DARK),
        scrim: hex(BACKGROUND_DARK).opacity(0.64),
        elevated_border: hex(ELEVATED_DARK),
        text,
        text_muted: hex(TEXT_MUTED_DARK),
        text_faint: hex(TEXT_FAINT_DARK),
        signal,
        accent_fill: hex(ACCENT_FILL),
        on_accent_fill: hex(INK),
        primary_fill: hex(ACCENT_FILL),
        on_primary: hex(INK),
        success: hex(SUCCESS_DARK),
        warning: hex(WARNING_DARK),
        error: hex(ERROR_DARK),
        info: hex(INFO_DARK),
        elsewhere: hex(ELSEWHERE_DARK),
        // On the dark page the mark is the accent; the cuts show the page.
        logo: hex(ACCENT_FILL),
        agent_claude: hex(CLAUDE_DARK),
        agent_codex: hex(PAPER_DARK_TEXT),
        agent_opencode: hex(OPENCODE_DARK),
        terminal: terminal_colours_tinted(text, background, signal, SELECTION_SIGNAL, ANSI_DARK),
    }
}

fn light() -> Palette {
    let (background, text, signal) = (hex(BACKGROUND_LIGHT), hex(TEXT_LIGHT), hex(SIGNAL_LIGHT));
    Palette {
        appearance: Appearance::Light,
        background,
        surface: hex(SURFACE_LIGHT),
        surface_2: hex(SURFACE_2_LIGHT),
        border: hex(BORDER_LIGHT),
        grid_mark: hex(GRID_MARK_LIGHT),
        guide: hex(GUIDE_LIGHT),
        scrim: hex(TEXT_LIGHT).opacity(0.46),
        elevated_border: hex(ELEVATED_LIGHT),
        text,
        text_muted: hex(TEXT_MUTED_LIGHT),
        text_faint: hex(TEXT_FAINT_LIGHT),
        signal,
        accent_fill: hex(ACCENT_FILL),
        on_accent_fill: hex(INK),
        primary_fill: hex(ACCENT_FILL),
        on_primary: hex(INK),
        success: hex(SUCCESS_LIGHT),
        warning: hex(WARNING_LIGHT),
        error: hex(ERROR_LIGHT),
        info: hex(INFO_LIGHT),
        elsewhere: hex(ELSEWHERE_LIGHT),
        // On paper the mark is ink; the cuts show the paper.
        logo: hex(INK),
        agent_claude: hex(CLAUDE_LIGHT),
        agent_codex: hex(TEXT_LIGHT),
        agent_opencode: hex(OPENCODE_LIGHT),
        terminal: terminal_colours_tinted(text, background, signal, SELECTION_SIGNAL, ansi_light()),
    }
}

/// The Leon theme.
pub(super) fn theme() -> Theme {
    Theme {
        light: light(),
        dark: dark(),
        typography: Typography {
            sans: "Inter",
            sans_features: &[],
            mono: "JetBrains Mono",
            // Paths, ids and shortcuts are read letter by letter: no ligatures.
            mono_features: &[("calt", 0)],
        },
        shape: Shape {
            radius: 6.0,
            radius_cell: 0.0,
            label_size: 11.0,
            crosshair: 11.0,
        },
        // The whole blueprint system of wuapi, drawn quietly (see
        // `brand/tokens.md`).
        lines: Lines {
            crosshairs: Crosshairs::All,
            corner_ticks: true,
            guides: true,
            empty_motif: true,
            tick_length: 8.0,
            weight: 1.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{colour_difference, contrast};
    use super::super::ThemeId;
    use super::*;
    use gpui_kit::{Hsla, Rgba};

    fn h(value: u32) -> Hsla {
        gpui_kit::rgb(value).into()
    }

    fn both() -> [(Appearance, Palette); 2] {
        [Appearance::Dark, Appearance::Light].map(|a| (a, ThemeId::Leon.palette(a)))
    }

    #[test]
    fn leon_is_the_brand_books_neutrals_with_the_acid_yellow_accent() {
        let dark = ThemeId::Leon.palette(Appearance::Dark);
        assert_eq!(dark.background, h(0x0A0A0A));
        assert_eq!(dark.surface, h(0x141414));
        assert_eq!(dark.surface_2, h(0x1C1C1C));
        assert_eq!(dark.border, h(0x262626));
        assert_eq!(dark.text, h(0xFAFAF9));
        assert_eq!(dark.text_muted, h(0xA8A29E));
        assert_eq!(dark.text_faint, h(0x8C8580));
        assert_eq!(dark.signal, h(0xFFEA00));
        assert_eq!(dark.accent_fill, h(0xFFEA00));
        assert_eq!(dark.on_accent_fill, h(0x0A0A0A));
        assert_eq!(dark.primary_fill, h(0xFFEA00));
        assert_eq!(dark.logo, h(0xFFEA00));
        assert_eq!(dark.warning, h(0xFF9500));
        let light = ThemeId::Leon.palette(Appearance::Light);
        assert_eq!(light.background, h(0xFAFAF9));
        assert_eq!(light.surface, h(0xFFFFFF));
        assert_eq!(light.surface_2, h(0xF5F5F4));
        assert_eq!(light.border, h(0xE7E5E4));
        assert_eq!(light.text, h(0x0C0A09));
        assert_eq!(light.text_muted, h(0x57534E));
        assert_eq!(light.text_faint, h(0x78716C));
        assert_eq!(light.signal, h(0x756600));
        assert_eq!(light.accent_fill, h(0xFFEA00));
        assert_eq!(light.primary_fill, h(0xFFEA00));
        assert_eq!(light.logo, h(0x0A0A0A));
        assert_eq!(light.warning, h(0xA06000));
    }

    #[test]
    fn the_accent_contrast_is_the_brand_books_on_the_page_and_a_card() {
        let (dark, light) = (both()[0].1, both()[1].1);
        assert!(contrast(dark.signal, dark.background) >= 16.0);
        assert!(contrast(dark.signal, dark.surface) >= 14.9);
        assert!(contrast(light.signal, light.background) >= 5.49);
        assert!(contrast(light.signal, light.surface) >= 5.7);
        assert!(contrast(light.signal, light.surface_2) >= 4.5);
    }

    #[test]
    fn the_orange_warning_reads_on_the_page_a_card_and_the_selected_row() {
        for (appearance, p) in both() {
            for (surface, name) in [
                (p.background, "the page"),
                (p.surface, "a card"),
                (p.surface_2, "the selected row"),
            ] {
                let ratio = contrast(p.warning, surface);
                assert!(ratio >= 4.5, "{appearance:?} on {name}: {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn every_state_colour_reads_on_the_selected_row_in_both_appearances() {
        for (appearance, p) in both() {
            for (name, colour) in [
                ("success", p.success),
                ("warning", p.warning),
                ("error", p.error),
                ("info", p.info),
                ("elsewhere", p.elsewhere),
                ("accent", p.signal),
            ] {
                let ratio = contrast(colour, p.surface_2);
                assert!(ratio >= 4.5, "{appearance:?} {name}: {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn the_warning_dot_next_to_claudes_mark_is_still_a_different_colour() {
        // The status dot and the agent's logo share a row of the tree.
        for (appearance, p) in both() {
            let apart = colour_difference(p.warning, p.agent_claude);
            assert!(apart >= 20.0, "{appearance:?}: {apart:.1}");
        }
    }

    #[test]
    fn in_leon_the_accent_fills_the_primary_button_and_the_logo_is_the_accent_on_dark() {
        for (appearance, p) in both() {
            assert_eq!(p.primary_fill, p.accent_fill, "{appearance:?}");
            assert_eq!(p.on_primary, p.on_accent_fill, "{appearance:?}");
        }
        let (dark, light) = (both()[0].1, both()[1].1);
        assert_eq!(dark.logo, dark.signal);
        assert_eq!(light.logo, light.on_accent_fill);
    }

    #[test]
    fn the_terminal_cursor_and_selection_follow_the_leon_accent() {
        for (appearance, p) in both() {
            assert_eq!(p.terminal.cursor, p.signal);
            // The selection is a tint of the accent: it is not the page.
            let (selection, background) =
                (Rgba::from(p.terminal.selection), Rgba::from(p.background));
            assert!(
                (selection.r - background.r).abs()
                    + (selection.g - background.g).abs()
                    + (selection.b - background.b).abs()
                    > 0.05,
                "{appearance:?}"
            );
        }
    }

    #[test]
    fn terminal_text_in_every_colour_stays_legible_on_the_selection() {
        // Black and bright black are mostly backgrounds; the rest are text.
        for (appearance, p) in both() {
            let t = p.terminal;
            for (i, colour) in t
                .ansi
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != 0 && *i != 8)
            {
                let ratio = contrast(*colour, t.selection);
                assert!(
                    ratio >= 3.0,
                    "{appearance:?} ansi {i} on the selection: {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn the_terminal_yellows_are_the_warning_orange_and_not_the_cursor() {
        for (appearance, p) in both() {
            let t = p.terminal;
            assert_eq!(t.ansi[3], p.warning, "{appearance:?}");
            for i in [3, 11] {
                let apart = colour_difference(t.ansi[i], t.cursor);
                assert!(
                    apart >= 20.0,
                    "{appearance:?} ansi {i} vs cursor: {apart:.1}"
                );
                let ratio = contrast(t.ansi[i], t.background);
                assert!(ratio >= 4.5, "{appearance:?} ansi {i}: {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn the_leon_theme_has_the_brand_fonts_and_shape() {
        let theme = ThemeId::Leon.theme();
        assert_eq!(theme.typography.sans, "Inter");
        assert_eq!(theme.typography.mono, "JetBrains Mono");
        assert_eq!(theme.shape.radius, 6.0);
        assert_eq!(theme.shape.radius_cell, 0.0);
        assert_eq!(theme.shape.label_size, 11.0);
        assert_eq!(theme.shape.crosshair, 11.0);
    }
}
