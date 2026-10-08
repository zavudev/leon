//! From keys, paste and the mouse to the bytes a program expects.
//!
//! [`encode_key`] maps a keystroke to an escape sequence. It answers `None`
//! for plain text (a letter, a digit, a symbol, with at most Shift held): the
//! text comes through the platform's text input path instead, so dead keys,
//! input methods and non-US layouts compose characters the way the system
//! says. Everything else (named keys, Ctrl, Alt) is decided here.
//!
//! | Key | Bytes |
//! | --- | --- |
//! | Enter, Tab, Backspace, Escape | `CR`, `HT`, `DEL`, `ESC` |
//! | Shift+Tab | `ESC [ Z` |
//! | Shift+Enter, Alt+Enter | `ESC CR` (a new line in Claude Code) |
//! | arrows | `ESC [ A` to `D`, or `ESC O A` to `D` in application cursor mode |
//! | Home, End | `ESC [ H`, `ESC [ F` (`ESC O` in application cursor mode) |
//! | PgUp, PgDn, Insert, Delete | `ESC [ 5 ~`, `6 ~`, `2 ~`, `3 ~` |
//! | F1 to F4, F5 to F20 | `ESC O P` to `S`, `ESC [ 15 ~` ... |
//! | Ctrl+letter | `0x01` to `0x1A` |
//! | Alt+key | `ESC` then the key |
//! | modified named keys | `ESC [ 1 ; m X` with `m = 1 + Shift + 2 Alt + 4 Ctrl` |

use alacritty_terminal::term::TermMode;
use gpui_kit::{Keystroke, Modifiers};

const ESC: u8 = 0x1b;

/// The modifier parameter of a modified named key: `1 + shift + 2 alt + 4 ctrl`.
fn modifier_parameter(modifiers: &Modifiers) -> u8 {
    1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.control)
}

fn has_modifier(modifiers: &Modifiers) -> bool {
    modifiers.shift || modifiers.alt || modifiers.control
}

/// `ESC [ <number> ; <m> <final>` or the plain forms when nothing is held.
fn csi(modifiers: &Modifiers, number: Option<u8>, last: char) -> Vec<u8> {
    let mut out = vec![ESC, b'['];
    if has_modifier(modifiers) {
        out.extend_from_slice(number.unwrap_or(1).to_string().as_bytes());
        out.push(b';');
        out.extend_from_slice(modifier_parameter(modifiers).to_string().as_bytes());
    } else if let Some(number) = number {
        out.extend_from_slice(number.to_string().as_bytes());
    }
    out.push(last as u8);
    out
}

/// A cursor-style key: `ESC [ X`, `ESC O X` in application cursor mode, and
/// the modified form when a modifier is held.
fn cursor_key(modifiers: &Modifiers, mode: TermMode, last: char) -> Vec<u8> {
    if !has_modifier(modifiers) && mode.contains(TermMode::APP_CURSOR) {
        vec![ESC, b'O', last as u8]
    } else {
        csi(modifiers, None, last)
    }
}

/// `ESC [ <number> ~`, with the modifier parameter when one is held.
fn tilde_key(modifiers: &Modifiers, number: u8) -> Vec<u8> {
    csi(modifiers, Some(number), '~')
}

/// F1 to F4 are `SS3 P` to `SS3 S`; modified, they take the CSI form.
fn low_function_key(modifiers: &Modifiers, last: char) -> Vec<u8> {
    if has_modifier(modifiers) {
        csi(modifiers, None, last)
    } else {
        vec![ESC, b'O', last as u8]
    }
}

/// The `~` number of F5 to F20.
fn function_number(n: u8) -> Option<u8> {
    Some(match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        13 => 25,
        14 => 26,
        15 => 28,
        16 => 29,
        17 => 31,
        18 => 32,
        19 => 33,
        20 => 34,
        _ => return None,
    })
}

/// The control character Ctrl makes of a key, if it makes one.
fn control_byte(key: char) -> Option<u8> {
    Some(match key {
        'a'..='z' => key as u8 - b'a' + 1,
        'A'..='Z' => key as u8 - b'A' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' | '-' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// The character Alt is prefixed to: the key as typed with Shift, ASCII when
/// it can be (on macOS Option composes `å`, but a terminal wants `ESC a`).
fn alt_character(stroke: &Keystroke) -> Option<String> {
    let mut chars = stroke.key.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if first.is_ascii_alphabetic() {
        let c = if stroke.modifiers.shift {
            first.to_ascii_uppercase()
        } else {
            first.to_ascii_lowercase()
        };
        return Some(c.to_string());
    }
    if let Some(typed) = stroke.key_char.as_deref() {
        if typed.chars().count() == 1 && typed.is_ascii() {
            return Some(typed.to_owned());
        }
    }
    Some(first.to_string())
}

/// The bytes a keystroke stands for, or `None` when it is text for the
/// platform's text input path (or nothing at all).
pub fn encode_key(stroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &stroke.modifiers;
    // The platform key is Leon's, or the system's: never a terminal's.
    if m.platform || m.function {
        return None;
    }
    let key = stroke.key.as_str();
    let alt_prefix = |mut bytes: Vec<u8>| {
        if m.alt {
            bytes.insert(0, ESC);
        }
        bytes
    };
    match key {
        // Shift+Enter sends what Alt+Enter does: a plain `CR` is "submit" to
        // Claude Code and friends, `ESC CR` is "a new line".
        "enter" if m.shift => return Some(vec![ESC, b'\r']),
        "enter" => return Some(alt_prefix(vec![b'\r'])),
        "escape" => return Some(alt_prefix(vec![ESC])),
        "backspace" => {
            return Some(if m.control {
                alt_prefix(vec![0x08])
            } else {
                alt_prefix(vec![0x7f])
            })
        }
        "tab" => {
            return Some(if m.shift {
                vec![ESC, b'[', b'Z']
            } else {
                alt_prefix(vec![b'\t'])
            })
        }
        "space" => {
            return if m.control {
                Some(alt_prefix(vec![0]))
            } else if m.alt {
                Some(vec![ESC, b' '])
            } else {
                None
            }
        }
        "up" => return Some(cursor_key(m, mode, 'A')),
        "down" => return Some(cursor_key(m, mode, 'B')),
        "right" => return Some(cursor_key(m, mode, 'C')),
        "left" => return Some(cursor_key(m, mode, 'D')),
        "home" => return Some(cursor_key(m, mode, 'H')),
        "end" => return Some(cursor_key(m, mode, 'F')),
        "pageup" => return Some(tilde_key(m, 5)),
        "pagedown" => return Some(tilde_key(m, 6)),
        "insert" => return Some(tilde_key(m, 2)),
        "delete" => return Some(tilde_key(m, 3)),
        "f1" => return Some(low_function_key(m, 'P')),
        "f2" => return Some(low_function_key(m, 'Q')),
        "f3" => return Some(low_function_key(m, 'R')),
        "f4" => return Some(low_function_key(m, 'S')),
        _ => {}
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return function_number(n).map(|number| tilde_key(m, number));
    }
    let mut chars = key.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if m.control {
        let byte = control_byte(first)?;
        return Some(alt_prefix(vec![byte]));
    }
    if m.alt {
        let mut bytes = vec![ESC];
        bytes.extend_from_slice(alt_character(stroke)?.as_bytes());
        return Some(bytes);
    }
    None
}

/// What is written to the child for pasted text: wrapped in the bracketed
/// paste markers when the program asked for them (an escape inside the text
/// is dropped, so the text cannot end the paste early), else with line breaks
/// as the Enter key sends them.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.bytes().filter(|byte| *byte != ESC));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// A mouse button, or the wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// The left button.
    Left,
    /// The middle button.
    Middle,
    /// The right button.
    Right,
    /// The wheel, away from the user.
    WheelUp,
    /// The wheel, towards the user.
    WheelDown,
}

/// What the mouse did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    /// A button went down (or the wheel turned).
    Press,
    /// A button went up.
    Release,
    /// The pointer moved with a button down.
    Drag,
    /// The pointer moved with no button down.
    Move,
}

/// Where the mouse is, in 0-based cells, and which modifiers are held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    /// The button.
    pub button: MouseButton,
    /// What happened.
    pub action: MouseAction,
    /// Column, from 0.
    pub col: usize,
    /// Row, from 0.
    pub row: usize,
    /// Shift held.
    pub shift: bool,
    /// Alt held.
    pub alt: bool,
    /// Ctrl held.
    pub ctrl: bool,
}

/// Whether the program has asked for mouse reports at all.
pub fn mouse_reporting(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

/// The report of a mouse event, or `None` when the program did not ask for
/// this kind of event. SGR encoding is used when the program asked for it,
/// the legacy X10 encoding (limited to 223 columns and rows) otherwise.
pub fn mouse_report(event: &MouseEvent, mode: TermMode) -> Option<Vec<u8>> {
    let wanted = match event.action {
        MouseAction::Press | MouseAction::Release => mode.intersects(TermMode::MOUSE_MODE),
        MouseAction::Drag => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
        MouseAction::Move => mode.contains(TermMode::MOUSE_MOTION),
    };
    if !wanted {
        return None;
    }
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code: u8 = match event.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if event.action == MouseAction::Release && !sgr {
        code = 3;
    }
    if matches!(event.action, MouseAction::Drag | MouseAction::Move) {
        code += 32;
        if event.action == MouseAction::Move {
            code = 35;
        }
    }
    if event.shift {
        code += 4;
    }
    if event.alt {
        code += 8;
    }
    if event.ctrl {
        code += 16;
    }
    let (col, row) = (event.col + 1, event.row + 1);
    if sgr {
        let last = if event.action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        Some(format!("\x1b[<{code};{col};{row}{last}").into_bytes())
    } else {
        if col > 223 || row > 223 {
            return None;
        }
        Some(vec![
            ESC,
            b'[',
            b'M',
            32 + code,
            32 + col as u8,
            32 + row as u8,
        ])
    }
}

/// The arrow keys a wheel turn sends to a full-screen program that has not
/// asked for mouse reports (the alternate scroll mode): `lines` presses of
/// Up or Down.
pub fn wheel_as_arrows(up: bool, lines: usize, mode: TermMode) -> Vec<u8> {
    let last = if up { b'A' } else { b'B' };
    let one: [u8; 3] = if mode.contains(TermMode::APP_CURSOR) {
        [ESC, b'O', last]
    } else {
        [ESC, b'[', last]
    };
    one.iter().copied().cycle().take(3 * lines).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(source: &str) -> Keystroke {
        Keystroke::parse(source).expect("a keystroke")
    }

    fn typed(key: &str, character: &str) -> Keystroke {
        let mut stroke = stroke(key);
        stroke.key_char = Some(character.to_owned());
        stroke
    }

    fn bytes(source: &str) -> Option<Vec<u8>> {
        encode_key(&stroke(source), TermMode::empty())
    }

    #[test]
    fn the_simple_named_keys_send_their_control_characters() {
        assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
        assert_eq!(bytes("escape"), Some(b"\x1b".to_vec()));
        assert_eq!(bytes("backspace"), Some(b"\x7f".to_vec()));
        assert_eq!(bytes("tab"), Some(b"\t".to_vec()));
        assert_eq!(bytes("shift-tab"), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn ctrl_c_and_ctrl_d_reach_the_program() {
        assert_eq!(bytes("ctrl-c"), Some(vec![0x03]));
        assert_eq!(bytes("ctrl-d"), Some(vec![0x04]));
        assert_eq!(bytes("ctrl-z"), Some(vec![0x1a]));
        assert_eq!(bytes("ctrl-shift-a"), Some(vec![0x01]));
    }

    #[test]
    fn ctrl_punctuation_and_ctrl_space_are_the_classic_controls() {
        assert_eq!(bytes("ctrl-space"), Some(vec![0]));
        assert_eq!(bytes("ctrl-["), Some(vec![0x1b]));
        assert_eq!(bytes("ctrl-\\"), Some(vec![0x1c]));
        assert_eq!(bytes("ctrl-]"), Some(vec![0x1d]));
        assert_eq!(bytes("ctrl-/"), Some(vec![0x1f]));
        assert_eq!(bytes("ctrl-backspace"), Some(vec![0x08]));
    }

    #[test]
    fn arrows_follow_the_application_cursor_mode() {
        let up = stroke("up");
        assert_eq!(encode_key(&up, TermMode::empty()), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            encode_key(&up, TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        for (key, last) in [("down", b'B'), ("right", b'C'), ("left", b'D')] {
            assert_eq!(bytes(key), Some(vec![0x1b, b'[', last]));
        }
    }

    #[test]
    fn home_end_and_the_paging_keys_have_their_sequences() {
        assert_eq!(bytes("home"), Some(b"\x1b[H".to_vec()));
        assert_eq!(bytes("end"), Some(b"\x1b[F".to_vec()));
        assert_eq!(bytes("pageup"), Some(b"\x1b[5~".to_vec()));
        assert_eq!(bytes("pagedown"), Some(b"\x1b[6~".to_vec()));
        assert_eq!(bytes("insert"), Some(b"\x1b[2~".to_vec()));
        assert_eq!(bytes("delete"), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn function_keys_cover_f1_to_f20() {
        assert_eq!(bytes("f1"), Some(b"\x1bOP".to_vec()));
        assert_eq!(bytes("f4"), Some(b"\x1bOS".to_vec()));
        assert_eq!(bytes("f5"), Some(b"\x1b[15~".to_vec()));
        assert_eq!(bytes("f10"), Some(b"\x1b[21~".to_vec()));
        assert_eq!(bytes("f12"), Some(b"\x1b[24~".to_vec()));
        assert_eq!(bytes("f20"), Some(b"\x1b[34~".to_vec()));
        assert_eq!(bytes("f21"), None);
    }

    #[test]
    fn modified_named_keys_carry_the_modifier_parameter() {
        assert_eq!(bytes("ctrl-up"), Some(b"\x1b[1;5A".to_vec()));
        assert_eq!(bytes("shift-left"), Some(b"\x1b[1;2D".to_vec()));
        assert_eq!(bytes("alt-right"), Some(b"\x1b[1;3C".to_vec()));
        assert_eq!(bytes("ctrl-shift-end"), Some(b"\x1b[1;6F".to_vec()));
        assert_eq!(bytes("ctrl-delete"), Some(b"\x1b[3;5~".to_vec()));
        assert_eq!(bytes("shift-f5"), Some(b"\x1b[15;2~".to_vec()));
        assert_eq!(bytes("ctrl-f1"), Some(b"\x1b[1;5P".to_vec()));
        // Application cursor mode only changes the unmodified form.
        assert_eq!(
            encode_key(&stroke("ctrl-up"), TermMode::APP_CURSOR),
            Some(b"\x1b[1;5A".to_vec())
        );
    }

    #[test]
    fn alt_is_an_escape_prefix() {
        assert_eq!(bytes("alt-b"), Some(b"\x1bb".to_vec()));
        assert_eq!(bytes("alt-shift-b"), Some(b"\x1bB".to_vec()));
        assert_eq!(bytes("alt-enter"), Some(b"\x1b\r".to_vec()));
        assert_eq!(bytes("shift-enter"), Some(b"\x1b\r".to_vec()));
        assert_eq!(bytes("alt-backspace"), Some(b"\x1b\x7f".to_vec()));
        assert_eq!(bytes("ctrl-alt-b"), Some(b"\x1b\x02".to_vec()));
        assert_eq!(bytes("alt-space"), Some(b"\x1b ".to_vec()));
    }

    #[test]
    fn alt_uses_the_key_not_the_composed_character() {
        // macOS Option+a types "å"; the terminal still wants ESC a.
        let mut option_a = stroke("alt-a");
        option_a.key_char = Some("å".to_owned());
        assert_eq!(
            encode_key(&option_a, TermMode::empty()),
            Some(b"\x1ba".to_vec())
        );
    }

    #[test]
    fn plain_text_is_left_to_the_platform_text_input() {
        assert_eq!(encode_key(&typed("a", "a"), TermMode::empty()), None);
        assert_eq!(encode_key(&typed("a", "A"), TermMode::empty()), None);
        assert_eq!(encode_key(&typed("1", "1"), TermMode::empty()), None);
        assert_eq!(encode_key(&typed("é", "é"), TermMode::empty()), None);
        assert_eq!(bytes("space"), None);
    }

    #[test]
    fn the_platform_key_is_never_a_terminal_key() {
        assert_eq!(bytes("cmd-c"), None);
        assert_eq!(bytes("cmd-up"), None);
    }

    #[test]
    fn unmapped_ctrl_chords_send_nothing() {
        assert_eq!(bytes("ctrl-1"), None);
        assert_eq!(bytes("ctrl-,"), None);
    }

    #[test]
    fn a_bracketed_paste_is_wrapped_and_cannot_end_early() {
        assert_eq!(
            paste_bytes("hi\nthere", true),
            b"\x1b[200~hi\nthere\x1b[201~".to_vec()
        );
        assert_eq!(
            paste_bytes("a\x1b[201~b", true),
            b"\x1b[200~a[201~b\x1b[201~".to_vec()
        );
    }

    #[test]
    fn an_unbracketed_paste_turns_line_breaks_into_enters() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc".to_vec());
    }

    fn click(action: MouseAction, button: MouseButton) -> MouseEvent {
        MouseEvent {
            button,
            action,
            col: 4,
            row: 9,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    #[test]
    fn nothing_is_reported_until_the_program_asks() {
        let event = click(MouseAction::Press, MouseButton::Left);
        assert_eq!(mouse_report(&event, TermMode::empty()), None);
        assert!(!mouse_reporting(TermMode::empty()));
    }

    #[test]
    fn sgr_reports_press_and_release_with_one_based_cells() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            mouse_report(&click(MouseAction::Press, MouseButton::Left), mode),
            Some(b"\x1b[<0;5;10M".to_vec())
        );
        assert_eq!(
            mouse_report(&click(MouseAction::Release, MouseButton::Left), mode),
            Some(b"\x1b[<0;5;10m".to_vec())
        );
        assert_eq!(
            mouse_report(&click(MouseAction::Press, MouseButton::Right), mode),
            Some(b"\x1b[<2;5;10M".to_vec())
        );
    }

    #[test]
    fn the_wheel_and_modifiers_are_part_of_the_button_code() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let mut event = click(MouseAction::Press, MouseButton::WheelUp);
        assert_eq!(mouse_report(&event, mode), Some(b"\x1b[<64;5;10M".to_vec()));
        event.ctrl = true;
        event.shift = true;
        assert_eq!(mouse_report(&event, mode), Some(b"\x1b[<84;5;10M".to_vec()));
    }

    #[test]
    fn motion_is_reported_only_in_the_modes_that_ask_for_it() {
        let drag = click(MouseAction::Drag, MouseButton::Left);
        let click_only = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(mouse_report(&drag, click_only), None);
        let with_drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert_eq!(
            mouse_report(&drag, with_drag),
            Some(b"\x1b[<32;5;10M".to_vec())
        );
        let hover = click(MouseAction::Move, MouseButton::Left);
        assert_eq!(mouse_report(&hover, with_drag), None);
        let all = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(mouse_report(&hover, all), Some(b"\x1b[<35;5;10M".to_vec()));
    }

    #[test]
    fn the_legacy_encoding_offsets_by_32_and_has_a_limit() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            mouse_report(&click(MouseAction::Press, MouseButton::Left), mode),
            Some(vec![0x1b, b'[', b'M', 32, 32 + 5, 32 + 10])
        );
        assert_eq!(
            mouse_report(&click(MouseAction::Release, MouseButton::Left), mode),
            Some(vec![0x1b, b'[', b'M', 35, 32 + 5, 32 + 10])
        );
        let mut far = click(MouseAction::Press, MouseButton::Left);
        far.col = 300;
        assert_eq!(mouse_report(&far, mode), None);
    }

    #[test]
    fn a_wheel_turn_can_become_arrow_presses() {
        assert_eq!(
            wheel_as_arrows(true, 2, TermMode::empty()),
            b"\x1b[A\x1b[A".to_vec()
        );
        assert_eq!(
            wheel_as_arrows(false, 1, TermMode::APP_CURSOR),
            b"\x1bOB".to_vec()
        );
    }
}
