//! Find in the terminal: the matches of a query over the whole scrollback and
//! the screen, and the current one.
//!
//! The search is the emulator's own ([`RegexSearch`]), which reads a line that
//! wrapped as one line, so a word split by the right edge of the window is
//! still found. [`pattern`] turns what is typed into the regular expression it
//! runs:
//!
//! * **Smart case.** Case-insensitive, unless the query holds an uppercase
//!   letter or the case toggle is on.
//! * **Whole word** wraps the pattern in ASCII word boundaries.
//! * **Regular expression** off (the default) searches for the text itself:
//!   every character with a meaning in a pattern is escaped.
//! * A pattern that does not compile is reported in [`FindState::error`], never
//!   a panic, and the previous matches are dropped.
//!
//! [`FindState`] holds the matches in reading order. The first search starts
//! at the bottom of what is on screen and goes **up** (the newest output is
//! what one usually wants): the current match is the last one at or above that
//! point. [`FindState::older`] steps up the history and [`FindState::newer`]
//! down, both wrapping around (and saying so in [`FindState::wrapped`]).
//! Searching again after the terminal printed more keeps the current match
//! where it was in the text where it can, and no more than [`MATCH_LIMIT`]
//! matches are kept.

use crate::terminal::EventProxy;
use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::Term;

/// The most matches kept. A search over ten thousand lines of dense text stops
/// here: a count beyond it means nothing to a reader.
pub const MATCH_LIMIT: usize = 10_000;

/// How a query is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Always case-sensitive (otherwise smart case).
    pub case_sensitive: bool,
    /// Only whole words.
    pub whole_word: bool,
    /// The query is a regular expression, not the text itself.
    pub regex: bool,
}

/// The regular expression that `query` stands for under `options`.
pub fn pattern(query: &str, options: FindOptions) -> String {
    let body = if options.regex {
        query.to_owned()
    } else {
        let mut escaped = String::with_capacity(query.len() * 2);
        for c in query.chars() {
            if "\\.+*?()|[]{}^$#&-~".contains(c) {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        escaped
    };
    let body = if options.whole_word {
        format!("(?-u:\\b)(?:{body})(?-u:\\b)")
    } else {
        body
    };
    let sensitive = options.case_sensitive || query.chars().any(char::is_uppercase);
    format!("{}{body}", if sensitive { "(?-i)" } else { "(?i)" })
}

/// The matches the grid shows highlighted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Highlights {
    /// Every match, in reading order.
    pub matches: Vec<Match>,
    /// The one the find bar is on.
    pub current: Option<usize>,
}

/// The matches of a query and where the keyboard is among them.
#[derive(Clone, Debug, Default)]
pub struct FindState {
    query: String,
    options: FindOptions,
    matches: Vec<Match>,
    current: Option<usize>,
    error: Option<String>,
    capped: bool,
    wrapped: bool,
    /// Where the current match was, as lines from the oldest one kept, to find
    /// it again after the history moved.
    anchor: Option<(i64, usize)>,
}

impl FindState {
    /// A search with nothing typed.
    pub fn new() -> Self {
        Self::default()
    }

    /// The query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// How the query is read.
    pub fn options(&self) -> FindOptions {
        self.options
    }

    /// Every match, in reading order (oldest first).
    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    /// The place of the current match among [`Self::matches`].
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// What the grid shows for this search.
    pub fn highlights(&self) -> Highlights {
        Highlights {
            matches: self.matches.clone(),
            current: self.current,
        }
    }

    /// The current match.
    pub fn current_match(&self) -> Option<&Match> {
        self.matches.get(self.current?)
    }

    /// Why the query cannot be searched for, if it cannot.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the search stopped at [`MATCH_LIMIT`].
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// Whether the last step went round the end of the buffer.
    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    /// Changes what is searched for. Call [`Self::search`] after.
    pub fn set(&mut self, query: &str, options: FindOptions) {
        if self.query != query || self.options != options {
            self.anchor = None;
            self.current = None;
        }
        self.query = query.to_owned();
        self.options = options;
    }

    /// Looks for the matches again in the buffer as it is now.
    pub fn search(&mut self, term: &Term<EventProxy>) {
        self.error = None;
        self.capped = false;
        self.wrapped = false;
        let previous = self.anchor;
        self.matches.clear();
        self.current = None;
        if self.query.is_empty() {
            self.anchor = None;
            return;
        }
        let mut regex = match RegexSearch::new(&pattern(&self.query, self.options)) {
            Ok(regex) => regex,
            Err(error) => {
                let text = error.to_string();
                self.error = Some(
                    text.lines()
                        .next()
                        .unwrap_or("invalid expression")
                        .to_owned(),
                );
                self.anchor = None;
                return;
            }
        };
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), Column(term.columns() - 1));
        for found in RegexIter::new(start, end, Direction::Right, term, &mut regex) {
            if self.matches.len() >= MATCH_LIMIT {
                self.capped = true;
                break;
            }
            self.matches.push(found);
        }
        if self.matches.is_empty() {
            self.anchor = None;
            return;
        }
        let history = term.grid().history_size() as i64;
        self.current = Some(match previous {
            // Where the current match was in the text, wherever it is now.
            Some((line, column)) => {
                let wanted = Point::new(Line((line - history) as i32), Column(column));
                self.nearest(wanted)
            }
            // The last match at or above the bottom of the screen.
            None => {
                let bottom = Point::new(term.bottommost_line(), Column(term.columns() - 1));
                self.matches
                    .iter()
                    .rposition(|found| *found.start() <= bottom)
                    .unwrap_or(self.matches.len() - 1)
            }
        });
        self.remember(term);
    }

    fn nearest(&self, wanted: Point) -> usize {
        let key = |point: Point| i64::from(point.line.0) * 100_000 + point.column.0 as i64;
        self.matches
            .iter()
            .enumerate()
            .min_by_key(|(_, found)| (key(*found.start()) - key(wanted)).abs())
            .map_or(0, |(at, _)| at)
    }

    fn remember(&mut self, term: &Term<EventProxy>) {
        let history = term.grid().history_size() as i64;
        self.anchor = self.current_match().map(|found| {
            (
                i64::from(found.start().line.0) + history,
                found.start().column.0,
            )
        });
    }

    /// Steps to the match above the current one (older output), wrapping.
    pub fn older(&mut self, term: &Term<EventProxy>) {
        self.step(term, false);
    }

    /// Steps to the match below the current one (newer output), wrapping.
    pub fn newer(&mut self, term: &Term<EventProxy>) {
        self.step(term, true);
    }

    fn step(&mut self, term: &Term<EventProxy>, down: bool) {
        self.wrapped = false;
        let len = self.matches.len();
        let Some(at) = self.current else { return };
        let next = if down {
            self.wrapped = at + 1 == len;
            (at + 1) % len
        } else {
            self.wrapped = at == 0;
            (at + len - 1) % len
        };
        self.current = Some(next);
        self.remember(term);
    }

    /// Scrolls the view so that the current match is on screen, near the
    /// middle when it was not.
    pub fn reveal(&self, term: &mut Term<EventProxy>) {
        let Some(found) = self.current_match() else {
            return;
        };
        let line = found.start().line.0;
        let rows = term.screen_lines() as i32;
        let offset = term.grid().display_offset() as i32;
        let (top, bottom) = (-offset, -offset + rows - 1);
        if line >= top && line <= bottom {
            return;
        }
        let history = term.grid().history_size() as i32;
        let wanted = (-line + rows / 2).clamp(0, history);
        term.scroll_display(Scroll::Delta(wanted - offset));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Headless;
    use crate::testing::theme;

    fn headless(cols: u16, rows: u16) -> Headless {
        Headless::new(cols, rows, theme())
    }

    fn found(h: &Headless, query: &str, options: FindOptions) -> FindState {
        let mut state = FindState::new();
        state.set(query, options);
        state.search(h.term());
        state
    }

    fn lines(state: &FindState) -> Vec<(i32, usize)> {
        state
            .matches()
            .iter()
            .map(|found| (found.start().line.0, found.start().column.0))
            .collect()
    }

    #[test]
    fn a_word_split_by_the_edge_of_the_window_is_found_whole() {
        let mut h = headless(10, 4);
        h.feed(b"abcdefghijklmnopqrstuvwxyz");
        let state = found(&h, "ijklmnop", FindOptions::default());
        assert_eq!(state.matches().len(), 1);
        let only = &state.matches()[0];
        assert_eq!(only.start().line, Line(0));
        assert_eq!(only.end().line, Line(1), "it ends on the next row");
    }

    #[test]
    fn matches_come_from_the_scrollback_as_well_as_the_screen() {
        let mut h = headless(20, 3);
        for n in 1..=12 {
            h.feed(format!("needle {n}\r\n").as_bytes());
        }
        let state = found(&h, "needle", FindOptions::default());
        assert_eq!(state.matches().len(), 12);
        assert!(
            state.matches()[0].start().line.0 < 0,
            "the first is in the history"
        );
    }

    #[test]
    fn smart_case_is_insensitive_until_an_uppercase_letter_is_typed() {
        let mut h = headless(30, 3);
        h.feed(b"Error error ERROR");
        assert_eq!(
            found(&h, "error", FindOptions::default()).matches().len(),
            3
        );
        assert_eq!(
            found(&h, "Error", FindOptions::default()).matches().len(),
            1
        );
        let sensitive = FindOptions {
            case_sensitive: true,
            ..FindOptions::default()
        };
        assert_eq!(found(&h, "error", sensitive).matches().len(), 1);
    }

    #[test]
    fn a_literal_query_is_escaped_and_a_regular_expression_is_not() {
        let mut h = headless(30, 3);
        h.feed(b"a.c abc a+c");
        assert_eq!(found(&h, "a.c", FindOptions::default()).matches().len(), 1);
        let regex = FindOptions {
            regex: true,
            ..FindOptions::default()
        };
        assert_eq!(found(&h, "a.c", regex).matches().len(), 3);
        assert_eq!(found(&h, "a\\+c", regex).matches().len(), 1);
        assert_eq!(found(&h, "a+c", FindOptions::default()).matches().len(), 1);
    }

    #[test]
    fn whole_word_skips_matches_inside_longer_words() {
        let mut h = headless(30, 3);
        h.feed(b"cat concat cats cat");
        let word = FindOptions {
            whole_word: true,
            ..FindOptions::default()
        };
        assert_eq!(found(&h, "cat", word).matches().len(), 2);
        assert_eq!(found(&h, "cat", FindOptions::default()).matches().len(), 4);
    }

    #[test]
    fn an_invalid_expression_is_reported_and_finds_nothing() {
        let mut h = headless(30, 3);
        h.feed(b"text (");
        let regex = FindOptions {
            regex: true,
            ..FindOptions::default()
        };
        let state = found(&h, "(", regex);
        assert!(state.error().is_some());
        assert!(state.matches().is_empty() && state.current().is_none());
        // The same text is fine as a literal.
        assert!(found(&h, "(", FindOptions::default()).error().is_none());
    }

    #[test]
    fn the_first_match_is_the_newest_and_older_goes_up_then_wraps() {
        let mut h = headless(20, 3);
        for n in 1..=5 {
            h.feed(format!("hit {n}\r\n").as_bytes());
        }
        let mut state = found(&h, "hit", FindOptions::default());
        assert_eq!(state.current(), Some(4), "the newest");
        state.older(h.term());
        assert_eq!(state.current(), Some(3));
        for _ in 0..3 {
            state.older(h.term());
        }
        assert_eq!(state.current(), Some(0));
        assert!(!state.wrapped());
        state.older(h.term());
        assert_eq!(state.current(), Some(4), "wrapped round to the newest");
        assert!(state.wrapped());
        state.newer(h.term());
        assert_eq!(state.current(), Some(0));
        assert!(state.wrapped());
    }

    #[test]
    fn revealing_a_match_in_the_scrollback_scrolls_the_view_to_it() {
        let mut h = headless(20, 4);
        h.feed(b"needle\r\n");
        for n in 1..=40 {
            h.feed(format!("filler {n}\r\n").as_bytes());
        }
        let state = found(&h, "needle", FindOptions::default());
        assert_eq!(h.term().grid().display_offset(), 0);
        state.reveal(h.term_mut());
        let offset = h.term().grid().display_offset() as i32;
        let line = state.matches()[0].start().line.0;
        assert!(
            line >= -offset && line < -offset + 4,
            "{line} in view at offset {offset}"
        );
    }

    #[test]
    fn searching_again_after_new_output_keeps_the_current_match_in_the_text() {
        let mut h = headless(20, 3);
        for n in 1..=4 {
            h.feed(format!("hit {n}\r\n").as_bytes());
        }
        let mut state = found(&h, "hit", FindOptions::default());
        state.older(h.term());
        state.older(h.term());
        let before = state.current().unwrap();
        h.feed(b"more\r\nmore\r\nhit 5\r\n");
        state.search(h.term());
        assert_eq!(state.matches().len(), 5);
        assert_eq!(state.current(), Some(before), "still the same line of text");
    }

    #[test]
    fn a_huge_result_set_is_capped() {
        let mut h = headless(50, 5);
        for _ in 0..(MATCH_LIMIT / 10 + 200) {
            h.feed(b"a a a a a a a a a a\r\n");
        }
        let state = found(&h, "a", FindOptions::default());
        assert_eq!(state.matches().len(), MATCH_LIMIT);
        assert!(state.capped());
        assert_eq!(lines(&state).len(), MATCH_LIMIT);
    }
}
