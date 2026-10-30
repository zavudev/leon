//! The code in an agent's answers: which language a fence names, the label
//! and the copy button in the corner of the block.
//!
//! The Markdown view highlights a fenced block with the grammars of the
//! editor, by the name after the fence. Agents write that name in many ways
//! (`sh`, `zsh`, `jsx`, `yml`, `rust,ignore`), so the fences are first
//! written with the names the grammars know ([`normalize`]); the editor's
//! own choice of language by extension ([`super::editor::detect`]) is the
//! one place that says what an extension means.

use std::borrow::Cow;

use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, SharedString};

use super::widgets::mono;
use crate::theme::{metrics, px, Palette};

/// The languages of the editor, by the name its grammars go by.
const LANGUAGES: &[&str] = &[
    "rust",
    "javascript",
    "typescript",
    "tsx",
    "python",
    "go",
    "json",
    "toml",
    "yaml",
    "bash",
    "markdown",
    "css",
    "html",
    "c",
    "cpp",
    "lua",
    "make",
    "diff",
    "java",
    "ruby",
];

/// The name of the grammar for what a fence says, or `None` for a language
/// the editor has none for (the block is then plain text).
pub fn canonical(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().to_ascii_lowercase();
    if tag.is_empty() {
        return None;
    }
    if let Some(name) = LANGUAGES.iter().find(|name| **name == tag) {
        return Some(name);
    }
    // What agents write for the same thing, before the extensions.
    let named = match tag.as_str() {
        "shell" | "console" | "terminal" | "shellscript" | "zsh" | "sh" => Some("bash"),
        "javascript" | "node" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "ts" => Some("typescript"),
        "py" | "python3" => Some("python"),
        "rs" => Some("rust"),
        "yml" => Some("yaml"),
        "golang" => Some("go"),
        "c++" | "cxx" | "hpp" => Some("cpp"),
        "md" => Some("markdown"),
        "jsonc" | "json5" => Some("json"),
        "patch" => Some("diff"),
        "makefile" => Some("make"),
        "htm" | "xml" | "svg" => Some("html"),
        "scss" => Some("css"),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    match super::editor::detect(&format!("x.{tag}")) {
        "text" => None,
        name => Some(name),
    }
}

/// `markdown` with the language of every opening fence written as the name
/// the grammars know. Everything else is left as it is.
pub fn normalize(markdown: &str) -> Cow<'_, str> {
    if !markdown.contains("```") && !markdown.contains("~~~") {
        return Cow::Borrowed(markdown);
    }
    let mut out = String::with_capacity(markdown.len());
    let mut changed = false;
    // The fence that is open: its character and how many of them.
    let mut open: Option<(char, usize)> = None;
    for line in markdown.split_inclusive('\n') {
        let body = line.trim_start();
        let fence = body
            .chars()
            .next()
            .filter(|c| matches!(c, '`' | '~'))
            .map(|c| (c, body.chars().take_while(|d| *d == c).count()))
            .filter(|(_, n)| *n >= 3);
        match (open, fence) {
            (Some((c, n)), Some((d, m))) if c == d && m >= n && body[m..].trim().is_empty() => {
                open = None;
                out.push_str(line);
            }
            (None, Some((c, n))) => {
                open = Some((c, n));
                let indent = &line[..line.len() - body.len()];
                let info = &body[n..];
                let tag_end = info
                    .find(|ch: char| ch.is_whitespace() || ch == ',' || ch == '{')
                    .unwrap_or(info.len());
                let tag = info[..tag_end].trim();
                match canonical(tag) {
                    Some(name) if name != tag => {
                        changed = true;
                        let fence: String = std::iter::repeat_n(c, n).collect();
                        out.push_str(&format!("{indent}{fence}{name}{}", &info[tag_end..]));
                    }
                    _ => out.push_str(line),
                }
            }
            _ => out.push_str(line),
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(markdown)
    }
}

/// What the corner of a block says of its language: the name in capitals, or
/// nothing for a block that names none.
pub fn label(lang: Option<&str>) -> Option<String> {
    let tag = lang?.trim();
    if tag.is_empty() {
        return None;
    }
    Some(canonical(tag).unwrap_or(tag).to_uppercase())
}

/// What sits in the corner of a fenced block: its language and a button that
/// copies its code. `name` is the selector of the pair; `code` is what the
/// button copies.
pub fn corner(
    name: String,
    lang: Option<&str>,
    code: SharedString,
    colours: &Palette,
) -> AnyElement {
    let hover = colours.surface_2;
    let copy = format!("{name}-copy");
    let tip = SharedString::from("Copy the code");
    div()
        .debug_selector({
            let name = name.clone();
            move || name.clone()
        })
        .flex()
        .items_center()
        .gap(px(6.))
        .children(label(lang).map(|words| {
            mono(words)
                .debug_selector({
                    let name = format!("{name}-lang");
                    move || name.clone()
                })
                .text_color(colours.text_faint)
        }))
        .child(
            div()
                .id(SharedString::from(copy.clone()))
                .debug_selector(move || copy.clone())
                .h(px(20.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .child(mono("COPY").text_color(colours.text_muted))
                .on_click(move |_: &ClickEvent, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(code.to_string()));
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_the_grammars_know_are_kept_and_the_aliases_are_taken_to_them() {
        for (tag, name) in [
            ("rust", "rust"),
            ("Rust", "rust"),
            ("rs", "rust"),
            ("sh", "bash"),
            ("zsh", "bash"),
            ("shell", "bash"),
            ("console", "bash"),
            ("jsx", "javascript"),
            ("js", "javascript"),
            ("ts", "typescript"),
            ("tsx", "tsx"),
            ("py", "python"),
            ("yml", "yaml"),
            ("c++", "cpp"),
            ("cc", "cpp"),
            ("jsonc", "json"),
            ("diff", "diff"),
            ("patch", "diff"),
            ("html", "html"),
        ] {
            assert_eq!(canonical(tag), Some(name), "{tag}");
        }
        for tag in ["", "  ", "text", "plaintext", "nonsense", "mermaid"] {
            assert_eq!(canonical(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn a_fence_is_written_with_the_name_of_its_grammar() {
        let text = "Run it:\n\n```sh\ncargo test\n```\n\nand\n\n```jsx title=\"a\"\n<A/>\n```\n";
        assert_eq!(
            normalize(text),
            "Run it:\n\n```bash\ncargo test\n```\n\nand\n\n```javascript title=\"a\"\n<A/>\n```\n"
        );
        // Extra words after a comma stay.
        assert_eq!(normalize("```rs,ignore\nx\n```"), "```rust,ignore\nx\n```");
        // An indented fence in a list, and a tilde fence.
        assert_eq!(
            normalize("1. a\n   ```py\n   x\n   ```\n~~~yml\na: 1\n~~~\n"),
            "1. a\n   ```python\n   x\n   ```\n~~~yaml\na: 1\n~~~\n"
        );
    }

    #[test]
    fn what_is_not_an_opening_fence_is_left_alone() {
        // Nothing to change: the very same text, not a copy.
        let plain = "no code here, only `sh` inline\n";
        assert!(matches!(normalize(plain), Cow::Borrowed(_)));
        let known = "```rust\nfn main() {}\n```\n";
        assert!(matches!(normalize(known), Cow::Borrowed(_)));
        // The code of a block is not read for fences, and a language the
        // editor has none for stays as written.
        let nested = "````md\n```sh\nx\n```\n````\n```mermaid\ngraph\n```\n";
        assert_eq!(
            normalize(nested),
            "````markdown\n```sh\nx\n```\n````\n```mermaid\ngraph\n```\n"
        );
        // An open fence at the end of a draft is still a fence.
        assert_eq!(normalize("```sh\nls"), "```bash\nls");
    }

    #[test]
    fn the_corner_names_the_language_in_capitals() {
        assert_eq!(label(Some("rs")).as_deref(), Some("RUST"));
        assert_eq!(label(Some("mermaid")).as_deref(), Some("MERMAID"));
        assert_eq!(label(Some("")), None);
        assert_eq!(label(None), None);
    }
}
