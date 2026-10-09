//! Drawing a file: its leaf, its tab and the header above it.

use super::super::live::LiveId;
use super::super::shell::Shell;
use super::document::{image_format, Body, EditorDoc, Picture};
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::input::Editor;
use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, MouseDownEvent, SharedString};

impl Shell {
    /// The leaf of a file: its editor, or what it is when it has none, with
    /// the accent outline when it is the focused one of several.
    pub(in crate::ui) fn render_file_pane(
        &self,
        id: LiveId,
        marked: bool,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let doc = self.files.get(&id)?;
        let content = match &doc.body {
            Body::Text(state) => {
                // The family is the terminal's; the size is the editor's own.
                let font = Self::terminal_font(cx).family;
                let size = crate::theme::px(crate::settings::int(cx, "editor_font_size") as f32);
                let editor = div()
                    .size_full()
                    .debug_selector(move || format!("file-{}", id.0))
                    .child(
                        Editor::new(state)
                            .bordered(false)
                            .size_full()
                            .font_family(font)
                            .text_size(size),
                    )
                    .into_any_element();
                if super::preview::is_markdown(&doc.path) {
                    self.render_markdown_body(id, doc, editor, colours, cx)
                } else if super::preview::is_svg(&doc.path) {
                    self.render_svg_body(id, doc, editor, colours, cx)
                } else {
                    editor
                }
            }
            Body::Image(picture) => {
                let viewer =
                    self.render_viewer(id, picture.image.clone(), picture.dimensions, colours, cx);
                picture_view(id, picture, viewer, colours)
            }
            Body::Binary(size) if image_format(&doc.path).is_some() => placeholder(
                "image-elsewhere",
                format!(
                    "{} is a picture on another machine, which Leon does not show yet.",
                    doc.name()
                ),
                colours,
            ),
            Body::Binary(size) => self.placeholder_with_action(
                id,
                "binary",
                format!("{} is not text ({}).", doc.name(), size_text(*size)),
                doc,
                colours,
                cx,
            ),
            Body::TooBig(size) => self.placeholder_with_action(
                id,
                "too-big",
                format!(
                    "{} is too big to edit here ({}).",
                    doc.name(),
                    size_text(*size)
                ),
                doc,
                colours,
                cx,
            ),
            Body::Lossy { state, shown, size } => {
                let note = if shown < size {
                    format!(
                        "Read only: the first {} of {}, bytes that are not text shown as \u{fffd}.",
                        size_text(*shown),
                        size_text(*size)
                    )
                } else {
                    "Read only: bytes that are not text are shown as \u{fffd}.".to_owned()
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .debug_selector(move || format!("file-lossy-{}", id.0))
                            .flex_none()
                            .px(px(12.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(colours.border)
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_faint)
                            .child(note),
                    )
                    .child(
                        div().flex_1().min_h_0().child(
                            Editor::new(state)
                                .bordered(false)
                                .size_full()
                                .readonly(true)
                                .font_family(Self::terminal_font(cx).family)
                                .text_size(crate::theme::px(crate::settings::int(
                                    cx,
                                    "editor_font_size",
                                )
                                    as f32)),
                        ),
                    )
                    .into_any_element()
            }
        };
        // The file changed on disk while there are changes here.
        let content = match doc.external {
            Some(_) => div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.render_changed_banner(id, doc, colours, cx))
                .child(div().flex_1().min_h_0().child(content))
                .into_any_element(),
            None => content,
        };
        Some(
            div()
                .debug_selector(move || format!("pane-{}", id.0))
                .relative()
                .size_full()
                .min_w_0()
                .min_h_0()
                .rounded(metrics::RADIUS_CELL())
                .border_1()
                .border_color(if marked {
                    colours.signal
                } else {
                    gpui_kit::transparent_black()
                })
                // A click in the editor makes it the focused pane of its tab.
                .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.pane = super::super::shell::Pane::Main;
                    this.open_live(id, window, cx);
                }))
                .child(content)
                .into_any_element(),
        )
    }

    /// A placeholder, with `Open anyway` under it for a file on this
    /// computer (the bytes of one elsewhere would have to cross the wire).
    fn placeholder_with_action(
        &self,
        id: LiveId,
        selector: &'static str,
        text: String,
        doc: &EditorDoc,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = colours.surface_2;
        let action = doc.machine.is_local().then(|| {
            div()
                .id(SharedString::from(format!("open-anyway-{}", id.0)))
                .debug_selector(move || format!("open-anyway-{}", id.0))
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .cursor_pointer()
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text)
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_anyway(id, window, cx);
                }))
                .child("Open anyway")
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .child(placeholder(selector, text, colours))
            .children(action)
            .into_any_element()
    }

    /// The banner above an editor whose file changed on disk while the text
    /// here has changes: not a dialog, the text can still be edited.
    fn render_changed_banner(
        &self,
        id: LiveId,
        doc: &EditorDoc,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = colours.surface_2;
        let button = |label: &'static str, which: &'static str| {
            div()
                .id(SharedString::from(format!("changed-{which}-{}", id.0)))
                .debug_selector(move || format!("changed-{which}-{}", id.0))
                .px(px(8.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .cursor_pointer()
                .text_color(colours.text)
                .hover(move |style| style.bg(hover))
                .child(label)
        };
        div()
            .debug_selector(move || format!("changed-banner-{}", id.0))
            .flex_none()
            .h(px(30.))
            .px(px(12.))
            .gap(px(8.))
            .flex()
            .items_center()
            .bg(colours.surface)
            .border_b_1()
            .border_color(colours.border)
            .text_size(metrics::TEXT_SMALL())
            .text_color(colours.warning)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(format!("{} changed on disk.", doc.name())),
            )
            .child(button("Reload", "reload").on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    this.reload_file(id, window, cx);
                },
            )))
            .child(button("Keep mine", "keep").on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.keep_mine(id, cx);
                },
            )))
            .into_any_element()
    }

    /// The tab of a file: its name, with a dot while it has unsaved changes.
    pub(in crate::ui) fn file_label(doc: &EditorDoc) -> String {
        if doc.dirty {
            format!("{} \u{2022}", doc.name())
        } else {
            doc.name().to_owned()
        }
    }

    /// What the header says of a file: the kind of thing, its name and a line
    /// about it.
    pub(in crate::ui) fn file_heading(&self, doc: &EditorDoc) -> (&'static str, String, String) {
        let machine = self
            .snapshot
            .machine(&doc.machine)
            .map_or_else(String::new, |machine| machine.name.to_uppercase());
        let state = if doc.conflict {
            " \u{b7} CHANGED ON DISK"
        } else if doc.dirty {
            " \u{b7} UNSAVED CHANGES"
        } else {
            ""
        };
        (
            "File",
            Self::file_label(doc),
            format!("{machine} \u{b7} {}{state}", doc.path),
        )
    }
}

/// A picture under which its size in pixels and bytes are said.
fn picture_view(
    id: LiveId,
    picture: &Picture,
    viewer: AnyElement,
    colours: &Palette,
) -> AnyElement {
    let facts = match picture.dimensions {
        Some((width, height)) => {
            format!("{width} \u{d7} {height} \u{b7} {}", size_text(picture.size))
        }
        None => size_text(picture.size),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().flex_1().min_h_0().child(viewer))
        .child(
            div()
                .debug_selector(move || format!("file-image-facts-{}", id.0))
                .flex_none()
                .px(px(12.))
                .h(px(24.))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(colours.border)
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.text_faint)
                .child(facts),
        )
        .into_any_element()
}

/// A leaf that says what the file is instead of showing it.
fn placeholder(selector: &'static str, text: String, colours: &Palette) -> AnyElement {
    div()
        .debug_selector(move || format!("file-{selector}"))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(metrics::TEXT_SMALL())
        .text_color(colours.text_faint)
        .px(px(16.))
        .child(text)
        .into_any_element()
}

/// A size in bytes, as a person reads it.
fn size_text(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} bytes"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}
