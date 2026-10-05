//! What a theme must meet, checked when a theme file is loaded.
//!
//! The built-in themes are held to these rules by `tests.rs`; a theme from a
//! file is held to them here, at load time, so that a file can never leave the
//! window unreadable.
//!
//! A rule is either an **error** or a **warning**.
//!
//! * An *error* makes the theme invalid: it is not applied, the palette lists
//!   it as invalid with its first error, and "Show theme problems" lists every
//!   one. Errors are what makes something unreadable or unusable: text below
//!   4.5:1 on the surfaces it is drawn on (3:1 for the accent and the state
//!   colours, which are lines and marks), a primary button label below 4.5:1,
//!   terminal text below 4.5:1 or an ANSI colour below 3:1 on the terminal's
//!   page, two of the accent and the four state colours closer than 10 in
//!   CIE76 (they could be taken for one another), and everything that is not a
//!   rule of colour: a file that does not parse, a colour that is not a colour,
//!   a number out of its range, a missing parent, a cycle, a reserved id.
//! * A *warning* is applied and reported. They are the built-in themes' own
//!   stricter numbers (text colours at 4.5:1, terminal text at 7:1, states
//!   20 apart), the quiet band of the blueprint lines, a font that is not
//!   there, and a key nothing reads.

use super::{Appearance, Palette, Theme};
use gpui_kit::{Hsla, Rgba};

/// How bad a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The theme is not applied.
    Error,
    /// The theme is applied.
    Warning,
}

/// One finding about a theme file.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    /// The file it is in.
    pub file: String,
    /// The key it is about (`dark.text`, `lines.weight`), empty for the file.
    pub key: String,
    /// How bad.
    pub severity: Severity,
    /// What is wrong.
    pub message: String,
    /// What was measured, when something was.
    pub measured: Option<f32>,
    /// What a theme needs, when something is needed.
    pub required: Option<f32>,
}

impl Problem {
    /// An error.
    pub fn error(file: &str, key: &str, message: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            key: key.to_owned(),
            severity: Severity::Error,
            message: message.into(),
            measured: None,
            required: None,
        }
    }

    /// A warning.
    pub fn warning(file: &str, key: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(file, key, message)
        }
    }

    fn measured(mut self, measured: f32, required: f32) -> Self {
        self.measured = Some(measured);
        self.required = Some(required);
        self
    }

    /// Whether the finding is about the file of this theme.
    pub fn file_of(&self, id: super::ThemeId) -> bool {
        super::registry::user_entry(id).is_some_and(|entry| {
            matches!(&entry.origin, super::registry::Origin::User(path)
                if path.file_name().is_some_and(|name| name.to_string_lossy() == self.file))
        })
    }

    /// Whether the theme cannot be used because of it.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The finding in one line, without the file.
    pub fn summary(&self) -> String {
        if self.key.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.key, self.message)
        }
    }

    /// The finding as a line of the report.
    pub fn line(&self) -> String {
        let kind = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let measure = match (self.measured, self.required) {
            (Some(measured), Some(required)) => {
                format!(" (measured {measured:.2}, required {required:.2})")
            }
            _ => String::new(),
        };
        format!("{}  {kind}  {}{measure}", self.file, self.summary())
    }
}

fn opaque(colour: Hsla) -> Rgba {
    Rgba::from(colour)
}

/// WCAG 2.x contrast ratio between two colours (opacity ignored).
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    fn luminance(colour: Hsla) -> f32 {
        let rgba = opaque(colour);
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

/// CIE76 colour difference between two colours: how far apart they look.
pub fn colour_difference(a: Hsla, b: Hsla) -> f32 {
    fn lab(colour: Hsla) -> [f32; 3] {
        let rgba = opaque(colour);
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

/// A pair of colours that must be told apart by at least `error` (a theme
/// below it is invalid) and, ideally, `warn`.
struct Pair {
    what: &'static str,
    key: &'static str,
    ratio: f32,
    error: f32,
    warn: f32,
}

fn pairs(p: &Palette) -> Vec<(Pair, Hsla, Hsla)> {
    let t = &p.terminal;
    let pair = |what, key, error, warn| Pair {
        what,
        key,
        ratio: 0.0,
        error,
        warn,
    };
    let mut all = vec![
        (
            pair("text on the page", "text", 4.5, 4.5),
            p.text,
            p.background,
        ),
        (pair("text on a card", "text", 4.5, 4.5), p.text, p.surface),
        (
            pair("text on the open row", "text", 4.5, 4.5),
            p.text,
            p.surface_2,
        ),
        (
            pair("muted text on the page", "text_muted", 4.5, 4.5),
            p.text_muted,
            p.background,
        ),
        (
            pair("muted text on a card", "text_muted", 4.5, 4.5),
            p.text_muted,
            p.surface,
        ),
        (
            pair("muted text on the open row", "text_muted", 4.5, 4.5),
            p.text_muted,
            p.surface_2,
        ),
        (
            pair("the primary button's label", "on_primary", 4.5, 7.0),
            p.on_primary,
            p.primary_fill,
        ),
        (
            pair("the label on an accent fill", "on_accent_fill", 4.5, 7.0),
            p.on_accent_fill,
            p.accent_fill,
        ),
        (
            pair("the accent on the page", "accent", 3.0, 4.5),
            p.signal,
            p.background,
        ),
        (
            pair("the accent on the open row", "accent", 3.0, 3.0),
            p.signal,
            p.surface_2,
        ),
        (
            pair("faint glyphs on the page", "text_faint", 1.5, 3.0),
            p.text_faint,
            p.background,
        ),
        (
            pair("the logo on the page", "logo", 3.0, 3.0),
            p.logo,
            p.background,
        ),
        (
            pair("the success colour on the page", "success", 3.0, 4.5),
            p.success,
            p.background,
        ),
        (
            pair("the warning colour on the page", "warning", 3.0, 4.5),
            p.warning,
            p.background,
        ),
        (
            pair("the error colour on the page", "error", 3.0, 4.5),
            p.error,
            p.background,
        ),
        (
            pair("the info colour on the page", "info", 3.0, 4.5),
            p.info,
            p.background,
        ),
        (
            pair("the elsewhere colour on the page", "elsewhere", 3.0, 4.5),
            p.elsewhere,
            p.background,
        ),
        (
            pair(
                "terminal text on the terminal's page",
                "terminal.foreground",
                4.5,
                7.0,
            ),
            t.foreground,
            t.background,
        ),
        (
            pair(
                "the terminal cursor on its page",
                "terminal.cursor",
                3.0,
                3.0,
            ),
            t.cursor,
            t.background,
        ),
        (
            pair(
                "terminal text on the selection",
                "terminal.selection",
                3.0,
                4.5,
            ),
            t.foreground,
            t.selection,
        ),
        (
            pair(
                "terminal text on a find match",
                "terminal.find_match",
                3.0,
                4.5,
            ),
            t.foreground,
            t.find_match,
        ),
        (
            pair(
                "the page's colour on the current find match",
                "terminal.find_match_current",
                3.0,
                4.5,
            ),
            t.background,
            t.find_match_current,
        ),
    ];
    for (i, colour) in t.ansi.iter().enumerate() {
        let (error, warn) = match i {
            0 => (1.2, 1.5),
            8 => (2.0, 3.5),
            _ => (3.0, 4.5),
        };
        all.push((
            pair(
                "an ANSI colour on the terminal's page",
                "terminal.ansi",
                error,
                warn,
            ),
            *colour,
            t.background,
        ));
    }
    all
}

/// Every finding about the palette of `appearance`.
fn palette_problems(file: &str, appearance: Appearance, p: &Palette, out: &mut Vec<Problem>) {
    let prefix = match appearance {
        Appearance::Light => "light",
        Appearance::Dark => "dark",
    };
    for (index, (mut pair, foreground, background)) in pairs(p).into_iter().enumerate() {
        pair.ratio = contrast(foreground, background);
        let key = format!("{prefix}.{}", pair.key);
        let what = if pair.key == "terminal.ansi" {
            format!("ANSI colour {}", index.saturating_sub(22))
        } else {
            pair.what.to_owned()
        };
        if pair.ratio < pair.error {
            out.push(
                Problem::error(
                    file,
                    &key,
                    format!(
                        "{what} is {:.2}:1; it needs at least {:.1}:1",
                        pair.ratio, pair.error
                    ),
                )
                .measured(pair.ratio, pair.error),
            );
        } else if pair.ratio < pair.warn {
            out.push(
                Problem::warning(
                    file,
                    &key,
                    format!(
                        "{what} is {:.2}:1; the built-in themes keep it at {:.1}:1",
                        pair.ratio, pair.warn
                    ),
                )
                .measured(pair.ratio, pair.warn),
            );
        }
    }
    // The accent and the four states must not be taken for one another.
    let states = [
        ("accent", p.signal),
        ("success", p.success),
        ("warning", p.warning),
        ("error", p.error),
        ("info", p.info),
        ("elsewhere", p.elsewhere),
    ];
    for (i, (a, ca)) in states.iter().enumerate() {
        for (b, cb) in &states[i + 1..] {
            let apart = colour_difference(*ca, *cb);
            let key = format!("{prefix}.{b}");
            if apart < 10.0 {
                out.push(
                    Problem::error(
                        file,
                        &key,
                        format!("{a} and {b} are {apart:.1} apart: they could be taken for one another; they need at least 10"),
                    )
                    .measured(apart, 10.0),
                );
            } else if apart < 20.0 {
                out.push(
                    Problem::warning(
                        file,
                        &key,
                        format!("{a} and {b} are {apart:.1} apart; the built-in themes keep them 20 apart"),
                    )
                    .measured(apart, 20.0),
                );
            }
        }
    }
    // The blueprint lines are quiet: visible, below the graphics contrast.
    for (name, colour, low, high) in [
        ("border", p.border, 1.1, 2.0),
        ("guide", p.guide, 1.1, 2.0),
        ("grid_mark", p.grid_mark, 1.3, 3.0),
    ] {
        let ratio = contrast(colour, p.background);
        if ratio < low || ratio >= high {
            out.push(
                Problem::warning(
                    file,
                    &format!("{prefix}.{name}"),
                    format!("{name} is {ratio:.2}:1 against the page; lines are quiet between {low} and {high}:1"),
                )
                .measured(ratio, low),
            );
        }
    }
    let veil = Rgba::from(p.scrim).a;
    if !(0.3..=0.9).contains(&veil) {
        out.push(Problem::warning(
            file,
            &format!("{prefix}.scrim"),
            format!("the veil is {:.0} % opaque; between 30 and 90 % lets the page show through and darkens it", veil * 100.0),
        ));
    }
}

/// Every finding about a whole theme.
pub fn check(file: &str, theme: &Theme) -> Vec<Problem> {
    let mut out = Vec::new();
    palette_problems(file, Appearance::Dark, &theme.dark, &mut out);
    palette_problems(file, Appearance::Light, &theme.light, &mut out);
    let apart = contrast(theme.light.background, theme.dark.background);
    if apart < 10.0 {
        out.push(
            Problem::warning(
                file,
                "light.background",
                format!("light and dark pages are {apart:.1}:1 apart: they are hardly two looks"),
            )
            .measured(apart, 10.0),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;

    #[test]
    fn the_built_in_themes_pass_every_rule_as_errors() {
        for id in ThemeId::ALL {
            let found = check("builtin", &id.theme());
            let errors: Vec<_> = found.iter().filter(|p| p.is_error()).collect();
            assert!(errors.is_empty(), "{id:?}: {errors:?}");
        }
    }

    #[test]
    fn contrast_and_difference_are_the_measures_the_built_in_tests_use() {
        let black = gpui_kit::rgb(0x000000).into();
        let white = gpui_kit::rgb(0xFFFFFF).into();
        assert!((contrast(black, white) - 21.0).abs() < 0.01);
        assert!(colour_difference(black, black) < 0.001);
        assert!(colour_difference(black, white) > 90.0);
    }

    #[test]
    fn a_finding_prints_as_one_line_with_what_was_measured_and_required() {
        let problem = Problem::error("a.toml", "dark.text", "too faint").measured(2.0, 4.5);
        assert_eq!(
            problem.line(),
            "a.toml  error  dark.text: too faint (measured 2.00, required 4.50)"
        );
    }
}
