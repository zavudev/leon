//! Text files in tabs, next to the terminals of the same project.
//!
//! A file is a leaf of a tab like a terminal is: the layout ([`super::panes`]),
//! the workspaces ([`super::workspace`]) and the focus know nothing of the
//! difference, so a file can be split beside a terminal, moved between and
//! closed like any pane. What tells them apart is the side table
//! `Shell::files`, from the leaf's [`LiveId`] (allocated from the counter the
//! terminals use) to its [`EditorDoc`]. Whatever treats a leaf as a terminal
//! asks `Shell::live` first and finds nothing for a file; whatever draws or
//! names a leaf asks the side table first.
//!
//! * [`document`]: the file, and the pure parts (language, line ends, paths).
//! * [`view`]: drawing a file's leaf and its tab and header.
//! * [`files_panel`], [`filetree`] and [`icons_map`]: the column that lists the
//!   files of the project in view (the state and the drawing, the pure tree of
//!   rows, and the icon of each file).
//! * [`preview`]: the rendered page of a Markdown file, beside the editor or
//!   instead of it.
//! * [`actions`]: opening, saving, reloading and closing, and the questions
//!   they ask (`steps.rs` has the flows, `palette.rs` runs their answers).
//!
//! The editor is gpui-component's code editor: the language by file name and
//! extension, line numbers, indent guides, search, tabs of four columns. A
//! file that is not text, or that is above the limit of the engine's reads,
//! shows what it is instead of an editor.

mod actions;
mod document;
mod drafts;
mod files_panel;
mod filetree;
mod guard;
mod icons_map;
mod preview;
mod quick;
mod session;
mod view;

#[cfg(test)]
pub use document::Body;
pub use document::EditorDoc;
#[cfg(test)]
pub use drafts::{write as write_draft, Draft};
pub use files_panel::{next_pane, FileTreeUi};
#[cfg(test)]
pub use filetree::RowKind;
#[allow(unused_imports)]
pub use preview::ViewMode;
pub use quick::{file_items, hit_items, QuickFiles, TextSearch, HIT_ROWS};
pub use session::{Saved as SavedFiles, FILE_NAME as OPEN_FILES_FILE};
