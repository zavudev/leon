//! The Zavu theme: Zavu's brand system (`zavu-brand.md` §2, §4, §5, §10).
//!
//! A monochrome page (black or paper), one Signal Violet accent that is spent
//! on focus, selection and the active marker and nowhere else (about three
//! percent of the surface), four system colours that only ever mean a state,
//! hairline rules instead of shadows, and nearly square corners. The primary
//! button and the glare are the foreground colour, never the accent. The one
//! deliberate exception to the single accent is the agents' marks, which wear
//! their brand colours. Space Grotesk carries the interface and Geist Mono
//! everything technical.
//!
//! These values are the application's original look and must not change: the
//! tests of this file pin every one of them.

use super::{
    hex, terminal_colours, Appearance, Crosshairs, Lines, Palette, Shape, Theme, Typography,
};

const VOID: u32 = 0x00_00_00;
const WHITE: u32 = 0xFF_FF_FF;
const PAPER: u32 = 0xFA_FA_FA;
const INK: u32 = 0x0A_0A_0B;

const ZINC_900: u32 = 0x18_18_1B;
const ZINC_800: u32 = 0x27_27_2A;
const ZINC_700: u32 = 0x3F_3F_46;
const ZINC_600: u32 = 0x52_52_5B;
const ZINC_500: u32 = 0x71_71_7A;
const ZINC_400: u32 = 0xA1_A1_AA;
const ZINC_300: u32 = 0xD4_D4_D8;
const ZINC_200: u32 = 0xE4_E4_E7;
const ZINC_100: u32 = 0xF4_F4_F5;

// Agent brand colours. Claude: Anthropic's clay, hex D97757 of the Claude mark
// (simple-icons `Claude`, source claude.ai); the light variant is the darker
// clay (Anthropic's "Crail" family) because D97757 is below 3:1 on paper.
// Codex: OpenAI's mark is officially monochrome, so ink on light and white on
// dark. opencode: its brand marks are monochrome too, `opencode-logo-light.svg`
// (#211E1E) on light and `opencode-logo-dark.svg` (#F1ECEC) on dark.
const CLAUDE_DARK: u32 = 0xD9_77_57;
const CLAUDE_LIGHT: u32 = 0xB8_58_3A;
const OPENCODE_DARK: u32 = 0xF1_EC_EC;
const OPENCODE_LIGHT: u32 = 0x21_1E_1E;

const SIGNAL_DARK: u32 = 0x61_5F_FF;
const SIGNAL_LIGHT: u32 = 0x43_40_C7;

const SUCCESS_DARK: u32 = 0x4D_F6_88;
const WARNING_DARK: u32 = 0xFF_DA_5E;
const ERROR_DARK: u32 = 0xFF_5E_5E;
const INFO_DARK: u32 = 0x6E_FA_FF;

const SUCCESS_LIGHT: u32 = 0x04_78_57;
const WARNING_LIGHT: u32 = 0xA1_62_07;
const ERROR_LIGHT: u32 = 0xB9_1C_1C;
const INFO_LIGHT: u32 = 0x0E_74_90;

// A session held by a process in another terminal: pink, away from the indigo
// accent, the green of a live session and the red of an error.
const ELSEWHERE_DARK: u32 = 0xF4_72_B6;
const ELSEWHERE_LIGHT: u32 = 0xA8_1F_6B;

// The terminal's ANSI colours: the four system colours stand for red, green,
// yellow and cyan so that a state means the same thing in a terminal and in the
// interface; blue and magenta stay cool and violet.
const ANSI_DARK: [u32; 16] = [
    0x3F_3F_46, 0xFF_5E_5E, 0x4D_F6_88, 0xFF_DA_5E, 0x7A_A2_FF, 0xC7_92_FF, 0x6E_FA_FF, 0xD4_D4_D8,
    0x71_71_7A, 0xFF_8A_8A, 0x86_FF_AD, 0xFF_E9_8F, 0xA5_C0_FF, 0xDD_B8_FF, 0xA5_FC_FF, 0xFA_FA_FA,
];
const ANSI_LIGHT: [u32; 16] = [
    0x27_27_2A, 0xB9_1C_1C, 0x04_78_57, 0x8A_52_06, 0x1D_4E_D8, 0x7E_22_CE, 0x0E_74_90, 0x52_52_5B,
    0x6B_6B_74, 0x9F_12_39, 0x15_6B_34, 0x7A_46_05, 0x1E_40_AF, 0x6B_21_A8, 0x0B_5E_75, 0x3F_3F_46,
];

fn light() -> Palette {
    let (background, text, signal) = (hex(PAPER), hex(INK), hex(SIGNAL_LIGHT));
    Palette {
        appearance: Appearance::Light,
        background,
        surface: hex(WHITE),
        surface_2: hex(ZINC_100),
        border: hex(ZINC_200),
        grid_mark: hex(ZINC_300),
        guide: hex(ZINC_200),
        scrim: hex(INK).opacity(0.46),
        elevated_border: hex(ZINC_400),
        text,
        text_muted: hex(ZINC_600),
        text_faint: hex(ZINC_500),
        signal,
        // The accent is never a fill in Zavu: a badge is the ink.
        accent_fill: text,
        on_accent_fill: background,
        primary_fill: text,
        on_primary: background,
        success: hex(SUCCESS_LIGHT),
        warning: hex(WARNING_LIGHT),
        error: hex(ERROR_LIGHT),
        info: hex(INFO_LIGHT),
        elsewhere: hex(ELSEWHERE_LIGHT),
        logo: text,
        agent_claude: hex(CLAUDE_LIGHT),
        agent_codex: hex(INK),
        agent_opencode: hex(OPENCODE_LIGHT),
        terminal: terminal_colours(text, background, signal, ANSI_LIGHT),
    }
}

fn dark() -> Palette {
    let (background, text, signal) = (hex(VOID), hex(WHITE), hex(SIGNAL_DARK));
    Palette {
        appearance: Appearance::Dark,
        background,
        surface: hex(ZINC_900),
        surface_2: hex(ZINC_800),
        border: hex(ZINC_800),
        grid_mark: hex(ZINC_700),
        guide: hex(ZINC_800),
        scrim: hex(VOID).opacity(0.64),
        elevated_border: hex(ZINC_700),
        text,
        text_muted: hex(ZINC_400),
        text_faint: hex(ZINC_500),
        signal,
        accent_fill: text,
        on_accent_fill: background,
        primary_fill: text,
        on_primary: background,
        success: hex(SUCCESS_DARK),
        warning: hex(WARNING_DARK),
        error: hex(ERROR_DARK),
        info: hex(INFO_DARK),
        elsewhere: hex(ELSEWHERE_DARK),
        logo: text,
        agent_claude: hex(CLAUDE_DARK),
        agent_codex: hex(WHITE),
        agent_opencode: hex(OPENCODE_DARK),
        terminal: terminal_colours(text, background, signal, ANSI_DARK),
    }
}

/// The Zavu theme's data.
pub(super) fn theme() -> Theme {
    Theme {
        light: light(),
        dark: dark(),
        typography: Typography {
            sans: "Space Grotesk",
            sans_features: &[],
            mono: "Geist Mono",
            mono_features: &[],
        },
        shape: Shape {
            radius: 2.0,
            radius_cell: 0.0,
            label_size: 10.5,
            crosshair: 9.0,
        },
        // The original look: the one crosshair under the sidebar's header and
        // no other blueprint line.
        lines: Lines {
            crosshairs: Crosshairs::Header,
            corner_ticks: false,
            guides: false,
            empty_motif: false,
            tick_length: 6.0,
            weight: 1.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::ThemeId;
    use super::*;
    use gpui_kit::{Hsla, Rgba};
    use leon_term::colors::mix;

    fn zavu(appearance: Appearance) -> Palette {
        ThemeId::Zavu.palette(appearance)
    }

    /// Every token of the original look, as it was written before themes were
    /// data. Zavu must keep these exactly.
    #[test]
    fn zavu_keeps_every_value_the_application_had_before_themes_were_data() {
        let h = |v: u32| -> Hsla { gpui_kit::rgb(v).into() };
        let tokens = |p: Palette| {
            [
                p.background,
                p.surface,
                p.surface_2,
                p.border,
                p.grid_mark,
                p.elevated_border,
                p.text,
                p.text_muted,
                p.text_faint,
                p.signal,
                p.primary_fill,
                p.on_primary,
                p.success,
                p.warning,
                p.error,
                p.info,
                p.logo,
                p.agent_claude,
                p.agent_codex,
                p.agent_opencode,
            ]
        };
        let dark = zavu(Appearance::Dark);
        assert_eq!(
            tokens(dark),
            [
                h(0x000000),
                h(0x18181B),
                h(0x27272A),
                h(0x27272A),
                h(0x3F3F46),
                h(0x3F3F46),
                h(0xFFFFFF),
                h(0xA1A1AA),
                h(0x71717A),
                h(0x615FFF),
                h(0xFFFFFF),
                h(0x000000),
                h(0x4DF688),
                h(0xFFDA5E),
                h(0xFF5E5E),
                h(0x6EFAFF),
                h(0xFFFFFF),
                h(0xD97757),
                h(0xFFFFFF),
                h(0xF1ECEC),
            ]
        );
        assert_eq!(dark.scrim, h(0x000000).opacity(0.64));
        let light = zavu(Appearance::Light);
        assert_eq!(
            tokens(light),
            [
                h(0xFAFAFA),
                h(0xFFFFFF),
                h(0xF4F4F5),
                h(0xE4E4E7),
                h(0xD4D4D8),
                h(0xA1A1AA),
                h(0x0A0A0B),
                h(0x52525B),
                h(0x71717A),
                h(0x4340C7),
                h(0x0A0A0B),
                h(0xFAFAFA),
                h(0x047857),
                h(0xA16207),
                h(0xB91C1C),
                h(0x0E7490),
                h(0x0A0A0B),
                h(0xB8583A),
                h(0x0A0A0B),
                h(0x211E1E),
            ]
        );
        assert_eq!(light.scrim, h(0x0A0A0B).opacity(0.46));
    }

    #[test]
    fn zavu_keeps_the_terminal_colours_the_application_had_before_themes_were_data() {
        let h = |v: u32| -> Hsla { gpui_kit::rgb(v).into() };
        let dark = zavu(Appearance::Dark).terminal;
        let light = zavu(Appearance::Light).terminal;
        assert_eq!(
            dark.ansi,
            [
                0x3F3F46, 0xFF5E5E, 0x4DF688, 0xFFDA5E, 0x7AA2FF, 0xC792FF, 0x6EFAFF, 0xD4D4D8,
                0x71717A, 0xFF8A8A, 0x86FFAD, 0xFFE98F, 0xA5C0FF, 0xDDB8FF, 0xA5FCFF, 0xFAFAFA,
            ]
            .map(h)
        );
        assert_eq!(
            light.ansi,
            [
                0x27272A, 0xB91C1C, 0x047857, 0x8A5206, 0x1D4ED8, 0x7E22CE, 0x0E7490, 0x52525B,
                0x6B6B74, 0x9F1239, 0x156B34, 0x7A4605, 0x1E40AF, 0x6B21A8, 0x0B5E75, 0x3F3F46,
            ]
            .map(h)
        );
        for (terminal, p) in [
            (dark, zavu(Appearance::Dark)),
            (light, zavu(Appearance::Light)),
        ] {
            assert_eq!(terminal.foreground, p.text);
            assert_eq!(terminal.background, p.background);
            assert_eq!(terminal.cursor, p.signal);
            assert_eq!(terminal.selection, mix(p.background, p.signal, 0.38));
        }
    }

    #[test]
    fn zavu_keeps_the_fonts_and_the_metrics_the_application_had_before_themes_were_data() {
        let theme = ThemeId::Zavu.theme();
        assert_eq!(theme.typography.sans, "Space Grotesk");
        assert_eq!(theme.typography.mono, "Geist Mono");
        assert!(theme.typography.sans_features.is_empty());
        assert!(theme.typography.mono_features.is_empty());
        assert_eq!(theme.shape.radius, 2.0);
        assert_eq!(theme.shape.radius_cell, 0.0);
        assert_eq!(theme.shape.label_size, 10.5);
        assert_eq!(theme.shape.crosshair, 9.0);
    }

    #[test]
    fn in_zavu_the_logo_and_the_primary_button_are_monochrome_and_never_the_accent() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let p = zavu(appearance);
            assert_eq!(p.logo, p.text, "the logo is the foreground colour");
            assert_eq!(p.primary_fill, p.text, "a primary button is the ink");
            assert_ne!(p.logo, p.signal, "the logo is never the accent");
            assert_ne!(p.accent_fill, p.signal, "the accent is never a fill");
        }
    }

    #[test]
    fn in_zavu_the_terminal_selection_is_a_violet_of_the_accent() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let t = zavu(appearance).terminal;
            assert_ne!(t.selection, t.background);
            assert!(t.selection.a > 0.999, "the selection is opaque");
            // Violet: its blue channel leads its green.
            let rgba = Rgba::from(t.selection);
            assert!(rgba.b > rgba.g, "{appearance:?}");
        }
    }

    #[test]
    fn zavu_corners_are_nearly_square() {
        for percent in super::super::SCALE_STEPS {
            super::super::set_scale(percent);
            let shape = ThemeId::Zavu.theme().shape;
            let radius = super::super::token(shape.radius).as_f32();
            assert!((0.0..=2.5).contains(&radius), "{percent}%: {radius}");
        }
        super::super::set_scale(100);
    }
}
