//! The themes folder: reading every theme file into the registry.
//!
//! The folder is `themes/` in the application's data directory, beside
//! `settings.json`. Every `*.toml` file in it is one theme (see
//! `docs/THEMES.md`). [`load_folder`] reads them all, resolves their parents,
//! checks them and replaces the registry's user themes with the result; it is
//! called at start, by "Reload themes" and by the watcher when a file changed.
//!
//! A font a theme asks for must be drawable: bundled with Leon (found in the
//! font files themselves, as `brand` embeds them) or installed on the system
//! (the application tells this module the installed families at start). A
//! family that is neither is a warning, and the parent's font is kept.

use super::file::{entries, parse, resolve_all, Env, Raw};
use super::registry;
use crate::brand;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

thread_local! {
    static INSTALLED: RefCell<Option<HashSet<String>>> = const { RefCell::new(None) };
    static BUNDLED: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// Tells the loader which font families the system has.
pub fn set_installed_fonts(names: impl IntoIterator<Item = String>) {
    INSTALLED.with(|installed| *installed.borrow_mut() = Some(names.into_iter().collect()));
}

/// Whether a family is stored in one of the fonts bundled with the application
/// (its name is in the file as ASCII or UTF-16).
pub fn bundled_font(name: &str) -> bool {
    BUNDLED.with(|cache| {
        if let Some(known) = cache.borrow().get(name) {
            return *known;
        }
        let ascii = name.as_bytes();
        let utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let has = |bytes: &[u8], needle: &[u8]| {
            !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
        };
        let found = brand::fonts()
            .iter()
            .any(|bytes| has(bytes, ascii) || has(bytes, &utf16));
        cache.borrow_mut().insert(name.to_owned(), found);
        found
    })
}

/// Whether a family can be drawn: bundled, or installed.
pub fn has_font(name: &str) -> bool {
    bundled_font(name)
        || INSTALLED.with(|installed| {
            installed
                .borrow()
                .as_ref()
                .is_some_and(|names| names.contains(name))
        })
}

/// What reading the folder found.
#[derive(Debug, Default)]
pub struct Summary {
    /// The ids of the themes that load.
    pub loaded: Vec<String>,
    /// The ids (or file names) of the themes that do not.
    pub invalid: Vec<String>,
}

/// The files of the folder, by name, with their text.
fn read(folder: &Path) -> Vec<Raw> {
    let Ok(listing) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| match std::fs::read_to_string(folder.join(&name)) {
            Ok(text) => parse(&name, &text),
            Err(error) => {
                let mut raw = parse(&name, "");
                raw.problems.push(super::check::Problem::error(
                    &name,
                    "",
                    format!("the file cannot be read: {error}"),
                ));
                raw
            }
        })
        .collect()
}

/// Reads the folder and replaces the registry's user themes with it. A folder
/// that does not exist holds no themes.
pub fn load_folder(folder: &Path, has_font: &dyn Fn(&str) -> bool) -> Summary {
    let loaded = resolve_all(read(folder), &Env { has_font });
    let mut summary = Summary::default();
    for item in &loaded {
        if item.is_valid() {
            summary.loaded.push(item.id.clone());
        } else {
            summary.invalid.push(item.id.clone());
        }
    }
    registry::replace_user(entries(loaded, folder));
    summary
}

/// [`load_folder`] with the fonts this process knows.
pub fn reload(folder: &Path) -> Summary {
    load_folder(folder, &has_font)
}
