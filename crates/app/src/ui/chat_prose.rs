//! The words of an agent's answer, drawn the airy way: a list is a gutter and
//! its text, not a column of lines that start with a dot.
//!
//! The Markdown view draws a list with its marker right against the edge and
//! no space between the items, and offers no way to say otherwise. So the
//! answer is cut at its lists ([`parts`]): what is not a list is still one
//! Markdown view each, and a list is drawn here, an item at a time, a marker
//! in a gutter of [`metrics::LIST_INDENT`] and the item's own Markdown (which
//! may hold a list of its own) beside it. The text of the views is still
//! selected and copied across, like any other on the page.

use std::sync::Arc;

use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, AnyElement, App, ClickEvent, HighlightStyle, SharedString, StyleRefinement, Window,
};
use markdown::{mdast::Node, ParseOptions};

use crate::theme::{metrics, px, Palette};

/// A piece of an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Markdown that is not a list, as it was written.
    Prose(String),
    /// A list: each item's Markdown, written from the left edge.
    List {
        /// Numbered, not dotted.
        ordered: bool,
        /// The number of the first item of a numbered list.
        start: u32,
        /// What each item says.
        items: Vec<String>,
    },
}

/// `markdown` cut at its lists. An answer without one is a single piece, the
/// very same text. A list stays in the prose when it cannot be drawn apart
/// from what is around it: a task list (its boxes are the view's), or an
/// answer that has links by reference (their definitions live elsewhere in
/// the text).
pub fn parts(markdown: &str) -> Vec<Part> {
    // A cheap look first: most answers have no list, and a list needs a
    // marker at the start of a line.
    if !markdown.lines().any(starts_a_list) {
        return vec![Part::Prose(markdown.to_owned())];
    }
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    let Ok(Node::Root(root)) = markdown::to_mdast(markdown, &options) else {
        return vec![Part::Prose(markdown.to_owned())];
    };
    if root
        .children
        .iter()
        .any(|node| matches!(node, Node::Definition(_) | Node::FootnoteDefinition(_)))
    {
        return vec![Part::Prose(markdown.to_owned())];
    }
    let mut out = Vec::new();
    // The span of source of the prose being gathered.
    let mut prose: Option<(usize, usize)> = None;
    let flush = |prose: &mut Option<(usize, usize)>, out: &mut Vec<Part>| {
        if let Some((from, to)) = prose.take() {
            out.push(Part::Prose(markdown[from..to].to_owned()));
        }
    };
    for node in &root.children {
        let Some(span) = node.position().map(|p| (p.start.offset, p.end.offset)) else {
            return vec![Part::Prose(markdown.to_owned())];
        };
        match drawn_list(node, markdown) {
            Some(list) => {
                flush(&mut prose, &mut out);
                out.push(list);
            }
            None => {
                prose = Some(match prose {
                    Some((from, _)) => (from, span.1),
                    None => span,
                });
            }
        }
    }
    flush(&mut prose, &mut out);
    if out.iter().all(|part| matches!(part, Part::Prose(_))) {
        // Nothing was a list after all: the text as it was written.
        return vec![Part::Prose(markdown.to_owned())];
    }
    out
}

/// Whether a line may be the start of a list item: a bullet or a number, then
/// a space. Fences and indented code that look like one are sorted out by the
/// parser.
fn starts_a_list(line: &str) -> bool {
    let line = line.trim_start();
    let Some(first) = line.chars().next() else {
        return false;
    };
    if matches!(first, '-' | '*' | '+') {
        return line[1..].starts_with([' ', '\t']) || line.len() == 1;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && line[digits..].starts_with(['.', ')'])
        && (line[digits + 1..].starts_with([' ', '\t']) || line.len() == digits + 1)
}

/// A list as a [`Part::List`], when it can be drawn apart.
fn drawn_list(node: &Node, source: &str) -> Option<Part> {
    let Node::List(list) = node else {
        return None;
    };
    let mut items = Vec::with_capacity(list.children.len());
    for item in &list.children {
        let Node::ListItem(item) = item else {
            return None;
        };
        if item.checked.is_some() {
            return None;
        }
        items.push(match item.children.first().and_then(Node::position) {
            None => String::new(),
            Some(first) => {
                let end = item.position.as_ref()?.end.offset;
                // The text of the item starts after its marker and goes on,
                // line after line, indented as far as that start.
                let indent = first.start.column.saturating_sub(1);
                dedent(&source[first.start.offset..end], indent)
            }
        });
    }
    Some(Part::List {
        ordered: list.ordered,
        start: list.start.unwrap_or(1),
        items,
    })
}

/// `text` without the first `indent` spaces of each line after the first.
fn dedent(text: &str, indent: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for (n, line) in text.split_inclusive('\n').enumerate() {
        if n == 0 {
            out.push_str(line);
            continue;
        }
        let spaces = line.chars().take(indent).take_while(|c| *c == ' ').count();
        out.push_str(&line[spaces..]);
    }
    out.trim_end().to_owned()
}

/// What the marker of an item says: a dot (a different one for each level of
/// lists in lists) or its number.
pub fn marker(ordered: bool, start: u32, ix: usize, depth: usize) -> String {
    if ordered {
        format!(
            "{}.",
            start.saturating_add(u32::try_from(ix).unwrap_or(u32::MAX))
        )
    } else {
        ["\u{2022}", "\u{25E6}", "\u{25AA}"][depth.min(2)].to_owned()
    }
}

/// What a click on a link of the Markdown does.
pub type LinkHandler = Arc<dyn Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync>;

/// What is needed to draw the Markdown of an answer.
pub struct Prose<'a> {
    /// The name of the answer: its views are named after it.
    pub id: &'a str,
    /// The name of the pair of language and copy of its code blocks.
    pub code: &'a str,
    /// The group whose hover shows the corner of its code blocks.
    pub group: &'a SharedString,
    /// What a click on a link does, when it is not what the view does.
    pub on_link: Option<LinkHandler>,
    /// The colours.
    pub colours: &'a Palette,
}

/// The style of the Markdown of the page: roomy paragraphs, flat code.
pub fn style(colours: &Palette) -> TextViewStyle {
    let gap = metrics::PARAGRAPH_GAP().as_f32() / metrics::TEXT_BODY().as_f32();
    let block = StyleRefinement::default()
        .bg(colours.surface)
        .border_1()
        .border_color(colours.border)
        .p(px(12.))
        .text_size(metrics::TEXT_CODE())
        .line_height(metrics::CODE_LEADING());
    TextViewStyle::default()
        .paragraph_gap(gpui_kit::rems(gap))
        .heading_font_size(|level, base| match level {
            1 => base * 1.21,
            2 => base * 1.07,
            _ => base,
        })
        .code_block(block)
        .inline_code(HighlightStyle {
            background_color: Some(colours.text.opacity(0.08)),
            ..Default::default()
        })
}

impl Prose<'_> {
    /// The answer (or the item of a list) `markdown`, drawn. `depth` is how
    /// many lists it is in; `n` tells apart the views of the same place.
    pub fn draw(&self, markdown: &str, depth: usize, n: &str) -> AnyElement {
        let parts = parts(markdown);
        if let [Part::Prose(words)] = parts.as_slice() {
            return self.view(words, n);
        }
        let gap = if depth == 0 {
            metrics::PARAGRAPH_GAP()
        } else {
            metrics::LIST_GAP()
        };
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(gap)
            .children(parts.iter().enumerate().map(|(at, part)| match part {
                Part::Prose(words) => self.view(words, &format!("{n}.{at}")),
                Part::List {
                    ordered,
                    start,
                    items,
                } => self.list(*ordered, *start, items, depth, &format!("{n}.{at}")),
            }))
            .into_any_element()
    }

    fn list(
        &self,
        ordered: bool,
        start: u32,
        items: &[String],
        depth: usize,
        n: &str,
    ) -> AnyElement {
        let colours = self.colours;
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(metrics::LIST_GAP())
            .children(items.iter().enumerate().map(|(ix, item)| {
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(metrics::LIST_INDENT())
                            .pr(metrics::LIST_MARKER_GAP())
                            .flex()
                            .justify_end()
                            .text_color(colours.text_muted)
                            .child(marker(ordered, start, ix, depth)),
                    )
                    .child(div().flex_1().min_w_0().child(self.draw(
                        item,
                        depth + 1,
                        &format!("{n}.{ix}"),
                    )))
            }))
            .into_any_element()
    }

    /// One piece of Markdown that is not a list.
    fn view(&self, markdown: &str, n: &str) -> AnyElement {
        let code = self.code.to_owned();
        let group = self.group.clone();
        let palette = *self.colours;
        let on_link = self.on_link.clone();
        let view = TextView::markdown(
            SharedString::from(if n.is_empty() {
                self.id.to_owned()
            } else {
                format!("{}:{n}", self.id)
            }),
            SharedString::from(markdown.to_owned()),
        )
        .style(style(self.colours))
        .selectable(true)
        .code_block_actions(move |block, _, _| {
            super::chat_code::corner(
                code.clone(),
                block.lang().as_deref(),
                block.code(),
                &palette,
                Some(group.clone()),
            )
        });
        match on_link {
            Some(on_link) => view
                .on_link_click(move |url, event, window, cx| on_link(url, event, window, cx))
                .into_any_element(),
            None => view.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(words: &str) -> Part {
        Part::Prose(words.to_owned())
    }

    fn list(ordered: bool, start: u32, items: &[&str]) -> Part {
        Part::List {
            ordered,
            start,
            items: items.iter().map(|item| (*item).to_owned()).collect(),
        }
    }

    #[test]
    fn an_answer_without_a_list_is_one_piece_as_it_was_written() {
        for words in [
            "",
            "Hello **there**.",
            "One.\n\nTwo, with `code`.\n",
            "```sh\n- not a list\n```\n",
            "    - indented code\n",
            "A - dash in a line.\n",
        ] {
            assert_eq!(parts(words), vec![prose(words)], "{words:?}");
        }
    }

    #[test]
    fn a_list_is_cut_out_of_the_words_around_it() {
        assert_eq!(
            parts("Here is what matters:\n\n- one\n- two\n\nThat is all.\n"),
            vec![
                prose("Here is what matters:"),
                list(false, 1, &["one", "two"]),
                prose("That is all."),
            ]
        );
    }

    #[test]
    fn a_numbered_list_keeps_its_first_number() {
        assert_eq!(
            parts("3. three\n4. four\n"),
            vec![list(true, 3, &["three", "four"])]
        );
        assert_eq!(parts("1) a\n2) b\n"), vec![list(true, 1, &["a", "b"])]);
    }

    #[test]
    fn what_an_item_says_is_written_from_the_left_edge() {
        assert_eq!(
            parts("- first\n  goes on\n- second\n  - nested\n  - also\n"),
            vec![list(
                false,
                1,
                &["first\ngoes on", "second\n- nested\n- also"]
            )]
        );
        assert_eq!(
            parts("1. run:\n\n   ```sh\n   cargo test\n   ```\n2. done\n"),
            vec![list(true, 1, &["run:\n\n```sh\ncargo test\n```", "done"])]
        );
    }

    #[test]
    fn an_item_keeps_its_inline_marks() {
        assert_eq!(
            parts("- **Testing**: the `clock` is injected\n"),
            vec![list(false, 1, &["**Testing**: the `clock` is injected"])]
        );
    }

    #[test]
    fn an_empty_item_says_nothing() {
        assert_eq!(parts("- \n- b\n"), vec![list(false, 1, &["", "b"])]);
    }

    #[test]
    fn a_list_that_cannot_be_drawn_apart_stays_in_the_prose() {
        // Boxes are the view's.
        let tasks = "- [ ] one\n- [x] two\n";
        assert_eq!(parts(tasks), vec![prose(tasks)]);
        // A reference is defined away from where it is used.
        let reference = "- see [the docs]\n\n[the docs]: https://example.com\n";
        assert_eq!(parts(reference), vec![prose(reference)]);
    }

    #[test]
    fn two_lists_with_words_between_are_three_pieces() {
        assert_eq!(
            parts("- a\n\nthen\n\n1. b\n"),
            vec![list(false, 1, &["a"]), prose("then"), list(true, 1, &["b"])]
        );
    }

    #[test]
    fn a_list_being_written_is_cut_like_a_whole_one() {
        assert_eq!(
            parts("Steps:\n\n- one\n- tw"),
            vec![prose("Steps:"), list(false, 1, &["one", "tw"])]
        );
    }

    #[test]
    fn the_marker_is_a_dot_or_a_number_that_counts_on() {
        assert_eq!(marker(false, 1, 0, 0), "\u{2022}");
        assert_eq!(marker(false, 1, 3, 1), "\u{25E6}");
        assert_eq!(marker(false, 1, 0, 9), "\u{25AA}");
        assert_eq!(marker(true, 1, 0, 0), "1.");
        assert_eq!(marker(true, 3, 2, 0), "5.");
    }
}
