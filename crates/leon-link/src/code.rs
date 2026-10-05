//! The short one-time pairing code.
//!
//! A code is ten symbols of an unambiguous alphabet, shown as `ABCD-EFG-HJK`:
//!
//! * the first four are the **room**: not secret, they only tell the relay
//!   which waiting host to connect a newcomer to;
//! * the last six are the **secret**: 30 bits that never leave the two
//!   computers and feed SPAKE2. A password-authenticated key exchange lets
//!   such a short secret be safe: an attacker, the relay included, gets one
//!   guess per attempt and learns nothing offline, and the host burns the code
//!   after a handful of failures (see [`crate::pairing::PairingOffer`]).

use std::fmt;

use leon_wire::id::{symbol_value, ALPHABET};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroize;

/// Symbols in the room part.
pub const ROOM_LEN: usize = leon_wire::relay::ROOM_LEN;
/// Symbols in the secret part.
pub const SECRET_LEN: usize = 6;
/// Symbols in a whole code.
pub const CODE_LEN: usize = ROOM_LEN + SECRET_LEN;

/// What typing a code looks like so far; for live validation in a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeShape {
    /// Nothing typed.
    Empty,
    /// A valid beginning: `have` of `need` symbols.
    Incomplete {
        /// Symbols so far.
        have: usize,
        /// Symbols a code has.
        need: usize,
    },
    /// A well-formed code.
    Complete,
    /// A character that is not in the alphabet (`0`, `1`, `I` and `O` are not
    /// used, so they are never mistaken for each other).
    BadCharacter(char),
    /// More symbols than a code has.
    TooLong,
}

/// Cleans typed input: drops separators and spaces, upper-cases.
fn normalise(input: &str) -> Vec<char> {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_' | '.'))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Classifies what has been typed.
pub fn shape(input: &str) -> CodeShape {
    let symbols = normalise(input);
    if symbols.is_empty() {
        return CodeShape::Empty;
    }
    if let Some(bad) = symbols.iter().find(|c| symbol_value(**c).is_none()) {
        return CodeShape::BadCharacter(*bad);
    }
    match symbols.len() {
        CODE_LEN => CodeShape::Complete,
        n if n > CODE_LEN => CodeShape::TooLong,
        have => CodeShape::Incomplete {
            have,
            need: CODE_LEN,
        },
    }
}

/// A pairing code.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    symbols: [u8; CODE_LEN],
}

impl PairingCode {
    /// A fresh random code.
    pub fn generate() -> Self {
        let mut random = [0u8; CODE_LEN];
        OsRng.fill_bytes(&mut random);
        let mut symbols = [0u8; CODE_LEN];
        for (symbol, byte) in symbols.iter_mut().zip(random) {
            // 32 symbols and a byte's low five bits: uniform.
            *symbol = ALPHABET[usize::from(byte & 31)];
        }
        Self { symbols }
    }

    /// Reads a typed code; `None` unless [`shape`] says it is complete.
    pub fn parse(input: &str) -> Option<Self> {
        if shape(input) != CodeShape::Complete {
            return None;
        }
        let mut symbols = [0u8; CODE_LEN];
        for (slot, ch) in symbols.iter_mut().zip(normalise(input)) {
            *slot = ch as u8;
        }
        Some(Self { symbols })
    }

    /// The public room.
    pub fn room(&self) -> String {
        String::from_utf8_lossy(&self.symbols[..ROOM_LEN]).into_owned()
    }

    /// The secret, as the SPAKE2 password.
    pub(crate) fn password(&self) -> &[u8] {
        &self.symbols[ROOM_LEN..]
    }

    /// The code as shown: `ABCD-EFG-HJK`.
    pub fn display(&self) -> String {
        let text = String::from_utf8_lossy(&self.symbols);
        format!("{}-{}-{}", &text[0..4], &text[4..7], &text[7..10])
    }
}

impl Drop for PairingCode {
    fn drop(&mut self) {
        self.symbols.zeroize();
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PairingCode(room {}, secret hidden)", self.room())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_code_has_ten_symbols_of_the_alphabet_and_parses_back() {
        for _ in 0..50 {
            let code = PairingCode::generate();
            let shown = code.display();
            assert_eq!(shown.len(), 12, "{shown}");
            assert_eq!(PairingCode::parse(&shown), Some(code.clone()));
            assert_eq!(code.room().len(), ROOM_LEN);
            assert_eq!(code.password().len(), SECRET_LEN);
        }
    }

    #[test]
    fn typing_is_forgiving_about_case_spaces_and_dashes() {
        let a = PairingCode::parse("abcd efg-hjk").unwrap();
        let b = PairingCode::parse("ABCDEFGHJK").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn the_shape_follows_the_typing() {
        assert_eq!(shape(""), CodeShape::Empty);
        assert_eq!(shape("  - "), CodeShape::Empty);
        assert_eq!(shape("AB2"), CodeShape::Incomplete { have: 3, need: 10 });
        assert_eq!(shape("ABCD-EFG-HJK"), CodeShape::Complete);
        assert_eq!(shape("ABCDEFGHJKM"), CodeShape::TooLong);
        assert_eq!(shape("AB0D"), CodeShape::BadCharacter('0'));
        assert_eq!(shape("ABOD"), CodeShape::BadCharacter('O'));
    }

    #[test]
    fn an_incomplete_or_wrong_code_does_not_parse() {
        assert!(PairingCode::parse("ABCD").is_none());
        assert!(PairingCode::parse("ABCD-EFG-HJ1").is_none());
    }

    #[test]
    fn debug_hides_the_secret_part() {
        let code = PairingCode::parse("ABCD-EFG-HJK").unwrap();
        let text = format!("{code:?}");
        assert!(text.contains("ABCD"));
        assert!(!text.contains("EFG"));
    }

    #[test]
    fn codes_are_not_repeated_in_a_sample() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            assert!(seen.insert(PairingCode::generate().display()));
        }
    }
}
