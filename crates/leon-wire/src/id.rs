//! Identifiers: a host's id and the unambiguous base32 they are written in.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// The base32 alphabet: digits and capitals without `0 1 I O`, so a code read
/// aloud or copied by hand is not misread. Input is case-insensitive.
pub const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Encodes `bytes` in unpadded base32 using the unambiguous alphabet.
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let (mut acc, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        acc = (acc << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Decodes what [`base32`] produced. `None` on a character outside the
/// alphabet. Either case is accepted.
pub fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in text.chars() {
        let value = symbol_value(ch)?;
        acc = (acc << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The value of one symbol; `None` outside the alphabet.
pub fn symbol_value(ch: char) -> Option<u32> {
    let upper = ch.to_ascii_uppercase();
    ALPHABET
        .iter()
        .position(|&s| char::from(s) == upper)
        .map(|p| p as u32)
}

/// Why an id could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdError {
    /// Not 26 symbols of the alphabet.
    #[error("not a host id")]
    Malformed,
}

/// Names a host: 16 bytes of a hash of its Ed25519 verifying key. The relay
/// checks that a host owns its id by a signature, so an id cannot be taken
/// by anyone who lacks the key.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct HostId([u8; 16]);

impl HostId {
    /// The id of the host whose signing key is `public`.
    pub fn from_verifying_key(public: &[u8; 32]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"leon-host-id-v1");
        hash.update(public);
        let digest = hash.finalize();
        let mut id = [0u8; 16];
        id.copy_from_slice(&digest[..16]);
        Self(id)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// An id from its raw bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// A short form for display: the first four groups of three.
    pub fn short(&self) -> String {
        let text = self.to_string();
        text.get(..12)
            .map(|head| {
                format!(
                    "{}-{}-{}-{}",
                    &head[0..3],
                    &head[3..6],
                    &head[6..9],
                    &head[9..12]
                )
            })
            .unwrap_or(text)
    }
}

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base32(&self.0))
    }
}

impl fmt::Debug for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HostId({})", self.short())
    }
}

impl FromStr for HostId {
    type Err = IdError;
    fn from_str(text: &str) -> Result<Self, IdError> {
        let bytes = unbase32(text.trim()).ok_or(IdError::Malformed)?;
        let id: [u8; 16] = bytes.try_into().map_err(|_| IdError::Malformed)?;
        let id = Self(id);
        // Reject non-canonical spellings (trailing padding bits set).
        if id.to_string().eq_ignore_ascii_case(text.trim()) {
            Ok(id)
        } else {
            Err(IdError::Malformed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips_every_length_up_to_forty() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = base32(&bytes);
            let back = unbase32(&text).unwrap();
            assert_eq!(&back[..bytes.len()], &bytes[..], "length {len}");
        }
    }

    #[test]
    fn the_alphabet_has_no_ambiguous_symbols() {
        for bad in ['0', 'O', '1', 'I'] {
            assert!(!ALPHABET.contains(&(bad as u8)), "{bad}");
        }
        let mut seen = ALPHABET.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 32);
    }

    #[test]
    fn a_host_id_is_derived_from_the_key_and_round_trips_as_text() {
        let id = HostId::from_verifying_key(&[7; 32]);
        assert_eq!(id, HostId::from_verifying_key(&[7; 32]));
        assert_ne!(id, HostId::from_verifying_key(&[8; 32]));
        let text = id.to_string();
        assert_eq!(text.len(), 26);
        assert_eq!(text.parse::<HostId>().unwrap(), id);
    }

    #[test]
    fn malformed_host_ids_are_refused() {
        for bad in ["", "abc", "0000000000000000000000000!", &"2".repeat(27)] {
            assert_eq!(bad.parse::<HostId>(), Err(IdError::Malformed), "{bad:?}");
        }
    }

    #[test]
    fn the_debug_form_shows_only_the_short_id() {
        let id = HostId::from_verifying_key(&[1; 32]);
        assert!(format!("{id:?}").len() < 30);
    }
}
