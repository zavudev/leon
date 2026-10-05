//! The terminal's buffer as text, and the operations that empty or select it.
//!
//! Everything here works on the emulator's [`Term`] and has no window in it,
//! so it is tested with the headless terminal.
//!
//! * [`text`] reads the scrollback and the screen, or the screen alone, as
//!   plain text, and [`ansi`] as text that carries the colours and attributes
//!   as SGR sequences. A line the program let wrap is **one** line: the
//!   emulator marks the last cell of a wrapped row, and the next row is
//!   joined to it. Trailing blanks are trimmed per line, wide characters and
//!   combining marks are kept, and trailing blank lines are dropped.
//! * [`clear_buffer`] and [`clear_scrollback`] empty the history (and the
//!   screen, keeping the line the cursor is on) inside the emulator, so they
//!   work while a program runs and send it nothing.
//! * [`select_all`] selects everything, scrollback included, so that the
//!   ordinary copy takes it.

use crate::terminal::EventProxy;
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, Color, Handler as _, NamedColor};

/// How much of the buffer to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// The scrollback and the screen.
    All,
    /// The rows on screen now (the live screen, not where the view is
    /// scrolled to).
    Screen,
}

/// What a clear did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleared {
    /// The history, and the screen if it was asked for, are empty.
    Done,
    /// A full-screen program has the screen: it owns it and the shell's
    /// history is out of reach, so nothing was changed.
    FullScreenProgram,
}

fn first_line(term: &Term<EventProxy>, extent: Extent) -> Line {
    match extent {
        Extent::All => term.topmost_line(),
        Extent::Screen => Line(0),
    }
}

/// The glyphs of a cell, as they are written out; `None` for the spacer that
/// follows a wide character.
fn glyphs(cell: &Cell, out: &mut String) {
    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
        return;
    }
    out.push(if cell.c == '\0' { ' ' } else { cell.c });
    if let Some(marks) = cell.zerowidth() {
        out.extend(marks);
    }
}

/// Calls `line` with each logical line: its cells and whether it is the end of
/// one (not wrapped into the next row). Rows of a wrapped line arrive in
/// order.
fn rows(term: &Term<EventProxy>, extent: Extent, mut row: impl FnMut(&[Cell], bool)) {
    let grid = term.grid();
    let last = Column(grid.columns() - 1);
    let mut cells: Vec<Cell> = Vec::with_capacity(grid.columns());
    let mut line = first_line(term, extent);
    while line <= term.bottommost_line() {
        cells.clear();
        cells.extend((0..grid.columns()).map(|col| grid[Point::new(line, Column(col))].clone()));
        let wrapped = grid[Point::new(line, last)].flags.contains(Flags::WRAPLINE);
        row(&cells, !wrapped);
        line += 1;
    }
}

/// The buffer as plain text: lines joined by `\n`, no trailing newline.
pub fn text(term: &Term<EventProxy>, extent: Extent) -> String {
    let mut out = String::new();
    let mut pending = String::new();
    rows(term, extent, |cells, ends| {
        for cell in cells {
            glyphs(cell, &mut pending);
        }
        if ends {
            out.push_str(pending.trim_end());
            out.push('\n');
            pending.clear();
        }
    });
    out.push_str(pending.trim_end());
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// The number of lines [`text`] holds.
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.split('\n').count()
    }
}

/// What the SGR sequence of a cell asks for.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    fg: Color,
    bg: Color,
    flags: Flags,
}

const ATTRIBUTES: [(Flags, u8); 6] = [
    (Flags::BOLD, 1),
    (Flags::DIM, 2),
    (Flags::ITALIC, 3),
    (Flags::INVERSE, 7),
    (Flags::HIDDEN, 8),
    (Flags::STRIKEOUT, 9),
];

fn underlined(flags: Flags) -> bool {
    flags.intersects(Flags::ALL_UNDERLINES)
}

fn colour_code(colour: Color, foreground: bool, codes: &mut Vec<String>) {
    let base = if foreground { 30 } else { 40 };
    let named = |name: NamedColor| -> Option<u8> {
        Some(match name {
            NamedColor::Black | NamedColor::DimBlack => 0,
            NamedColor::Red | NamedColor::DimRed => 1,
            NamedColor::Green | NamedColor::DimGreen => 2,
            NamedColor::Yellow | NamedColor::DimYellow => 3,
            NamedColor::Blue | NamedColor::DimBlue => 4,
            NamedColor::Magenta | NamedColor::DimMagenta => 5,
            NamedColor::Cyan | NamedColor::DimCyan => 6,
            NamedColor::White | NamedColor::DimWhite => 7,
            NamedColor::BrightBlack => 8,
            NamedColor::BrightRed => 9,
            NamedColor::BrightGreen => 10,
            NamedColor::BrightYellow => 11,
            NamedColor::BrightBlue => 12,
            NamedColor::BrightMagenta => 13,
            NamedColor::BrightCyan => 14,
            NamedColor::BrightWhite => 15,
            _ => return None,
        })
    };
    match colour {
        Color::Named(name) => match named(name) {
            Some(n) if n < 8 => codes.push((base + n).to_string()),
            Some(n) => codes.push((base + 60 + n - 8).to_string()),
            None => codes.push((base + 9).to_string()),
        },
        Color::Indexed(index) => codes.push(format!("{};5;{index}", base + 8)),
        Color::Spec(rgb) => codes.push(format!("{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b)),
    }
}

fn sgr(look: Look) -> String {
    let mut codes = vec!["0".to_owned()];
    for (flag, code) in ATTRIBUTES {
        if look.flags.contains(flag) {
            codes.push(code.to_string());
        }
    }
    if underlined(look.flags) {
        codes.push("4".to_owned());
    }
    colour_code(look.fg, true, &mut codes);
    colour_code(look.bg, false, &mut codes);
    format!("\x1b[{}m", codes.join(";"))
}

/// The buffer as text with its colours and attributes written as SGR
/// sequences, ready to be shown by a terminal again (`cat file.ansi`).
pub fn ansi(term: &Term<EventProxy>, extent: Extent) -> String {
    let default = Look {
        fg: Color::Named(NamedColor::Foreground),
        bg: Color::Named(NamedColor::Background),
        flags: Flags::empty(),
    };
    let mut out = String::new();
    let mut pending = String::new();
    let mut current = default;
    rows(term, extent, |cells, ends| {
        // A logical line is trimmed of its trailing plain blanks only.
        let mut line = String::new();
        let mut blanks = String::new();
        for cell in cells {
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let look = Look {
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags
                    & (Flags::ALL_UNDERLINES
                        | Flags::BOLD
                        | Flags::DIM
                        | Flags::ITALIC
                        | Flags::INVERSE
                        | Flags::HIDDEN
                        | Flags::STRIKEOUT),
            };
            let blank = (cell.c == ' ' || cell.c == '\0') && look == default;
            let mut glyph = String::new();
            glyphs(cell, &mut glyph);
            if blank {
                blanks.push_str(&glyph);
                continue;
            }
            if !blanks.is_empty() {
                // Blanks of the page's own look: not drawn in what came before.
                if current != default {
                    line.push_str(&sgr(default));
                    current = default;
                }
                line.push_str(&blanks);
                blanks.clear();
            }
            if look != current {
                line.push_str(&sgr(look));
                current = look;
            }
            line.push_str(&glyph);
        }
        pending.push_str(&line);
        if !ends {
            pending.push_str(&blanks);
            return;
        }
        if current != default {
            pending.push_str("\x1b[0m");
            current = default;
        }
        out.push_str(&pending);
        out.push('\n');
        pending.clear();
    });
    out.push_str(&pending);
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// Empties the history. The screen stays.
pub fn clear_scrollback(term: &mut Term<EventProxy>) -> Cleared {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return Cleared::FullScreenProgram;
    }
    term.clear_screen(ClearMode::Saved);
    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
    Cleared::Done
}

/// Empties the history and the screen, and leaves the line the cursor is on
/// (the prompt) at the top, the cursor on it where it was.
pub fn clear_buffer(term: &mut Term<EventProxy>) -> Cleared {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return Cleared::FullScreenProgram;
    }
    let cursor = term.grid().cursor.point;
    let kept = term.grid()[cursor.line].clone();
    term.clear_screen(ClearMode::All);
    term.clear_screen(ClearMode::Saved);
    term.grid_mut()[Line(0)] = kept;
    term.goto(0, cursor.column.0);
    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
    Cleared::Done
}

/// Selects the whole buffer, scrollback included.
pub fn select_all(term: &mut Term<EventProxy>) {
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(term.bottommost_line(), Column(term.columns() - 1));
    let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
    selection.update(end, Side::Right);
    term.selection = Some(selection);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Headless;
    use crate::testing::theme;

    fn headless(cols: u16, rows: u16) -> Headless {
        Headless::new(cols, rows, theme())
    }

    #[test]
    fn a_line_the_program_let_wrap_is_one_line_in_the_text() {
        let mut h = headless(10, 4);
        h.feed(b"abcdefghijklmnopqrstuvwxy\r\nshort");
        assert_eq!(
            text(h.term(), Extent::All),
            "abcdefghijklmnopqrstuvwxy\nshort"
        );
    }

    #[test]
    fn a_hard_break_is_not_joined_and_trailing_blanks_and_blank_lines_are_dropped() {
        let mut h = headless(10, 6);
        h.feed(b"one   \r\n\r\ntwo  \r\n\r\n\r\n");
        assert_eq!(text(h.term(), Extent::All), "one\n\ntwo");
    }

    #[test]
    fn the_scrollback_is_in_the_text_and_the_screen_alone_is_not() {
        let mut h = headless(10, 3);
        for n in 1..=8 {
            h.feed(format!("line {n}\r\n").as_bytes());
        }
        let all = text(h.term(), Extent::All);
        assert!(all.starts_with("line 1\nline 2"), "{all}");
        assert!(all.ends_with("line 8"), "{all}");
        let screen = text(h.term(), Extent::Screen);
        assert!(!screen.contains("line 1"), "{screen}");
        assert!(screen.contains("line 8"), "{screen}");
    }

    #[test]
    fn wide_characters_and_combining_marks_survive() {
        let mut h = headless(20, 3);
        h.feed("日本語 e\u{301}x".as_bytes());
        assert_eq!(text(h.term(), Extent::All), "日本語 e\u{301}x");
    }

    #[test]
    fn counting_lines_counts_the_lines_of_the_text() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\nb\n\nc"), 4);
    }

    #[test]
    fn colours_and_attributes_round_trip_through_the_ansi_text() {
        let mut h = headless(30, 3);
        h.feed(b"\x1b[1;31mred bold\x1b[0m plain \x1b[38;5;200;48;2;1;2;3mmix\x1b[0m \x1b[4;7mul\x1b[0m");
        let written = ansi(h.term(), Extent::All);
        assert!(written.contains("\x1b["), "{written:?}");
        let mut again = headless(30, 3);
        again.feed(written.as_bytes());
        for col in 0..30 {
            let a = &h.term().grid()[Point::new(Line(0), Column(col))];
            let b = &again.term().grid()[Point::new(Line(0), Column(col))];
            assert_eq!((a.c, a.fg, a.bg), (b.c, b.fg, b.bg), "column {col}");
            assert_eq!(
                a.flags & (Flags::BOLD | Flags::INVERSE | Flags::ALL_UNDERLINES),
                b.flags & (Flags::BOLD | Flags::INVERSE | Flags::ALL_UNDERLINES),
                "column {col}"
            );
        }
    }

    #[test]
    fn the_ansi_text_of_plain_output_has_no_sequences() {
        let mut h = headless(10, 3);
        h.feed(b"plain\r\ntext");
        assert_eq!(ansi(h.term(), Extent::All), "plain\ntext");
    }

    #[test]
    fn clearing_the_buffer_empties_the_history_and_keeps_the_cursor_line_on_top() {
        let mut h = headless(20, 4);
        for n in 1..=9 {
            h.feed(format!("old {n}\r\n").as_bytes());
        }
        h.feed(b"READY> typed");
        assert_eq!(clear_buffer(h.term_mut()), Cleared::Done);
        assert_eq!(text(h.term(), Extent::All), "READY> typed");
        assert_eq!(h.term().grid().history_size(), 0);
        let cursor = h.term().grid().cursor.point;
        assert_eq!(cursor.line, Line(0));
        assert_eq!(cursor.column, Column(12));
        // The program carries on from where the cursor is.
        h.feed(b"!\r\nnext");
        assert_eq!(text(h.term(), Extent::All), "READY> typed!\nnext");
    }

    #[test]
    fn clearing_the_scrollback_keeps_the_screen() {
        let mut h = headless(20, 3);
        for n in 1..=8 {
            h.feed(format!("old {n}\r\n").as_bytes());
        }
        h.feed(b"prompt");
        let before = text(h.term(), Extent::Screen);
        assert_eq!(clear_scrollback(h.term_mut()), Cleared::Done);
        assert_eq!(h.term().grid().history_size(), 0);
        assert_eq!(text(h.term(), Extent::All), before);
    }

    #[test]
    fn a_full_screen_program_keeps_its_screen_and_nothing_is_cleared() {
        let mut h = headless(20, 3);
        h.feed(b"shell history\r\n\x1b[?1049hfull screen");
        assert_eq!(clear_buffer(h.term_mut()), Cleared::FullScreenProgram);
        assert_eq!(clear_scrollback(h.term_mut()), Cleared::FullScreenProgram);
        assert!(text(h.term(), Extent::Screen).contains("full screen"));
    }

    #[test]
    fn selecting_all_selects_the_scrollback_too() {
        let mut h = headless(10, 3);
        for n in 1..=6 {
            h.feed(format!("row {n}\r\n").as_bytes());
        }
        select_all(h.term_mut());
        let selected = h.term().selection_to_string().unwrap();
        assert!(
            selected.contains("row 1") && selected.contains("row 6"),
            "{selected}"
        );
    }
}
