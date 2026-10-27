//! An image on the system clipboard, as bytes to keep in a file.
//!
//! The toolkit reads an image from the clipboard on macOS and Windows. On
//! Linux it does not report images at all (see `ui/paste.rs`), so there the
//! clipboard is asked through the desktop's own tool: `wl-paste` under
//! Wayland, `xclip` under X11. Which image type to ask for is [`best_type`],
//! from the types the clipboard lists.
//!
//! On macOS the toolkit gives the clipboard's text before its image, so an
//! image copied with its address (from a browser) reads as the address.
//! When what it gives looks like that, `osascript` is asked for the image
//! as PNG ([`looks_like_an_address`]). Files copied in the Finder are
//! [`files`], attached as they are.

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
        ImageFormat::Tiff => "tiff",
        _ => "img",
    }
}

/// The image on the clipboard, if there is one: the toolkit's first, else
/// the desktop's tool on Linux.
pub fn read(cx: &App) -> Option<ClipImage> {
    let item = cx.read_from_clipboard();
    if let Some(item) = &item {
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
    if cfg!(target_os = "macos") {
        let text = item.as_ref().and_then(|item| item.text());
        if text.as_deref().is_none_or(looks_like_an_address) {
            return read_with_osascript();
        }
    }
    None
}

/// Files copied in the file manager (the Finder, Files, Explorer): the
/// paths the clipboard holds. None when it holds none.
pub fn files(cx: &App) -> Vec<std::path::PathBuf> {
    cx.read_from_clipboard()
        .into_iter()
        .flat_map(|item| item.into_entries())
        .flat_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => paths.paths().to_vec(),
            _ => Vec::new(),
        })
        .collect()
}

/// Whether the clipboard's text may be what a browser puts beside an image
/// it copies: one line that is an address. Words and code are pasted as
/// they are, without asking anything else.
pub fn looks_like_an_address(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && !text.contains('\n')
        && ["http://", "https://", "file://", "data:image/"]
            .iter()
            .any(|scheme| text.starts_with(scheme))
}

/// Asks AppleScript for the clipboard's image as PNG, through a file: it
/// converts whatever image the clipboard holds (TIFF, PDF…).
fn read_with_osascript() -> Option<ClipImage> {
    const SCRIPT: [&str; 9] = [
        "on run argv",
        "set d to (the clipboard as «class PNGf»)",
        "set f to open for access (POSIX file (item 1 of argv)) with write permission",
        "set eof f to 0",
        "write d to f",
        "close access f",
        "return \"ok\"",
        "end run",
        "",
    ];
    let path = std::env::temp_dir().join(format!("leon-clipboard-{}.png", std::process::id()));
    let mut arguments = Vec::new();
    for line in SCRIPT.iter().filter(|line| !line.is_empty()) {
        arguments.push("-e".to_owned());
        arguments.push((*line).to_owned());
    }
    arguments.push(path.to_string_lossy().into_owned());
    run("osascript", arguments)?;
    let bytes = std::fs::read(&path).ok();
    let _ = std::fs::remove_file(&path);
    let bytes = bytes.filter(|bytes| !bytes.is_empty())?;
    Some(ClipImage {
        bytes,
        extension: "png",
    })
}

/// A tool that reads the clipboard: its program, the arguments that list
/// the types, and the arguments that print one type.
type Tool = (
    &'static str,
    &'static [&'static str],
    fn(&str) -> Vec<String>,
);

/// Asks `wl-paste` (Wayland) or `xclip` (X11) for the clipboard's image.
fn read_with_tool() -> Option<ClipImage> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (program, list, take): Tool = if wayland {
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
    fn only_an_address_on_one_line_may_hide_an_image() {
        assert!(looks_like_an_address("https://example.com/cat.png"));
        assert!(looks_like_an_address(" file:///Users/ada/shot.png\n"));
        assert!(!looks_like_an_address("fix the build"));
        assert!(!looks_like_an_address("https://a.dev\nand more"));
        assert!(!looks_like_an_address(""));
    }

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
