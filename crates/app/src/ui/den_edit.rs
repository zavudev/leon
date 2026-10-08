//! Changing the Den: the editor's tools, the dens to choose from, and the
//! files they are kept in.
//!
//! The room is the Den view's ([`leon_den::DenView`]): its editor
//! ([`leon_den::editor`]) decides what a click, a drag or a key does to the
//! layout, and the lions go on living in the room while it changes. This
//! module is what the window adds around it: a bar of buttons over the room,
//! a strip of things to pick under it (the catalogue by category, the
//! floors, and the dens to choose from, each drawn from its own art), the
//! keys of the editor, and where a den is kept.
//!
//! # Whose den it is
//!
//! A built-in den is never changed. The first change to one makes a den of
//! the user's from it ("My office", in `dens/my-office.json` beside the
//! settings, see [`super::den_store`]) and every change after is written to
//! that file at once, so choosing another den loses nothing: the one that
//! was left is still in the list. The den in use is the setting `den`.
//!
//! # Keys while editing
//!
//! Escape puts down what is in hand, then lets go of the selection, then
//! leaves the editor. `R` turns, Delete removes, the arrows move the
//! selected piece or else the pointer's tile, Enter and Space click that
//! tile, `.` and `,` take the next and the previous thing of the strip in
//! hand, Tab changes the strip, Shift with an arrow moves a wall, `F` and
//! `W` change the floor and the walls, and the secondary key with `Z` undoes
//! (with Shift: redoes).

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{
    div, img, AnyElement, ClickEvent, Context, Div, Keystroke, ObjectFit, RenderImage,
    SharedString, Stateful, Window,
};
use leon_den::catalogue::{Category, CATALOGUE};
use leon_den::editor::{Brush, Edge, Editor};
use leon_den::layout::Why;
use leon_den::prefabs::{prefab, OFFICE};
use leon_den::world::Tile;
use leon_den::DenLayout;

use super::den_store::{self, DenChoice};
use super::shell::Shell;
use super::widgets::mono;
use crate::engine::StatusKind;
use crate::schema::{self, Value};
use crate::settings;
use crate::theme::{metrics, px, Palette};

/// A picture of the strip: the image and its size in device pixels.
pub type Thumb = (Arc<RenderImage>, i32, i32);

/// The den in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveDen {
    /// A built-in den's id, or the name of the user's file.
    pub id: String,
    /// What it is called.
    pub name: String,
    /// The built-in den it was made from, for a den of the user's.
    pub based_on: Option<String>,
    /// Whether it is the user's: a file that changes are written to.
    pub user: bool,
}

/// What the strip under the room shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// The pieces of a category of the catalogue.
    Pieces(Category),
    /// The floors, as carpets to lay, and the styles of the room.
    Floors,
    /// The dens to choose from.
    Dens,
}

impl Default for Tab {
    fn default() -> Self {
        Tab::Pieces(Category::ALL[0])
    }
}

impl Tab {
    /// Every tab, in the order of the strip.
    pub fn all() -> Vec<Tab> {
        let mut all: Vec<Tab> = Category::ALL.into_iter().map(Tab::Pieces).collect();
        all.extend([Tab::Floors, Tab::Dens]);
        all
    }

    /// Its label.
    pub fn name(self) -> &'static str {
        match self {
            Tab::Pieces(category) => category.name(),
            Tab::Floors => "Floors",
            Tab::Dens => "Dens",
        }
    }

    /// The pieces of the catalogue it shows.
    pub fn pieces(self) -> Vec<&'static str> {
        match self {
            Tab::Pieces(category) => CATALOGUE
                .iter()
                .filter(|entry| entry.category == category)
                .map(|entry| entry.id)
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// What a button of the editor, or its key, does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Opens the editor, or leaves it.
    Edit,
    /// Takes the last change back.
    Undo,
    /// Makes it again.
    Redo,
    /// Turns what is in hand, or the selected piece.
    Rotate,
    /// Removes the selected piece.
    Delete,
    /// The next floor (the previous one with `true`).
    Floor(bool),
    /// The next walls.
    Wall(bool),
    /// Moves a wall by a tile.
    Resize(Edge, i32),
    /// Goes back to the built-in den this one was made from.
    Reset,
    /// Shows another strip.
    Tab(Tab),
    /// Takes a piece of the catalogue in hand.
    Piece(&'static str),
    /// Takes a carpet in hand, or the eraser of carpets.
    Carpet(Option<&'static str>),
    /// Chooses a den.
    Den(String),
}

/// The step from `now` to the next of `count`, or the previous one, around
/// the ends; from nothing, the first or the last.
pub fn step(now: Option<usize>, count: usize, back: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (now, back) {
        (None, false) => 0,
        (None, true) => count - 1,
        (Some(now), false) => (now + 1) % count,
        (Some(now), true) => (now + count - 1) % count,
    })
}

/// How many device pixels a pixel of art is drawn with in a box of `room`
/// logical pixels: whole, at least one, at most two pixels of the interface.
pub fn thumb_unit(art: i32, room: f32, scale: f32) -> i32 {
    let most = (2. * scale).floor().max(1.) as i32;
    let fits = (room * scale / art.max(1) as f32).floor() as i32;
    fits.clamp(1, most)
}

impl Shell {
    /// The folder of the user's dens; none while the settings live in memory
    /// only.
    pub(super) fn dens_folder(&self, cx: &gpui_kit::App) -> Option<PathBuf> {
        let _ = self;
        settings::sibling(den_store::FOLDER, cx)
    }

    /// Every den there is to choose, with the one in use.
    pub(super) fn dens(&self, cx: &gpui_kit::App) -> (Vec<DenChoice>, String) {
        let current = match &self.den.active {
            Some(active) => active.id.clone(),
            None => den_store::resolve(None, &settings::text(cx, "den")).0,
        };
        (den_store::list(self.dens_folder(cx).as_deref()), current)
    }

    fn set_den_setting(&mut self, id: &str, cx: &mut Context<Self>) {
        if settings::text(cx, "den") == id {
            return;
        }
        if let Some(def) = schema::find("den") {
            settings::set_value(cx, def, Value::Text(id.to_owned()));
        }
    }

    /// Puts the den of the setting in the view: when the Den is opened. What
    /// its file got wrong is said in the status line.
    pub(super) fn den_load(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.den.view.clone() else {
            return;
        };
        let folder = self.dens_folder(cx);
        let wanted = settings::text(cx, "den");
        // A den of the user's that could not be written is kept in memory.
        if folder.is_none()
            && self
                .den
                .active
                .as_ref()
                .is_some_and(|active| active.user && active.id == wanted)
        {
            return;
        }
        let (id, loaded) = den_store::resolve(folder.as_deref(), &wanted);
        self.den_show(&id, &loaded.layout, cx);
        view.update(cx, |den, cx| den.set_layout(&loaded.layout, cx));
        if let Some(first) = loaded.notes.first() {
            let more = match loaded.notes.len() - 1 {
                0 => String::new(),
                more => format!(" (and {more} more)"),
            };
            self.engine
                .report(StatusKind::Error, format!("Den: {first}{more}"));
        }
    }

    fn den_show(&mut self, id: &str, layout: &DenLayout, _: &mut Context<Self>) {
        let user = !den_store::is_prefab(id);
        self.den.active = Some(ActiveDen {
            id: id.to_owned(),
            name: layout.name.clone(),
            based_on: if user {
                layout.based_on.clone()
            } else {
                Some(id.to_owned())
            },
            user,
        });
    }

    /// Keeps the room as the editor left it: the user's den is written to
    /// its file, and a built-in den that was changed becomes a den of the
    /// user's first.
    pub(super) fn den_keep(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.den.view.clone() else {
            return;
        };
        let mut layout = view.read(cx).layout();
        let folder = self.dens_folder(cx);
        let Some(mut active) = self.den.active.clone() else {
            return;
        };
        if !active.user {
            // Back to the den as it is built in: nothing to keep.
            if prefab(&active.id).is_some_and(|prefab| prefab.layout == layout) {
                return;
            }
            let name = den_store::own_name(&active.name);
            let Some(id) = den_store::free_id(folder.as_deref(), &name) else {
                return;
            };
            active = ActiveDen {
                based_on: Some(active.id.clone()),
                id,
                name,
                user: true,
            };
            self.den.active = Some(active.clone());
            self.set_den_setting(&active.id, cx);
            self.engine.report(
                StatusKind::Info,
                match &folder {
                    Some(folder) => format!(
                        "{} is yours now: {}",
                        active.name,
                        folder.join(format!("{}.json", active.id)).display()
                    ),
                    None => format!(
                        "{} is yours until Leon closes: the settings are kept in memory.",
                        active.name
                    ),
                },
            );
        }
        layout.name = active.name.clone();
        layout.based_on = active.based_on.clone();
        let Some(folder) = folder else {
            return;
        };
        if let Err(error) = den_store::write(&folder, &active.id, &layout) {
            self.engine.report(
                StatusKind::Error,
                format!("Could not write {}.json: {error}.", active.id),
            );
        }
        cx.notify();
    }

    /// Chooses a den: it takes the place of the room. The den that was in
    /// use stays in the list, with every change made to it.
    pub(super) fn den_choose(&mut self, id: &str, cx: &mut Context<Self>) {
        let folder = self.dens_folder(cx);
        let (id, loaded) = den_store::resolve(folder.as_deref(), id);
        self.den_show(&id, &loaded.layout, cx);
        self.set_den_setting(&id, cx);
        if let Some(view) = self.den.view.clone() {
            view.update(cx, |den, cx| den.set_layout(&loaded.layout, cx));
        }
        match loaded.notes.first() {
            Some(note) => self
                .engine
                .report(StatusKind::Error, format!("Den: {note}")),
            None => self
                .engine
                .report(StatusKind::Info, format!("Den: {}.", loaded.layout.name)),
        }
        cx.notify();
    }

    /// The room as it is now, whether the Den was opened or not.
    fn den_layout_now(&self, cx: &gpui_kit::App) -> DenLayout {
        match &self.den.view {
            Some(view) => view.read(cx).layout(),
            None => {
                den_store::resolve(self.dens_folder(cx).as_deref(), &settings::text(cx, "den"))
                    .1
                    .layout
            }
        }
    }

    fn den_active_now(&self, cx: &gpui_kit::App) -> ActiveDen {
        if let Some(active) = &self.den.active {
            return active.clone();
        }
        let (id, loaded) =
            den_store::resolve(self.dens_folder(cx).as_deref(), &settings::text(cx, "den"));
        let user = !den_store::is_prefab(&id);
        ActiveDen {
            based_on: if user {
                loaded.layout.based_on.clone()
            } else {
                Some(id.clone())
            },
            name: loaded.layout.name,
            id,
            user,
        }
    }

    /// Saves the room as a new den of the user's with this name, and uses
    /// it. From a built-in den this is how it is duplicated.
    pub(super) fn den_save_as(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        let Some(folder) = self.dens_folder(cx) else {
            self.engine.report(
                StatusKind::Info,
                "There is no dens folder: the settings are kept in memory.",
            );
            return;
        };
        let Some(id) = den_store::free_id(Some(&folder), name) else {
            self.engine.report(
                StatusKind::Error,
                "Use a name with letters or digits in it.",
            );
            return;
        };
        let before = self.den_active_now(cx);
        let mut layout = self.den_layout_now(cx);
        layout.name = name.to_owned();
        layout.based_on = before.based_on.clone();
        match den_store::write(&folder, &id, &layout) {
            Ok(path) => {
                self.den.active = Some(ActiveDen {
                    id: id.clone(),
                    name: name.to_owned(),
                    based_on: before.based_on,
                    user: true,
                });
                self.set_den_setting(&id, cx);
                self.engine.report(
                    StatusKind::Info,
                    format!("Saved the den as {name}: {}", path.display()),
                );
            }
            Err(error) => self.engine.report(
                StatusKind::Error,
                format!("Could not write {id}.json: {error}."),
            ),
        }
        cx.notify();
    }

    /// Gives the user's den in use another name, and its file with it.
    pub(super) fn den_rename(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        let active = self.den_active_now(cx);
        let Some(folder) = self.dens_folder(cx).filter(|_| active.user) else {
            self.engine.report(
                StatusKind::Error,
                format!("{} is built in: save it under a name first.", active.name),
            );
            return;
        };
        let id = if den_store::id_for(name) == active.id {
            Some(active.id.clone())
        } else {
            den_store::free_id(Some(&folder), name)
        };
        let Some(id) = id else {
            self.engine.report(
                StatusKind::Error,
                "Use a name with letters or digits in it.",
            );
            return;
        };
        let mut layout = self.den_layout_now(cx);
        layout.name = name.to_owned();
        layout.based_on = active.based_on.clone();
        if let Err(error) = den_store::write(&folder, &id, &layout) {
            self.engine.report(
                StatusKind::Error,
                format!("Could not write {id}.json: {error}."),
            );
            return;
        }
        if id != active.id {
            if let Err(error) = den_store::remove(&folder, &active.id) {
                self.engine.report(
                    StatusKind::Error,
                    format!("Could not remove {}.json: {error}.", active.id),
                );
            }
        }
        self.den.active = Some(ActiveDen {
            id: id.clone(),
            name: name.to_owned(),
            ..active.clone()
        });
        self.set_den_setting(&id, cx);
        self.engine.report(
            StatusKind::Info,
            format!("{} is called {name} now.", active.name),
        );
        cx.notify();
    }

    /// Deletes the user's den in use: its file is removed, and the built-in
    /// den it was made from takes its place.
    pub(super) fn den_delete(&mut self, cx: &mut Context<Self>) {
        let active = self.den_active_now(cx);
        let Some(folder) = self.dens_folder(cx).filter(|_| active.user) else {
            self.engine.report(
                StatusKind::Error,
                format!("{} is built in: it cannot be deleted.", active.name),
            );
            return;
        };
        if let Err(error) = den_store::remove(&folder, &active.id) {
            self.engine.report(
                StatusKind::Error,
                format!("Could not remove {}.json: {error}.", active.id),
            );
            return;
        }
        let next = active.based_on.clone().unwrap_or_else(|| OFFICE.to_owned());
        self.den_choose(&next, cx);
        self.engine.report(
            StatusKind::Info,
            format!("Deleted {}. The den is {} again.", active.name, {
                self.den_active_now(cx).name
            }),
        );
    }

    /// Shows the dens folder in the file manager: where a den is copied
    /// from, to share it, and where one from elsewhere is put.
    pub(super) fn open_dens_folder(&mut self, cx: &mut Context<Self>) {
        let Some(folder) = self.dens_folder(cx) else {
            self.engine.report(
                StatusKind::Info,
                "There is no dens folder: the settings are kept in memory.",
            );
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&folder) {
            self.engine.report(
                StatusKind::Error,
                format!("Could not create {}: {error}.", folder.display()),
            );
            return;
        }
        let reveal = self.options.reveal.clone();
        reveal(cx, &folder);
        self.engine.report(
            StatusKind::Info,
            format!("Dens folder: {}", folder.display()),
        );
    }

    /// Whether the room is being edited.
    pub(super) fn den_editing(&self, cx: &gpui_kit::App) -> bool {
        self.den
            .view
            .as_ref()
            .is_some_and(|view| view.read(cx).is_editing())
    }

    /// Opens the editor of the Den, opening the Den first if it is closed,
    /// or leaves the editor when it is open.
    pub(super) fn toggle_den_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.den_open() {
            self.toggle_den(window, cx);
        }
        self.den_tool(Tool::Edit, cx);
    }

    /// Does what a button of the editor does.
    pub(super) fn den_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        let Some(view) = self.den.view.clone() else {
            return;
        };
        let editing = view.read(cx).is_editing();
        let edit = |change: &mut dyn FnMut(&mut Editor) -> Result<(), Why>,
                    cx: &mut Context<Self>| {
            view.update(cx, |den, cx| den.edit(|editor| change(editor), cx))
        };
        match tool {
            Tool::Edit => {
                view.update(cx, |den, cx| {
                    if editing {
                        den.stop_editing(cx);
                    } else {
                        den.select(None, cx);
                        den.start_editing(cx);
                    }
                });
                if !editing && self.den.tab == Tab::Dens {
                    self.den.tab = Tab::default();
                }
            }
            Tool::Tab(tab) => {
                self.den.tab = tab;
                if !editing {
                    view.update(cx, |den, cx| den.start_editing(cx));
                }
            }
            Tool::Den(id) => self.den_choose(&id, cx),
            _ if !editing => {}
            Tool::Undo => {
                edit(
                    &mut |editor| {
                        editor.undo();
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Redo => {
                edit(
                    &mut |editor| {
                        editor.redo();
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Rotate => {
                edit(&mut |editor| editor.rotate(), cx);
            }
            Tool::Delete => {
                edit(
                    &mut |editor| {
                        editor.delete();
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Floor(back) => {
                edit(
                    &mut |editor| {
                        editor.cycle_floor(back);
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Wall(back) => {
                edit(
                    &mut |editor| {
                        editor.cycle_wall(back);
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Resize(edge, by) => {
                edit(&mut |editor| editor.resize(edge, by), cx);
            }
            Tool::Reset => {
                let active = self.den_active_now(cx);
                let Some(prefab) = active.based_on.as_deref().and_then(prefab) else {
                    self.engine.report(
                        StatusKind::Error,
                        format!("{} was not made from a built-in den.", active.name),
                    );
                    return;
                };
                let mut layout = prefab.layout;
                if active.user {
                    layout.name = active.name.clone();
                }
                edit(
                    &mut |editor| {
                        editor.replace(layout.clone());
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Piece(id) => {
                edit(
                    &mut |editor| {
                        // The same piece again puts it down.
                        if matches!(editor.brush(), Brush::Piece { id: held, .. } if *held == id) {
                            editor.drop_brush();
                        } else {
                            editor.pick(id);
                        }
                        Ok(())
                    },
                    cx,
                );
            }
            Tool::Carpet(style) => {
                edit(
                    &mut |editor| {
                        let held = match (editor.brush(), style) {
                            (Brush::Carpet(held), Some(style)) => *held == style,
                            (Brush::BareFloor, None) => true,
                            _ => false,
                        };
                        if held {
                            editor.drop_brush();
                        } else {
                            editor.pick_carpet(style);
                        }
                        Ok(())
                    },
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// What a key does while the room is edited. `true` when taken.
    pub(super) fn den_edit_key(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(view) = self.den.view.clone() else {
            return false;
        };
        let m = &stroke.modifiers;
        let key = stroke.key.as_str();
        if (m.platform || m.control) && !m.alt && !m.function && key == "z" {
            self.den_tool(if m.shift { Tool::Redo } else { Tool::Undo }, cx);
            return true;
        }
        if m.platform || m.control || m.alt || m.function {
            return false;
        }
        let (brush, selected, pointer, cols, rows) = view
            .read(cx)
            .editor(|editor| {
                (
                    editor.brush().clone(),
                    editor.selected(),
                    editor.pointer(),
                    editor.layout().cols,
                    editor.layout().rows,
                )
            })
            .unwrap_or((Brush::Hand, None, None, 0, 0));
        let arrow = match key {
            "left" => Some((-1, 0)),
            "right" => Some((1, 0)),
            "up" => Some((0, -1)),
            "down" => Some((0, 1)),
            _ => None,
        };
        let edit = |change: &mut dyn FnMut(&mut Editor) -> Result<(), Why>,
                    cx: &mut Context<Self>| {
            view.update(cx, |den, cx| den.edit(|editor| change(editor), cx))
        };
        match (key, m.shift) {
            ("escape", false) => {
                if brush != Brush::Hand {
                    edit(
                        &mut |editor| {
                            editor.drop_brush();
                            Ok(())
                        },
                        cx,
                    );
                } else if selected.is_some() {
                    edit(
                        &mut |editor| {
                            editor.deselect();
                            Ok(())
                        },
                        cx,
                    );
                } else {
                    self.den_tool(Tool::Edit, cx);
                }
            }
            ("e", false) => self.den_tool(Tool::Edit, cx),
            ("r", false) => self.den_tool(Tool::Rotate, cx),
            ("delete" | "backspace", false) => self.den_tool(Tool::Delete, cx),
            ("f", back) => self.den_tool(Tool::Floor(back), cx),
            ("w", back) => self.den_tool(Tool::Wall(back), cx),
            ("tab", back) => {
                let tabs = Tab::all();
                let now = tabs.iter().position(|tab| *tab == self.den.tab);
                if let Some(next) = step(now, tabs.len(), back) {
                    self.den_tool(Tool::Tab(tabs[next]), cx);
                }
            }
            ("." | ",", false) => self.den_step_strip(key == ",", &brush, cx),
            (_, true) if arrow.is_some() => {
                let (dx, dy) = arrow.unwrap_or_default();
                let (edge, by) = if dx != 0 {
                    (Edge::Right, dx)
                } else {
                    (Edge::Bottom, dy)
                };
                self.den_tool(Tool::Resize(edge, by), cx);
            }
            (_, false) if arrow.is_some() => {
                let (dx, dy) = arrow.unwrap_or_default();
                if brush == Brush::Hand && selected.is_some() {
                    edit(&mut |editor| editor.nudge(dx, dy), cx);
                } else {
                    // The keyboard's pointer: from the middle of the room.
                    let from = pointer.unwrap_or(Tile::new(cols / 2, rows / 2));
                    let to = Tile::new(
                        (from.x + dx).clamp(0, (cols - 1).max(0)),
                        (from.y + dy).clamp(0, (rows - 1).max(0)),
                    );
                    edit(
                        &mut |editor| {
                            editor.point(Some(to));
                            Ok(())
                        },
                        cx,
                    );
                }
            }
            ("enter" | "space", false) => {
                let at = pointer.unwrap_or(Tile::new(cols / 2, rows / 2));
                edit(
                    &mut |editor| {
                        let pressed = editor.press(at);
                        editor.release();
                        pressed
                    },
                    cx,
                );
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Takes the next thing of the strip in hand, or the previous one.
    fn den_step_strip(&mut self, back: bool, brush: &Brush, cx: &mut Context<Self>) {
        match self.den.tab {
            Tab::Pieces(_) => {
                let pieces = self.den.tab.pieces();
                let now = match brush {
                    Brush::Piece { id, .. } => pieces.iter().position(|piece| piece == id),
                    _ => None,
                };
                if let Some(next) = step(now, pieces.len(), back) {
                    // Not through `Tool::Piece`: the same piece is not put down.
                    if let Some(view) = self.den.view.clone() {
                        view.update(cx, |den, cx| {
                            den.edit(
                                |editor| {
                                    editor.pick(pieces[next]);
                                    Ok(())
                                },
                                cx,
                            )
                        });
                    }
                }
            }
            Tab::Floors => {
                let floors = leon_den::assets::FLOORS;
                let now = match brush {
                    Brush::Carpet(style) => floors.iter().position(|floor| floor.id == *style),
                    _ => None,
                };
                if let Some(next) = step(now, floors.len(), back) {
                    if let Some(view) = self.den.view.clone() {
                        view.update(cx, |den, cx| {
                            den.edit(
                                |editor| {
                                    editor.pick_carpet(Some(floors[next].id));
                                    Ok(())
                                },
                                cx,
                            )
                        });
                    }
                }
            }
            Tab::Dens => {
                let (dens, current) = self.dens(cx);
                let now = dens.iter().position(|den| den.id == current);
                if let Some(next) = step(now, dens.len(), back) {
                    let id = dens[next].id.clone();
                    self.den_choose(&id, cx);
                }
            }
        }
    }

    /// A picture of the Den's own art, as large as `unit` device pixels a
    /// pixel of art, made once.
    fn den_thumb(
        &self,
        key: String,
        unit: i32,
        make: impl FnOnce() -> Option<leon_den::bitmap::Bitmap>,
    ) -> Option<Thumb> {
        let key = format!("{key}@{unit}");
        if let Some(found) = self.den.thumbs.borrow().get(&key) {
            return Some(found.clone());
        }
        let picture = make()?.scaled(unit);
        let (w, h) = (picture.w, picture.h);
        let made = (Arc::new(leon_den::view::render_image(picture)), w, h);
        self.den.thumbs.borrow_mut().insert(key, made.clone());
        Some(made)
    }

    /// The bar over the room: the den's name and what can be done to it.
    pub(super) fn render_den_bar(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let editing = self.den_editing(cx);
        let active = self.den_active_now(cx);
        let view = self.den.view.as_ref().map(|view| view.read(cx));
        let (undo, redo, selected, holding) = view
            .and_then(|view| {
                view.editor(|editor| {
                    (
                        editor.can_undo(),
                        editor.can_redo(),
                        editor.selected().is_some(),
                        *editor.brush() != Brush::Hand,
                    )
                })
            })
            .unwrap_or_default();
        let mut tools: Vec<(&'static str, &'static str, Tool, bool)> = Vec::new();
        if editing {
            tools.extend([
                ("den-undo", "Undo", Tool::Undo, undo),
                ("den-redo", "Redo", Tool::Redo, redo),
                ("den-rotate", "Turn", Tool::Rotate, selected || holding),
                ("den-delete", "Remove", Tool::Delete, selected),
                ("den-floor", "Floor", Tool::Floor(false), true),
                ("den-wall", "Walls", Tool::Wall(false), true),
                ("den-wider", "Wider", Tool::Resize(Edge::Right, 1), true),
                (
                    "den-narrower",
                    "Narrower",
                    Tool::Resize(Edge::Right, -1),
                    true,
                ),
                ("den-deeper", "Deeper", Tool::Resize(Edge::Bottom, 1), true),
                (
                    "den-shallower",
                    "Shallower",
                    Tool::Resize(Edge::Bottom, -1),
                    true,
                ),
                (
                    "den-reset",
                    "Reset to prefab",
                    Tool::Reset,
                    active.based_on.is_some(),
                ),
                ("den-done", "Done", Tool::Edit, true),
            ]);
        } else {
            tools.extend([
                ("den-choose", "Choose a den", Tool::Tab(Tab::Dens), true),
                ("den-edit", "Edit the Den", Tool::Edit, true),
            ]);
        }
        let kind = if active.user { "YOURS" } else { "BUILT IN" };
        div()
            .debug_selector(|| "den-bar".into())
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .py(px(6.))
            .border_b_1()
            .border_color(colours.border)
            .child(
                mono(active.name.clone())
                    .debug_selector(|| "den-name".into())
                    .text_color(colours.text),
            )
            .child(mono(kind).text_color(colours.text_faint))
            .child(div().flex_1())
            .children(tools.into_iter().map(|(id, label, tool, enabled)| {
                let lit = id == "den-done";
                tool_button(id.into(), label, enabled, lit, colours).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        if enabled {
                            this.den_tool(tool.clone(), cx);
                        }
                    },
                ))
            }))
    }

    /// The strip under the room while it is edited: tabs, and the things of
    /// the tab to pick.
    pub(super) fn render_den_strip(&self, colours: &Palette, cx: &mut Context<Self>) -> Div {
        let scale = self
            .den
            .view
            .as_ref()
            .map_or(1., |view| view.read(cx).scale())
            .max(0.5);
        let brush = self
            .den
            .view
            .as_ref()
            .and_then(|view| view.read(cx).editor(|editor| editor.brush().clone()))
            .unwrap_or(Brush::Hand);
        let tab = self.den.tab;
        let tabs =
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(Tab::all().into_iter().map(|one| {
                    let id: SharedString = format!("den-tab-{}", one.name().to_lowercase()).into();
                    tool_button(id, one.name(), true, one == tab, colours).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| this.den_tool(Tool::Tab(one), cx),
                    ))
                }));
        let card = |id: SharedString,
                    picture: Option<AnyElement>,
                    label: String,
                    chosen: bool,
                    tool: Tool,
                    cx: &mut Context<Self>| {
            let selector = id.clone();
            div()
                .id(id)
                .debug_selector(move || selector.to_string())
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .justify_end()
                .gap(px(4.))
                .p(px(6.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(if chosen {
                    colours.signal
                } else {
                    colours.border
                })
                .when(chosen, |card| card.bg(colours.surface_2))
                .cursor_pointer()
                .children(picture)
                .child(mono(label).text_color(if chosen {
                    colours.text
                } else {
                    colours.text_muted
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.den_tool(tool.clone(), cx);
                }))
        };
        let picture = |made: Option<Thumb>| {
            made.map(|(image, w, h)| {
                img(image)
                    .flex_none()
                    .w(gpui_kit::px(w as f32 / scale))
                    .h(gpui_kit::px(h as f32 / scale))
                    .into_any_element()
            })
        };
        let mut cards: Vec<Stateful<Div>> = Vec::new();
        match tab {
            Tab::Pieces(_) => {
                for id in tab.pieces() {
                    let Some(entry) = leon_den::catalogue::find(id) else {
                        continue;
                    };
                    let turn = match &brush {
                        Brush::Piece { id: held, turn } if *held == id => *turn,
                        _ => 0,
                    };
                    let view = entry.view(turn);
                    let art = (view.w.max(view.h + 1)) * 16;
                    let unit = thumb_unit(art, 56., scale);
                    let made = self.den_thumb(format!("piece-{id}-{turn}"), unit, || {
                        leon_den::paint::piece(id, turn)
                    });
                    let held = matches!(&brush, Brush::Piece { id: held, .. } if *held == id);
                    cards.push(card(
                        format!("den-piece-{id}").into(),
                        picture(made),
                        entry.name.to_owned(),
                        held,
                        Tool::Piece(id),
                        cx,
                    ));
                }
            }
            Tab::Floors => {
                let unit = thumb_unit(16, 32., scale);
                for floor in leon_den::assets::FLOORS {
                    let made = self.den_thumb(format!("floor-{}", floor.id), unit, || {
                        leon_den::paint::floor_tile(floor.id)
                    });
                    cards.push(card(
                        format!("den-carpet-{}", floor.id).into(),
                        picture(made),
                        floor.name.to_owned(),
                        brush == Brush::Carpet(floor.id),
                        Tool::Carpet(Some(floor.id)),
                        cx,
                    ));
                }
                cards.push(card(
                    "den-carpet-none".into(),
                    None,
                    "No carpet".to_owned(),
                    brush == Brush::BareFloor,
                    Tool::Carpet(None),
                    cx,
                ));
            }
            Tab::Dens => {
                let (dens, current) = self.dens(cx);
                let folder = self.dens_folder(cx);
                let palette = super::den_view::den_palette(colours);
                for den in dens {
                    // The den in use is drawn as it is now, any other as its
                    // file says.
                    let layout = if den.id == current {
                        self.den_layout_now(cx)
                    } else {
                        den_store::resolve(folder.as_deref(), &den.id).1.layout
                    };
                    let key = format!("den-{}-{:x}", den.id, fingerprint(&layout));
                    let made = self.den_thumb(key, 1, || {
                        Some(leon_den::paint::thumbnail(&layout, &palette))
                    });
                    // A whole room is shown small: by the height of the strip.
                    let shown = made.map(|(image, w, h)| {
                        let height = 72.;
                        img(image)
                            .flex_none()
                            .h(px(height))
                            .w(px(height * w as f32 / h.max(1) as f32))
                            .object_fit(ObjectFit::Contain)
                            .into_any_element()
                    });
                    cards.push(card(
                        format!("den-pick-{}", den.id).into(),
                        shown,
                        den.name.clone(),
                        den.id == current,
                        Tool::Den(den.id.clone()),
                        cx,
                    ));
                }
            }
        }
        div()
            .debug_selector(|| "den-strip".into())
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(colours.border)
            .child(tabs)
            .child(
                div()
                    .id("den-strip-things")
                    .flex()
                    .items_end()
                    .gap(px(6.))
                    .overflow_x_scroll()
                    .children(cards),
            )
    }
}

/// A number that changes when a layout does: the key of its picture.
fn fingerprint(layout: &DenLayout) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    layout.to_json().hash(&mut hasher);
    hasher.finish()
}

/// A button of the Den's bars: a mono label in a hairline box, lit when it
/// is the thing in use and faint when there is nothing for it to do.
fn tool_button(
    id: SharedString,
    label: &'static str,
    enabled: bool,
    lit: bool,
    colours: &Palette,
) -> Stateful<Div> {
    let selector = id.clone();
    mono(label)
        .id(id)
        .debug_selector(move || selector.to_string())
        .px(px(8.))
        .py(px(3.))
        .rounded(metrics::RADIUS())
        .border_1()
        .border_color(if lit {
            colours.signal
        } else {
            colours.elevated_border
        })
        .text_color(if !enabled {
            colours.text_faint
        } else if lit {
            colours.text
        } else {
            colours.text_muted
        })
        .when(enabled, |button| button.cursor_pointer())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_has_a_tab_for_every_category_then_the_floors_and_the_dens() {
        let tabs = Tab::all();
        assert_eq!(tabs.len(), Category::ALL.len() + 2);
        assert_eq!(tabs[0], Tab::default());
        assert_eq!(&tabs[tabs.len() - 2..], [Tab::Floors, Tab::Dens]);
        // Every piece of the catalogue is on one tab, once.
        let mut shown: Vec<&str> = tabs.iter().flat_map(|tab| tab.pieces()).collect();
        assert_eq!(shown.len(), CATALOGUE.len());
        shown.sort_unstable();
        shown.dedup();
        assert_eq!(shown.len(), CATALOGUE.len());
        let mut names: Vec<&str> = tabs.iter().map(|tab| tab.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), tabs.len(), "no two tabs share a name");
    }

    #[test]
    fn stepping_goes_around_the_ends_and_starts_at_one_of_them() {
        assert_eq!(step(None, 3, false), Some(0));
        assert_eq!(step(None, 3, true), Some(2));
        assert_eq!(step(Some(2), 3, false), Some(0));
        assert_eq!(step(Some(0), 3, true), Some(2));
        assert_eq!(step(Some(1), 3, false), Some(2));
        assert_eq!(step(None, 0, false), None);
        assert_eq!(step(Some(4), 0, true), None);
    }

    #[test]
    fn a_thumbnail_is_drawn_in_whole_device_pixels() {
        // A tile in a box of 32: two pixels of the interface a pixel of art.
        assert_eq!(thumb_unit(16, 32., 1.), 2);
        assert_eq!(thumb_unit(16, 32., 2.), 4);
        // At 125 % that is two device pixels: 2.5 is not whole.
        assert_eq!(thumb_unit(16, 32., 1.25), 2);
        // A wide piece is drawn smaller, to fit, and never with no pixel.
        assert_eq!(thumb_unit(48, 56., 1.), 1);
        assert_eq!(thumb_unit(48, 56., 2.), 2);
        assert_eq!(thumb_unit(480, 56., 1.), 1);
        assert_eq!(thumb_unit(0, 56., 1.), 2);
    }
}
