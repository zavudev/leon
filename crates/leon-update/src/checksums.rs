//! `SHA256SUMS`: reading it and holding a file to it.
//!
//! The file is the one `sha256sum` writes: `<64 hex>  <name>` per line (a `*`
//! before the name, for binary mode, is accepted). It is read strictly. Any line
//! that is not that, or a name that is listed twice, makes the whole file
//! unusable for that name, and a file is believed only when its hash is in
//! the file exactly once and equal to the file's own, compared in constant time.

use sha2::{Digest as _, Sha256};
use std::io::Read as _;
use std::path::Path;
use subtle::ConstantTimeEq as _;

/// Why a file was not verified.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// A line of `SHA256SUMS` is not `<hash>  <name>`.
    #[error("SHA256SUMS has a line that is not a checksum (line {0})")]
    Malformed(usize),
    /// The file is not listed.
    #[error("{0} is not in SHA256SUMS")]
    Missing(String),
    /// The file is listed more than once.
    #[error("{0} is in SHA256SUMS more than once")]
    Duplicate(String),
    /// The hash does not match.
    #[error("the checksum of {0} does not match SHA256SUMS")]
    Mismatch(String),
    /// The size is not the one GitHub states.
    #[error("{name} is {found} bytes, not the {expected} that the release states")]
    Size {
        /// The file.
        name: String,
        /// What it is.
        found: u64,
        /// What the release says.
        expected: u64,
    },
    /// The file could not be read.
    #[error("cannot read the file: {0}")]
    Io(String),
}

/// The 32 bytes a 64-digit hex text spells, in either case.
pub fn decode_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (place, pair) in text.as_bytes().chunks(2).enumerate() {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out[place] = (high * 16 + low) as u8;
    }
    Some(out)
}

/// The lower-case hex of a hash.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Every `(hash, name)` of the file, in order. CRLF line ends, a final line
/// without one and blank lines are fine; nothing else that is not a checksum
/// line is.
pub fn parse(text: &str) -> Result<Vec<([u8; 32], String)>, Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut entries = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let malformed = || Error::Malformed(index + 1);
        let (hash, rest) = line.split_once(' ').ok_or_else(malformed)?;
        let hash = decode_hex(hash).ok_or_else(malformed)?;
        // `sha256sum` writes two spaces for text mode, a space and `*` for binary.
        let name = rest
            .strip_prefix(' ')
            .or_else(|| rest.strip_prefix('*'))
            .ok_or_else(malformed)?;
        if name.is_empty() || name.contains(['/', '\\']) || name.trim() != name {
            return Err(malformed());
        }
        entries.push((hash, name.to_owned()));
    }
    Ok(entries)
}

/// The hash `text` lists for `name`: there has to be exactly one line.
pub fn expected(text: &str, name: &str) -> Result<[u8; 32], Error> {
    let entries = parse(text)?;
    let mut found = entries.iter().filter(|(_, listed)| listed == name);
    let first = found
        .next()
        .ok_or_else(|| Error::Missing(name.to_owned()))?;
    if found.next().is_some() {
        return Err(Error::Duplicate(name.to_owned()));
    }
    Ok(first.0)
}

/// The SHA-256 and the size of a file.
pub fn hash_file(path: &Path) -> std::io::Result<([u8; 32], u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((hasher.finalize().into(), size))
}

/// Holds the file at `path` to `SHA256SUMS` (`sums`) under `name`, to the size
/// the release states and, when GitHub states one, to its digest as well.
pub fn verify_file(
    path: &Path,
    name: &str,
    sums: &str,
    size: u64,
    digest: Option<[u8; 32]>,
) -> Result<(), Error> {
    let listed = expected(sums, name)?;
    let (actual, found) = hash_file(path).map_err(|error| Error::Io(error.to_string()))?;
    if found != size {
        return Err(Error::Size {
            name: name.to_owned(),
            found,
            expected: size,
        });
    }
    let mut good = bool::from(actual.ct_eq(&listed));
    if let Some(stated) = digest {
        good &= bool::from(actual.ct_eq(&stated));
    }
    if good {
        Ok(())
    } else {
        Err(Error::Mismatch(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sums(files: &[(&str, &[u8])]) -> String {
        files
            .iter()
            .map(|(name, bytes)| format!("{}  {name}\n", hex(&Sha256::digest(bytes))))
            .collect()
    }

    fn file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn a_good_file_verifies() {
        let (_dir, path) = file(b"leon");
        let text = sums(&[("a.zip", b"other"), ("leon.zip", b"leon")]);
        assert_eq!(verify_file(&path, "leon.zip", &text, 4, None), Ok(()));
    }

    #[test]
    fn a_wrong_hash_is_a_mismatch() {
        let (_dir, path) = file(b"leon");
        let text = sums(&[("leon.zip", b"not leon")]);
        assert_eq!(
            verify_file(&path, "leon.zip", &text, 4, None),
            Err(Error::Mismatch("leon.zip".into()))
        );
    }

    #[test]
    fn a_name_that_is_not_listed_is_missing() {
        let (_dir, path) = file(b"leon");
        let text = sums(&[("other.zip", b"leon")]);
        assert_eq!(
            verify_file(&path, "leon.zip", &text, 4, None),
            Err(Error::Missing("leon.zip".into()))
        );
        assert_eq!(
            verify_file(&path, "leon.zip", "", 4, None),
            Err(Error::Missing("leon.zip".into()))
        );
    }

    #[test]
    fn a_name_listed_twice_is_refused_even_with_the_same_hash() {
        let (_dir, path) = file(b"leon");
        let line = sums(&[("leon.zip", b"leon")]);
        let text = format!("{line}{line}");
        assert_eq!(
            verify_file(&path, "leon.zip", &text, 4, None),
            Err(Error::Duplicate("leon.zip".into()))
        );
    }

    #[test]
    fn a_file_with_windows_line_ends_and_a_binary_marker_is_read() {
        let (_dir, path) = file(b"leon");
        let hash = hex(&Sha256::digest(b"leon"));
        let text = format!(
            "{}  other.zip\r\n\r\n{hash} *leon.zip\r\n",
            hex(&Sha256::digest(b"x"))
        );
        assert_eq!(verify_file(&path, "leon.zip", &text, 4, None), Ok(()));
        // And with a byte order mark and no final newline.
        let text = format!("\u{feff}{hash}  leon.zip");
        assert_eq!(verify_file(&path, "leon.zip", &text, 4, None), Ok(()));
    }

    #[test]
    fn a_line_that_is_not_a_checksum_spoils_the_file() {
        let hash = "ab".repeat(32);
        for bad in [
            "garbage".to_owned(),
            format!("{hash} leon.zip"),
            format!("{} leon.zip", "ab".repeat(31)),
            format!("{hash}  dir/leon.zip"),
            format!("{hash}  "),
            format!("{}  leon.zip", "zz".repeat(32)),
            format!("{hash}   leon.zip"),
        ] {
            let text = format!("{hash}  fine.zip\n{bad}\n");
            assert!(
                matches!(expected(&text, "fine.zip"), Err(Error::Malformed(2))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_size_has_to_be_the_one_the_release_states() {
        let (_dir, path) = file(b"leon");
        let text = sums(&[("leon.zip", b"leon")]);
        assert_eq!(
            verify_file(&path, "leon.zip", &text, 5, None),
            Err(Error::Size {
                name: "leon.zip".into(),
                found: 4,
                expected: 5
            })
        );
    }

    #[test]
    fn a_stated_digest_must_agree_too() {
        let (_dir, path) = file(b"leon");
        let text = sums(&[("leon.zip", b"leon")]);
        let right: [u8; 32] = Sha256::digest(b"leon").into();
        assert_eq!(
            verify_file(&path, "leon.zip", &text, 4, Some(right)),
            Ok(())
        );
        assert_eq!(
            verify_file(&path, "leon.zip", &text, 4, Some([0; 32])),
            Err(Error::Mismatch("leon.zip".into()))
        );
    }

    #[test]
    fn hex_is_read_in_both_cases_and_only_whole() {
        assert_eq!(decode_hex(&"Ab".repeat(32)), Some([0xab; 32]));
        assert_eq!(decode_hex(&"ab".repeat(31)), None);
        assert_eq!(decode_hex(&"é".repeat(32)), None);
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
    }

    #[test]
    fn the_real_v0_1_0_file_parses() {
        let text = "2c6a1c6894b1dfd4fcb346f883d425d03a6d36d06aaf0d727e71437ce10cc9b8  leon-0.1.0-linux-x86_64.tar.gz\n\
                    3028b0596f12346f4f1100dad087b0f0f5ff09c7948d7b05463e72990784ee64  leon-0.1.0-macos-aarch64.dmg\n";
        assert_eq!(parse(text).unwrap().len(), 2);
        assert!(expected(text, "leon-0.1.0-macos-aarch64.dmg").is_ok());
    }
}
