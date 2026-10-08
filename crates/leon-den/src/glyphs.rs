//! The glyphs of the Den: the bubbles over a lion's head, the corners of the
//! selection and the little pixel font of the name tags.
//!
//! These are the only things the Den draws that are not pictures from a
//! file: they are chrome, in the colours of the host's theme, so they are
//! drawn here in letters, one per pixel, each letter a role of the palette
//! (see [`crate::palette::Role::of`]). The painter lays them over the room
//! at the size of its art.

use std::sync::LazyLock;

use crate::sprite::Sprite;

macro_rules! sprite {
    ($(#[$meta:meta])* $name:ident = [$($row:literal),* $(,)?]) => {
        $(#[$meta])*
        pub static $name: LazyLock<Sprite> = LazyLock::new(|| Sprite::parse(&[$($row),*]));
    };
}

sprite! {
    /// `!` on the accent: a cub waits for a new order. 7 by 10.
    BANG = [
        ".ooooo.",
        "oyyyyyo",
        "oyyYyyo",
        "oyyYyyo",
        "oyyYyyo",
        "oyyyyyo",
        "oyyYyyo",
        "oyyyyyo",
        ".ooooo.",
        "...o...",
    ]
}

sprite! {
    /// `!` in a warning triangle: a permission prompt. 11 by 11.
    BANG_URGENT = [
        "....ooo....",
        "...ooaoo...",
        "...oaaao...",
        "..ooaOaoo..",
        "..oaaOaao..",
        ".ooaaOaaoo.",
        ".oaaaOaaao.",
        "ooaaaaaaaoo",
        "oaaaaOaaaao",
        "oaaaaaaaaao",
        "ooooooooooo",
    ]
}

sprite! {
    /// The same sign lit up: the second frame of its flash.
    BANG_URGENT_ALT = [
        "....ooo....",
        "...ooyoo...",
        "...oyyyo...",
        "..ooyYyoo..",
        "..oyyYyyo..",
        ".ooyyYyyoo.",
        ".oyyyYyyyo.",
        "ooyyyyyyyoo",
        "oyyyyYyyyyo",
        "oyyyyyyyyyo",
        "ooooooooooo",
    ]
}

sprite! {
    /// `?` on the info colour: nothing is known of what the cub does.
    QUESTION = [
        ".ooooo.",
        "oiiiiio",
        "oiOOOio",
        "oiiiOio",
        "oiiOiio",
        "oiiiiio",
        "oiiOiio",
        "oiiiiio",
        ".ooooo.",
        "...o...",
    ]
}

sprite! {
    /// A cross on the error colour: the cub fainted.
    CROSS = [
        ".ooooo.",
        "orrrrro",
        "orOrOro",
        "orrOrro",
        "orOrOro",
        "orrrrro",
        ".ooooo.",
        "...o...",
    ]
}

sprite! {
    /// A tick on the success colour: the command passed.
    TICK = [
        ".ooooo.",
        "occccco",
        "occccOo",
        "ocOcOco",
        "occOcco",
        "occccco",
        ".ooooo.",
        "...o...",
    ]
}

sprite! {
    /// A thought bubble, empty: the dots are drawn in it one by one. 11 by 8.
    THOUGHT = [
        ".ooooooooo.",
        "owwwwwwwwwo",
        "owwwwwwwwwo",
        "owwwwwwwwwo",
        ".ooooooooo.",
        "...oo......",
        "..o........",
        "...........",
    ]
}

sprite! {
    /// A small `z`.
    ZED_SMALL = [
        "www",
        ".w.",
        "www",
    ]
}

sprite! {
    /// A bigger `Z`.
    ZED = [
        "wwwww",
        "...w.",
        "..w..",
        ".w...",
        "wwwww",
    ]
}

sprite! {
    /// The corner of the selection: an L of two pixels, top left. The other
    /// three are this one mirrored.
    CORNER = [
        "oooooo",
        "oyyyyo",
        "oyoooo",
        "oyo...",
        "oyo...",
        "ooo...",
    ]
}

sprite! {
    /// The corner of the hover outline.
    CORNER_HOVER = [
        "www",
        "w..",
        "w..",
    ]
}

sprite! {
    /// A sheet of paper thrown out of the shelf.
    PAPER = [
        "www",
        "wbw",
        "www",
    ]
}

sprite! {
    /// A dot of a thought.
    DOT = [
        "b",
    ]
}

// ------------------------------------------------------------ the font

/// A glyph of the little font: three pixels wide, five high, one bit per
/// pixel, the top row first and the left pixel the high bit of each row.
pub type Glyph = [u8; 5];

/// The width of a glyph.
pub const GLYPH_W: i32 = 3;
/// The height of a glyph.
pub const GLYPH_H: i32 = 5;
/// From the left edge of a glyph to the next one.
pub const GLYPH_ADVANCE: i32 = 4;

/// The glyph of a character in the little font. It has the capitals, the
/// digits, a few signs and a small `v` and `z`; other small letters are drawn
/// as their capital, and anything else as a small box.
pub fn glyph(letter: char) -> Glyph {
    // A letter with an accent is drawn as the letter: five rows of three
    // pixels have no room for the accent.
    let letter = fold(letter).unwrap_or(letter);
    let rows: [&str; 5] = match letter {
        'v' => ["...", "...", "#.#", "#.#", ".#."],
        'z' => ["...", "##.", ".#.", ".##", "..."],
        other => match other.to_ascii_uppercase() {
            'A' => [".#.", "#.#", "###", "#.#", "#.#"],
            'B' => ["##.", "#.#", "##.", "#.#", "##."],
            'C' => [".##", "#..", "#..", "#..", ".##"],
            'D' => ["##.", "#.#", "#.#", "#.#", "##."],
            'E' => ["###", "#..", "##.", "#..", "###"],
            'F' => ["###", "#..", "##.", "#..", "#.."],
            'G' => [".##", "#..", "#.#", "#.#", ".##"],
            'H' => ["#.#", "#.#", "###", "#.#", "#.#"],
            'I' => ["###", ".#.", ".#.", ".#.", "###"],
            'J' => ["..#", "..#", "..#", "#.#", ".#."],
            'K' => ["#.#", "#.#", "##.", "#.#", "#.#"],
            'L' => ["#..", "#..", "#..", "#..", "###"],
            'M' => ["#.#", "###", "###", "#.#", "#.#"],
            'N' => ["##.", "#.#", "#.#", "#.#", "#.#"],
            'O' => [".#.", "#.#", "#.#", "#.#", ".#."],
            'P' => ["##.", "#.#", "##.", "#..", "#.."],
            'Q' => [".#.", "#.#", "#.#", "###", ".##"],
            'R' => ["##.", "#.#", "##.", "#.#", "#.#"],
            'S' => [".##", "#..", ".#.", "..#", "##."],
            'T' => ["###", ".#.", ".#.", ".#.", ".#."],
            'U' => ["#.#", "#.#", "#.#", "#.#", "###"],
            'V' => ["#.#", "#.#", "#.#", "#.#", ".#."],
            'W' => ["#.#", "#.#", "###", "###", "#.#"],
            'X' => ["#.#", "#.#", ".#.", "#.#", "#.#"],
            'Y' => ["#.#", "#.#", ".#.", ".#.", ".#."],
            'Z' => ["###", "..#", ".#.", "#..", "###"],
            '0' => ["###", "#.#", "#.#", "#.#", "###"],
            '1' => [".#.", "##.", ".#.", ".#.", "###"],
            '2' => ["##.", "..#", ".#.", "#..", "###"],
            '3' => ["##.", "..#", ".#.", "..#", "##."],
            '4' => ["#.#", "#.#", "###", "..#", "..#"],
            '5' => ["###", "#..", "##.", "..#", "##."],
            '6' => [".##", "#..", "###", "#.#", "###"],
            '7' => ["###", "..#", ".#.", ".#.", ".#."],
            '8' => ["###", "#.#", "###", "#.#", "###"],
            '9' => ["###", "#.#", "###", "..#", "##."],
            '\u{2026}' => ["...", "...", "...", "...", "#.#"],
            ' ' => ["...", "...", "...", "...", "..."],
            '.' => ["...", "...", "...", "...", ".#."],
            '-' => ["...", "...", "###", "...", "..."],
            '_' => ["...", "...", "...", "...", "###"],
            '!' => [".#.", ".#.", ".#.", "...", ".#."],
            '?' => ["##.", "..#", ".#.", "...", ".#."],
            ':' => ["...", ".#.", "...", ".#.", "..."],
            '/' => ["..#", "..#", ".#.", "#..", "#.."],
            '+' => ["...", ".#.", "###", ".#.", "..."],
            _ => ["...", "###", "#.#", "###", "..."],
        },
    };
    rows.map(|row| {
        row.bytes()
            .fold(0u8, |bits, pixel| bits << 1 | u8::from(pixel == b'#'))
    })
}

/// The plain letter of a letter of the Latin alphabets with an accent or
/// another mark (`á` is `a`, `ñ` is `n`, `ç` is `c`, `ø` is `o`), `None` for
/// anything else. The German sharp s and the ligatures have no single
/// letter: [`plain`] writes them out.
pub fn fold(letter: char) -> Option<char> {
    let base = match letter {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' | 'ð' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => {
            // The capitals are the same letters.
            let lower = letter.to_lowercase().next()?;
            if lower == letter {
                return None;
            }
            return fold(lower).map(|base| base.to_ascii_uppercase());
        }
    };
    Some(base)
}

/// A text in the letters the little font has: accents are taken off, `ß`,
/// `æ` and `œ` are written out, the inverted marks of Spanish are left out.
/// What the font cannot write at all (another script) is left as it is, to
/// be drawn as a box or dropped by the caller.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for letter in text.chars() {
        match letter {
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'Æ' => out.push_str("AE"),
            'œ' => out.push_str("oe"),
            'Œ' => out.push_str("OE"),
            '¿' | '¡' => {}
            other => out.push(fold(other).unwrap_or(other)),
        }
    }
    out
}

/// Whether the little font has a letter of its own for a character.
pub fn writes(letter: char) -> bool {
    letter == ' ' || letter == '\u{2026}' || glyph(letter) != glyph('\u{1}')
}

/// How wide a text is in the little font, in pixels.
pub fn text_width(text: &str) -> i32 {
    let letters = text.chars().count() as i32;
    (letters * GLYPH_ADVANCE - 1).max(0)
}
