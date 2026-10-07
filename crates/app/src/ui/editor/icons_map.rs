//! Which icon a file or folder wears in the file tree.
//!
//! The icons are glyphs of the bundled "Symbols Nerd Font Mono" (see
//! `assets/ASSETS.md`), mostly from its Seti-UI set, which draws one mark per
//! language and tool. [`glyph`] answers with the character and the *token* of
//! the colour, never a colour: the tree resolves the token against the palette
//! of the theme in use, so the icons follow the theme and its appearance.
//!
//! The lookup goes from the most to the least specific: the exact file name
//! (`Cargo.toml`, `Dockerfile`, `.gitignore`...), then the extension, then a
//! generic file or folder.

use crate::theme::Palette;
use gpui_kit::Hsla;

/// The family the glyphs are drawn with.
pub const FAMILY: &str = "Symbols Nerd Font Mono";

/// A colour of the palette. The six hues are the theme's ANSI colours, so a
/// theme's icons are as saturated as its terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// Secondary text.
    Muted,
    /// The accent.
    Signal,
    /// The theme's red.
    Red,
    /// The theme's green.
    Green,
    /// The theme's yellow.
    Yellow,
    /// The theme's blue.
    Blue,
    /// The theme's magenta.
    Magenta,
    /// The theme's cyan.
    Cyan,
}

impl Token {
    /// The colour of this token in `palette`.
    pub fn resolve(self, palette: &Palette) -> Hsla {
        let ansi = &palette.terminal.ansi;
        match self {
            Token::Muted => palette.text_muted,
            Token::Signal => palette.signal,
            Token::Red => ansi[1],
            Token::Green => ansi[2],
            Token::Yellow => ansi[3],
            Token::Blue => ansi[4],
            Token::Magenta => ansi[5],
            Token::Cyan => ansi[6],
        }
    }
}

const FOLDER: char = '\u{e5ff}';
const FOLDER_OPEN: char = '\u{e5fe}';
const FILE: char = '\u{f016}';

/// The glyph and colour token of an entry called `name`. `open` only matters
/// for a folder.
pub fn glyph(name: &str, is_dir: bool, open: bool) -> (char, Token) {
    if is_dir {
        return match open {
            true => (FOLDER_OPEN, Token::Signal),
            false => (FOLDER, Token::Muted),
        };
    }
    let lower = name.to_ascii_lowercase();
    if let Some(found) = by_name(&lower) {
        return found;
    }
    lower
        .rsplit_once('.')
        .and_then(|(_, extension)| by_extension(extension))
        .unwrap_or((FILE, Token::Muted))
}

/// Files known by their whole name (compared in lower case).
fn by_name(name: &str) -> Option<(char, Token)> {
    use Token::*;
    Some(match name {
        "cargo.lock" => ('\u{e672}', Muted),
        "cargo.toml" | "rust-toolchain" | "rust-toolchain.toml" => ('\u{eb29}', Yellow),
        "go.mod" | "go.sum" => ('\u{e627}', Cyan),
        "package.json" | "package-lock.json" => ('\u{e616}', Red),
        "tsconfig.json" | "jsconfig.json" => ('\u{e628}', Blue),
        "yarn.lock" | "pnpm-lock.yaml" | "bun.lockb" | "bun.lock" | "poetry.lock"
        | "gemfile.lock" | "flake.lock" | "composer.lock" => ('\u{e672}', Muted),
        "dockerfile"
        | "containerfile"
        | ".dockerignore"
        | "docker-compose.yml"
        | "docker-compose.yaml"
        | "compose.yml"
        | "compose.yaml" => ('\u{e650}', Blue),
        "makefile" | "gnumakefile" | "justfile" | "cmakelists.txt" => ('\u{e673}', Muted),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" | ".mailmap" => {
            ('\u{e65d}', Red)
        }
        "readme" | "readme.md" | "readme.txt" | "readme.rst" | "changelog" | "changelog.md" => {
            ('\u{eaa4}', Cyan)
        }
        "license" | "license.md" | "license.txt" | "licence" | "copying" | "notice" => {
            ('\u{e60a}', Yellow)
        }
        ".env" | ".editorconfig" | ".prettierrc" | ".eslintrc" | ".eslintrc.json" | ".npmrc"
        | ".nvmrc" | ".rustfmt.toml" | "rustfmt.toml" | "clippy.toml" | ".envrc" => {
            ('\u{e615}', Muted)
        }
        "gemfile" | "rakefile" => ('\u{e605}', Red),
        "requirements.txt" | "pyproject.toml" | "pipfile" | "setup.py" => ('\u{e606}', Yellow),
        _ if name.starts_with("dockerfile.") => ('\u{e650}', Blue),
        _ if name.starts_with(".env.") => ('\u{e615}', Muted),
        _ => return None,
    })
}

/// Files known by their extension (lower case, without the dot).
fn by_extension(extension: &str) -> Option<(char, Token)> {
    use Token::*;
    Some(match extension {
        "rs" => ('\u{e68b}', Red),
        "ts" | "mts" | "cts" => ('\u{e628}', Blue),
        "tsx" | "jsx" => ('\u{e625}', Cyan),
        "js" | "mjs" | "cjs" => ('\u{e60c}', Yellow),
        "py" | "pyi" | "pyw" => ('\u{e606}', Yellow),
        "go" => ('\u{e627}', Cyan),
        "md" | "markdown" | "mdx" | "rst" => ('\u{e609}', Cyan),
        "json" | "jsonc" | "json5" => ('\u{e60b}', Yellow),
        "toml" | "ini" | "cfg" | "conf" | "env" | "properties" => ('\u{e615}', Muted),
        "yaml" | "yml" => ('\u{e6a8}', Magenta),
        "html" | "htm" => ('\u{e60e}', Red),
        "css" => ('\u{e614}', Blue),
        "scss" | "sass" | "less" => ('\u{e603}', Magenta),
        "sh" | "bash" | "zsh" | "fish" => ('\u{e691}', Green),
        "ps1" | "psm1" => ('\u{e683}', Blue),
        "c" | "h" => ('\u{e649}', Blue),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => ('\u{e646}', Blue),
        "cs" => ('\u{e648}', Magenta),
        "java" | "jar" => ('\u{e66d}', Red),
        "kt" | "kts" => ('\u{e634}', Magenta),
        "swift" => ('\u{e699}', Red),
        "rb" | "erb" => ('\u{e605}', Red),
        "php" => ('\u{e608}', Magenta),
        "lua" => ('\u{e620}', Blue),
        "zig" => ('\u{e6a9}', Yellow),
        "ex" | "exs" => ('\u{e62d}', Magenta),
        "hs" => ('\u{e61f}', Magenta),
        "scala" => ('\u{e68e}', Red),
        "dart" => ('\u{e64c}', Cyan),
        "r" => ('\u{e68a}', Blue),
        "vue" => ('\u{e6a0}', Green),
        "svelte" => ('\u{e697}', Red),
        "sql" | "db" | "sqlite" => ('\u{f1c0}', Muted),
        "xml" | "plist" => ('\u{e619}', Yellow),
        "csv" | "tsv" => ('\u{e64a}', Green),
        "lock" => ('\u{e672}', Muted),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "icns" | "avif" => {
            ('\u{e60d}', Magenta)
        }
        "svg" => ('\u{e698}', Yellow),
        "pdf" => ('\u{e67d}', Red),
        "mp4" | "mov" | "mkv" | "webm" | "avi" => ('\u{e69f}', Magenta),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => ('\u{e638}', Cyan),
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "7z" | "rar" => ('\u{e6aa}', Yellow),
        "txt" | "log" => ('\u{f0f6}', Muted),
        "asm" | "s" => ('\u{e6ab}', Muted),
        "license" => ('\u{e60a}', Yellow),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blocks of "Symbols Nerd Font Mono" that the tree draws from: the
    /// Seti-UI and custom icons, the Devicons, the Font Awesome set, the
    /// Codicons and the Material Design icons.
    const NERD_RANGES: [std::ops::RangeInclusive<u32>; 5] = [
        0xe5fa..=0xe6b7,
        0xe700..=0xe8ef,
        0xea60..=0xec1e,
        0xed00..=0xf2ff,
        0xf0001..=0xf1af0,
    ];

    fn in_the_font(glyph: char) -> bool {
        NERD_RANGES
            .iter()
            .any(|range| range.contains(&u32::from(glyph)))
    }

    #[test]
    fn exact_names_come_before_extensions_before_the_generic_file() {
        // The name beats the extension: Cargo.toml is not a plain toml file.
        assert_eq!(glyph("Cargo.toml", false, false).0, '\u{eb29}');
        assert_eq!(glyph("config.toml", false, false).0, '\u{e615}');
        assert_eq!(glyph("Cargo.lock", false, false).0, '\u{e672}');
        assert_eq!(glyph("Dockerfile", false, false).0, '\u{e650}');
        assert_eq!(glyph("Dockerfile.dev", false, false).0, '\u{e650}');
        assert_eq!(glyph("package.json", false, false).0, '\u{e616}');
        assert_eq!(glyph("data.json", false, false).0, '\u{e60b}');
        assert_eq!(glyph(".gitignore", false, false).0, '\u{e65d}');
        assert_eq!(glyph("README.md", false, false).0, '\u{eaa4}');
        assert_eq!(glyph("notes.md", false, false).0, '\u{e609}');
        assert_eq!(glyph("LICENSE", false, false).0, '\u{e60a}');
        assert_eq!(glyph("Makefile", false, false).0, '\u{e673}');
        // The extension is read in lower case; only the last one counts.
        assert_eq!(glyph("MAIN.RS", false, false).0, '\u{e68b}');
        assert_eq!(glyph("archive.tar.gz", false, false).0, '\u{e6aa}');
        assert_eq!(glyph("lib.rs", false, false), ('\u{e68b}', Token::Red));
        // Unknown ones, and names that are only a dot or have none.
        assert_eq!(glyph("mystery.zzz", false, false), (FILE, Token::Muted));
        assert_eq!(glyph("noextension", false, false), (FILE, Token::Muted));
        assert_eq!(glyph(".hidden", false, false), (FILE, Token::Muted));
        assert_eq!(glyph("trailing.", false, false), (FILE, Token::Muted));
    }

    #[test]
    fn a_folder_is_open_or_closed_whatever_its_name() {
        assert_eq!(glyph("src", true, false), (FOLDER, Token::Muted));
        assert_eq!(glyph("src", true, true), (FOLDER_OPEN, Token::Signal));
        assert_eq!(glyph("Cargo.toml", true, false).0, FOLDER);
    }

    #[test]
    fn every_mapped_codepoint_lies_in_the_nerd_font_symbol_ranges() {
        let mut seen = std::collections::BTreeSet::new();
        let names = [
            "cargo.toml",
            "cargo.lock",
            "rust-toolchain",
            "go.mod",
            "package.json",
            "tsconfig.json",
            "yarn.lock",
            "dockerfile",
            "dockerfile.dev",
            "makefile",
            ".gitignore",
            "readme.md",
            "license",
            ".env",
            ".env.local",
            "gemfile",
            "requirements.txt",
        ];
        for name in names {
            let (glyph, _) = by_name(name).unwrap_or_else(|| panic!("{name}"));
            seen.insert(glyph);
        }
        for extension in [
            "rs", "ts", "tsx", "jsx", "js", "py", "go", "md", "json", "toml", "yaml", "yml",
            "html", "css", "scss", "sh", "ps1", "c", "h", "cpp", "hpp", "cs", "java", "kt",
            "swift", "rb", "php", "lua", "zig", "ex", "hs", "scala", "dart", "r", "vue", "svelte",
            "sql", "xml", "csv", "lock", "png", "jpg", "gif", "svg", "pdf", "mp4", "mp3", "zip",
            "gz", "txt", "asm", "license",
        ] {
            let (glyph, _) = by_extension(extension).unwrap_or_else(|| panic!("{extension}"));
            seen.insert(glyph);
        }
        seen.extend([FOLDER, FOLDER_OPEN, FILE]);
        for glyph in seen {
            assert!(
                in_the_font(glyph),
                "U+{:X} is outside the font",
                u32::from(glyph)
            );
        }
    }

    #[test]
    fn a_token_resolves_to_the_theme_colours() {
        use crate::theme::{Appearance, ThemeId};
        let palette = ThemeId::Leon.theme().palette(Appearance::Dark);
        assert_eq!(Token::Muted.resolve(&palette), palette.text_muted);
        assert_eq!(Token::Signal.resolve(&palette), palette.signal);
        assert_eq!(Token::Red.resolve(&palette), palette.terminal.ansi[1]);
        assert_eq!(Token::Cyan.resolve(&palette), palette.terminal.ansi[6]);
    }
}
