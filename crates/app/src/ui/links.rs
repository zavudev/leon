//! The files an agent's messages name, as links.
//!
//! A message that says `out/video.mp4: 52 s` or points at `src/main.rs:12`
//! shows the path as a link when the file is there, and a click opens it the
//! way a web link opens: the markdown view draws and handles any link alike,
//! so what is done here is giving it the address (`chat::paths` rewrites the
//! text) and receiving the click ([`on_link`]).
//!
//! Whether a file exists is looked at off the interface thread, once, and
//! kept ([`Links`]): drawing a message only reads what is known, asks for
//! what is not and is drawn again when the answer is there. What is opened
//! in Leon (text and pictures) and what goes to the system (everything else,
//! and folders) is told when the file is looked at.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::{App, AppContext as _, ClickEvent, Context, MouseButton, SharedString, Window};
use leon_core::MachineId;

use super::shell::Shell;
use crate::chat::paths;
use crate::engine::StatusKind;

/// How many files are remembered, and how many rewritten messages.
const KNOWN: usize = 4000;
const MEMO: usize = 400;

/// Where a file is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opens {
    /// In Leon: text in the editor, a picture in the viewer.
    Here,
    /// By the system: the program the person has for it, or the file manager
    /// for a folder.
    System,
}

/// What looking at `path` found: `None` when there is nothing there.
pub fn inspect(path: &Path) -> Option<Opens> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        return Some(Opens::System);
    }
    if super::editor::image_format(&path.to_string_lossy()).is_some() {
        return Some(Opens::Here);
    }
    // Text if the start of it has no NUL and is UTF-8 (a character may be cut
    // at the end of what was read).
    let mut head = [0u8; 8192];
    let read = std::fs::File::open(path)
        .and_then(|mut file| std::io::Read::read(&mut file, &mut head))
        .ok()?;
    let head = &head[..read];
    let text = !head.contains(&0)
        && match std::str::from_utf8(head) {
            Ok(_) => true,
            Err(error) => error.error_len().is_none() && head.len() - error.valid_up_to() < 4,
        };
    Some(if text { Opens::Here } else { Opens::System })
}

#[derive(Default)]
struct State {
    /// What was found of each path asked about.
    known: HashMap<PathBuf, Option<Opens>>,
    /// The paths being looked at.
    looking: HashSet<PathBuf>,
    /// Counts what was learned: a rewritten message is good for one.
    generation: u64,
    /// The messages already rewritten, by their text and folder.
    memo: HashMap<(u64, String), (u64, String)>,
}

/// What is known of the files messages name.
#[derive(Clone, Default)]
pub struct Links(Rc<RefCell<State>>);

impl Links {
    /// `text` with the files it names that exist as links. A file that is not
    /// known yet is looked at in the background, and the window is drawn
    /// again once it is. `cwd` is the folder a relative path starts from.
    pub fn linked(
        &self,
        text: &str,
        cwd: Option<&str>,
        home: Option<&Path>,
        cx: &mut Context<Shell>,
    ) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let key = (hasher.finish(), cwd.unwrap_or_default().to_owned());
        {
            let state = self.0.borrow();
            if let Some((generation, out)) = state.memo.get(&key) {
                if *generation == state.generation {
                    return out.clone();
                }
            }
        }
        let named = paths::find(text, cwd.map(Path::new), home);
        let wanted: Vec<PathBuf> = {
            let mut state = self.0.borrow_mut();
            let mut wanted: Vec<PathBuf> = Vec::new();
            for place in &named {
                if !state.known.contains_key(&place.path)
                    && !state.looking.contains(&place.path)
                    && !wanted.contains(&place.path)
                {
                    wanted.push(place.path.clone());
                }
            }
            state.looking.extend(wanted.iter().cloned());
            wanted
        };
        if !wanted.is_empty() {
            self.look(wanted, cx);
        }
        let mut state = self.0.borrow_mut();
        let out = paths::rewrite(text, &named, |path| {
            state.known.get(path).is_some_and(Option::is_some)
        });
        if state.memo.len() >= MEMO {
            state.memo.clear();
        }
        let generation = state.generation;
        state.memo.insert(key, (generation, out.clone()));
        out
    }

    /// Looks at `wanted` off the interface thread and remembers what is
    /// there.
    fn look(&self, wanted: Vec<PathBuf>, cx: &mut Context<Shell>) {
        let links = self.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    wanted
                        .into_iter()
                        .map(|path| {
                            let found = inspect(&path);
                            (path, found)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |_, cx| {
                let mut state = links.0.borrow_mut();
                if state.known.len() + found.len() > KNOWN {
                    state.known.clear();
                }
                for (path, found) in found {
                    state.looking.remove(&path);
                    state.known.insert(path, found);
                }
                state.generation += 1;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What is known of `path`, as far as the window has looked.
    #[cfg(test)]
    #[cfg_attr(not(leon_posix_tests), allow(dead_code))]
    pub fn known(&self, path: &Path) -> Option<Option<Opens>> {
        self.0.borrow().known.get(path).copied()
    }
}

/// Whether a click is what opens a link: the primary or the middle button, or
/// the keyboard, as the markdown view does for a web link.
fn opens(event: &ClickEvent) -> bool {
    match event {
        ClickEvent::Mouse(click) => {
            matches!(click.up.button, MouseButton::Left | MouseButton::Middle)
        }
        ClickEvent::Keyboard(_) => true,
        ClickEvent::Touch(click) => !click.long_press,
    }
}

/// What the markdown view does with a click on a link of a message: a file
/// is opened (see [`Shell::open_link`]), a web address as the view does it.
#[allow(clippy::type_complexity)]
pub(super) fn on_link(
    cx: &Context<Shell>,
    local: bool,
) -> impl Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync + 'static {
    let shell = cx.weak_entity();
    move |url, event, window, cx| {
        if !opens(event) {
            return;
        }
        // A file address is this computer's: a conversation on another
        // machine does not open it.
        if !local && url.starts_with("file:") {
            return;
        }
        let url = url.to_string();
        shell
            .update(cx, |shell, cx| shell.open_link(&url, window, cx))
            .ok();
    }
}

impl Shell {
    /// Opens what a link of a message points at: a web address in the
    /// browser; a file in Leon when it is text or a picture, else with the
    /// program the system has for it; a folder in the file manager.
    pub(super) fn open_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line, _)) = paths::parse_url(url) else {
            (self.options.open_url)(cx, url);
            return;
        };
        self.open_path(&path, line, window, cx);
    }

    /// Opens a file of this computer: in Leon when it is text or a picture,
    /// else with the program the system has for it; a folder in the file
    /// manager. What is not there is said, and opens nothing.
    pub(super) fn open_path(
        &mut self,
        path: &Path,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match inspect(path) {
            None => self.engine.report(
                StatusKind::Info,
                format!("{} is not there any more.", path.display()),
            ),
            Some(Opens::Here) => self.open_file(
                MachineId::local(),
                path.to_string_lossy().into_owned(),
                line,
                window,
                cx,
            ),
            Some(Opens::System) => {
                let url = paths::url_of(path, None, None);
                (self.options.open_url)(cx, &url);
            }
        }
    }
}
