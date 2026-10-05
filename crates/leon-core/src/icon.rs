//! A project's logo: what kinds of image Leon accepts, how an image is
//! recognised from its bytes, and the record the store keeps.
//!
//! Recognition is by content, never by file name: the first bytes decide the
//! format, and a file that claims to be a PNG but is not one is refused. The
//! accepted formats are the ones the UI toolkit can decode (PNG, WebP, ICO,
//! JPEG) or render (SVG); anything else is skipped rather than drawn wrong.
//! Every image is capped at [`MAX_ICON_BYTES`].

use serde::{Deserialize, Serialize};

use crate::ids::ProjectId;

/// The largest image Leon stores or sends over a connection: 256 KiB.
pub const MAX_ICON_BYTES: usize = 256 * 1024;

/// The largest side, in pixels, a raster logo may declare. Bigger images are
/// refused: a logo is drawn at a few dozen pixels.
pub const MAX_ICON_SIDE: u32 = 8_192;

/// The image formats Leon accepts for a logo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconFormat {
    /// PNG.
    Png,
    /// WebP.
    Webp,
    /// Windows icon, possibly with several sizes (the toolkit's decoder takes
    /// the best entry).
    Ico,
    /// JPEG.
    Jpeg,
    /// SVG.
    Svg,
}

impl IconFormat {
    /// The short tag stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Ico => "ico",
            Self::Jpeg => "jpeg",
            Self::Svg => "svg",
        }
    }

    /// Parses the tag of [`IconFormat::as_str`].
    pub fn parse(tag: &str) -> Option<Self> {
        [Self::Png, Self::Webp, Self::Ico, Self::Jpeg, Self::Svg]
            .into_iter()
            .find(|format| format.as_str() == tag)
    }
}

/// The format of `bytes` when they are a well-formed image of a format Leon
/// accepts and within the size cap; `None` otherwise.
pub fn sniff(bytes: &[u8]) -> Option<IconFormat> {
    if bytes.is_empty() || bytes.len() > MAX_ICON_BYTES {
        return None;
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        // The first chunk is IHDR: 4 bytes length, "IHDR", width, height.
        let header = bytes.get(8..24)?;
        let (width, height) = (be32(&header[8..12]), be32(&header[12..16]));
        let sane = |side: u32| (1..=MAX_ICON_SIDE).contains(&side);
        return (&header[4..8] == b"IHDR" && sane(width) && sane(height))
            .then_some(IconFormat::Png);
    }
    if bytes.len() >= 16 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(IconFormat::Webp);
    }
    if bytes.starts_with(&[0, 0, 1, 0]) {
        return ico_is_well_formed(bytes).then_some(IconFormat::Ico);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(IconFormat::Jpeg);
    }
    svg_is_acceptable(bytes).then_some(IconFormat::Svg)
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// An ICO file: a directory of 1 to 64 entries of 16 bytes, each pointing at
/// image data inside the file.
fn ico_is_well_formed(bytes: &[u8]) -> bool {
    let Some(count) = bytes
        .get(4..6)
        .map(|c| usize::from(u16::from_le_bytes([c[0], c[1]])))
    else {
        return false;
    };
    if !(1..=64).contains(&count) || bytes.len() < 6 + 16 * count {
        return false;
    }
    (0..count).any(|index| {
        let entry = &bytes[6 + 16 * index..6 + 16 * (index + 1)];
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        size > 0
            && offset >= 6 + 16 * count
            && offset
                .checked_add(size)
                .is_some_and(|end| end <= bytes.len())
    })
}

/// An SVG: UTF-8 text with an `<svg` element near the top and nothing that
/// runs (no scripts).
fn svg_is_acceptable(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}');
    let head: String = text.chars().take(2_048).collect::<String>().to_lowercase();
    head.contains("<svg") && !text.to_lowercase().contains("<script")
}

/// A short stable fingerprint of an image's bytes (FNV-1a, 64 bits, in hex),
/// used to tell whether an image changed and as the key of the decoded-image
/// cache.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}{:08x}", bytes.len() as u32)
}

/// Where a project's logo came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IconKind {
    /// An image file found in the repository.
    Detected,
    /// The Git host's avatar of the repository's owner.
    Avatar,
    /// A file the user chose.
    Custom,
    /// Nothing was found: the folder glyph is shown.
    Folder,
}

impl IconKind {
    /// The short tag stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::Avatar => "avatar",
            Self::Custom => "custom",
            Self::Folder => "folder",
        }
    }

    /// Parses the tag of [`IconKind::as_str`].
    pub fn parse(tag: &str) -> Option<Self> {
        [Self::Detected, Self::Avatar, Self::Custom, Self::Folder]
            .into_iter()
            .find(|kind| kind.as_str() == tag)
    }
}

/// An image with its format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// What the bytes are.
    pub format: IconFormat,
    /// The image file's bytes.
    pub bytes: Vec<u8>,
}

impl IconImage {
    /// The image in `bytes`, when [`sniff`] accepts it.
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        sniff(&bytes).map(|format| Self { format, bytes })
    }
}

/// What a detection decided for a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIcon {
    /// Where the logo came from.
    pub kind: IconKind,
    /// Which rule and file, the host and owner, or the file's name:
    /// `nextjs-app:app/icon.png`, `github.com/zavudev`.
    pub source: String,
    /// The image; absent for [`IconKind::Folder`].
    pub image: Option<IconImage>,
}

/// A project's logo as the UI reads it: everything but the bytes, which are
/// fetched by [`ProjectIcon::hash`] only for an icon that is about to be drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIcon {
    /// The project.
    pub project_id: ProjectId,
    /// Where the logo in effect came from (a user's choice wins).
    pub kind: IconKind,
    /// See [`NewIcon::source`].
    pub source: String,
    /// The image's format, absent for the folder glyph.
    pub format: Option<IconFormat>,
    /// The image's [`content_hash`], absent for the folder glyph.
    pub hash: Option<String>,
    /// The repository's `host/owner/repo` as its origin remote names it, when
    /// detection read one.
    pub remote: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        bytes
    }

    fn ico(entries: usize, data_at: u32, data_len: u32, total: usize) -> Vec<u8> {
        let mut bytes = vec![0, 0, 1, 0];
        bytes.extend((entries as u16).to_le_bytes());
        for _ in 0..entries {
            bytes.extend([16, 16, 0, 0, 1, 0, 32, 0]);
            bytes.extend(data_len.to_le_bytes());
            bytes.extend(data_at.to_le_bytes());
        }
        bytes.resize(total, 0);
        bytes
    }

    #[test]
    fn a_png_is_recognised_by_its_signature_and_header() {
        assert_eq!(sniff(&png(64, 64)), Some(IconFormat::Png));
    }

    #[test]
    fn a_png_with_a_zero_or_absurd_side_is_refused() {
        assert_eq!(sniff(&png(0, 64)), None);
        assert_eq!(sniff(&png(64, MAX_ICON_SIDE + 1)), None);
    }

    #[test]
    fn a_truncated_png_is_refused() {
        assert_eq!(sniff(&png(8, 8)[..12]), None);
    }

    #[test]
    fn a_webp_is_recognised_by_its_riff_container() {
        let mut bytes = b"RIFF\x10\0\0\0WEBPVP8 ".to_vec();
        bytes.extend([0; 8]);
        assert_eq!(sniff(&bytes), Some(IconFormat::Webp));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVEfmt \0\0\0\0"), None);
    }

    #[test]
    fn an_ico_needs_a_directory_whose_entry_points_inside_the_file() {
        assert_eq!(sniff(&ico(2, 38, 10, 64)), Some(IconFormat::Ico));
        assert_eq!(sniff(&ico(1, 22, 100, 64)), None, "data past the end");
        assert_eq!(sniff(&ico(1, 2, 10, 64)), None, "data inside the directory");
        assert_eq!(sniff(&ico(0, 22, 10, 64)), None, "no entries");
    }

    #[test]
    fn a_jpeg_is_recognised_by_its_marker() {
        assert_eq!(
            sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]),
            Some(IconFormat::Jpeg)
        );
    }

    #[test]
    fn an_svg_is_recognised_as_text_and_refused_when_it_carries_a_script() {
        assert_eq!(
            sniff(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some(IconFormat::Svg)
        );
        assert_eq!(sniff(b"\xef\xbb\xbf<svg/>"), Some(IconFormat::Svg));
        assert_eq!(sniff(b"<svg><script>alert(1)</script></svg>"), None);
        assert_eq!(sniff(b"<html>not an image</html>"), None);
    }

    #[test]
    fn text_and_unknown_formats_are_refused_whatever_their_name() {
        assert_eq!(sniff(b"hello"), None);
        assert_eq!(sniff(b"GIF89a....."), None, "gif is not offered");
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn an_image_over_the_cap_is_refused() {
        let mut big = png(8, 8);
        big.resize(MAX_ICON_BYTES + 1, 0);
        assert_eq!(sniff(&big), None);
        big.truncate(MAX_ICON_BYTES);
        assert_eq!(sniff(&big), Some(IconFormat::Png));
    }

    #[test]
    fn the_content_hash_tells_images_apart_and_is_stable() {
        let a = content_hash(b"one");
        assert_eq!(a, content_hash(b"one"));
        assert_ne!(a, content_hash(b"two"));
        assert_eq!(a.len(), 24);
    }

    #[test]
    fn tags_round_trip() {
        for kind in [
            IconKind::Detected,
            IconKind::Avatar,
            IconKind::Custom,
            IconKind::Folder,
        ] {
            assert_eq!(IconKind::parse(kind.as_str()), Some(kind));
        }
        for format in [
            IconFormat::Png,
            IconFormat::Webp,
            IconFormat::Ico,
            IconFormat::Jpeg,
            IconFormat::Svg,
        ] {
            assert_eq!(IconFormat::parse(format.as_str()), Some(format));
        }
    }
}
