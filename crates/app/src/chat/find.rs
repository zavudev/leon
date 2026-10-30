//! Finding words in a conversation: which messages hold them, where, and the
//! way from one match to the next.
//!
//! Only what a person reads as words is searched: what they said and what the
//! agent answered. The match is without regard to case.
//!
//! Pure: items and a query in, matches out.

use std::ops::Range;

use super::model::Item;

/// One place a query is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The index of the item in the conversation.
    pub item: usize,
    /// Where in its text, in bytes.
    pub range: Range<usize>,
}

/// Every match of `query` in the messages of `items`, in reading order. An
/// empty (or all-blank) query matches nothing.
pub fn matches(items: &[Item], query: &str) -> Vec<Match> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .enumerate()
        .filter_map(|(item, it)| match it {
            Item::User { text, .. } | Item::Agent { text, .. } => Some((item, text.as_str())),
            _ => None,
        })
        .flat_map(|(item, text)| {
            ranges(text, query)
                .into_iter()
                .map(move |range| Match { item, range })
        })
        .collect()
}

/// The ranges of `text` that read as `query`, ignoring case, not overlapping.
fn ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    // The text as lowercase characters, each with the bytes of the
    // character it came from: lowercasing may make one character of
    // several, or change its size in bytes.
    let folded: Vec<(char, Range<usize>)> = text
        .char_indices()
        .flat_map(|(at, c)| {
            let end = at + c.len_utf8();
            c.to_lowercase().map(move |lower| (lower, at..end))
        })
        .collect();
    let wanted: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut found = Vec::new();
    let mut from = 0;
    while from + wanted.len() <= folded.len() {
        let window = &folded[from..from + wanted.len()];
        if window.iter().map(|(c, _)| *c).eq(wanted.iter().copied()) {
            let start = window[0].1.start;
            let end = window[window.len() - 1].1.end;
            found.push(start..end);
            from += wanted.len();
        } else {
            from += 1;
        }
    }
    found
}

/// The match after (`forward`) or before `current` of `total`, going round
/// from one end to the other; `None` when there are none.
pub fn step(current: usize, total: usize, forward: bool) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let current = current.min(total - 1);
    Some(if forward {
        (current + 1) % total
    } else {
        (current + total - 1) % total
    })
}

/// The match to show first when the query changes: the first one at or after
/// the item the person is reading, else the first of all; `None` when there
/// are none.
pub fn first_from(found: &[Match], item: usize) -> Option<usize> {
    if found.is_empty() {
        return None;
    }
    Some(found.iter().position(|m| m.item >= item).unwrap_or(0))
}

/// "3 / 12", or "No matches" for a query that found nothing.
pub fn count_text(current: usize, total: usize) -> String {
    if total == 0 {
        "No matches".to_owned()
    } else {
        format!("{} / {total}", current.min(total - 1) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Item {
        Item::User {
            text: text.into(),
            at: None,
        }
    }

    fn answer(text: &str) -> Item {
        Item::Agent {
            text: text.into(),
            streaming: false,
            at: None,
        }
    }

    fn items() -> Vec<Item> {
        vec![
            said("fix the Parser"),
            Item::Thought {
                text: "parser parser".into(),
            },
            answer("The parser is fixed. Parsers are fun, parser!"),
            Item::Notice {
                tone: crate::chat::model::Tone::Plain,
                text: "parser".into(),
            },
        ]
    }

    #[test]
    fn a_query_is_found_in_what_was_said_and_answered_without_regard_to_case() {
        let found = matches(&items(), "PARSER");
        let places: Vec<(usize, Range<usize>)> =
            found.iter().map(|m| (m.item, m.range.clone())).collect();
        // The thought and the notice are not read as words.
        assert_eq!(places, [(0, 8..14), (2, 4..10), (2, 21..27), (2, 38..44)]);
        assert_eq!(&"fix the Parser"[8..14], "Parser");
        let answered = "The parser is fixed. Parsers are fun, parser!";
        assert_eq!(&answered[21..27], "Parser");
        assert_eq!(&answered[38..44], "parser");
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert!(matches(&items(), "").is_empty());
        assert!(matches(&items(), "   ").is_empty());
        assert!(matches(&items(), "zzz").is_empty());
    }

    #[test]
    fn ranges_are_those_of_the_original_text_even_where_lowercasing_changes_it() {
        // `İ` lowercases to two characters (`i` and a combining dot).
        let found = matches(&[answer("a İx b")], "x");
        assert_eq!(found.len(), 1);
        let text = "a İx b";
        assert_eq!(&text[found[0].range.clone()], "x");
        let found = matches(&[answer("Straße STRASSE")], "straße");
        assert_eq!(&"Straße STRASSE"[found[0].range.clone()], "Straße");
    }

    #[test]
    fn matches_do_not_overlap() {
        let found = matches(&[answer("aaaa")], "aa");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn stepping_goes_round_in_both_directions() {
        assert_eq!(step(0, 3, true), Some(1));
        assert_eq!(step(2, 3, true), Some(0));
        assert_eq!(step(0, 3, false), Some(2));
        assert_eq!(step(1, 3, false), Some(0));
        assert_eq!(step(0, 1, true), Some(0));
        assert_eq!(step(0, 0, true), None);
        // A position past the end (the matches shrank) counts as the last.
        assert_eq!(step(9, 3, true), Some(0));
    }

    #[test]
    fn the_first_match_shown_is_the_first_one_in_view_or_after() {
        let found = matches(&items(), "parser");
        assert_eq!(first_from(&found, 0), Some(0));
        assert_eq!(first_from(&found, 2), Some(1));
        // Nothing after: back to the first.
        assert_eq!(first_from(&found, 3), Some(0));
        assert_eq!(first_from(&[], 0), None);
    }

    #[test]
    fn the_count_reads_as_a_position_in_the_matches() {
        assert_eq!(count_text(0, 12), "1 / 12");
        assert_eq!(count_text(11, 12), "12 / 12");
        assert_eq!(count_text(0, 0), "No matches");
    }
}
