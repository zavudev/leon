//! What every theme must meet, checked for all of them in both appearances.
//!
//! The rules that belong to one theme (Zavu's single accent, Leon's filled
//! primary button) are tested in that theme's file. A theme added to
//! [`ThemeId::ALL`] is held to everything here, and fails these tests until its
//! data is complete.

use super::*;
use gpui_kit::Rgba;

/// WCAG 2.x contrast ratio between two opaque colours.
pub(super) fn contrast(a: Hsla, b: Hsla) -> f32 {
    fn luminance(colour: Hsla) -> f32 {
        let rgba = Rgba::from(colour);
        assert!(rgba.a > 0.999, "contrast needs opaque colours");
        let linear = |c: f32| {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
    }
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// CIE76 colour difference between two opaque colours: how far apart they look,
/// whatever their luminance (an orange and a yellow can be 1.5:1 in contrast and
/// still be two colours).
pub(super) fn colour_difference(a: Hsla, b: Hsla) -> f32 {
    fn lab(colour: Hsla) -> [f32; 3] {
        let rgba = Rgba::from(colour);
        let linear = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(rgba.r), linear(rgba.g), linear(rgba.b));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f32| {
            if t > 0.008856 {
                t.cbrt()
            } else {
                7.787 * t + 16. / 116.
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
    }
    let (a, b) = (lab(a), lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Every theme in both appearances, with a name to put in a failure.
fn all() -> Vec<(String, Palette)> {
    ThemeId::ALL
        .into_iter()
        .flat_map(|id| {
            [Appearance::Light, Appearance::Dark]
                .into_iter()
                .map(move |appearance| {
                    (
                        format!("{} {appearance:?}", id.name()),
                        id.palette(appearance),
                    )
                })
        })
        .collect()
}

#[test]
fn text_stays_legible_on_every_surface_in_every_theme() {
    const TEXT: f32 = 4.5;
    for (theme, p) in all() {
        let pairs = [
            ("text on the page", p.text, p.background),
            ("text on a card", p.text, p.surface),
            ("text on the open row", p.text, p.surface_2),
            ("muted text on the page", p.text_muted, p.background),
            ("muted text on a card", p.text_muted, p.surface),
            ("muted text on the open row", p.text_muted, p.surface_2),
            ("primary button label", p.on_primary, p.primary_fill),
            ("label on an accent fill", p.on_accent_fill, p.accent_fill),
            ("accent text on the page", p.signal, p.background),
            ("error text on the page", p.error, p.background),
            ("error text on a card", p.error, p.surface),
            ("warning text on the page", p.warning, p.background),
            ("warning text on a card", p.warning, p.surface),
            ("success text on the page", p.success, p.background),
            ("info text on the page", p.info, p.background),
            ("elsewhere text on the page", p.elsewhere, p.background),
        ];
        for (name, foreground, background) in pairs {
            let ratio = contrast(foreground, background);
            assert!(
                ratio >= TEXT,
                "{theme}: {name} is {ratio:.2}:1, below {TEXT}:1"
            );
        }
    }
}

#[test]
fn glyphs_and_marks_reach_the_graphics_contrast_in_every_theme() {
    const GRAPHIC: f32 = 3.0;
    for (theme, p) in all() {
        let pairs = [
            ("faint glyph on the page", p.text_faint, p.background),
            ("faint glyph on the open row", p.text_faint, p.surface_2),
            ("accent as a graphic on the page", p.signal, p.background),
            ("accent as a graphic on the open row", p.signal, p.surface_2),
            ("focus ring on the page", p.signal, p.background),
            ("focus ring on a card", p.signal, p.surface),
            ("status LED, success", p.success, p.background),
            ("status LED, error", p.error, p.background),
            ("status LED, warning", p.warning, p.background),
            ("status LED, info", p.info, p.background),
            ("status LED, elsewhere", p.elsewhere, p.background),
            ("the logo on the page", p.logo, p.background),
            ("lifted outline on a card", p.elevated_border, p.surface),
        ];
        for (name, foreground, background) in pairs {
            let ratio = contrast(foreground, background);
            let wanted = if name.starts_with("lifted") {
                1.2
            } else {
                GRAPHIC
            };
            assert!(
                ratio >= wanted,
                "{theme}: {name} is {ratio:.2}:1, below {wanted}:1"
            );
        }
    }
}

#[test]
fn the_accent_and_the_four_states_are_pairwise_distinguishable_in_every_theme() {
    // CIE76 distance: 2.3 is just noticeable, 20 is plainly another colour.
    // Every pair among the accent, success, warning, error and info is held to
    // it, so no state can be taken for another or for the accent.
    const APART: f32 = 20.0;
    for (theme, p) in all() {
        let colours = [
            ("accent", p.signal),
            ("success", p.success),
            ("warning", p.warning),
            ("error", p.error),
            ("info", p.info),
            ("elsewhere", p.elsewhere),
        ];
        for (i, (a, ca)) in colours.iter().enumerate() {
            for (b, cb) in &colours[i + 1..] {
                let apart = colour_difference(*ca, *cb);
                assert!(
                    apart >= APART,
                    "{theme}: {a} and {b} are {apart:.1} apart, below {APART}"
                );
            }
        }
    }
}

#[test]
fn the_accent_fill_and_the_text_on_it_work_in_every_theme() {
    for (theme, p) in all() {
        let ratio = contrast(p.on_accent_fill, p.accent_fill);
        assert!(
            ratio >= 7.0,
            "{theme}: label on the accent fill {ratio:.2}:1"
        );
        let ratio = contrast(p.on_primary, p.primary_fill);
        assert!(
            ratio >= 7.0,
            "{theme}: label on the primary fill {ratio:.2}:1"
        );
    }
}

#[test]
fn text_stays_legible_on_the_accent_tint_of_a_text_field_selection_in_every_theme() {
    for (theme, p) in all() {
        let selected = mix(p.background, p.signal, FIELD_SELECTION);
        let ratio = contrast(p.text, selected);
        assert!(ratio >= 4.5, "{theme}: text on a selection {ratio:.2}:1");
    }
}

#[test]
fn each_agent_maps_to_its_own_token_in_every_theme() {
    use leon_core::AgentKind;
    for (theme, p) in all() {
        assert_eq!(p.agent(AgentKind::Claude), p.agent_claude, "{theme}");
        assert_eq!(p.agent(AgentKind::Codex), p.agent_codex, "{theme}");
        assert_eq!(p.agent(AgentKind::Opencode), p.agent_opencode, "{theme}");
        assert_ne!(p.agent_claude, p.agent_codex, "{theme}");
        assert_ne!(p.agent_claude, p.agent_opencode, "{theme}");
    }
}

#[test]
fn agent_marks_reach_the_component_contrast_on_every_surface_in_every_theme() {
    const GRAPHIC: f32 = 3.0;
    for (theme, p) in all() {
        for agent in leon_core::AgentKind::ALL {
            let mark = p.agent(agent);
            for (surface, name) in [
                (p.background, "the page"),
                (p.surface, "a card"),
                (p.surface_2, "the selected row"),
            ] {
                let ratio = contrast(mark, surface);
                assert!(
                    ratio >= GRAPHIC,
                    "{theme}: {agent:?} on {name} is {ratio:.2}:1, below {GRAPHIC}:1"
                );
            }
        }
    }
}

#[test]
fn the_veil_lets_the_page_show_through_and_darkens_it_in_every_theme() {
    for (theme, p) in all() {
        assert!(p.scrim.a > 0.3 && p.scrim.a < 0.9, "{theme}");
    }
}

#[test]
fn the_terminal_page_is_the_pages_colour_and_its_text_the_interfaces_in_every_theme() {
    for (theme, p) in all() {
        assert_eq!(p.terminal.background, p.background, "{theme}");
        assert_eq!(p.terminal.foreground, p.text, "{theme}");
        assert_eq!(p.terminal.cursor, p.signal, "{theme}");
        assert_ne!(p.terminal.selection, p.terminal.background, "{theme}");
        assert!(p.terminal.selection.a > 0.999, "{theme}: opaque selection");
    }
}

#[test]
fn terminal_text_is_legible_on_the_terminal_page_in_every_theme() {
    for (theme, p) in all() {
        let t = p.terminal;
        assert!(contrast(t.foreground, t.background) >= 7.0, "{theme} text");
        assert!(contrast(t.cursor, t.background) >= 3.0, "{theme} cursor");
        assert!(
            contrast(t.foreground, t.selection) >= 4.5,
            "{theme} text on the selection"
        );
        // Black and white (0 and 7, and their bright twins) are used as text as
        // well as backgrounds; the rest are used as text.
        for (i, colour) in t.ansi.iter().enumerate() {
            let wanted = match i {
                0 => 1.5, // black: mostly a background colour
                8 => 3.5, // bright black: comments and hints
                _ => 4.5,
            };
            let ratio = contrast(*colour, t.background);
            assert!(
                ratio >= wanted,
                "{theme} ansi {i} is {ratio:.2}:1, below {wanted}"
            );
        }
    }
}

#[test]
fn the_ansi_colours_are_sixteen_distinct_colours_in_every_theme() {
    for (theme, p) in all() {
        let ansi = p.terminal.ansi;
        for i in 0..16 {
            for j in (i + 1)..16 {
                assert_ne!(ansi[i], ansi[j], "{theme}: {i} and {j}");
            }
        }
    }
}

#[test]
fn the_terminal_reds_greens_and_cyans_are_the_state_colours_in_every_theme() {
    for (theme, p) in all() {
        let ansi = p.terminal.ansi;
        assert_eq!(ansi[1], p.error, "{theme}");
        assert_eq!(ansi[2], p.success, "{theme}");
        assert_eq!(ansi[6], p.info, "{theme}");
    }
}

#[test]
fn every_theme_defines_every_token_of_its_own_with_nothing_borrowed_from_another() {
    for (theme, p) in all() {
        // Every token but the veil is opaque: a token left at a default would
        // be transparent black.
        for (name, colour) in [
            ("background", p.background),
            ("surface", p.surface),
            ("surface_2", p.surface_2),
            ("border", p.border),
            ("grid_mark", p.grid_mark),
            ("elevated_border", p.elevated_border),
            ("text", p.text),
            ("text_muted", p.text_muted),
            ("text_faint", p.text_faint),
            ("signal", p.signal),
            ("accent_fill", p.accent_fill),
            ("on_accent_fill", p.on_accent_fill),
            ("primary_fill", p.primary_fill),
            ("on_primary", p.on_primary),
            ("success", p.success),
            ("warning", p.warning),
            ("error", p.error),
            ("info", p.info),
            ("elsewhere", p.elsewhere),
            ("logo", p.logo),
            ("agent_claude", p.agent_claude),
            ("agent_codex", p.agent_codex),
            ("agent_opencode", p.agent_opencode),
        ] {
            assert!(Rgba::from(colour).a > 0.999, "{theme}: {name} is not set");
        }
        assert!(p.scrim.a > 0.0, "{theme}: the veil is not set");
    }
    // The palette of an id is that theme's own, in the appearance asked for.
    for id in ThemeId::ALL {
        let theme = id.theme();
        assert_eq!(theme.light.appearance, Appearance::Light, "{id:?}");
        assert_eq!(theme.dark.appearance, Appearance::Dark, "{id:?}");
        assert!(!theme.typography.sans.is_empty(), "{id:?}");
        assert!(!theme.typography.mono.is_empty(), "{id:?}");
        assert!(theme.shape.label_size > 0.0, "{id:?}");
    }
    // No theme is another one under a new name.
    for a in ThemeId::ALL {
        for b in ThemeId::ALL.into_iter().filter(|b| *b != a) {
            for appearance in [Appearance::Light, Appearance::Dark] {
                assert_ne!(
                    a.palette(appearance).signal,
                    b.palette(appearance).signal,
                    "{a:?} and {b:?} share an accent"
                );
            }
        }
    }
}

#[test]
fn the_light_and_dark_palettes_of_a_theme_are_different_looks() {
    for id in ThemeId::ALL {
        let (light, dark) = (id.palette(Appearance::Light), id.palette(Appearance::Dark));
        assert_ne!(light.background, dark.background, "{id:?}");
        assert_ne!(light.text, dark.text, "{id:?}");
        assert!(contrast(light.background, dark.background) > 10.0, "{id:?}");
    }
}

#[test]
fn theme_ids_are_unique_and_stable() {
    // These are written to settings files and typed on the command line: a
    // release must not change them.
    let slugs: Vec<&str> = ThemeId::ALL.iter().map(|id| id.slug()).collect();
    assert_eq!(slugs, ["leon", "zavu"]);
    let names: Vec<&str> = ThemeId::ALL.iter().map(|id| id.name()).collect();
    assert_eq!(names, ["Leon", "Zavu"]);
    let mut unique = slugs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ThemeId::ALL.len());
    for id in ThemeId::ALL {
        assert_eq!(ThemeId::parse(id.slug()), Some(id));
        assert_eq!(ThemeId::parse(id.name()), Some(id), "{id:?} by its name");
    }
}

#[test]
fn the_default_theme_is_leon() {
    assert_eq!(ThemeId::DEFAULT, ThemeId::Leon);
    assert_eq!(ThemeId::default(), ThemeId::Leon);
}

#[test]
fn a_theme_is_read_in_any_case_with_dashes_dots_spaces_or_underscores() {
    assert_eq!(ThemeId::parse("LEON"), Some(ThemeId::Leon));
    assert_eq!(ThemeId::parse(" Leon "), Some(ThemeId::Leon));
    assert_eq!(ThemeId::parse("leon-lime"), None);
    assert_eq!(ThemeId::parse("leon-bone"), None);
    assert_eq!(ThemeId::parse("Zavu"), Some(ThemeId::Zavu));
    assert_eq!(ThemeId::parse("solarized"), None);
    assert_eq!(ThemeId::parse(""), None);
}

#[test]
fn stepping_through_the_themes_wraps_in_both_directions() {
    assert_eq!(ThemeId::Leon.step(true), ThemeId::Zavu);
    assert_eq!(ThemeId::Zavu.step(true), ThemeId::Leon);
    assert_eq!(ThemeId::Leon.step(false), ThemeId::Zavu);
    assert_eq!(ThemeId::Zavu.step(false), ThemeId::Leon);
}

#[test]
fn an_unknown_theme_id_in_json_is_the_default_and_a_known_one_round_trips() {
    let unknown: ThemeId = serde_json::from_str("\"solarized\"").unwrap();
    assert_eq!(unknown, ThemeId::DEFAULT);
    for id in ThemeId::ALL {
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.slug()));
        assert_eq!(serde_json::from_str::<ThemeId>(&json).unwrap(), id);
    }
}

#[test]
fn the_retired_candidate_themes_in_saved_settings_fall_back_to_leon_without_error() {
    for retired in ["leon-lime", "leon-bone"] {
        let id: ThemeId = serde_json::from_str(&format!("\"{retired}\"")).unwrap();
        assert_eq!(id, ThemeId::Leon, "{retired}");
    }
}

#[test]
fn the_registry_lists_exactly_leon_and_zavu() {
    assert_eq!(ThemeId::ALL, [ThemeId::Leon, ThemeId::Zavu]);
}

#[test]
fn every_font_family_a_theme_names_is_bundled() {
    // A family's name is stored in its font file, as ASCII (Mac records) or as
    // UTF-16 (Windows records).
    let named_in = |bytes: &[u8], name: &str| {
        let ascii = name.as_bytes();
        let utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|window| window == needle);
        has(ascii) || has(&utf16)
    };
    let fonts = brand::fonts();
    for id in ThemeId::ALL {
        let typography = id.theme().typography;
        for family in [typography.sans, typography.mono] {
            assert!(
                fonts.iter().any(|bytes| named_in(bytes, family)),
                "{id:?}: {family} is not bundled"
            );
        }
    }
}

#[test]
fn the_label_and_the_crosshair_follow_the_theme_in_use() {
    let global = |id: ThemeId| LOOK.with(|look| look.set(Some(Look::from(&id.theme()))));
    set_scale(100);
    global(ThemeId::Zavu);
    assert_eq!(metrics::TEXT_LABEL().as_f32(), 10.5);
    assert_eq!(metrics::RADIUS().as_f32(), 2.0);
    assert_eq!(metrics::CROSSHAIR().as_f32(), 9.0);
    assert_eq!(fonts::sans(), "Space Grotesk");
    assert_eq!(fonts::mono(), "Geist Mono");
    global(ThemeId::Leon);
    assert_eq!(metrics::TEXT_LABEL().as_f32(), 11.0);
    assert_eq!(metrics::RADIUS().as_f32(), 6.0);
    assert_eq!(metrics::RADIUS_CELL().as_f32(), 0.0);
    assert_eq!(metrics::CROSSHAIR().as_f32(), 11.0);
    assert_eq!(fonts::sans(), "Inter");
    assert_eq!(fonts::mono(), "JetBrains Mono");
    assert_eq!(
        fonts::mono_features().tag_value_list(),
        &[("calt".to_owned(), 0)]
    );
    assert!(fonts::sans_features().tag_value_list().is_empty());
    LOOK.with(|look| look.set(None));
}

#[test]
fn the_scale_snaps_to_the_steps_on_offer() {
    assert_eq!(nearest_step(100), 100);
    assert_eq!(nearest_step(104), 100);
    assert_eq!(nearest_step(115), 110);
    assert_eq!(nearest_step(500), 125);
    assert_eq!(nearest_step(0), 80);
}

#[test]
fn stepping_the_scale_stops_at_both_ends() {
    assert_eq!(next_step(100, true), 110);
    assert_eq!(next_step(100, false), 90);
    assert_eq!(next_step(125, true), 125);
    assert_eq!(next_step(80, false), 80);
}

#[test]
fn sizes_follow_the_scale_but_hairlines_do_not() {
    set_scale(125);
    assert!(px(10.0).as_f32() > 12.0);
    assert_eq!(hairline().as_f32(), 1.0);
    set_scale(100);
    assert_eq!(px(10.0).as_f32(), 10.0);
}

/// `cargo test -p leon print_contrast_table -- --ignored --nocapture`.
#[test]
#[ignore = "prints the measured ratios of every theme; not a check"]
fn print_contrast_table() {
    for (theme, p) in all() {
        println!(
            "{theme:<18} accent/page {:5.2}  accent/card {:5.2}  text/page {:5.2}  muted/page {:5.2}  faint/page {:5.2}  warning/page {:5.2}  accent~warning dE {:5.1}  fill/ink {:5.2}",
            contrast(p.signal, p.background),
            contrast(p.signal, p.surface),
            contrast(p.text, p.background),
            contrast(p.text_muted, p.background),
            contrast(p.text_faint, p.background),
            contrast(p.warning, p.background),
            colour_difference(p.signal, p.warning),
            contrast(p.on_accent_fill, p.accent_fill),
        );
    }
}

#[test]
fn the_glare_is_legible_in_every_theme_and_appearance() {
    // The glare is one silhouette in the `logo` colour; its cuts show the
    // page, so the one pair that matters is the mark against the page.
    for (theme, p) in all() {
        let ratio = contrast(p.logo, p.background);
        assert!(
            ratio >= 3.0,
            "{theme}: the glare on the page is {ratio:.2}:1"
        );
    }
}

#[test]
fn every_theme_defines_its_line_group_in_both_appearances() {
    for (theme, p) in all() {
        for (name, colour) in [("grid_mark", p.grid_mark), ("guide", p.guide)] {
            assert!(Rgba::from(colour).a > 0.999, "{theme}: {name} is not set");
        }
    }
    for id in ThemeId::ALL {
        let lines = id.theme().lines;
        assert!(
            lines.weight == 1.0 || lines.weight == 2.0,
            "{id:?}: weight {}",
            lines.weight
        );
        assert!(
            lines.tick_length >= 3.0 && lines.tick_length <= 24.0,
            "{id:?}: tick length {}",
            lines.tick_length
        );
        let arm = id.theme().shape.crosshair;
        assert!((5.0..=31.0).contains(&arm), "{id:?}: crosshair arm {arm}");
    }
}

#[test]
fn line_colours_sit_in_the_quiet_band_against_their_page_in_every_theme() {
    // Quiet band: a line is visible but never competes with content.
    //   rules (`border`)        1.1 to 2.0 : 1
    //   guides                  1.1 to 2.0 : 1, and quieter than the crosshair
    //   crosshair and ticks     1.3 to below 3.0 : 1, the graphics threshold
    // Text starts at 4.5:1 and graphics at 3:1, so every line is below both.
    for (theme, p) in all() {
        let rule = contrast(p.border, p.background);
        let guide = contrast(p.guide, p.background);
        let mark = contrast(p.grid_mark, p.background);
        assert!((1.1..=2.0).contains(&rule), "{theme}: rule {rule:.2}:1");
        assert!((1.1..=2.0).contains(&guide), "{theme}: guide {guide:.2}:1");
        assert!((1.3..3.0).contains(&mark), "{theme}: crosshair {mark:.2}:1");
        assert!(
            guide <= mark,
            "{theme}: the guide is the quietest after the rule"
        );
        // They stay visible on a card as well.
        assert!(
            contrast(p.grid_mark, p.surface) >= 1.2,
            "{theme}: on a card"
        );
    }
}

#[test]
fn zavu_keeps_its_one_crosshair_and_leon_draws_the_whole_line_system() {
    let zavu = ThemeId::Zavu.theme().lines;
    assert_eq!(zavu.crosshairs, Crosshairs::Header);
    assert!(!zavu.corner_ticks && !zavu.guides && !zavu.empty_motif);
    let leon = ThemeId::Leon.theme().lines;
    assert_eq!(leon.crosshairs, Crosshairs::All);
    assert!(leon.corner_ticks && leon.guides && leon.empty_motif);
}

#[test]
fn the_footer_is_one_rule_across_the_window_only_where_the_theme_asks_for_guides() {
    set_scale(100);
    LOOK.with(|look| look.set(Some(Look::from(&ThemeId::Zavu.theme()))));
    assert_eq!(metrics::FOOTER_HEIGHT(), metrics::STATUS_HEIGHT());
    LOOK.with(|look| look.set(Some(Look::from(&ThemeId::Leon.theme()))));
    assert_eq!(metrics::FOOTER_HEIGHT(), metrics::TOOLS_HEIGHT());
    assert_eq!(metrics::TOOLS_HEIGHT().as_f32(), 41.0);
    LOOK.with(|look| look.set(None));
}

#[test]
fn find_matches_keep_terminal_text_legible_in_every_theme() {
    for (theme, p) in all() {
        let t = p.terminal;
        // Ordinary text on a match, and the page's colour (what the current
        // match draws its text in) on the current one.
        assert!(
            contrast(t.foreground, t.find_match) >= 4.5,
            "{theme}: text on a match {:.2}:1",
            contrast(t.foreground, t.find_match)
        );
        assert!(
            contrast(t.background, t.find_match_current) >= 4.5,
            "{theme}: text on the current match {:.2}:1",
            contrast(t.background, t.find_match_current)
        );
        // Coloured text stays readable on a match (the ANSI colours but the
        // two blacks, which are mostly backgrounds).
        for (i, colour) in t
            .ansi
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 0 && *i != 8)
        {
            assert!(
                contrast(*colour, t.find_match) >= 3.0,
                "{theme}: ansi {i} on a match {:.2}:1",
                contrast(*colour, t.find_match)
            );
        }
        // A match is not the page, the selection or the current match.
        assert!(
            colour_difference(t.find_match, t.background) >= 8.0,
            "{theme}: vs page"
        );
        assert!(
            colour_difference(t.find_match, t.selection) >= 2.3,
            "{theme}: vs selection (a just-noticeable step)"
        );
        assert!(
            colour_difference(t.find_match, t.find_match_current) >= 20.0,
            "{theme}: the current match is another colour"
        );
        assert!(Rgba::from(t.find_match).a > 0.999 && Rgba::from(t.find_match_current).a > 0.999);
    }
}
