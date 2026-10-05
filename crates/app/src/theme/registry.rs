//! The registry of themes: the built-in ones and the ones loaded from files,
//! each under a string id.
//!
//! A [`ThemeId`] is a small copyable handle on an id (the id's text, kept for
//! the life of the process), so that the settings, the palette and the command
//! line can hold themes without caring where they came from. What an id stands
//! for is looked up here: the built-in ids (`leon`, `zavu`) are fixed and
//! reserved, and a user theme can never take one; user themes are replaced as
//! a whole whenever the themes folder is read again.
//!
//! A user theme that fails to load keeps the last version that was good, if
//! there was one, so a broken save never leaves the window unreadable, and is
//! listed as invalid with its first problem. The registry lives in a thread
//! local, like the interface scale: the interface is one thread, and each test
//! has its own.

use super::check::Problem;
use super::{leon, zavu, Theme};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// A string kept for the life of the process, once.
pub fn intern(text: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(known) = pool.get(text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// The ids that belong to the built-in themes: no file may use them.
pub const RESERVED: [&str; 2] = ["leon", "zavu"];

/// Where a theme comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A file of the themes folder.
    User(PathBuf),
}

/// What is known of one theme.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Its id.
    pub id: ThemeId,
    /// The name shown.
    pub name: &'static str,
    /// A few words beside the name.
    pub detail: &'static str,
    /// Where it comes from.
    pub origin: Origin,
    /// The theme: the last good version, for a user theme whose file is
    /// broken now.
    pub theme: Option<Theme>,
    /// Whether the file as it is now loads: `false` keeps `theme` on screen but
    /// the theme cannot be chosen.
    pub valid: bool,
    /// What was found in its file, errors and warnings.
    pub problems: Vec<Problem>,
}

thread_local! {
    static USER: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    /// Ids a saved choice named that no theme answers to, until read.
    static MISSING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A theme's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemeId(pub(super) &'static str);

#[allow(non_upper_case_globals)]
impl ThemeId {
    /// Leon's brand theme with the acid-yellow accent.
    pub const Leon: ThemeId = ThemeId("leon");
    /// Zavu: violet on black, Space Grotesk and Geist Mono.
    pub const Zavu: ThemeId = ThemeId("zavu");

    /// The built-in themes, in the order they are listed.
    pub const ALL: [ThemeId; 2] = [Self::Leon, Self::Zavu];

    /// The theme a fresh install wears.
    pub const DEFAULT: ThemeId = Self::Leon;

    /// The stable id: saved in the settings, typed after `--theme-name`.
    pub fn slug(self) -> &'static str {
        self.0
    }

    /// Whether it is one of the built-in themes.
    pub fn is_builtin(self) -> bool {
        RESERVED.contains(&self.0)
    }

    fn entry<R>(self, read: impl FnOnce(&Entry) -> R) -> Option<R> {
        USER.with(|user| user.borrow().iter().find(|e| e.id == self).map(read))
    }

    /// The name shown to the user.
    pub fn name(self) -> &'static str {
        match self.0 {
            "leon" => "Leon",
            "zavu" => "Zavu",
            _ => self.entry(|entry| entry.name).unwrap_or(self.0),
        }
    }

    /// A few words on what the theme is, shown beside its name. For a theme
    /// that cannot be used, why.
    pub fn detail(self) -> &'static str {
        match self.0 {
            "leon" => "acid yellow accent, Inter, 6 px corners",
            "zavu" => "violet accent, Space Grotesk, square",
            _ => self.entry(|entry| entry.detail).unwrap_or(""),
        }
    }

    /// Whether the theme can be chosen: built-in, or a file that loads.
    pub fn is_usable(self) -> bool {
        self.is_builtin() || self.entry(|entry| entry.valid).unwrap_or(false)
    }

    /// Why the theme cannot be chosen, when it cannot.
    pub fn problem(self) -> Option<String> {
        if self.is_usable() {
            return None;
        }
        Some(match self.entry(|entry| first_error(&entry.problems)) {
            Some(Some(text)) => text,
            _ => format!("There is no theme {:?}.", self.0),
        })
    }

    /// Reads a slug or a name, in any case, with spaces, dots or underscores
    /// for dashes (`LEON`, `leon_theme`), among every theme, user ones
    /// included.
    pub fn parse(text: &str) -> Option<Self> {
        let wanted = normalise(text);
        all()
            .into_iter()
            .find(|id| id.slug() == wanted || normalise(id.name()) == wanted)
    }

    /// The theme after (`forward`) or before this one in the list of themes
    /// that can be chosen, wrapping.
    pub fn step(self, forward: bool) -> Self {
        let list = usable();
        let at = list.iter().position(|id| *id == self).unwrap_or(0);
        let len = list.len();
        list[if forward {
            (at + 1) % len
        } else {
            (at + len - 1) % len
        }]
    }

    /// The theme's data. A user theme's last good version; the default theme
    /// for an id nothing answers to.
    pub fn theme(self) -> Theme {
        match self.0 {
            "leon" => leon::theme(),
            "zavu" => zavu::theme(),
            _ => self
                .entry(|entry| entry.theme)
                .flatten()
                .unwrap_or_else(|| Self::DEFAULT.theme()),
        }
    }

    /// The palette of this theme in `appearance`.
    pub fn palette(self, appearance: super::Appearance) -> super::Palette {
        self.theme().palette(appearance)
    }
}

impl Default for ThemeId {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn normalise(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c == '_' || c == '\u{b7}' || c == ' ' {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn first_error(problems: &[Problem]) -> Option<String> {
    problems
        .iter()
        .find(|problem| problem.is_error())
        .map(Problem::summary)
}

/// Every theme of the registry: the built-in ones, then the user's by name,
/// the ones that cannot be chosen included.
pub fn all() -> Vec<ThemeId> {
    let mut list: Vec<ThemeId> = ThemeId::ALL.to_vec();
    USER.with(|user| {
        let mut entries: Vec<&Entry> = Vec::new();
        let user = user.borrow();
        entries.extend(user.iter());
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        list.extend(entries.iter().map(|entry| entry.id));
    });
    list
}

/// The themes that can be chosen, in the order of [`all`].
pub fn usable() -> Vec<ThemeId> {
    all().into_iter().filter(|id| id.is_usable()).collect()
}

/// What the registry knows of a user theme.
pub fn user_entry(id: ThemeId) -> Option<Entry> {
    id.entry(Entry::clone)
}

/// Every problem of every user theme, in the order of the themes.
pub fn problems() -> Vec<Problem> {
    USER.with(|user| {
        let mut entries = user.borrow().clone();
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        entries
            .into_iter()
            .flat_map(|entry| entry.problems)
            .collect()
    })
}

/// Replaces the user themes with what the folder holds now. A theme whose file
/// is broken keeps the last good version it had.
pub fn replace_user(mut loaded: Vec<Entry>) {
    USER.with(|user| {
        let mut user = user.borrow_mut();
        for entry in &mut loaded {
            if entry.theme.is_none() {
                entry.theme = user
                    .iter()
                    .find(|old| old.id == entry.id)
                    .and_then(|old| old.theme);
            }
        }
        *user = loaded;
    });
}

/// Forgets every user theme (tests).
#[cfg(test)]
pub fn clear_user() {
    USER.with(|user| user.borrow_mut().clear());
    MISSING.with(|missing| missing.borrow_mut().clear());
}

/// Notes that a saved choice named a theme nothing answers to.
pub fn note_missing(id: &str) {
    MISSING.with(|missing| missing.borrow_mut().push(id.to_owned()));
}

/// The ids noted as missing since the last call.
pub fn take_missing() -> Vec<String> {
    MISSING.with(|missing| std::mem::take(&mut *missing.borrow_mut()))
}

/// The id of a theme text names, for the id of a file: lower case words.
pub fn id_of(text: &str) -> ThemeId {
    ThemeId(intern(&normalise(text)))
}
