//! From cells to what is drawn: background rectangles and runs of text.
//!
//! Painting one cell at a time would shape thousands of one-character
//! strings per frame. [`Batcher`] instead takes the visible cells in reading
//! order and merges them: neighbouring cells of one background colour become
//! one rectangle, neighbouring cells of one text style become one run that is
//! shaped once. Its buffers are cleared, not freed, between frames, so
//! painting allocates only while the screen grows.
//!
//! Rules, each covered by a test:
//!
//! * a cell with the default background has no rectangle;
//! * blank cells without decoration never start a run, and trailing blanks
//!   are dropped, but blanks between two cells of one style stay inside the
//!   run so the text is one string;
//! * a wide character is a run of its own that spans two columns;
//! * a new line ends every run and rectangle.

use alacritty_terminal::term::cell::Flags;
use gpui_kit::Hsla;
use std::ops::Range;

/// One visible cell, with its colours already resolved (inverse, dim and the
/// cursor applied by the caller).
#[derive(Clone, Copy, Debug)]
pub struct CellInput<'a> {
    /// Row on screen, from 0.
    pub row: usize,
    /// Column, from 0.
    pub col: usize,
    /// The character; a space or NUL for an empty cell.
    pub ch: char,
    /// Combining characters drawn with it.
    pub zerowidth: &'a [char],
    /// Text colour.
    pub fg: Hsla,
    /// Background colour, `None` for the terminal's own (not painted).
    pub bg: Option<Hsla>,
    /// The cell's attributes.
    pub flags: Flags,
}

/// A rectangle of one background colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BgRect {
    /// Row.
    pub row: usize,
    /// First column.
    pub col: usize,
    /// Width in columns.
    pub cols: usize,
    /// Colour.
    pub color: Hsla,
}

/// A run of text of one style, shaped as one string.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// Row.
    pub row: usize,
    /// First column.
    pub col: usize,
    /// Width in columns.
    pub cols: usize,
    /// Where its text is in [`Batcher::text`].
    pub text: Range<usize>,
    /// Text colour.
    pub fg: Hsla,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underlined (any style).
    pub underline: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// A single wide character.
    pub wide: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

struct Open {
    run: Run,
    style: Style,
    /// Blank cells seen since the last visible one, not yet in the text.
    pending_blanks: usize,
}

/// Collects rectangles and runs for one frame.
#[derive(Default)]
pub struct Batcher {
    /// Background rectangles, in reading order.
    pub rects: Vec<BgRect>,
    /// Text runs, in reading order.
    pub runs: Vec<Run>,
    /// The text of every run, back to back.
    pub text: String,
    open: Option<Open>,
}

fn is_blank(ch: char) -> bool {
    ch == ' ' || ch == '\0'
}

impl Batcher {
    /// An empty batcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the last frame, keeping the buffers.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.runs.clear();
        self.text.clear();
        self.open = None;
    }

    /// The text of a run.
    pub fn text_of(&self, run: &Run) -> &str {
        &self.text[run.text.clone()]
    }

    fn close_run(&mut self) {
        if let Some(open) = self.open.take() {
            // Trailing blanks are not part of the run.
            let mut run = open.run;
            run.text.end = self.text.len();
            self.runs.push(run);
        }
    }

    /// Adds the next cell. Cells must come in reading order.
    pub fn push(&mut self, cell: &CellInput<'_>) {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            return;
        }
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2 } else { 1 };

        if let Some(color) = cell.bg {
            match self.rects.last_mut() {
                Some(last)
                    if last.row == cell.row
                        && last.color == color
                        && last.col + last.cols == cell.col =>
                {
                    last.cols += width;
                }
                _ => self.rects.push(BgRect {
                    row: cell.row,
                    col: cell.col,
                    cols: width,
                    color,
                }),
            }
        }

        let hidden = cell.flags.contains(Flags::HIDDEN);
        let ch = if hidden { ' ' } else { cell.ch };
        let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
        let strikethrough = cell.flags.contains(Flags::STRIKEOUT);
        let blank = is_blank(ch) && !underline && !strikethrough && !wide;
        let style = Style {
            fg: cell.fg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline,
            strikethrough,
        };

        // A new line or a gap ends the run.
        if let Some(open) = &self.open {
            let end = open.run.col + open.run.cols + open.pending_blanks;
            if open.run.row != cell.row || end != cell.col {
                self.close_run();
            }
        }

        if blank {
            if let Some(open) = &mut self.open {
                open.pending_blanks += 1;
            }
            return;
        }

        if wide {
            self.close_run();
            let start = self.text.len();
            self.text.push(ch);
            self.text.extend(cell.zerowidth);
            self.runs.push(Run {
                row: cell.row,
                col: cell.col,
                cols: 2,
                text: start..self.text.len(),
                fg: style.fg,
                bold: style.bold,
                italic: style.italic,
                underline: style.underline,
                strikethrough: style.strikethrough,
                wide: true,
            });
            return;
        }

        let joins = self.open.as_ref().is_some_and(|open| open.style == style);
        if joins {
            let open = self.open.as_mut().expect("checked above");
            for _ in 0..open.pending_blanks {
                self.text.push(' ');
            }
            open.run.cols += open.pending_blanks + 1;
            open.pending_blanks = 0;
        } else {
            self.close_run();
            self.open = Some(Open {
                run: Run {
                    row: cell.row,
                    col: cell.col,
                    cols: 1,
                    text: self.text.len()..self.text.len(),
                    fg: style.fg,
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                    strikethrough: style.strikethrough,
                    wide: false,
                },
                style,
                pending_blanks: 0,
            });
        }
        self.text.push(ch);
        self.text.extend(cell.zerowidth);
    }

    /// Ends the frame: closes the run in progress.
    pub fn finish(&mut self) {
        self.close_run();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::hsla;

    const FG: Hsla = hsla(0.0, 0.0, 1.0, 1.0);
    const RED: Hsla = hsla(0.0, 1.0, 0.5, 1.0);
    const BLUE: Hsla = hsla(0.6, 1.0, 0.5, 1.0);

    fn cell(row: usize, col: usize, ch: char) -> CellInput<'static> {
        CellInput {
            row,
            col,
            ch,
            zerowidth: &[],
            fg: FG,
            bg: None,
            flags: Flags::empty(),
        }
    }

    fn batch(cells: &[CellInput<'_>]) -> Batcher {
        let mut batcher = Batcher::new();
        for cell in cells {
            batcher.push(cell);
        }
        batcher.finish();
        batcher
    }

    fn row_of(row: usize, text: &str) -> Vec<CellInput<'static>> {
        text.chars()
            .enumerate()
            .map(|(col, ch)| cell(row, col, ch))
            .collect()
    }

    fn texts(batcher: &Batcher) -> Vec<(usize, usize, String)> {
        batcher
            .runs
            .iter()
            .map(|run| (run.row, run.col, batcher.text_of(run).to_owned()))
            .collect()
    }

    #[test]
    fn cells_of_one_style_become_one_run() {
        let batcher = batch(&row_of(0, "hello"));
        assert_eq!(texts(&batcher), [(0, 0, "hello".to_owned())]);
        assert_eq!(batcher.runs[0].cols, 5);
    }

    #[test]
    fn a_change_of_colour_or_weight_starts_a_new_run() {
        let mut cells = row_of(0, "abcd");
        cells[2].fg = RED;
        cells[3].fg = RED;
        cells[3].flags = Flags::BOLD;
        let batcher = batch(&cells);
        assert_eq!(
            texts(&batcher),
            [
                (0, 0, "ab".to_owned()),
                (0, 2, "c".to_owned()),
                (0, 3, "d".to_owned())
            ]
        );
        assert!(batcher.runs[2].bold && !batcher.runs[1].bold);
    }

    #[test]
    fn blanks_between_words_stay_in_the_run() {
        let batcher = batch(&row_of(0, "ls  -la"));
        assert_eq!(texts(&batcher), [(0, 0, "ls  -la".to_owned())]);
        assert_eq!(batcher.runs[0].cols, 7);
    }

    #[test]
    fn trailing_and_leading_blanks_are_not_shaped() {
        let batcher = batch(&row_of(0, "  hi   "));
        assert_eq!(texts(&batcher), [(0, 2, "hi".to_owned())]);
    }

    #[test]
    fn a_blank_row_makes_no_runs() {
        assert!(batch(&row_of(0, "          ")).runs.is_empty());
    }

    #[test]
    fn a_new_line_ends_the_run() {
        let mut cells = row_of(0, "ab");
        cells.extend(row_of(1, "cd"));
        let batcher = batch(&cells);
        assert_eq!(
            texts(&batcher),
            [(0, 0, "ab".to_owned()), (1, 0, "cd".to_owned())]
        );
    }

    #[test]
    fn blanks_that_differ_in_colour_do_not_split_a_run() {
        let mut cells = row_of(0, "a b");
        cells[1].fg = RED;
        let batcher = batch(&cells);
        assert_eq!(texts(&batcher), [(0, 0, "a b".to_owned())]);
    }

    #[test]
    fn underlined_blanks_are_drawn() {
        let mut cells = row_of(0, "a b");
        cells[1].flags = Flags::UNDERLINE;
        let batcher = batch(&cells);
        assert!(batcher
            .runs
            .iter()
            .any(|run| run.underline && batcher.text_of(run) == " "));
    }

    #[test]
    fn default_background_cells_have_no_rectangle() {
        assert!(batch(&row_of(0, "abc")).rects.is_empty());
    }

    #[test]
    fn neighbouring_cells_of_one_background_are_one_rectangle() {
        let mut cells = row_of(0, "abcd");
        for cell in &mut cells[..3] {
            cell.bg = Some(BLUE);
        }
        let batcher = batch(&cells);
        assert_eq!(
            batcher.rects,
            [BgRect {
                row: 0,
                col: 0,
                cols: 3,
                color: BLUE
            }]
        );
    }

    #[test]
    fn rectangles_split_on_colour_gap_and_row() {
        let mut cells = row_of(0, "abcde");
        cells[0].bg = Some(BLUE);
        cells[1].bg = Some(RED);
        cells[3].bg = Some(RED);
        let mut next = row_of(1, "a");
        next[0].bg = Some(RED);
        cells.extend(next);
        let batcher = batch(&cells);
        let spans: Vec<_> = batcher
            .rects
            .iter()
            .map(|r| (r.row, r.col, r.cols))
            .collect();
        assert_eq!(spans, [(0, 0, 1), (0, 1, 1), (0, 3, 1), (1, 0, 1)]);
    }

    #[test]
    fn a_wide_character_is_its_own_two_column_run() {
        let mut cells = vec![cell(0, 0, 'a')];
        let mut wide = cell(0, 1, '世');
        wide.flags = Flags::WIDE_CHAR;
        let mut spacer = cell(0, 2, ' ');
        spacer.flags = Flags::WIDE_CHAR_SPACER;
        cells.extend([wide, spacer, cell(0, 3, 'b')]);
        let batcher = batch(&cells);
        assert_eq!(
            texts(&batcher),
            [
                (0, 0, "a".to_owned()),
                (0, 1, "世".to_owned()),
                (0, 3, "b".to_owned())
            ]
        );
        assert!(batcher.runs[1].wide);
        assert_eq!(batcher.runs[1].cols, 2);
    }

    #[test]
    fn a_wide_cell_with_a_background_covers_both_columns() {
        let mut wide = cell(0, 0, '世');
        wide.flags = Flags::WIDE_CHAR;
        wide.bg = Some(RED);
        let mut spacer = cell(0, 1, ' ');
        spacer.flags = Flags::WIDE_CHAR_SPACER;
        spacer.bg = Some(RED);
        let batcher = batch(&[wide, spacer]);
        assert_eq!(
            batcher.rects,
            [BgRect {
                row: 0,
                col: 0,
                cols: 2,
                color: RED
            }]
        );
    }

    #[test]
    fn combining_characters_travel_with_their_cell() {
        let marks = ['\u{301}'];
        let mut e = cell(0, 0, 'e');
        e.zerowidth = &marks;
        let batcher = batch(&[e, cell(0, 1, 'x')]);
        assert_eq!(texts(&batcher), [(0, 0, "e\u{301}x".to_owned())]);
        assert_eq!(batcher.runs[0].cols, 2);
    }

    #[test]
    fn hidden_text_is_not_drawn() {
        let mut cells = row_of(0, "pw");
        for cell in &mut cells {
            cell.flags = Flags::HIDDEN;
        }
        assert!(batch(&cells).runs.is_empty());
    }

    #[test]
    fn clearing_keeps_the_buffers_for_the_next_frame() {
        let mut batcher = batch(&row_of(0, "hello world"));
        let capacity = batcher.text.capacity();
        batcher.clear();
        assert!(batcher.runs.is_empty() && batcher.rects.is_empty() && batcher.text.is_empty());
        for cell in row_of(0, "again") {
            batcher.push(&cell);
        }
        batcher.finish();
        assert_eq!(batcher.text.capacity(), capacity);
        assert_eq!(texts(&batcher), [(0, 0, "again".to_owned())]);
    }
}
