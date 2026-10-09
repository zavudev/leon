//! A file open in the editor: what it is, where it is and whether it changed.
//!
//! [`EditorDoc`] is the side table entry of a leaf that is a file (see
//! `mod.rs`). Its text lives in the editor's own state; what is kept here is
//! what the text was when it was last read or saved (a hash, so that undoing
//! back to it makes the file clean again), the revision a save must find on
//! disk, and the two things the editor does not hold: the byte order mark
//! and the line ends, put back when the file is saved.
//!
//! The pure parts ([`decode`], [`encode`], [`language_of`]) are tested below.

use super::super::shell::Shell;
use super::drafts::Draft;
use crate::files::{FileContent, FileRevision, ImageContent, Stamp};
use gpui_kit::component::input::{EditorState, InputEvent, TabSize};
use gpui_kit::component::text::TextViewState;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Image, ImageFormat, Subscription, Task, Window,
};
use leon_core::MachineId;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The editor settings that are plain values: line numbers, indent guides,
/// wrapping and the tab.
fn editor_prefs(cx: &App) -> (bool, bool, bool, TabSize) {
    use crate::settings::{flag, int};
    (
        flag(cx, "editor_line_numbers"),
        flag(cx, "editor_indent_guides"),
        flag(cx, "editor_soft_wrap"),
        TabSize {
            tab_size: int(cx, "editor_tab_size").clamp(1, 8) as usize,
            hard_tabs: false,
        },
    )
}

/// How a file ends its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    /// `\n`.
    Lf,
    /// `\r\n`, on every line.
    Crlf,
}

/// A file's text as the editor holds it, with what the editor does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The text without its byte order mark and, for a file of `\r\n` lines,
    /// with `\n` lines.
    pub body: String,
    /// How the file ended its lines.
    pub eol: Eol,
    /// Whether the file started with a byte order mark.
    pub bom: bool,
}

/// Splits what a file holds into what the editor shows and what it hides.
/// A file whose lines do not all end the same way keeps its `\r`s in the
/// text: nothing is changed that the person did not change.
pub fn decode(raw: &str) -> Decoded {
    let (bom, text) = match raw.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let lines = text.matches('\n').count();
    let crlf = text.matches("\r\n").count();
    if lines > 0 && crlf == lines {
        Decoded {
            body: text.replace("\r\n", "\n"),
            eol: Eol::Crlf,
            bom,
        }
    } else {
        Decoded {
            body: text.to_owned(),
            eol: Eol::Lf,
            bom,
        }
    }
}

/// What [`decode`] undoes: the bytes to write for the editor's text.
pub fn encode(body: &str, eol: Eol, bom: bool) -> String {
    let mut out = String::with_capacity(body.len() + 3);
    if bom {
        out.push('\u{feff}');
    }
    match eol {
        Eol::Lf => out.push_str(body),
        Eol::Crlf => out.push_str(&body.replace('\n', "\r\n")),
    }
    out
}

/// A number that differs when the text does: enough to tell whether it is
/// what was saved.
pub fn fingerprint(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The name of the language a file is highlighted as, by its name and then
/// its extension; `"text"` for what is not known.
pub fn detect(path: &str) -> &'static str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name {
        "Makefile" | "makefile" | "GNUmakefile" => return "make",
        "Rakefile" | "Gemfile" => return "ruby",
        ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" => return "bash",
        _ => {}
    }
    let Some((_, extension)) = name.rsplit_once('.') else {
        return "text";
    };
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyi" => "python",
        "go" => "go",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        "md" | "markdown" => "markdown",
        "css" | "scss" => "css",
        "html" | "htm" => "html",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "lua" => "lua",
        "mk" => "make",
        "diff" | "patch" => "diff",
        "java" => "java",
        "rb" | "gemspec" => "ruby",
        _ => "text",
    }
}

/// [`detect`], or plain text when the grammars are not built in.
pub fn language_of(path: &str) -> &'static str {
    if cfg!(feature = "languages") {
        detect(path)
    } else {
        "text"
    }
}

/// The format of a picture the viewer shows, by the extension of its name.
/// SVG is not one: it is text, and stays in the editor where it can be
/// changed.
pub fn image_format(path: &str) -> Option<ImageFormat> {
    let (_, extension) = file_name(path).rsplit_once('.')?;
    Some(match extension.to_ascii_lowercase().as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        "ico" => ImageFormat::Ico,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

/// The width and height of a picture, read from its header; `None` for bytes
/// that are not a picture of a format the build knows.
fn dimensions_of(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// The last name of a path.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The path a person typed, as the machine knows it: an absolute path stays
/// as it is and any other is taken from `cwd`, written the way `cwd` is.
pub fn resolve(cwd: &str, typed: &str) -> String {
    let typed = typed.trim();
    let drive = typed.len() > 2
        && typed.as_bytes()[0].is_ascii_alphabetic()
        && typed.as_bytes()[1] == b':'
        && matches!(typed.as_bytes()[2], b'/' | b'\\');
    if typed.starts_with(['/', '\\']) || drive || cwd.is_empty() {
        return typed.to_owned();
    }
    let separator = if cwd.contains('\\') && !cwd.contains('/') {
        '\\'
    } else {
        '/'
    };
    let typed = typed.trim_start_matches("./");
    format!("{}{separator}{typed}", cwd.trim_end_matches(['/', '\\']))
}

/// The folder a file is in.
pub fn parent_of(path: &str) -> &str {
    match path.rsplit_once(['/', '\\']) {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => "",
    }
}

/// What a leaf shows for a file.
pub enum Body {
    /// Text, in the editor.
    Text(Entity<EditorState>),
    /// Bytes that are not text: said instead of shown.
    Binary(u64),
    /// More than the editor opens: said instead of shown.
    TooBig(u64),
    /// A picture, drawn instead of an editor.
    Image(Picture),
    /// What `Open anyway` showed of such a file: its bytes as lossy text, in
    /// an editor that cannot change them. Never saved.
    Lossy {
        /// The read-only editor.
        state: Entity<EditorState>,
        /// How many bytes of the file it shows.
        shown: u64,
        /// How many bytes the file has.
        size: u64,
    },
}

/// A picture open in a leaf: its bytes, held for the toolkit to decode off the
/// UI thread, and what is known of it.
pub struct Picture {
    /// What is drawn. A picture read again is another image (the toolkit
    /// caches by the bytes), so a file that changed on disk is drawn anew.
    pub image: Arc<Image>,
    /// The size of the file, in bytes.
    pub size: u64,
    /// Width and height in pixels, when the header said.
    pub dimensions: Option<(u32, u32)>,
    /// What the file was on disk when the bytes were read.
    pub stamp: Stamp,
}

impl Picture {
    /// A picture from the bytes of a file named `path`.
    fn new(path: &str, bytes: Vec<u8>, stamp: Stamp) -> Option<Self> {
        let format = image_format(path)?;
        Some(Self {
            size: bytes.len() as u64,
            dimensions: dimensions_of(&bytes),
            image: Arc::new(Image::from_bytes(format, bytes)),
            stamp,
        })
    }
}

/// The most of a file that `Open anyway` shows.
pub const LOSSY_LIMIT: usize = 8 * 1024 * 1024;

/// A file open in a leaf.
pub struct EditorDoc {
    /// The machine the file is on.
    pub machine: MachineId,
    /// Its path there.
    pub path: String,
    /// The folder whose workspace the file's tab is in.
    pub folder: String,
    /// What is shown.
    pub body: Body,
    /// The revision the file had when it was read or last saved; a save
    /// refuses a file that has another.
    pub revision: Option<FileRevision>,
    /// The fingerprint of the text that was read or last saved.
    pub saved: u64,
    /// Whether the text is not what was read or last saved.
    pub dirty: bool,
    /// Whether a save found the file changed by somebody else.
    pub conflict: bool,
    /// Whether a save is on its way.
    pub saving: bool,
    /// Whether the file closes when the save that is on its way is done.
    pub close_after_save: bool,
    /// How the lines end on disk.
    pub eol: Eol,
    /// Whether the file has a byte order mark.
    pub bom: bool,
    /// The language it is highlighted as.
    pub language: &'static str,
    /// The pending look at whether the text still differs.
    pub check: Option<Task<()>>,
    /// The revision the file has on disk now, when it is not the one that was
    /// read and the text has changes that were not saved: the banner above
    /// the editor asks what to do about the two versions.
    pub external: Option<FileRevision>,
    /// The pending write of the draft.
    pub draft_task: Option<Task<()>>,
    /// What a Markdown file shows: the text, the page or both.
    pub mode: super::preview::ViewMode,
    /// The page of a Markdown file, made when it is first shown and kept
    /// (with its scroll position) while the file is open.
    pub preview: Option<Entity<TextViewState>>,
    /// The pending update of the page with the text.
    pub preview_task: Option<Task<()>>,
    /// The page of an SVG file: the text as a picture, made when it is first
    /// shown and again after each pause in the typing.
    pub svg: Option<Arc<Image>>,
    /// Zoom and pan of a picture or of the page of an SVG file.
    pub view: super::viewer::Viewport,
    _change: Option<Subscription>,
}

impl EditorDoc {
    /// A file that was read. `id` is its leaf, which hears of every change of
    /// the text.
    pub fn open(
        machine: MachineId,
        path: String,
        folder: String,
        content: FileContent,
        id: super::super::live::LiveId,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Self {
        let mut doc = Self::new(machine, path, folder, Body::Binary(0));
        match content {
            FileContent::Text { text, revision } => {
                let decoded = decode(&text);
                let state = doc.editor(&decoded.body, window, cx);
                doc._change = Some(
                    cx.subscribe(&state, move |this, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.editor_changed(id, cx);
                        }
                    }),
                );
                doc.saved = fingerprint(&decoded.body);
                doc.revision = Some(revision);
                doc.eol = decoded.eol;
                doc.bom = decoded.bom;
                doc.body = Body::Text(state);
            }
            FileContent::Binary { size } => doc.body = Body::Binary(size),
            FileContent::TooBig { size } => doc.body = Body::TooBig(size),
        }
        doc
    }

    /// A picture that was read. It has no text, so nothing is saved, searched
    /// or kept as a draft: it is the same as a file that is not text, except
    /// that it is drawn. A file above the limit of the viewer says so instead.
    pub fn open_image(
        machine: MachineId,
        path: String,
        folder: String,
        content: ImageContent,
    ) -> Self {
        let body = match content {
            ImageContent::Bytes { bytes, stamp } => match Picture::new(&path, bytes, stamp) {
                Some(picture) => Body::Image(picture),
                None => Body::Binary(0),
            },
            ImageContent::TooBig { size } => Body::TooBig(size),
        };
        Self::new(machine, path, folder, body)
    }

    /// Replaces the picture with what the file holds now.
    pub fn show_picture(&mut self, content: ImageContent) {
        let ImageContent::Bytes { bytes, stamp } = content else {
            return;
        };
        if let Some(picture) = Picture::new(&self.path, bytes, stamp) {
            self.body = Body::Image(picture);
        }
    }

    /// The picture, when the file is one.
    pub fn picture(&self) -> Option<&Picture> {
        match &self.body {
            Body::Image(picture) => Some(picture),
            _ => None,
        }
    }

    fn new(machine: MachineId, path: String, folder: String, body: Body) -> Self {
        Self {
            machine,
            language: language_of(&path),
            path,
            folder,
            body,
            revision: None,
            saved: 0,
            dirty: false,
            conflict: false,
            saving: false,
            close_after_save: false,
            eol: Eol::Lf,
            bom: false,
            check: None,
            external: None,
            draft_task: None,
            mode: super::preview::ViewMode::Edit,
            preview: None,
            preview_task: None,
            svg: None,
            view: super::viewer::Viewport::default(),
            _change: None,
        }
    }

    /// The editor for `text`: line numbers, indent guides, search, tabs of
    /// four columns and long lines that scroll.
    fn editor(
        &self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) -> Entity<EditorState> {
        let language = if crate::settings::flag(cx, "editor_highlight") {
            self.language
        } else {
            "text"
        };
        let (numbers, guides, wrap, tab) = editor_prefs(cx);
        cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(numbers)
                .indent_guides(guides)
                .searchable(true)
                .soft_wrap(wrap)
                .tab_size(tab)
                .default_value(text.to_owned())
        })
    }

    /// Puts the settings on the editor again: they apply to files that are
    /// open as soon as they change.
    pub fn apply_prefs(&self, window: &mut Window, cx: &mut App) {
        let Some(state) = self.state() else { return };
        let language = if crate::settings::flag(cx, "editor_highlight") {
            self.language
        } else {
            "text"
        };
        let (numbers, guides, wrap, tab) = editor_prefs(cx);
        state.update(cx, |state, cx| {
            state.set_tab_size(tab, cx);
            state.set_soft_wrap(wrap, window, cx);
            state.set_line_number(numbers, window, cx);
            state.set_indent_guides(guides, window, cx);
            // Changing the language drops the highlighter: only when it is
            // another.
            if state.language_name().as_ref() != language {
                state.set_highlighter(language, cx);
            }
        });
    }

    /// The name shown on the tab.
    pub fn name(&self) -> &str {
        file_name(&self.path)
    }

    /// The editor, when the file is text.
    pub fn state(&self) -> Option<&Entity<EditorState>> {
        match &self.body {
            Body::Text(state) => Some(state),
            _ => None,
        }
    }

    /// Replaces the placeholder of a file that is not shown with its bytes as
    /// lossy text (see [`Body::Lossy`]).
    pub fn show_lossy(
        &mut self,
        text: &str,
        shown: u64,
        size: u64,
        window: &mut Window,
        cx: &mut Context<Shell>,
    ) {
        let state = self.editor(text, window, cx);
        self.body = Body::Lossy { state, shown, size };
    }

    /// The text as it is in the editor now.
    pub fn text(&self, cx: &App) -> Option<String> {
        self.state().map(|state| state.read(cx).value().to_string())
    }

    /// The bytes a save writes.
    pub fn contents(&self, cx: &App) -> Option<String> {
        self.text(cx).map(|body| encode(&body, self.eol, self.bom))
    }

    /// Looks at whether the text differs from what was saved.
    pub fn refresh_dirty(&mut self, cx: &App) {
        if let Some(text) = self.text(cx) {
            self.dirty = fingerprint(&text) != self.saved;
        }
    }

    /// The file holds the editor's text `body` as it was when it was written;
    /// the revision is the one the file has now.
    pub fn saved_as(&mut self, body: &str, revision: FileRevision, cx: &App) {
        self.saved = fingerprint(body);
        self.revision = Some(revision);
        self.conflict = false;
        self.external = None;
        self.saving = false;
        self.refresh_dirty(cx);
    }

    /// Puts back the text of a draft as changes that were not saved. When the
    /// file has another revision than the one the draft began from, it
    /// changed on disk since: a save asks what to do (`conflict`).
    pub fn restore_draft(&mut self, draft: &Draft, window: &mut Window, cx: &mut Context<Shell>) {
        let Some(state) = self.state() else { return };
        let text = draft.text.clone();
        state.update(cx, |state, cx| state.set_value(text, window, cx));
        self.refresh_dirty(cx);
        self.conflict =
            self.revision.as_ref().map(FileRevision::as_str) != draft.revision.as_deref();
    }

    /// Replaces the text with what was read again: nothing is left to save.
    pub fn reloaded(&mut self, content: FileContent, window: &mut Window, cx: &mut Context<Shell>) {
        let FileContent::Text { text, revision } = content else {
            return;
        };
        let decoded = decode(&text);
        if let Some(state) = self.state() {
            let body = decoded.body.clone();
            // The cursor stays where it was, as far as the new text allows.
            let cursor = state.read(cx).cursor_position();
            state.update(cx, |state, cx| {
                state.set_value(body, window, cx);
                state.set_cursor_position(cursor, window, cx);
            });
        }
        self.saved = fingerprint(&decoded.body);
        self.revision = Some(revision);
        self.eol = decoded.eol;
        self.bom = decoded.bom;
        self.dirty = false;
        self.conflict = false;
        self.external = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_comes_from_the_name_then_the_extension() {
        for (path, language) in [
            ("/p/src/main.rs", "rust"),
            ("a.TSX", "tsx"),
            ("a.test.ts", "typescript"),
            ("C:\\p\\run.py", "python"),
            ("/p/Makefile", "make"),
            ("/p/.bashrc", "bash"),
            ("/p/config.yml", "yaml"),
            ("/p/README.md", "markdown"),
            ("/p/include/a.hpp", "cpp"),
            ("/p/LICENSE", "text"),
            ("/p/archive.tar.xyz", "text"),
            ("/p/.hidden", "text"),
            ("/p.d/file", "text"),
        ] {
            assert_eq!(detect(path), language, "{path}");
        }
    }

    #[test]
    fn pictures_are_known_by_their_extension_and_svg_stays_text() {
        for (path, format) in [
            ("/p/a.png", Some(ImageFormat::Png)),
            ("/p/A.JPG", Some(ImageFormat::Jpeg)),
            ("C:\\p\\a.jpeg", Some(ImageFormat::Jpeg)),
            ("a.webp", Some(ImageFormat::Webp)),
            ("a.tif", Some(ImageFormat::Tiff)),
            ("a.gif", Some(ImageFormat::Gif)),
            ("a.bmp", Some(ImageFormat::Bmp)),
            ("a.ico", Some(ImageFormat::Ico)),
            ("/p/logo.svg", None),
            ("/p/png", None),
            ("/p.png/readme", None),
            ("/p/a.png.txt", None),
        ] {
            assert_eq!(image_format(path), format, "{path}");
        }
    }

    #[test]
    fn a_picture_says_its_size_from_the_header_and_nothing_for_garbage() {
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(dimensions_of(&png), Some((3, 2)));
        assert_eq!(dimensions_of(b"not a picture at all"), None);
        assert_eq!(dimensions_of(b""), None);
    }

    #[test]
    fn a_typed_path_is_taken_from_the_folder_unless_it_is_absolute() {
        assert_eq!(resolve("/p/app", "src/a.rs"), "/p/app/src/a.rs");
        assert_eq!(resolve("/p/app/", "./a.rs"), "/p/app/a.rs");
        assert_eq!(resolve("/p/app", " /etc/hosts "), "/etc/hosts");
        assert_eq!(resolve("C:\\p", "src\\a.rs"), "C:\\p\\src\\a.rs");
        assert_eq!(resolve("C:\\p", "D:\\a.rs"), "D:\\a.rs");
        assert_eq!(resolve("", "a.rs"), "a.rs");
    }

    #[test]
    fn the_folder_of_a_path_is_what_comes_before_its_last_name() {
        assert_eq!(parent_of("/p/app/a.rs"), "/p/app");
        assert_eq!(parent_of("/a.rs"), "/");
        assert_eq!(parent_of("C:\\p\\a.rs"), "C:\\p");
        assert_eq!(parent_of("a.rs"), "");
    }

    #[test]
    fn a_crlf_file_and_its_mark_come_back_as_they_were() {
        let raw = "\u{feff}one\r\ntwo\r\n";
        let decoded = decode(raw);
        assert_eq!(decoded.body, "one\ntwo\n");
        assert_eq!((decoded.eol, decoded.bom), (Eol::Crlf, true));
        assert_eq!(encode(&decoded.body, decoded.eol, decoded.bom), raw);
    }

    #[test]
    fn lines_that_do_not_all_end_alike_are_left_exactly_as_they_are() {
        let raw = "one\r\ntwo\nthree";
        let decoded = decode(raw);
        assert_eq!((decoded.body.as_str(), decoded.eol), (raw, Eol::Lf));
        assert_eq!(encode(&decoded.body, decoded.eol, decoded.bom), raw);
        assert_eq!(decode("no line end").eol, Eol::Lf);
    }

    #[test]
    fn the_fingerprint_tells_the_saved_text_from_another() {
        assert_eq!(fingerprint("a\n"), fingerprint("a\n"));
        assert_ne!(fingerprint("a\n"), fingerprint("a\n "));
    }
}
