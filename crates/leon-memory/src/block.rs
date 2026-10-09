//! The managed block: the few lines of an instruction file that tell an agent
//! the shared memory exists.
//!
//! The block sits between two marker comments that name Leon. [`insert`] puts
//! it at the end of a text, or replaces the one already there; [`remove`]
//! takes it out. Both leave every other byte alone, and they are exact
//! inverses: `remove(insert(text)) == text` for any text without a block, and
//! `insert(insert(text)) == insert(text)`. To make that hold the block follows
//! the text it joins: it uses the text's line ending (`\r\n` when the text
//! does), and it ends with a line break only when the text did, so a file
//! keeps its final newline or its lack of one.
//!
//! The wording is the same on every computer. An instruction file is usually
//! committed, so the block names no path: it points at two environment
//! variables Leon sets in its terminals, and says to ignore the block where
//! they are unset. It is short on purpose: an agent reads it in every session.

use crate::{ENV_BIN, ENV_MEMORY};

/// What the line that opens the block starts with.
const BEGIN_PREFIX: &str = "<!-- leon:memory:begin";
/// The line that opens the block in a file that was there before.
const BEGIN: &str =
    "<!-- leon:memory:begin (managed by Leon; `leon memory disable` removes it) -->";
/// The line that opens it in a file Leon made to hold it. Taking the block out
/// of such a file removes the file; out of any other, it leaves the file as it
/// was, also when that was empty.
const BEGIN_CREATED: &str =
    "<!-- leon:memory:begin (this file was created and is managed by Leon; \
                             `leon memory disable` removes it) -->";
/// The line that closes the block.
pub const END: &str = "<!-- leon:memory:end -->";

/// The lines of the block, markers included.
pub fn lines() -> Vec<String> {
    lines_with(BEGIN)
}

fn lines_with(begin: &str) -> Vec<String> {
    vec![
        begin.to_owned(),
        "## Shared memory".to_owned(),
        String::new(),
        format!(
            "If `${ENV_MEMORY}` is set, read that file before starting work: it holds notes \
             from earlier sessions of every agent in this project."
        ),
        format!("- Search it: `\"${ENV_BIN}\" memory search <words>`"),
        format!(
            "- Save a durable decision, convention or discovery, one fact per entry: \
             `\"${ENV_BIN}\" memory add --kind <decision|convention|discovery|preference|note> \
             \"<the fact>\"` (add `--topic <key>` to a fact that may change: the same key \
             revises it)"
        ),
        "- Outside Leon both variables are unset: ignore this section.".to_owned(),
        END.to_owned(),
    ]
}

/// The line ending a text uses: `\r\n` when its first line ends so.
fn line_ending(text: &str) -> &'static str {
    match text.find('\n') {
        Some(at) if text[..at].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

/// The block as text with `eol` between its lines and none after the last.
fn block(eol: &str) -> String {
    lines().join(eol)
}

/// The same for a file Leon made.
fn created_block(eol: &str) -> String {
    lines_with(BEGIN_CREATED).join(eol)
}

/// The text of a file made to hold the block and nothing else.
pub fn create() -> String {
    format!("{}\n", created_block("\n"))
}

/// Whether a text is a file Leon made for the block that still holds nothing
/// else: the file to remove when the block goes.
pub fn created_alone(text: &str) -> bool {
    find(text).is_some_and(|(start, _)| text[start..].starts_with(BEGIN_CREATED))
        && remove(text).is_empty()
}

/// Where the first whole block is: from the start of its opening line to the
/// end of its closing marker. An opening line without a closing one is not a
/// block.
fn find(text: &str) -> Option<(usize, usize)> {
    // A marker counts at the start of a line only.
    let opens_at = |at: usize| at == 0 || text[..at].ends_with('\n');
    let mut from = 0;
    while let Some(found) = text[from..].find(BEGIN_PREFIX) {
        let start = from + found;
        from = start + BEGIN_PREFIX.len();
        if !opens_at(start) {
            continue;
        }
        let end = from + text[from..].find(END)?;
        // An opening line followed by another before any closing one was
        // never closed: the block is the later one.
        let reopened = text[from..end]
            .match_indices(BEGIN_PREFIX)
            .any(|(at, _)| opens_at(from + at));
        if !reopened {
            return Some((start, end + END.len()));
        }
    }
    None
}

/// Whether a text holds the block.
pub fn present(text: &str) -> bool {
    find(text).is_some()
}

/// Whether a text holds the block exactly as this build writes it.
pub fn current(text: &str) -> bool {
    let eol = line_ending(text);
    find(text).is_some_and(|(start, end)| {
        text[start..end] == block(eol) || text[start..end] == created_block(eol)
    })
}

/// The text with the block: the one it has replaced, or a new one at the end
/// after an empty line.
pub fn insert(text: &str) -> String {
    let eol = line_ending(text);
    if let Some((start, end)) = find(text) {
        // A file Leon made stays marked as one.
        let block = if text[start..].starts_with(BEGIN_CREATED) {
            created_block(eol)
        } else {
            block(eol)
        };
        return format!("{}{block}{}", &text[..start], &text[end..]);
    }
    let block = block(eol);
    if text.is_empty() {
        return format!("{block}{eol}");
    }
    if text.ends_with('\n') {
        format!("{text}{eol}{block}{eol}")
    } else {
        // No final newline before, none after.
        format!("{text}{eol}{eol}{block}")
    }
}

/// The text without its block, and without the empty line [`insert`] put
/// before it. A text without a block comes back as it is.
pub fn remove(text: &str) -> String {
    let Some((start, end)) = find(text) else {
        return text.to_owned();
    };
    let eol = line_ending(text);
    let (before, after) = (&text[..start], &text[end..]);
    if after.is_empty() {
        // The block ends the text and the text has no final newline: the two
        // line breaks before it are the block's.
        let before = before.strip_suffix(eol).unwrap_or(before);
        let before = before.strip_suffix(eol).unwrap_or(before);
        return before.to_owned();
    }
    // The block's own line break goes with it.
    let after = after.strip_prefix(eol).unwrap_or(after);
    if after.is_empty() {
        // The block was the end: so does the empty line before it.
        let before = match before.strip_suffix(eol) {
            Some(shorter) if shorter.is_empty() || shorter.ends_with('\n') => shorter,
            _ => before,
        };
        return before.to_owned();
    }
    format!("{before}{after}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texts without a block, of every shape that matters.
    fn samples() -> Vec<String> {
        let mut samples: Vec<String> = [
            "",
            "\n",
            "\n\n",
            "a",
            "a\n",
            "a\n\n",
            "a\n\n\n",
            "# Project\n\nUse pnpm.\n",
            "# Project\n\nUse pnpm.",
            "\r\n",
            "a\r\n",
            "a\r\nb",
            "# Project\r\n\r\nUse pnpm.\r\n",
            "# Project\r\n\r\nUse pnpm.\r\n\r\n",
            "mixed\r\nline\nendings\n",
            "an opening marker alone\n<!-- leon:memory:begin -->\nis not a block\n",
            "inline <!-- leon:memory:begin --> and <!-- leon:memory:end --> are prose\n",
            "tabs\tand trailing spaces   \n",
            "ünïcödé 🦁\n",
        ]
        .map(str::to_owned)
        .to_vec();
        samples.push("line\n".repeat(500));
        samples
    }

    #[test]
    fn removing_what_was_inserted_gives_the_text_back_byte_for_byte() {
        for text in samples() {
            let with = insert(&text);
            assert!(present(&with), "{text:?}");
            assert!(current(&with), "{text:?}");
            assert_eq!(remove(&with), text, "{text:?}");
        }
    }

    #[test]
    fn inserting_twice_is_inserting_once() {
        for text in samples() {
            let once = insert(&text);
            assert_eq!(insert(&once), once, "{text:?}");
        }
    }

    #[test]
    fn everything_before_the_block_is_untouched() {
        for text in samples() {
            assert!(insert(&text).starts_with(&text), "{text:?}");
            assert_eq!(remove(&text), text, "{text:?}");
            assert!(!present(&text), "{text:?}");
        }
    }

    #[test]
    fn the_block_follows_the_texts_line_ending_and_final_newline() {
        let unix = insert("a\n");
        assert!(!unix.contains('\r'));
        assert!(unix.ends_with("<!-- leon:memory:end -->\n"));
        assert!(unix.starts_with("a\n\n<!-- leon:memory:begin"));

        let windows = insert("a\r\n");
        assert_eq!(
            windows.matches('\n').count(),
            windows.matches("\r\n").count()
        );
        assert!(windows.ends_with("<!-- leon:memory:end -->\r\n"));

        let open = insert("a");
        assert!(open.ends_with("<!-- leon:memory:end -->"));
        assert!(insert("").ends_with("<!-- leon:memory:end -->\n"));
    }

    #[test]
    fn an_old_block_is_replaced_where_it_is() {
        let old =
            "top\n\n<!-- leon:memory:begin v0 -->\nold words\n<!-- leon:memory:end -->\n\nbottom\n";
        assert!(present(old));
        assert!(!current(old));
        let new = insert(old);
        assert!(new.starts_with("top\n\n<!-- leon:memory:begin (managed"));
        assert!(new.ends_with("<!-- leon:memory:end -->\n\nbottom\n"));
        assert!(!new.contains("old words"));
        assert!(current(&new));
        assert_eq!(new.matches(END).count(), 1);
    }

    /// The block as the first build of the memory wrote it, before `--topic`.
    const FIRST_WORDING: &str = "<!-- leon:memory:begin (managed by Leon; `leon memory disable` removes it) -->\n## Shared memory\n\nIf `$LEON_MEMORY` is set, read that file before starting work: it holds notes from earlier sessions of every agent in this project.\n- Search it: `\"$LEON_BIN\" memory search <words>`\n- Save a durable decision, convention or discovery, one fact per entry: `\"$LEON_BIN\" memory add --kind <decision|convention|discovery|preference|note> \"<the fact>\"`\n- Outside Leon both variables are unset: ignore this section.\n<!-- leon:memory:end -->";

    #[test]
    fn a_block_of_an_earlier_wording_is_replaced_in_place_and_still_comes_out_clean() {
        for original in [
            "# Project\n\nUse pnpm.\n",
            "# Project\r\n",
            "no newline",
            "",
        ] {
            let eol = line_ending(original);
            let old_block = FIRST_WORDING.replace('\n', eol);
            // What the earlier build left in the file.
            let old = if original.is_empty() {
                format!("{old_block}{eol}")
            } else if original.ends_with('\n') {
                format!("{original}{eol}{old_block}{eol}")
            } else {
                format!("{original}{eol}{eol}{old_block}")
            };
            assert!(present(&old) && !current(&old), "{original:?}");
            let new = insert(&old);
            assert!(current(&new), "{original:?}");
            assert!(new.contains("--topic"));
            assert_eq!(new.matches(END).count(), 1);
            // Exactly what turning it on afresh would have written.
            assert_eq!(new, insert(original), "{original:?}");
            assert_eq!(remove(&new), original, "{original:?}");
            // And the old block itself comes out as cleanly.
            assert_eq!(remove(&old), original, "{original:?}");
        }
    }

    #[test]
    fn a_file_leon_made_is_told_from_one_that_was_empty() {
        let made = create();
        assert!(current(&made));
        assert!(created_alone(&made));
        assert_eq!(remove(&made), "");
        // An empty file that got the block is not a file Leon made.
        let was_empty = insert("");
        assert_ne!(was_empty, made);
        assert!(current(&was_empty));
        assert!(!created_alone(&was_empty));
        assert_eq!(remove(&was_empty), "");
        // Somebody wrote into the file Leon made: it is theirs now.
        let grown = format!("{made}\n# Notes\n");
        assert!(!created_alone(&grown));
        assert_eq!(remove(&grown), "\n# Notes\n");
        // Bringing an older block of such a file up to date keeps the mark.
        let old = made.replace("## Shared memory", "## Old title");
        assert!(!current(&old));
        assert_eq!(insert(&old), made);
        assert_eq!(insert(&made), made);
        // The wording differs in the first line only, and names no path.
        assert!(!made.contains('/'));
        assert_eq!(made.lines().count(), lines().len());
    }

    #[test]
    fn a_block_in_the_middle_goes_and_what_follows_it_stays() {
        let text = format!("top\n\n{}\n\nbottom\n", block("\n"));
        assert_eq!(remove(&text), "top\n\n\nbottom\n");
        let glued = format!("top\n{}\nbottom", block("\n"));
        assert_eq!(remove(&glued), "top\nbottom");
    }

    #[test]
    fn a_block_somebody_moved_to_the_top_is_removed_without_its_neighbours() {
        let text = format!("{}\n# Project\n", block("\n"));
        assert_eq!(remove(&text), "# Project\n");
    }

    #[test]
    fn the_wording_names_no_path_and_is_short() {
        let text = block("\n");
        assert!(!text.contains('/'), "{text}");
        assert!(!text.contains('\\'), "{text}");
        assert!(text.contains("$LEON_MEMORY"));
        assert!(text.contains("\"$LEON_BIN\" memory search"));
        assert!(text.contains("\"$LEON_BIN\" memory add"));
        assert!(text.contains("one fact per entry"));
        assert!(text.contains("Outside Leon"));
        assert!(text.contains("--topic <key>"));
        assert!(text.len() < 760, "{} bytes", text.len());
        assert_eq!(lines().len(), 8);
        assert!(lines().iter().all(|line| !line.contains('\n')));
    }
}
