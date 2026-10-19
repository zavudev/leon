//! An image on the system clipboard, as bytes to keep in a file.
//!
//! The toolkit reads an image from the clipboard on macOS and Windows. On
//! Linux it does not report images at all (see `ui/paste.rs`), so there the
//! clipboard is asked through the desktop's own tool: `wl-paste` under
//! Wayland, `xclip` under X11. Which image type to ask for is [`best_type`],
//! from the types the clipboard lists.

use gpui_kit::{App, ClipboardEntry, ImageFormat};

/// An image taken from the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipImage {
    /// Its bytes, as the clipboard held them.
    pub bytes: Vec<u8>,
    /// The extension of a file of it: `png`, `jpg`…
    pub extension: &'static str,
}

/// The image types asked for, best first, with their extensions.
const TYPES: [(&str, &str); 5] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
    ("image/bmp", "bmp"),
];

/// The image type to ask the clipboard for, of the ones it lists (one per
/// line, as `wl-paste --list-types` and `xclip -t TARGETS` print them).
pub fn best_type(listed: &str) -> Option<(&'static str, &'static str)> {
    TYPES
        .into_iter()
        .find(|(mime, _)| listed.lines().any(|line| line.trim() == *mime))
}

/// The extension of a file of a toolkit's image format.
fn extension_of(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        _ => "img",
    }
}

/// The image on the clipboard, if there is one: the toolkit's first, else
/// the desktop's tool on Linux.
pub fn read(cx: &App) -> Option<ClipImage> {
    if let Some(item) = cx.read_from_clipboard() {
        for entry in item.entries() {
            if let ClipboardEntry::Image(image) = entry {
                return Some(ClipImage {
                    bytes: image.bytes().to_vec(),
                    extension: extension_of(image.format()),
                });
            }
        }
    }
    if cfg!(target_os = "linux") {
        return read_with_tool();
    }
    None
}

/// Asks `wl-paste` (Wayland) or `xclip` (X11) for the clipboard's image.
fn read_with_tool() -> Option<ClipImage> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (program, list, take): (&str, &[&str], fn(&str) -> Vec<String>) = if wayland {
        ("wl-paste", &["--list-types"], |mime| {
            vec!["--no-newline".into(), "--type".into(), mime.into()]
        })
    } else {
        (
            "xclip",
            &["-selection", "clipboard", "-t", "TARGETS", "-o"],
            |mime| {
                vec![
                    "-selection".into(),
                    "clipboard".into(),
                    "-t".into(),
                    mime.into(),
                    "-o".into(),
                ]
            },
        )
    };
    let listed = run(program, list.iter().map(|arg| (*arg).to_owned()).collect())?;
    let (mime, extension) = best_type(&String::from_utf8_lossy(&listed))?;
    let bytes = run(program, take(mime))?;
    (!bytes.is_empty()).then_some(ClipImage { bytes, extension })
}

/// What `program` prints, when it ran and succeeded.
fn run(program: &str, arguments: Vec<String>) -> Option<Vec<u8>> {
    let output = leon_remote::spawn::std_child(program)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_image_type_listed_is_asked_for() {
        let listed = "text/plain\nimage/jpeg\nimage/png\nSTRING\n";
        assert_eq!(best_type(listed), Some(("image/png", "png")));
        assert_eq!(best_type("image/webp"), Some(("image/webp", "webp")));
        assert_eq!(best_type("text/plain\nUTF8_STRING"), None);
        // A type is a whole line, not a part of one.
        assert_eq!(best_type("x-image/png-ish"), None);
    }
}
