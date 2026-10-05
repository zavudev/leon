//! Themes from files: parsing, inheritance, derivation, the rules, the
//! registry, the reference and the example. Everything works on strings and
//! temporary directories: nothing reads the real configuration directory.

use super::check::{colour_difference, contrast, Problem};
use super::file::{parse, resolve_all, Env, Loaded};
use super::registry::{self, ThemeId};
use super::tokens;
use super::{Appearance, Palette};
use gpui_kit::Rgba;

fn fonts(name: &str) -> bool {
    ["Inter", "JetBrains Mono", "Space Grotesk", "Geist Mono"].contains(&name)
}

fn load(files: &[(&str, &str)]) -> Vec<Loaded> {
    let raws = files.iter().map(|(name, text)| parse(name, text)).collect();
    resolve_all(raws, &Env { has_font: &fonts })
}

fn one(text: &str) -> Loaded {
    load(&[("t.toml", text)]).remove(0)
}

fn errors(item: &Loaded) -> Vec<&Problem> {
    item.problems.iter().filter(|p| p.is_error()).collect()
}

fn first_error(item: &Loaded) -> String {
    errors(item)
        .first()
        .map(|p| p.summary())
        .unwrap_or_else(|| "(no error)".to_owned())
}

fn bytes(colour: gpui_kit::Hsla) -> (u8, u8, u8, u8) {
    let c = Rgba::from(colour);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    (q(c.r), q(c.g), q(c.b), q(c.a))
}

fn p(item: &Loaded, appearance: Appearance) -> Palette {
    item.theme.expect("the theme loaded").palette(appearance)
}

const MINIMAL: &str = "id = \"mine\"\nname = \"Mine\"\nextends = \"leon\"\n";

#[test]
fn a_file_that_only_extends_is_its_parent_under_a_new_name() {
    let item = one(MINIMAL);
    assert!(item.is_valid(), "{:?}", item.problems);
    assert_eq!(item.id, "mine");
    assert_eq!(item.name, "Mine");
    let leon = ThemeId::Leon.theme();
    let theme = item.theme.unwrap();
    assert_eq!(bytes(theme.dark.signal), bytes(leon.dark.signal));
    assert_eq!(theme.shape, leon.shape);
    assert_eq!(theme.lines, leon.lines);
}

#[test]
fn overriding_one_token_changes_that_token_and_what_follows_the_accent() {
    let item = one(&format!("{MINIMAL}[dark]\naccent = \"#4DA3FF\"\n"));
    assert!(item.is_valid(), "{:?}", item.problems);
    let (dark, light) = (p(&item, Appearance::Dark), p(&item, Appearance::Light));
    let leon = ThemeId::Leon;
    assert_eq!(bytes(dark.signal), (0x4D, 0xA3, 0xFF, 255));
    // Derived from the accent, as Leon derives them.
    assert_eq!(dark.terminal.cursor, dark.signal);
    assert_eq!(dark.terminal.find_match_current, dark.signal);
    assert_eq!(dark.logo, dark.signal);
    assert_eq!(dark.primary_fill, dark.signal);
    assert_eq!(dark.accent_fill, dark.signal);
    assert_ne!(
        bytes(dark.terminal.selection),
        bytes(leon.palette(Appearance::Dark).terminal.selection)
    );
    assert!(contrast(dark.on_primary, dark.primary_fill) >= 4.5);
    // The other appearance is untouched.
    assert_eq!(
        bytes(light.signal),
        bytes(leon.palette(Appearance::Light).signal)
    );
    assert_eq!(
        bytes(light.logo),
        bytes(leon.palette(Appearance::Light).logo)
    );
    // An explicit token beats the derivation.
    let set = one(&format!(
        "{MINIMAL}[dark]\naccent = \"#4DA3FF\"\nlogo = \"#FFFFFF\"\n[dark.terminal]\ncursor = \"#FF00FF\"\n"
    ));
    let dark = p(&set, Appearance::Dark);
    assert_eq!(bytes(dark.logo), (255, 255, 255, 255));
    assert_eq!(bytes(dark.terminal.cursor), (255, 0, 255, 255));
}

#[test]
fn a_chain_of_two_inherits_through_both() {
    let items = load(&[
        (
            "a.toml",
            "id = \"a\"\nname = \"A\"\nextends = \"leon\"\n[dark]\naccent = \"#4DA3FF\"\n[shape]\nradius = 10\n",
        ),
        (
            "b.toml",
            "id = \"b\"\nname = \"B\"\nextends = \"a\"\n[shape]\nradius_cell = 4.0\n",
        ),
    ]);
    assert!(
        items.iter().all(Loaded::is_valid),
        "{:?}",
        items.iter().map(|i| &i.problems).collect::<Vec<_>>()
    );
    let b = items[1].theme.unwrap();
    assert_eq!(b.shape.radius, 10.0, "from a");
    assert_eq!(b.shape.radius_cell, 4.0, "its own");
    assert_eq!(bytes(b.dark.signal), (0x4D, 0xA3, 0xFF, 255), "a's accent");
}

#[test]
fn a_parent_may_be_listed_after_its_child() {
    let items = load(&[
        ("b.toml", "id = \"b\"\nextends = \"a\"\n"),
        ("a.toml", "id = \"a\"\nextends = \"zavu\"\n"),
    ]);
    assert!(items.iter().all(Loaded::is_valid));
    assert_eq!(items[0].theme.unwrap().shape, ThemeId::Zavu.theme().shape);
}

#[test]
fn a_cycle_is_rejected_for_every_file_in_it_and_says_how_it_goes_round() {
    let items = load(&[
        ("a.toml", "id = \"a\"\nextends = \"b\"\n"),
        ("b.toml", "id = \"b\"\nextends = \"a\"\n"),
        ("c.toml", "id = \"c\"\nextends = \"c\"\n"),
    ]);
    for item in &items {
        assert!(!item.is_valid(), "{}", item.id);
    }
    let text: String = items
        .iter()
        .flat_map(|i| i.problems.iter().map(Problem::line))
        .collect();
    assert!(text.contains("cannot extend itself"), "{text}");
    assert!(
        text.contains("a -> b -> a") || text.contains("b -> a -> b"),
        "{text}"
    );
    assert!(text.contains("c -> c"), "{text}");
}

#[test]
fn a_missing_parent_is_an_error_with_a_suggestion_for_a_near_miss() {
    let item = one("id = \"x\"\nextends = \"leen\"\n");
    assert!(!item.is_valid());
    let error = first_error(&item);
    assert!(
        error.contains("no theme `leen`") && error.contains("did you mean `leon`"),
        "{error}"
    );
}

#[test]
fn a_file_cannot_take_a_built_in_id() {
    for id in ["leon", "zavu"] {
        let item = one(&format!("id = \"{id}\"\nextends = \"leon\"\n"));
        assert!(!item.is_valid());
        let error = first_error(&item);
        assert!(error.contains("built-in") && error.contains(id), "{error}");
        assert_ne!(item.id, id, "it is not listed as the built-in");
    }
}

#[test]
fn two_files_with_one_id_are_told_apart() {
    let items = load(&[
        ("a.toml", "id = \"same\"\nextends = \"leon\"\n"),
        ("b.toml", "id = \"same\"\nextends = \"leon\"\n"),
    ]);
    assert!(items[0].is_valid());
    assert!(!items[1].is_valid());
    assert!(first_error(&items[1]).contains("already the id of a.toml"));
}

#[test]
fn a_file_that_is_not_toml_is_one_error_and_no_panic() {
    let item = one("id = \"x\"\n[dark\naccent = ");
    assert!(!item.is_valid());
    assert!(first_error(&item).contains("not valid TOML"));
}

#[test]
fn a_file_without_an_id_is_invalid_and_is_listed_by_its_name() {
    let item = one("extends = \"leon\"\n");
    assert!(!item.is_valid());
    assert!(first_error(&item).contains("no `id`"));
    assert_eq!(item.id, "t");
}

#[test]
fn a_standalone_theme_has_to_set_every_token_and_says_which_are_missing() {
    let item = one("id = \"bare\"\n[dark]\naccent = \"#4DA3FF\"\n");
    assert!(!item.is_valid());
    let error = first_error(&item);
    assert!(
        error.contains("without `extends`") && error.contains("dark.background"),
        "{error}"
    );
}

#[test]
fn unknown_keys_are_warnings_with_a_suggestion_and_do_not_stop_the_theme() {
    let item = one(&format!("{MINIMAL}[dark]\naccnt = \"#4DA3FF\"\nbackgroud = \"#000000\"\n[shape]\nradus = 3\n[lines]\nweigth = 1\n"));
    assert!(item.is_valid(), "{:?}", item.problems);
    let text: String = item
        .problems
        .iter()
        .map(Problem::line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("dark.accnt") && text.contains("did you mean `accent`"),
        "{text}"
    );
    assert!(text.contains("did you mean `background`"), "{text}");
    assert!(text.contains("did you mean `radius`"), "{text}");
    assert!(text.contains("did you mean `weight`"), "{text}");
    // A top-level typo too.
    let top = one(&format!("{MINIMAL}[dak]\n"));
    assert!(top
        .problems
        .iter()
        .any(|p| p.summary().contains("did you mean `dark`")));
}

#[test]
fn a_bad_colour_is_an_error_naming_the_key() {
    for bad in ["red", "#12", "#GGGGGG", "123456"] {
        let item = one(&format!("{MINIMAL}[dark]\naccent = \"{bad}\"\n"));
        assert!(!item.is_valid(), "{bad}");
        let error = first_error(&item);
        assert!(error.starts_with("dark.accent:"), "{error}");
    }
    let wrong = one(&format!("{MINIMAL}[dark]\naccent = 5\n"));
    assert!(first_error(&wrong).contains("must be a colour"));
}

#[test]
fn only_the_veil_may_be_translucent() {
    let veil = one(&format!("{MINIMAL}[dark]\nscrim = \"#000000A0\"\n"));
    assert!(veil.is_valid(), "{:?}", veil.problems);
    let other = one(&format!("{MINIMAL}[dark]\nsurface = \"#141414A0\"\n"));
    assert!(first_error(&other).contains("opaque"));
}

#[test]
fn numbers_out_of_their_range_and_unknown_choices_are_errors() {
    for (table, key, value) in [
        ("shape", "radius", "99"),
        ("shape", "label_size", "2"),
        ("lines", "tick_length", "100"),
        ("lines", "weight", "3"),
        ("lines", "weight", "1.5"),
        ("lines", "crosshairs", "\"everywhere\""),
        ("lines", "corner_ticks", "\"yes\""),
    ] {
        let item = one(&format!("{MINIMAL}[{table}]\n{key} = {value}\n"));
        assert!(!item.is_valid(), "{key} = {value}");
        assert!(
            first_error(&item).starts_with(&format!("{table}.{key}:")),
            "{}",
            first_error(&item)
        );
    }
    let near = one(&format!("{MINIMAL}[lines]\ncrosshairs = \"al\"\n"));
    assert!(
        first_error(&near).contains("did you mean `all`"),
        "{}",
        first_error(&near)
    );
}

#[test]
fn text_that_cannot_be_read_is_an_error_with_what_was_measured_and_required() {
    let item = one(&format!("{MINIMAL}[dark]\ntext = \"#2A2A2A\"\n"));
    assert!(!item.is_valid());
    let problem = errors(&item)
        .into_iter()
        .find(|p| p.key == "dark.text")
        .expect("a text error");
    assert!(problem.measured.unwrap() < 4.5);
    assert_eq!(problem.required, Some(4.5));
    assert!(problem.line().contains("measured") && problem.line().contains("required 4.50"));
}

#[test]
fn each_rule_of_legibility_has_a_failing_fixture() {
    // (what is set, the key that must be reported)
    for (table, setting, key) in [
        ("dark", "text_muted = \"#202020\"", "dark.text_muted"),
        ("dark", "accent = \"#111111\"", "dark.accent"),
        (
            "dark",
            "primary_fill = \"#FFFFFF\"\non_primary = \"#F5F5F5\"",
            "dark.on_primary",
        ),
        ("dark", "success = \"#101010\"", "dark.success"),
        ("light", "text = \"#F0F0F0\"", "light.text"),
        (
            "dark.terminal",
            "foreground = \"#1F1F1F\"",
            "dark.terminal.foreground",
        ),
        (
            "dark.terminal",
            "cursor = \"#0B0B0B\"",
            "dark.terminal.cursor",
        ),
        (
            "dark.terminal",
            "find_match = \"#FAFAF9\"",
            "dark.terminal.find_match",
        ),
    ] {
        let item = one(&format!("{MINIMAL}[{table}]\n{setting}\n"));
        assert!(!item.is_valid(), "{key}");
        assert!(
            errors(&item).iter().any(|p| p.key == key),
            "{key} is reported; found {:?}",
            errors(&item).iter().map(|p| &p.key).collect::<Vec<_>>()
        );
    }
}

#[test]
fn an_ansi_colour_that_vanishes_on_the_terminal_page_is_an_error_and_the_list_must_have_sixteen() {
    let mut ansi: Vec<String> = ThemeId::Leon
        .palette(Appearance::Dark)
        .terminal
        .ansi
        .iter()
        .map(|c| tokens::colour_text(*c))
        .collect();
    ansi[4] = "#0D0D0D".to_owned();
    let list = ansi
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let item = one(&format!("{MINIMAL}[dark.terminal]\nansi = [{list}]\n"));
    assert!(!item.is_valid());
    assert!(errors(&item)
        .iter()
        .any(|p| p.key == "dark.terminal.ansi" && p.message.contains("ANSI colour 4")));
    let short = one(&format!("{MINIMAL}[dark.terminal]\nansi = [\"#FFFFFF\"]\n"));
    assert!(first_error(&short).contains("sixteen"));
}

#[test]
fn the_accent_and_the_states_must_be_told_apart() {
    let item = one(&format!("{MINIMAL}[dark]\naccent = \"#4DF688\"\n"));
    assert!(!item.is_valid());
    let error = first_error(&item);
    assert!(error.contains("could be taken for one another"), "{error}");
}

#[test]
fn stricter_numbers_than_the_errors_are_warnings_and_the_theme_is_still_applied() {
    // Muted text between 4.5 and the built-in's bar is fine; a quiet-band
    // line outside its band is a warning.
    let item = one(&format!(
        "{MINIMAL}[dark]\nguide = \"#000000\"\ngrid_mark = \"#FFFFFF\"\n"
    ));
    assert!(item.is_valid(), "{:?}", item.problems);
    let warnings: Vec<_> = item.problems.iter().filter(|p| !p.is_error()).collect();
    assert!(
        warnings.iter().any(|p| p.key == "dark.guide"),
        "{warnings:?}"
    );
    assert!(warnings.iter().any(|p| p.key == "dark.grid_mark"));
    assert!(warnings
        .iter()
        .all(|p| p.measured.is_some() || !p.message.is_empty()));
}

#[test]
fn a_font_that_is_not_there_falls_back_to_the_parents_and_warns() {
    let item = one(&format!(
        "{MINIMAL}[fonts]\nsans = \"Comic Neue\"\nmono = \"Geist Mono\"\n"
    ));
    assert!(item.is_valid(), "{:?}", item.problems);
    let theme = item.theme.unwrap();
    assert_eq!(theme.typography.sans, "Inter", "the parent's");
    assert_eq!(
        theme.typography.mono, "Geist Mono",
        "an available one is taken"
    );
    assert!(item
        .problems
        .iter()
        .any(|p| p.key == "fonts.sans" && !p.is_error()));
}

#[test]
fn the_lines_of_a_theme_can_be_turned_on_and_off_from_a_file() {
    let item = one(&format!(
        "{MINIMAL}[lines]\ncrosshairs = \"header\"\ncorner_ticks = false\nguides = false\nempty_motif = false\ntick_length = 5\nweight = 2\n"
    ));
    let lines = item.theme.unwrap().lines;
    assert_eq!(lines.crosshairs, super::Crosshairs::Header);
    assert!(!lines.corner_ticks && !lines.guides && !lines.empty_motif);
    assert_eq!((lines.tick_length, lines.weight), (5.0, 2.0));
}

#[test]
fn a_bare_terminal_table_is_both_appearances() {
    let item = one(&format!("{MINIMAL}[terminal]\ncursor = \"#FF00FF\"\n"));
    assert_eq!(
        bytes(p(&item, Appearance::Dark).terminal.cursor),
        (255, 0, 255, 255)
    );
    assert_eq!(
        bytes(p(&item, Appearance::Light).terminal.cursor),
        (255, 0, 255, 255)
    );
}

// ----- export, reference and the example -------------------------------------------------

fn same_values(a: &super::Theme, b: &super::Theme) {
    for token in tokens::TOKENS {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let (x, y) = ((token.get)(a, appearance), (token.get)(b, appearance));
            match (x, y) {
                (tokens::Value::Colour(x), tokens::Value::Colour(y)) => {
                    assert_eq!(bytes(x), bytes(y), "{} {appearance:?}", token.key);
                }
                (tokens::Value::Colours(x), tokens::Value::Colours(y)) => {
                    let (x, y): (Vec<_>, Vec<_>) = (
                        x.into_iter().map(bytes).collect(),
                        y.into_iter().map(bytes).collect(),
                    );
                    assert_eq!(x, y, "{} {appearance:?}", token.key);
                }
                (x, y) => assert_eq!(x, y, "{} {appearance:?}", token.key),
            }
        }
    }
}

#[test]
fn exporting_a_built_in_theme_and_loading_it_back_gives_the_same_values() {
    for id in ThemeId::ALL {
        let text = super::author::render(
            &id.theme(),
            &format!("{}-copy", id.slug()),
            "Copy",
            "",
            None,
        );
        let item = one(&text);
        assert!(item.is_valid(), "{id:?}: {:?}", errors(&item));
        same_values(&id.theme(), &item.theme.unwrap());
    }
}

#[test]
fn the_template_for_a_new_theme_loads_as_the_theme_it_extends() {
    for id in ThemeId::ALL {
        let text = super::author::render(&id.theme(), "my-copy", "My copy", "", Some(id.slug()));
        let item = one(&text);
        assert!(item.is_valid(), "{id:?}: {:?}", errors(&item));
        same_values(&id.theme(), &item.theme.unwrap());
    }
}

/// The part of `docs/THEMES.md` between its markers.
fn reference_in_the_docs() -> String {
    // A checkout may have turned the line endings into CRLF (Git on Windows
    // does by default); the text is the same.
    let docs = include_str!("../../../../docs/THEMES.md").replace("\r\n", "\n");
    let begin = docs.find("<!-- tokens:begin -->").expect("a begin marker");
    let end = docs.find("<!-- tokens:end -->").expect("an end marker");
    docs[begin + "<!-- tokens:begin -->".len()..end]
        .trim()
        .to_owned()
}

#[test]
fn the_token_reference_in_the_docs_is_generated_from_the_token_list_and_current() {
    assert_eq!(
        reference_in_the_docs(),
        tokens::reference().trim(),
        "docs/THEMES.md is out of date: regenerate it with \
         `cargo test -p leon print_theme_reference -- --ignored --nocapture`"
    );
}

/// `cargo test -p leon print_theme_reference -- --ignored --nocapture`.
#[test]
#[ignore = "prints the token reference of docs/THEMES.md; not a check"]
fn print_theme_reference() {
    println!("{}", tokens::reference());
}

#[test]
fn every_token_is_documented_once_with_a_distinct_key_per_table() {
    let mut seen = std::collections::HashSet::new();
    for token in tokens::TOKENS {
        assert!(!token.doc.is_empty(), "{}", token.key);
        assert!(
            seen.insert((token.section, token.key)),
            "{} twice",
            token.key
        );
    }
    let reference = tokens::reference();
    for token in tokens::TOKENS {
        assert!(
            reference.contains(&format!("| `{}` |", token.key)),
            "{}",
            token.key
        );
    }
}

#[test]
fn the_example_theme_loads_and_passes_validation() {
    let text = include_str!("../../../../examples/themes/ocean.toml");
    let item = one(text);
    assert!(item.is_valid(), "{:?}", item.problems);
    assert!(item.problems.iter().all(|p| !p.is_error()));
    assert_eq!(item.id, "ocean");
    let dark = p(&item, Appearance::Dark);
    assert_eq!(bytes(dark.signal), (0x4D, 0xA3, 0xFF, 255));
    assert!(!item.theme.unwrap().lines.corner_ticks);
}

// ----- the registry ------------------------------------------------------------------------

fn install(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    registry::clear_user();
    super::user::load_folder(dir.path(), &fonts);
    dir
}

#[test]
fn user_themes_join_the_registry_after_the_built_in_ones_and_can_be_parsed_and_stepped_to() {
    let _dir = install(&[(
        "a.toml",
        "id = \"alpha\"\nname = \"Alpha\"\nextends = \"leon\"\n",
    )]);
    let all = registry::all();
    assert_eq!(&all[..2], &ThemeId::ALL);
    assert_eq!(all[2].slug(), "alpha");
    assert_eq!(ThemeId::parse("ALPHA"), Some(all[2]));
    assert_eq!(ThemeId::parse("Alpha"), Some(all[2]), "by name too");
    assert_eq!(ThemeId::Zavu.step(true), all[2]);
    assert_eq!(all[2].step(true), ThemeId::Leon, "wraps");
    assert_eq!(all[2].name(), "Alpha");
    assert!(all[2].detail().starts_with("user theme"));
    assert!(ThemeId::Leon.is_builtin() && !all[2].is_builtin());
    registry::clear_user();
}

#[test]
fn a_user_file_cannot_shadow_a_built_in_theme_in_the_registry() {
    let _dir = install(&[(
        "leon.toml",
        "id = \"leon\"\nname = \"Fake\"\nextends = \"zavu\"\n",
    )]);
    assert_eq!(ThemeId::Leon.name(), "Leon");
    assert_eq!(
        ThemeId::Leon.theme().shape.radius,
        6.0,
        "still the built-in"
    );
    let listed: Vec<_> = registry::all().iter().map(|id| id.slug()).collect();
    assert_eq!(listed.iter().filter(|slug| **slug == "leon").count(), 1);
    assert!(registry::problems()
        .iter()
        .any(|p| p.summary().contains("built-in")));
    registry::clear_user();
}

#[test]
fn an_invalid_theme_is_listed_with_its_first_reason_and_cannot_be_chosen_or_stepped_to() {
    let _dir = install(&[
        ("ok.toml", "id = \"fine\"\nextends = \"leon\"\n"),
        (
            "bad.toml",
            "id = \"bad\"\nextends = \"leon\"\n[dark]\ntext = \"#202020\"\n",
        ),
    ]);
    let bad = ThemeId::parse("bad").expect("it is listed");
    assert!(!bad.is_usable());
    assert!(
        bad.problem().unwrap().starts_with("dark.text:"),
        "{:?}",
        bad.problem()
    );
    assert!(bad.detail().starts_with("invalid: dark.text"));
    let fine = ThemeId::parse("fine").unwrap();
    assert!(fine.is_usable());
    assert!(!registry::usable().contains(&bad));
    assert_eq!(
        fine.step(true),
        ThemeId::Leon,
        "stepping skips the invalid one"
    );
    registry::clear_user();
}

#[test]
fn a_broken_save_keeps_the_last_good_version_on_screen() {
    let dir = install(&[(
        "t.toml",
        "id = \"live\"\nextends = \"leon\"\n[dark]\naccent = \"#4DA3FF\"\n",
    )]);
    let id = ThemeId::parse("live").unwrap();
    assert_eq!(
        bytes(id.palette(Appearance::Dark).signal),
        (0x4D, 0xA3, 0xFF, 255)
    );
    std::fs::write(
        dir.path().join("t.toml"),
        "id = \"live\"\nextends = \"leon\"\n[dark]\naccent = \"nonsense\"\n",
    )
    .unwrap();
    super::user::load_folder(dir.path(), &fonts);
    assert!(!id.is_usable(), "invalid now");
    assert_eq!(
        bytes(id.palette(Appearance::Dark).signal),
        (0x4D, 0xA3, 0xFF, 255),
        "the last good one is what is drawn"
    );
    // Fixed again.
    std::fs::write(
        dir.path().join("t.toml"),
        "id = \"live\"\nextends = \"leon\"\n[dark]\naccent = \"#FF8040\"\n",
    )
    .unwrap();
    super::user::load_folder(dir.path(), &fonts);
    assert!(id.is_usable());
    assert_eq!(
        bytes(id.palette(Appearance::Dark).signal),
        (0xFF, 0x80, 0x40, 255)
    );
    registry::clear_user();
}

#[test]
fn a_saved_theme_whose_file_is_gone_reads_as_the_default_and_is_noted() {
    registry::clear_user();
    let id: ThemeId = serde_json::from_str("\"vanished\"").unwrap();
    assert_eq!(id, ThemeId::DEFAULT);
    assert_eq!(registry::take_missing(), ["vanished"]);
    assert!(registry::take_missing().is_empty(), "noted once");
    // With the file there, it is that theme.
    let _dir = install(&[("v.toml", "id = \"vanished\"\nextends = \"zavu\"\n")]);
    let id: ThemeId = serde_json::from_str("\"vanished\"").unwrap();
    assert_eq!(id.slug(), "vanished");
    assert!(registry::take_missing().is_empty());
    registry::clear_user();
}

#[test]
fn colours_are_written_and_read_the_same_way() {
    for text in ["#000000", "#FFEA00", "#0A0A0A", "#12345678"] {
        assert_eq!(
            tokens::colour_text(tokens::parse_colour(text).unwrap()),
            text
        );
    }
    assert!(tokens::parse_colour("#FFF").is_err());
    let _ = colour_difference(
        tokens::parse_colour("#000000").unwrap(),
        tokens::parse_colour("#FFFFFF").unwrap(),
    );
}
