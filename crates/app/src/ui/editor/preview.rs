//! The preview of a Markdown file: drawn beside the editor or instead of it.
//!
//! A file named `*.md` or `*.markdown` has a [`ViewMode`]: the text (`Edit`),
//! the rendered page (`Preview`) or both side by side (`Split`). The page is
//! gpui-component's `TextView` over a `TextViewState` that lives with the
//! document, so the scroll position of each half is still there after the
//! mode changes. While the page shows, a change of the text is passed on to it
//! after a pause ([`Options::editor_debounce`](super::super::shell::Options)).
//!
//! The text view does not read files, so images are settled here before the
//! text is handed over ([`prepare`]): a relative image of a file on this
//! computer is read and embedded as a `data:` URL; on another machine it is
//! replaced by its description, since the bytes would have to cross the wire
//! for a picture nobody asked to see. A link is Leon's to follow
//! ([`resolve_link`]): a web address opens in the browser, a path opens the
//! file in a tab, on the machine the document is on.

use super::super::live::LiveId;
use super::super::shell::Shell;
use super::document::{parent_of, EditorDoc};
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, SharedString, Window};

/// What a Markdown file shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// The text, in the editor.
    #[default]
    Edit,
    /// The rendered page.
    Preview,
    /// The editor on the left, the page on the right.
    Split,
}

impl ViewMode {
    /// The mode `TogglePreview` goes to: edit, then both, then the page.
    pub fn next(self) -> Self {
        match self {
            Self::Edit => Self::Split,
            Self::Split => Self::Preview,
            Self::Preview => Self::Edit,
        }
    }

    /// The word the toolbar and the saved layout use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Preview => "preview",
            Self::Split => "split",
        }
    }

    /// The mode a saved layout named; anything else is `Edit`.
    pub fn parse(name: &str) -> Self {
        match name {
            "preview" => Self::Preview,
            "split" => Self::Split,
            _ => Self::Edit,
        }
    }

    /// Whether the page is drawn.
    pub fn shows_page(self) -> bool {
        self != Self::Edit
    }
}

/// Whether a file has a preview: by its extension.
pub fn is_markdown(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

/// Whether a file is SVG: text that is also a picture, so its page is drawn
/// from the text.
pub fn is_svg(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("svg"))
}

/// Whether a file has a page beside its text: Markdown and SVG.
pub fn has_page(path: &str) -> bool {
    is_markdown(path) || is_svg(path)
}

/// The text of an SVG file as the picture it draws.
fn svg_page(text: &str) -> std::sync::Arc<gpui_kit::Image> {
    std::sync::Arc::new(gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Svg,
        text.as_bytes().to_vec(),
    ))
}

/// The largest picture embedded in a page.
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// What a link in a page leads to.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A web address (or a mail address): the system opens it.
    Web(String),
    /// A path on the machine of the document, absolute.
    Path(String),
    /// Somewhere inside the page, or nowhere.
    Nothing,
}

/// Where a link of a document in `base` (its folder) leads.
pub fn resolve_link(base: &str, link: &str) -> Target {
    let link = link.trim();
    if let Some(scheme) = scheme_of(link) {
        return match scheme.to_ascii_lowercase().as_str() {
            "http" | "https" | "mailto" => Target::Web(link.to_owned()),
            _ => Target::Nothing,
        };
    }
    let path = link.split(['#', '?']).next().unwrap_or("");
    if path.is_empty() {
        return Target::Nothing;
    }
    Target::Path(join(base, &percent_decode(path)))
}

/// The scheme of a link (`https` of `https://x`), unless it is a Windows
/// drive (`C:\x`).
fn scheme_of(link: &str) -> Option<&str> {
    let (scheme, rest) = link.split_once(':')?;
    let letters = scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
    let drive = scheme.len() == 1 && rest.starts_with(['/', '\\']);
    (letters && !drive).then_some(scheme)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let pair = std::str::from_utf8(&bytes[at + 1..at + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            if let Some(byte) = pair {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `relative` taken from `base`, written the way `base` is, without `.` and
/// `..` in it. An absolute `relative` stays on the drive of `base`.
fn join(base: &str, relative: &str) -> String {
    let separator = if base.contains('\\') && !base.contains('/') {
        '\\'
    } else {
        '/'
    };
    let absolute = relative.starts_with(['/', '\\']);
    let drive = base.len() >= 2 && base.as_bytes()[1] == b':';
    let unix_root = base.starts_with(['/', '\\']);
    let mut parts: Vec<&str> = Vec::new();
    // What popping a `..` may not go below: the drive, if there is one.
    let mut floor = 0;
    if !absolute {
        parts.extend(base.split(['/', '\\']).filter(|part| !part.is_empty()));
    } else if drive {
        parts.push(&base[..2]);
    }
    if drive {
        floor = 1;
    }
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() > floor {
                    parts.pop();
                }
            }
            part => parts.push(part),
        }
    }
    let rooted = absolute && !drive || (!absolute && unix_root);
    let body = parts.join(&separator.to_string());
    if rooted {
        format!("{separator}{body}")
    } else {
        body
    }
}

/// The page's text with its images settled (see the module documentation).
/// `local` says whether the document is on this computer.
pub fn prepare(markdown: &str, base: &str, local: bool) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut fenced = false;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced || !line.contains("![") {
            out.push_str(line);
        } else {
            out.push_str(&settle_images(line, base, local));
        }
    }
    out
}

fn settle_images(line: &str, base: &str, local: bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some((alt, tail)) = after.split_once("](") else {
            out.push_str(&rest[start..]);
            return out;
        };
        // An alt text with a bracket of its own is not an image we understand.
        if alt.contains(['\n', ']']) {
            out.push_str("![");
            rest = after;
            continue;
        }
        let Some(close) = tail.find(')') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inside = &tail[..close];
        let destination = inside
            .trim_start()
            .split(char::is_whitespace)
            .next()
            .unwrap_or("")
            .trim_matches(['<', '>']);
        let kept = format!("![{alt}]({inside})");
        let embedded = match resolve_link(base, destination) {
            _ if destination.starts_with("data:") || destination.starts_with("//") => {
                Some(kept.clone())
            }
            Target::Web(_) => Some(kept.clone()),
            Target::Nothing => Some(kept.clone()),
            Target::Path(path) if local => embed(&path).map(|url| format!("![{alt}]({url})")),
            Target::Path(_) => None,
        };
        match embedded {
            Some(text) => out.push_str(&text),
            None => {
                let label = if alt.trim().is_empty() {
                    destination
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(destination)
                } else {
                    alt.trim()
                };
                out.push_str(&format!("*[image: {label}]*"));
            }
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A picture of this computer as a `data:` URL, if it is one the view draws.
fn embed(path: &str) -> Option<String> {
    let mime = image_mime(path)?;
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

fn image_mime(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

/// Standard base64, padded.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

impl Shell {
    /// The document is Markdown, so it can be previewed.
    fn previewable(&self, id: LiveId) -> bool {
        self.files
            .get(&id)
            .is_some_and(|doc| doc.state().is_some() && has_page(&doc.path))
    }

    /// The file on screen shows its rendered page and no editor.
    pub(in crate::ui) fn focused_file_is_page_only(&self) -> bool {
        self.focused_file()
            .and_then(|id| self.files.get(&id))
            .is_some_and(|doc| doc.mode == ViewMode::Preview)
    }

    /// `TogglePreview`: the file on screen goes to the next mode.
    pub(in crate::ui) fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.focused_file() else {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "There is no file to preview.",
            );
            return;
        };
        if !self.previewable(id) {
            self.engine.report(
                crate::engine::StatusKind::Info,
                "Only a Markdown or SVG file has a preview.",
            );
            return;
        }
        let next = self.files[&id].mode.next();
        self.set_view_mode(id, next, window, cx);
    }

    /// Shows a Markdown file as `mode`.
    pub(in crate::ui) fn set_view_mode(
        &mut self,
        id: LiveId,
        mode: ViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.previewable(id) {
            return;
        }
        let text = self.files[&id].text(cx).unwrap_or_default();
        let base = parent_of(&self.files[&id].path).to_owned();
        let doc = self.files.get_mut(&id).expect("checked above");
        doc.mode = mode;
        if mode.shows_page() && is_svg(&doc.path) {
            doc.svg = Some(svg_page(&text));
        } else if mode.shows_page() && doc.preview.is_none() {
            let first = prepare(&text, &base, false);
            doc.preview = Some(cx.new(|cx| TextViewState::markdown(&first, cx)));
        }
        if mode.shows_page() {
            self.refresh_preview(id, true, cx);
        }
        self.sync_focus(window, cx);
        // An editor that was not drawn cannot take the keyboard until it is.
        let shell = cx.entity();
        window.on_next_frame(move |window, cx| {
            shell.update(cx, |this, cx| this.sync_focus(window, cx));
        });
        cx.notify();
    }

    /// Passes the text on to the page: at once when `now`, else after the
    /// pause that follows the last change.
    pub(in crate::ui) fn refresh_preview(&mut self, id: LiveId, now: bool, cx: &mut Context<Self>) {
        let pause = if now {
            std::time::Duration::ZERO
        } else {
            self.options.editor_debounce
        };
        let Some(doc) = self.files.get_mut(&id) else {
            return;
        };
        if !doc.mode.shows_page() {
            return;
        }
        if is_svg(&doc.path) {
            doc.preview_task = Some(cx.spawn(async move |this, cx| {
                if !pause.is_zero() {
                    cx.background_executor().timer(pause).await;
                }
                this.update(cx, |this, cx| {
                    if let Some(doc) = this.files.get_mut(&id) {
                        if let Some(text) = doc.text(cx) {
                            doc.svg = Some(svg_page(&text));
                        }
                    }
                    cx.notify();
                })
                .ok();
            }));
            return;
        }
        if doc.preview.is_none() {
            return;
        }
        doc.preview_task = Some(cx.spawn(async move |this, cx| {
            if !pause.is_zero() {
                cx.background_executor().timer(pause).await;
            }
            let Ok(Some((text, base, local, state))) = this.update(cx, |this, cx| {
                let doc = this.files.get(&id)?;
                Some((
                    doc.text(cx)?,
                    parent_of(&doc.path).to_owned(),
                    doc.machine.is_local(),
                    doc.preview.clone()?,
                ))
            }) else {
                return;
            };
            let page = cx
                .background_executor()
                .spawn(async move { prepare(&text, &base, local) })
                .await;
            state.update(cx, |state, cx| state.set_text(&page, cx));
        }));
    }

    /// A link in the page of `id` was clicked.
    pub(in crate::ui) fn preview_link(
        &mut self,
        id: LiveId,
        link: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.files.get(&id) else {
            return;
        };
        match resolve_link(parent_of(&doc.path), link) {
            Target::Web(url) => (self.options.open_url)(cx, &url),
            Target::Path(path) => {
                let machine = doc.machine.clone();
                self.open_file(machine, path, None, window, cx);
            }
            Target::Nothing => {}
        }
    }

    /// The leaf of a file with its preview, as `doc.mode` says.
    pub(in crate::ui) fn render_markdown_body(
        &self,
        id: LiveId,
        doc: &EditorDoc,
        editor: AnyElement,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = doc
            .preview
            .as_ref()
            .filter(|_| doc.mode.shows_page())
            .map(|state| {
                let shell = cx.weak_entity();
                div()
                    .debug_selector(move || format!("preview-{}", id.0))
                    .size_full()
                    .min_w_0()
                    .px(px(16.))
                    .child(
                        TextView::new(state)
                            .scrollable(true)
                            .selectable(true)
                            .on_link_click(
                                move |link: &SharedString, _: &ClickEvent, window, cx| {
                                    let link = link.to_string();
                                    shell
                                        .update(cx, |this, cx| {
                                            this.preview_link(id, &link, window, cx)
                                        })
                                        .ok();
                                },
                            ),
                    )
                    .into_any_element()
            });
        self.compose_page(id, doc.mode, editor, page, colours, cx)
    }

    /// The leaf of an SVG file: its text, its drawn page, or both.
    pub(in crate::ui) fn render_svg_body(
        &self,
        id: LiveId,
        doc: &EditorDoc,
        editor: AnyElement,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = doc
            .svg
            .clone()
            .filter(|_| doc.mode.shows_page())
            .map(|image| {
                div()
                    .debug_selector(move || format!("preview-{}", id.0))
                    .size_full()
                    .min_w_0()
                    .child(self.render_viewer(id, image, None, colours, cx))
                    .into_any_element()
            });
        self.compose_page(id, doc.mode, editor, page, colours, cx)
    }

    /// The editor and the page as `mode` says, under the toolbar.
    fn compose_page(
        &self,
        id: LiveId,
        mode: ViewMode,
        editor: AnyElement,
        page: Option<AnyElement>,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = match (mode, page) {
            (ViewMode::Split, Some(page)) => div()
                .size_full()
                .flex()
                .child(div().flex_1().min_w_0().h_full().child(editor))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_l_1()
                        .border_color(colours.border)
                        .child(page),
                )
                .into_any_element(),
            (ViewMode::Preview, Some(page)) => page,
            _ => editor,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_preview_toolbar(id, mode, colours, cx))
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// The three buttons above a Markdown file.
    fn render_preview_toolbar(
        &self,
        id: LiveId,
        mode: ViewMode,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = colours.surface_2;
        let mut row = div()
            .debug_selector(move || format!("preview-toolbar-{}", id.0))
            .flex_none()
            .h(px(24.))
            .px(px(8.))
            .gap(px(2.))
            .flex()
            .items_center()
            .justify_end()
            .border_b_1()
            .border_color(colours.border);
        for (target, label) in [
            (ViewMode::Edit, "Edit"),
            (ViewMode::Split, "Split"),
            (ViewMode::Preview, "Preview"),
        ] {
            let active = mode == target;
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "preview-{}-{}",
                        target.name(),
                        id.0
                    )))
                    .debug_selector(move || format!("preview-{}-{}", target.name(), id.0))
                    .px(px(8.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .rounded(metrics::RADIUS())
                    .cursor_pointer()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(if active {
                        colours.signal
                    } else {
                        colours.text_muted
                    })
                    .when(active, |this| this.bg(hover))
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.set_view_mode(id, target, window, cx);
                    }))
                    .child(label),
            );
        }
        row.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_and_markdown_names_have_a_page_and_nothing_else_does() {
        assert!(is_svg("/p/logo.SVG") && has_page("/p/logo.svg") && has_page("/p/a.md"));
        assert!(!has_page("/p/a.png") && !has_page("/p/svg") && !has_page("/p/a.svg.txt"));
    }

    #[test]
    fn only_markdown_names_have_a_preview() {
        assert!(is_markdown("/p/README.md"));
        assert!(is_markdown("C:\\p\\notes.MARKDOWN"));
        assert!(!is_markdown("/p/main.rs"));
        assert!(!is_markdown("/p/md"));
    }

    #[test]
    fn the_modes_go_round_and_are_named_back() {
        let mut mode = ViewMode::Edit;
        let mut seen = Vec::new();
        for _ in 0..3 {
            mode = mode.next();
            seen.push(mode);
        }
        assert_eq!(seen, [ViewMode::Split, ViewMode::Preview, ViewMode::Edit]);
        for mode in [ViewMode::Edit, ViewMode::Split, ViewMode::Preview] {
            assert_eq!(ViewMode::parse(mode.name()), mode);
        }
        assert_eq!(ViewMode::parse("nonsense"), ViewMode::Edit);
    }

    #[test]
    fn links_are_the_web_or_a_path_taken_from_the_document() {
        assert_eq!(
            resolve_link("/p/docs", "https://a.dev/x"),
            Target::Web("https://a.dev/x".into())
        );
        assert_eq!(
            resolve_link("/p/docs", "mailto:a@b.c"),
            Target::Web("mailto:a@b.c".into())
        );
        assert_eq!(
            resolve_link("/p/docs", "javascript:alert(1)"),
            Target::Nothing
        );
        assert_eq!(resolve_link("/p/docs", "#top"), Target::Nothing);
        assert_eq!(
            resolve_link("/p/docs", "guide.md#install"),
            Target::Path("/p/docs/guide.md".into())
        );
        assert_eq!(
            resolve_link("/p/docs", "../src/main.rs?plain=1"),
            Target::Path("/p/src/main.rs".into())
        );
        assert_eq!(
            resolve_link("/p/docs", "./a%20b.md"),
            Target::Path("/p/docs/a b.md".into())
        );
        assert_eq!(
            resolve_link("/p/docs", "/etc/hosts"),
            Target::Path("/etc/hosts".into())
        );
        assert_eq!(
            resolve_link("/p", "../../../x.md"),
            Target::Path("/x.md".into()),
            "the root is as far up as it goes"
        );
        assert_eq!(
            resolve_link("C:\\p\\docs", "..\\a.md"),
            Target::Path("C:\\p\\a.md".into())
        );
        assert_eq!(
            resolve_link("C:\\p", "../../a.md"),
            Target::Path("C:\\a.md".into())
        );
    }

    #[test]
    fn an_image_of_this_computer_is_embedded_and_one_elsewhere_is_said() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), [1u8, 2, 3, 4]).unwrap();
        let base = dir.path().to_str().unwrap();
        let text = "before ![logo](a.png) after\n![web](https://x.dev/i.png)\n";
        let here = prepare(text, base, true);
        assert!(
            here.contains("![logo](data:image/png;base64,AQIDBA==)"),
            "{here}"
        );
        assert!(here.contains("![web](https://x.dev/i.png)"));
        let there = prepare(text, "/srv/docs", false);
        assert!(there.contains("before *[image: logo]* after"), "{there}");
        assert!(there.contains("![web](https://x.dev/i.png)"));
        // A missing or unknown image is said too, and code is left alone.
        let odd = prepare("![x](missing.png) ![y](a.txt)\n", base, true);
        assert_eq!(odd, "*[image: x]* *[image: y]*\n");
        let code = "```\n![x](a.png)\n```\n";
        assert_eq!(prepare(code, base, true), code);
    }

    #[test]
    fn base64_pads_like_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }
}
