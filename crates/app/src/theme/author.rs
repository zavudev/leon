//! Writing theme files: a template that extends the current theme, and a full
//! standalone export.
//!
//! Both are generated from the token list ([`super::tokens::TOKENS`]) and the
//! theme's own values, so a file written here lists every token with its
//! current value and a line of what it is for.

use super::tokens::{value_text, Section, Token, TOKENS};
use super::{Appearance, Theme};

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn lines_of(
    out: &mut String,
    theme: &Theme,
    appearance: Appearance,
    token: &Token,
    commented: bool,
) {
    let mark = if commented { "# " } else { "" };
    out.push_str(&format!("# {}\n", token.doc));
    out.push_str(&format!(
        "{mark}{} = {}\n",
        token.key,
        value_text(&(token.get)(theme, appearance))
    ));
}

fn table(
    out: &mut String,
    theme: &Theme,
    name: &str,
    section: Section,
    appearance: Appearance,
    commented: bool,
) {
    out.push_str(&format!("\n[{name}]\n"));
    for token in TOKENS.iter().filter(|token| token.section == section) {
        lines_of(out, theme, appearance, token, commented);
    }
}

/// The text of a theme file for `theme`. With `extends`, the tokens are
/// listed as comments (every one inherits until it is uncommented and
/// changed); without, every token is set.
pub fn render(theme: &Theme, id: &str, name: &str, author: &str, extends: Option<&str>) -> String {
    let commented = extends.is_some();
    let mut out = String::new();
    out.push_str("# A Leon theme. Every key is documented in docs/THEMES.md.\n");
    out.push_str("# Save the file and the open window follows it.\n\n");
    out.push_str(&format!("id = {}\nname = {}\n", quoted(id), quoted(name)));
    if !author.is_empty() {
        out.push_str(&format!("author = {}\n", quoted(author)));
    }
    if let Some(parent) = extends {
        out.push_str(&format!(
            "extends = {}\n\n# Only the keys you uncomment and change differ from the parent. Set `accent`\n# alone and what follows it (the terminal's cursor and selection, the logo)\n# follows.\n",
            quoted(parent)
        ));
    }
    for (appearance, label) in [(Appearance::Dark, "dark"), (Appearance::Light, "light")] {
        table(
            &mut out,
            theme,
            label,
            Section::Palette,
            appearance,
            commented,
        );
        table(
            &mut out,
            theme,
            &format!("{label}.terminal"),
            Section::Terminal,
            appearance,
            commented,
        );
    }
    table(
        &mut out,
        theme,
        "fonts",
        Section::Fonts,
        Appearance::Dark,
        commented,
    );
    table(
        &mut out,
        theme,
        "shape",
        Section::Shape,
        Appearance::Dark,
        commented,
    );
    table(
        &mut out,
        theme,
        "lines",
        Section::Lines,
        Appearance::Dark,
        commented,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;

    #[test]
    fn a_template_lists_every_token_as_a_comment_under_an_extends() {
        let text = render(&ThemeId::Leon.theme(), "mine", "Mine", "", Some("leon"));
        assert!(text.contains("extends = \"leon\""));
        for token in TOKENS {
            assert!(
                text.contains(&format!("# {} = ", token.key)),
                "{} is listed",
                token.key
            );
        }
        // Nothing is set: the file parses to an empty override.
        let parsed: toml::Table = text.parse().unwrap();
        let dark = parsed["dark"].as_table().unwrap();
        assert_eq!(dark.len(), 1, "only the terminal's table, itself empty");
        assert!(dark["terminal"].as_table().unwrap().is_empty());
    }

    #[test]
    fn an_export_sets_every_token() {
        let text = render(&ThemeId::Zavu.theme(), "zavu-copy", "Zavu copy", "", None);
        assert!(!text.contains("extends"));
        let parsed: toml::Table = text.parse().unwrap();
        for appearance in ["dark", "light"] {
            let table = parsed[appearance].as_table().unwrap();
            assert!(table.contains_key("accent") && table.contains_key("surface_2"));
            assert!(
                table["terminal"].as_table().unwrap()["ansi"]
                    .as_array()
                    .unwrap()
                    .len()
                    == 16
            );
        }
    }

    #[test]
    fn quotes_in_a_name_are_escaped() {
        let text = render(
            &ThemeId::Leon.theme(),
            "q",
            "A \"quoted\" name",
            "",
            Some("leon"),
        );
        let parsed: toml::Table = text.parse().unwrap();
        assert_eq!(parsed["name"].as_str(), Some("A \"quoted\" name"));
    }
}
