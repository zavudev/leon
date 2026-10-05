//! The Settings screen: a large card over the window with the sections on
//! the left, a search field on top and the options on the right.
//!
//! **Why an overlay and not a page.** The main pane is where terminals live
//! and must keep painting, resizing and receiving output; a card over the
//! window leaves them alone, closes with Escape to exactly where the keyboard
//! was (the same path the shortcuts sheet and the palette use) and shows a
//! change to the theme, the interface size or a terminal's font behind it as
//! it is made.
//!
//! Everything shown is generated from the schema (`crate::schema`): the
//! options of a section, their control, their default and their "modified"
//! marker. Choosing changes the setting at once (there is no Save button).
//!
//! **Keyboard.** `Tab` and `Shift+Tab` move between the section list, the
//! search field and the options. In the options `J`/`K` or the arrows move,
//! `Enter` or `Space` change the option (a toggle flips, a choice or a list
//! opens the palette's question, a number or a text starts editing, a button
//! runs), `Left`/`Right` step a choice, a number or a toggle, `R` puts the
//! option back to its default, `/` goes to the search. While a field has the
//! keyboard only `Enter`, `Escape`, `Tab` and the arrows that leave it are
//! Leon's; every other key is the text's.

use super::sheet::{sheet_rows, SheetRow};
use super::shell::{Overlay, Shell};
use super::widgets::{key_cap, led, mono, section_label};
use crate::fuzzy::score;
use crate::icons::{icon, IconName};
use crate::schema::{self, Def, Kind, Section, Value};
use crate::settings;
use crate::theme::{metrics, px, Palette};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Entity, Keystroke, ScrollHandle, Stateful, Window};
use leon_core::{Machine, MachineId, MachineKind};

/// Where the keyboard is inside the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The list of sections.
    Sections,
    /// The search field.
    Search,
    /// The options.
    Options,
}

/// A text or a number being typed into an option.
#[derive(Clone, Debug)]
pub struct Edit {
    /// The setting.
    pub def: &'static Def,
    /// Why what was typed cannot be used.
    pub error: Option<String>,
}

/// What the screen remembers while it is open.
pub struct SettingsUi {
    /// The section shown when nothing is searched.
    pub section: Section,
    /// Where the keyboard is.
    pub zone: Zone,
    /// The option the keyboard is on, among [`Shell::settings_entries`].
    pub cursor: usize,
    /// The search field.
    pub search: Entity<InputState>,
    /// What it holds.
    pub query: String,
    /// The field an option is typed into.
    pub input: Entity<InputState>,
    /// The option being typed into.
    pub edit: Option<Edit>,
    /// Whether "Reset all settings" is waiting for its confirmation.
    pub confirm_reset: bool,
    /// The scroll of the options.
    pub scroll: ScrollHandle,
    /// Whether the screen is to be shown again when the palette's question
    /// about an option ends.
    pub return_from_palette: bool,
    /// Whether the sidebar's filter had the keyboard when the screen opened.
    pub restore_filter: bool,
    /// The project roots that were removed, read from the store when the
    /// screen opens, a section is chosen and the store changes.
    pub dismissed: Vec<Dismissed>,
}

impl SettingsUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            section: Section::Appearance,
            zone: Zone::Options,
            cursor: 0,
            search: cx.new(|cx| InputState::new(window, cx).placeholder("Search every setting")),
            query: String::new(),
            input: cx.new(|cx| InputState::new(window, cx)),
            edit: None,
            confirm_reset: false,
            scroll: ScrollHandle::new(),
            return_from_palette: false,
            restore_filter: false,
            dismissed: Vec::new(),
        }
    }
}

/// One line of the options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A setting, with its control.
    Setting(&'static Def),
    /// A command and its chords (read-only).
    Key(SheetRow),
    /// An SSH machine, with what can be done to it.
    Machine(Machine),
    /// A project root that was removed and is not adopted again by itself.
    Dismissed(Dismissed),
}

/// A project root somebody removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dismissed {
    /// The machine it is on.
    pub machine: MachineId,
    /// That machine's name.
    pub machine_name: String,
    /// The root.
    pub root: String,
}

/// The lines of the options for a section and a search. With nothing typed:
/// the settings of the section, or every command for the keyboard section.
/// With a search: the settings of every section that answer it, best first,
/// then the commands.
pub fn entries(section: Section, query: &str, mac: bool) -> Vec<Entry> {
    let query = query.trim();
    let keys = || -> Vec<SheetRow> {
        sheet_rows(mac)
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .collect()
    };
    if query.is_empty() {
        return match section {
            Section::Keyboard => keys().into_iter().map(Entry::Key).collect(),
            other => schema::of_section(other).map(Entry::Setting).collect(),
        };
    }
    let mut found: Vec<(u32, Entry)> = schema::SETTINGS
        .iter()
        .filter(|def| def.platform.here())
        .filter_map(|def| {
            let by_label = score(query, def.label);
            let by_words = score(query, def.keywords)
                .or_else(|| score(query, def.section.title()))
                .filter(|score| *score >= 400);
            Some((by_label.or(by_words)?, Entry::Setting(def)))
        })
        .collect();
    found.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let mut list: Vec<Entry> = found.into_iter().map(|(_, entry)| entry).collect();
    list.extend(
        keys()
            .into_iter()
            .filter(|row| score(query, &row.label).is_some())
            .map(Entry::Key),
    );
    list
}

fn secondary(stroke: &Keystroke) -> bool {
    if crate::platform::is_mac() {
        stroke.modifiers.platform
    } else {
        stroke.modifiers.control
    }
}

impl Shell {
    /// The lines shown now: the settings of the section (or of the search),
    /// then the machines and the removed project roots where they belong.
    pub(super) fn settings_entries(&self) -> Vec<Entry> {
        let ui = &self.settings_ui;
        let mut list = entries(ui.section, &ui.query, crate::platform::is_mac());
        let query = ui.query.trim();
        let wanted = |label: &str| query.is_empty() || score(query, label).is_some();
        let here = |section: Section| !query.is_empty() || ui.section == section;
        if here(Section::Machines) {
            list.extend(
                self.snapshot
                    .machines
                    .iter()
                    .filter(|machine| machine.kind != MachineKind::Local)
                    .filter(|machine| wanted(&machine.name))
                    .cloned()
                    .map(Entry::Machine),
            );
        }
        if here(Section::Projects) {
            list.extend(
                ui.dismissed
                    .iter()
                    .filter(|root| wanted(&root.root))
                    .cloned()
                    .map(Entry::Dismissed),
            );
        }
        list
    }

    /// Reads the removed project roots of every machine again.
    pub(super) fn refresh_dismissed(&mut self) {
        let store = self.engine.store().clone();
        self.settings_ui.dismissed = self
            .snapshot
            .machines
            .iter()
            .flat_map(|machine| {
                let mut roots: Vec<String> = store
                    .dismissed_roots(&machine.id)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                roots.sort();
                roots.into_iter().map(|root| Dismissed {
                    machine: machine.id.clone(),
                    machine_name: machine.name.clone(),
                    root,
                })
            })
            .collect();
    }

    /// Whether the controls of an option go under its text: the card is too
    /// narrow (the window, or the interface size) to give the text a readable
    /// column beside them.
    pub(super) fn settings_stacked(&self) -> bool {
        let card = metrics::SETTINGS_WIDTH().min(self.viewport.width - px(32.));
        let text = card - metrics::SETTINGS_NAV() - metrics::SETTINGS_CONTROL() - px(64.);
        text < metrics::SETTINGS_TEXT_MIN()
    }

    /// The setting of the line the keyboard is on.
    fn settings_current(&self) -> Option<&'static Def> {
        match self.settings_entries().get(self.settings_ui.cursor) {
            Some(Entry::Setting(def)) => Some(def),
            _ => None,
        }
    }

    // ----- opening and closing -----------------------------------------------------------------

    /// Opens the screen (the chord, the gear, the menu and the palette all come
    /// here).
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay == Overlay::Settings {
            return;
        }
        let from_filter = self.overlay == Overlay::None && self.filter_focused(window, cx);
        self.close_overlay(window, cx);
        self.settings_ui.restore_filter = from_filter;
        self.settings_ui.return_from_palette = false;
        self.settings_ui.edit = None;
        self.settings_ui.confirm_reset = false;
        self.settings_ui.zone = Zone::Options;
        self.settings_ui.cursor = 0;
        self.refresh_dismissed();
        self.settings_ui.query.clear();
        self.settings_ui
            .search
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.overlay = Overlay::Settings;
        self.settings_focus(window, cx);
        cx.notify();
    }

    /// Closes the screen and gives the keyboard back to where it was.
    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui.edit = None;
        self.settings_ui.confirm_reset = false;
        self.overlay = Overlay::None;
        if self.settings_ui.restore_filter {
            self.settings_ui.restore_filter = false;
            self.focus_filter(window, cx);
        } else {
            self.focus.focus(window, cx);
            self.sync_focus(window, cx);
        }
        cx.notify();
    }

    /// Puts the keyboard where the zone says: a field has it while one is
    /// being typed into, the window otherwise.
    pub(super) fn settings_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_ui.edit.is_some() {
            self.settings_ui
                .input
                .update(cx, |field, cx| field.focus(window, cx));
        } else if self.settings_ui.zone == Zone::Search {
            self.settings_ui
                .search
                .update(cx, |field, cx| field.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// The search field changed.
    pub(super) fn settings_search_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.settings_ui.search.read(cx).value().to_string();
        if text == self.settings_ui.query {
            return;
        }
        self.settings_ui.query = text;
        self.settings_ui.cursor = 0;
        self.settings_ui.scroll.scroll_to_item(0);
        cx.notify();
    }

    // ----- keys ------------------------------------------------------------------------------

    /// The keys of the screen. `true` when taken.
    pub(super) fn settings_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &stroke.modifiers;
        if secondary(stroke) || m.alt || m.function || m.platform || m.control {
            // Chords of the registry (the palette, the sheet...) stay Leon's.
            return false;
        }
        let key = stroke.key.as_str();
        let editing = self.settings_ui.edit.is_some();
        match key {
            "escape" => {
                if editing {
                    self.settings_end_edit(window, cx);
                } else if self.settings_ui.confirm_reset {
                    self.settings_ui.confirm_reset = false;
                } else if self.settings_ui.zone == Zone::Search
                    && !self.settings_ui.query.is_empty()
                {
                    self.settings_ui
                        .search
                        .update(cx, |field, cx| field.set_value("", window, cx));
                    self.settings_ui.query.clear();
                    self.settings_ui.cursor = 0;
                } else {
                    self.close_settings(window, cx);
                    return true;
                }
                cx.notify();
                return true;
            }
            "tab" => {
                if editing {
                    return true;
                }
                let order = [Zone::Sections, Zone::Search, Zone::Options];
                let at = order
                    .iter()
                    .position(|zone| *zone == self.settings_ui.zone)
                    .unwrap_or(0);
                let next = if m.shift { at + 2 } else { at + 1 } % 3;
                self.settings_ui.zone = order[next];
                self.settings_focus(window, cx);
                cx.notify();
                return true;
            }
            _ => {}
        }
        if editing {
            if key == "enter" {
                self.settings_commit_edit(window, cx);
                cx.notify();
                return true;
            }
            return false;
        }
        match self.settings_ui.zone {
            Zone::Search => match key {
                "enter" | "down" => {
                    self.settings_ui.zone = Zone::Options;
                    self.settings_focus(window, cx);
                    cx.notify();
                    true
                }
                "up" => {
                    self.settings_ui.zone = Zone::Sections;
                    self.settings_focus(window, cx);
                    cx.notify();
                    true
                }
                _ => false,
            },
            Zone::Sections => {
                match key {
                    "j" | "down" => self.settings_step_section(1, cx),
                    "k" | "up" => self.settings_step_section(-1, cx),
                    "enter" | "right" | "l" => self.settings_ui.zone = Zone::Options,
                    "/" => {
                        self.settings_ui.zone = Zone::Search;
                        self.settings_focus(window, cx);
                    }
                    _ => {}
                }
                cx.notify();
                true
            }
            Zone::Options => {
                self.settings_option_key(key, m.shift, window, cx);
                cx.notify();
                true
            }
        }
    }

    fn settings_step_section(&mut self, delta: i32, cx: &mut Context<Self>) {
        let all = Section::ALL;
        let at = all
            .iter()
            .position(|section| *section == self.settings_ui.section)
            .unwrap_or(0) as i32;
        let next = (at + delta).clamp(0, all.len() as i32 - 1) as usize;
        self.settings_pick_section(all[next], cx);
    }

    /// Shows another section and clears the search.
    pub(super) fn settings_pick_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.settings_ui.section = section;
        self.refresh_dismissed();
        self.settings_ui.cursor = 0;
        self.settings_ui.confirm_reset = false;
        self.settings_ui.scroll.scroll_to_item(0);
        cx.notify();
    }

    /// Opens the screen on one setting, with the keyboard on it: what "turn it
    /// on in Settings" leads to.
    pub(super) fn settings_goto(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(def) = schema::find(key) else {
            return;
        };
        self.open_settings(window, cx);
        self.settings_pick_section(def.section, cx);
        if let Some(at) = self
            .settings_entries()
            .iter()
            .position(|entry| matches!(entry, Entry::Setting(found) if found.key == key))
        {
            self.settings_ui.cursor = at;
            self.settings_ui.scroll.scroll_to_item(at);
        }
        self.settings_ui.zone = Zone::Options;
        self.settings_focus(window, cx);
        cx.notify();
    }

    fn settings_option_key(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.settings_entries().len();
        let last = count.saturating_sub(1);
        let at = self.settings_ui.cursor;
        let moved = match key {
            "j" | "down" => Some((at + 1).min(last)),
            "k" | "up" => Some(at.saturating_sub(1)),
            "pagedown" => Some((at + 6).min(last)),
            "pageup" => Some(at.saturating_sub(6)),
            "home" | "g" if !shift => Some(0),
            "end" | "g" => Some(last),
            _ => None,
        };
        if let Some(to) = moved {
            self.settings_ui.confirm_reset = false;
            self.settings_ui.cursor = to;
            self.settings_ui.scroll.scroll_to_item(to);
            return;
        }
        let entry = self.settings_entries().get(at).cloned();
        match (key, &entry) {
            ("e", Some(Entry::Machine(machine))) => {
                self.settings_edit_machine(machine.id.clone(), window, cx);
                return;
            }
            ("backspace" | "delete" | "x", Some(Entry::Machine(machine))) => {
                self.settings_remove_machine(machine.id.clone(), window, cx);
                return;
            }
            _ => {}
        }
        match key {
            "enter" | "space" => self.settings_activate(window, cx),
            "left" | "h" => self.settings_nudge(-1, cx),
            "right" | "l" => self.settings_nudge(1, cx),
            "r" | "backspace" | "delete" => self.settings_reset_here(cx),
            "/" => {
                self.settings_ui.zone = Zone::Search;
                self.settings_focus(window, cx);
            }
            _ => {}
        }
    }

    // ----- changing an option ------------------------------------------------------------------

    /// Enter on the line the keyboard is on.
    pub(super) fn settings_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self
            .settings_entries()
            .get(self.settings_ui.cursor)
            .cloned()
        {
            Some(Entry::Setting(def)) => self.settings_activate_def(def, window, cx),
            Some(Entry::Machine(machine)) => {
                self.engine.submit(crate::engine::Op::Probe(machine.id));
            }
            Some(Entry::Dismissed(dismissed)) => self.settings_restore_root(dismissed),
            Some(Entry::Key(_)) | None => {}
        }
    }

    /// Opens the Connect screen on the machine, pre-filled.
    pub(super) fn settings_edit_machine(
        &mut self,
        machine: MachineId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_connect(Some(machine), false, window, cx);
    }

    /// Asks before removing a machine, in the palette.
    pub(super) fn settings_remove_machine(
        &mut self,
        machine: MachineId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show(&super::tree::NodeId::Machine(machine));
        self.settings_ui.return_from_palette = true;
        self.begin_flow(crate::keys::Command::RemoveMachine, window, cx);
    }

    /// Lets discovery adopt a removed project root again.
    pub(super) fn settings_restore_root(&mut self, dismissed: Dismissed) {
        self.engine.submit(crate::engine::Op::RestoreRoot {
            machine: dismissed.machine,
            root: dismissed.root,
        });
    }

    /// What activating a setting does, by its kind.
    pub(super) fn settings_activate_def(
        &mut self,
        def: &'static Def,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match def.kind {
            Kind::Toggle => self.settings_nudge_def(def, 1, cx),
            Kind::Choice(_) | Kind::List { .. } => {
                self.settings_ui.return_from_palette = true;
                self.begin_setting_flow(def, window, cx);
            }
            Kind::Number { .. } | Kind::Text { .. } | Kind::Path { .. } => {
                self.settings_begin_edit(def, window, cx)
            }
            Kind::Action if def.key == "reset_all" => {
                if self.settings_ui.confirm_reset {
                    self.settings_ui.confirm_reset = false;
                    self.run_setting_action("reset_all", window, cx);
                } else {
                    self.settings_ui.confirm_reset = true;
                }
            }
            Kind::Action => self.run_setting_action(def.key, window, cx),
        }
    }

    fn settings_begin_edit(
        &mut self,
        def: &'static Def,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = settings::value_opt(cx, def.key)
            .map(|value| match value {
                Value::Int(number) => number.to_string(),
                other => other.display(),
            })
            .unwrap_or_default();
        self.settings_ui.edit = Some(Edit { def, error: None });
        self.settings_ui
            .input
            .update(cx, |field, cx| field.set_value(current, window, cx));
        self.settings_focus(window, cx);
    }

    fn settings_end_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_ui.edit = None;
        self.settings_focus(window, cx);
    }

    fn settings_commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.settings_ui.edit.clone() else {
            return;
        };
        let typed = self.settings_ui.input.read(cx).value().to_string();
        let checked = edit.def.parse_input(&typed).and_then(|value| {
            super::prefs::check_value(edit.def, &value, self.options.system.as_ref())
                .map(|()| value)
        });
        match checked {
            Ok(value) => {
                settings::set_value(cx, edit.def, value);
                self.settings_end_edit(window, cx);
            }
            Err(why) => {
                if let Some(edit) = self.settings_ui.edit.as_mut() {
                    edit.error = Some(why);
                }
            }
        }
    }

    /// Left or right on the line the keyboard is on.
    fn settings_nudge(&mut self, direction: i64, cx: &mut Context<Self>) {
        if let Some(def) = self.settings_current() {
            self.settings_nudge_def(def, direction, cx);
        }
    }

    /// Steps a setting by one: a choice to its neighbour, a number by its step,
    /// a toggle flipped.
    pub(super) fn settings_nudge_def(
        &mut self,
        def: &'static Def,
        direction: i64,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = settings::value_opt(cx, def.key) else {
            return;
        };
        if let Some(next) = def.nudge(&current, direction) {
            // Skip a theme nobody can wear.
            self.choose_setting(def, next, cx);
        }
    }

    /// Puts the setting the keyboard is on back to its default.
    fn settings_reset_here(&mut self, cx: &mut Context<Self>) {
        if let Some(def) = self.settings_current() {
            self.settings_reset(def, cx);
        }
    }

    /// Puts a setting back to its default.
    pub(super) fn settings_reset(&mut self, def: &'static Def, cx: &mut Context<Self>) {
        if !def.is_action() {
            settings::reset_value(cx, def);
            cx.notify();
        }
    }

    /// A click on a line selects it.
    pub(super) fn settings_select(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_ui.cursor = index;
        self.settings_ui.zone = Zone::Options;
        self.settings_ui.confirm_reset = false;
        if self.settings_ui.edit.is_some() {
            self.settings_ui.edit = None;
        }
        self.settings_focus(window, cx);
        cx.notify();
    }

    // ----- drawing ----------------------------------------------------------------------------------

    /// The Settings card.
    pub(super) fn render_settings(
        &self,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let ui = &self.settings_ui;
        let entries = self.settings_entries();
        let searching = !ui.query.trim().is_empty();
        let hover = colours.surface_2;

        let mut nav = div()
            .flex_none()
            .w(metrics::SETTINGS_NAV())
            .border_r_1()
            .border_color(colours.border)
            .py_2()
            .flex()
            .flex_col();
        for (number, section) in Section::ALL.into_iter().enumerate() {
            let on = section == ui.section && !searching;
            let here = on && ui.zone == super::settings_screen::Zone::Sections;
            nav = nav.child(
                div()
                    .id(("settings-section", number))
                    .debug_selector(move || format!("settings-section-{number}"))
                    .relative()
                    .h(px(32.))
                    .px_4()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(metrics::TEXT_BODY())
                    .text_color(if on { colours.text } else { colours.text_muted })
                    .when(on, |this| this.bg(colours.surface_2))
                    .when(here, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .bg(colours.signal),
                        )
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.settings_ui.zone = Zone::Sections;
                        this.settings_pick_section(section, cx);
                        this.settings_ui.query.clear();
                        this.settings_ui
                            .search
                            .update(cx, |field, cx| field.set_value("", window, cx));
                        this.settings_focus(window, cx);
                    }))
                    .child(section.title()),
            );
        }

        let mut rows = div()
            .id("settings-options")
            .debug_selector(|| "settings-options".into())
            .track_scroll(&ui.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col();
        if entries.is_empty() {
            rows = rows.child(
                div()
                    .debug_selector(|| "settings-empty".into())
                    .p_4()
                    .text_color(colours.text_muted)
                    .child("No setting answers that."),
            );
        }
        for (index, entry) in entries.iter().enumerate() {
            rows = rows.child(self.render_settings_row(index, entry, colours, cx));
        }

        let (title, blurb) = if searching {
            ("Search".to_owned(), format!("{} found", entries.len()))
        } else {
            (ui.section.title().to_owned(), ui.section.blurb().to_owned())
        };
        let note = (!searching && ui.section == Section::Keyboard).then(|| {
            div()
                .debug_selector(|| "settings-keyboard-note".into())
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(colours.border)
                .text_size(metrics::TEXT_SMALL())
                .text_color(colours.warning)
                .child("Shortcuts are not customisable yet: this list is read-only.")
        });
        let hints = if ui.edit.is_some() {
            "ENTER APPLY   ESC CANCEL"
        } else {
            match ui.zone {
                Zone::Sections => "J/K SECTION   ENTER OPTIONS   TAB NEXT   ESC CLOSE",
                Zone::Search => "TYPE TO SEARCH   ENTER OPTIONS   TAB NEXT   ESC CLEAR",
                Zone::Options => match entries.get(ui.cursor) {
                    Some(Entry::Machine(_)) => {
                        "J/K MOVE   ENTER PROBE   E EDIT   X REMOVE   / SEARCH   TAB NEXT   ESC CLOSE"
                    }
                    _ => {
                        "J/K MOVE   ENTER CHANGE   ←/→ STEP   R RESET   / SEARCH   TAB NEXT   ESC CLOSE"
                    }
                },
            }
        };
        self.card("settings", colours)
            .w(metrics::SETTINGS_WIDTH())
            .max_w(self.viewport.width - px(32.))
            .h(self.viewport.height - px(112.))
            .max_h(px(760.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .h(metrics::HEADER_HEIGHT())
                    .px_4()
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(section_label("Settings", colours))
                    .child(
                        div()
                            .id("settings-search")
                            .debug_selector(|| "settings-search".into())
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.settings_ui.zone = Zone::Search;
                                this.settings_focus(window, cx);
                                cx.notify();
                            }))
                            .child(icon(IconName::Search, px(16.), colours.text_muted))
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&ui.search).appearance(false)),
                            ),
                    )
                    .child(key_cap(crate::keys::key_label("escape"), colours)),
            )
            .child(
                div().flex_1().min_h_0().flex().child(nav).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_none()
                                .px_4()
                                .py_2()
                                .border_b_1()
                                .border_color(colours.border)
                                .flex()
                                .items_baseline()
                                .gap_3()
                                .child(
                                    div()
                                        .debug_selector(|| "settings-title".into())
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(metrics::TEXT_SMALL())
                                        .text_color(colours.text_muted)
                                        .child(blurb),
                                ),
                        )
                        .children(note)
                        .child(rows),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(colours.border)
                    .child(
                        mono(hints)
                            .debug_selector(|| "settings-hints".into())
                            .text_color(colours.text_muted),
                    ),
            )
    }

    fn render_settings_row(
        &self,
        index: usize,
        entry: &Entry,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let ui = &self.settings_ui;
        let on = index == ui.cursor;
        let here = on && ui.zone == Zone::Options;
        let hover = colours.surface_2;
        let base = div()
            .id(("settings-row", index))
            .relative()
            // As tall as its content, never squeezed by the list: a row that
            // shrinks to fit paints its text over the next one.
            .flex_none()
            .w_full()
            .min_h(metrics::SETTINGS_ROW())
            .px_4()
            .py(metrics::SETTINGS_ROW_PAD())
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .border_b_1()
            .border_color(colours.border)
            .when(on, |this| this.bg(colours.surface_2))
            .when(here, |this| {
                this.child(
                    div()
                        .debug_selector(|| "settings-cursor".into())
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(colours.signal),
                )
            })
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.settings_select(index, window, cx);
            }));
        match entry {
            Entry::Key(row) => {
                let label = row.label.clone();
                base.debug_selector(move || format!("settings-key-{index}"))
                    .child(div().min_w_0().truncate().child(label))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(4.))
                            .children(row.keys.iter().map(|text| key_cap(text.clone(), colours)))
                            .when(row.keys.is_empty(), |this| {
                                this.text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_faint)
                                    .child("palette only")
                            }),
                    )
            }
            Entry::Setting(def) => self.render_setting(base, def, index, colours, cx),
            Entry::Machine(machine) => self.render_machine(base, machine, index, colours, cx),
            Entry::Dismissed(dismissed) => {
                let root = dismissed.clone();
                let selector = format!("settings-dismissed-{index}");
                base.debug_selector(move || selector.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().truncate().child(dismissed.root.clone()))
                            .child(
                                div()
                                    .text_size(metrics::TEXT_SMALL())
                                    .text_color(colours.text_muted)
                                    .child(format!(
                                        "Removed from {}. Discovery does not bring it back by itself.",
                                        dismissed.machine_name
                                    )),
                            ),
                    )
                    .child(
                        mono("REMOVE FROM THE LIST".to_owned())
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(metrics::RADIUS())
                            .border_1()
                            .border_color(colours.elevated_border)
                            .cursor_pointer()
                            .debug_selector(move || format!("settings-restore-{index}"))
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.settings_select(index, window, cx);
                                    this.settings_restore_root(root.clone());
                                }),
                            ),
                    )
            }
        }
    }

    fn render_machine(
        &self,
        base: Stateful<Div>,
        machine: &Machine,
        index: usize,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = machine.id.clone();
        let destination = match &machine.kind {
            MachineKind::Ssh {
                host,
                user,
                port,
                identity_file,
            } => {
                let mut text = String::new();
                if let Some(user) = user {
                    text.push_str(&format!("{user}@"));
                }
                text.push_str(host);
                if let Some(port) = port {
                    text.push_str(&format!(":{port}"));
                }
                if let Some(identity) = identity_file {
                    text.push_str(&format!(" · key {identity}"));
                }
                text
            }
            MachineKind::Relay { host_id, name, .. } => {
                let short = host_id.get(..12).unwrap_or(host_id);
                format!("{name} · relay · {short}")
            }
            MachineKind::Local => "this computer".to_owned(),
        };
        let state = match self.engine.machine_state(&machine.id) {
            crate::engine::MachineState::Unknown => "not probed".to_owned(),
            crate::engine::MachineState::Probing => "probing…".to_owned(),
            crate::engine::MachineState::Online(_) => "online".to_owned(),
            crate::engine::MachineState::Offline(why) => format!("offline: {why}"),
        };
        let button = |id_text: &'static str, label: &'static str| {
            let selector = format!("{id_text}-{index}");
            mono(label.to_owned())
                .px(px(8.))
                .py(px(3.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.elevated_border)
                .cursor_pointer()
                .debug_selector(move || selector.clone())
        };
        let (probe, edit, remove) = (id.clone(), id.clone(), id);
        base.debug_selector(move || format!("settings-machine-{index}"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().child(machine.name.clone()))
                    .child(
                        div()
                            .truncate()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(format!("{destination} · {state}")),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap_2()
                    .child(button("settings-probe", "PROBE").on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.settings_select(index, window, cx);
                            this.engine.submit(crate::engine::Op::Probe(probe.clone()));
                        }),
                    ))
                    .child(button("settings-edit", "EDIT…").on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.settings_select(index, window, cx);
                            this.settings_edit_machine(edit.clone(), window, cx);
                        }),
                    ))
                    .child(button("settings-remove", "REMOVE…").on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.settings_select(index, window, cx);
                            this.settings_remove_machine(remove.clone(), window, cx);
                        }),
                    )),
            )
    }

    fn render_setting(
        &self,
        base: Stateful<Div>,
        def: &'static Def,
        index: usize,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key = def.key;
        let modified = settings::is_modified(cx, def);
        let value = settings::value_opt(cx, key);
        let editing = self
            .settings_ui
            .edit
            .as_ref()
            .filter(|edit| edit.def.key == key);
        let searching = !self.settings_ui.query.trim().is_empty();
        let button = |id: &'static str, text: String| {
            mono(text)
                .px(px(8.))
                .py(px(3.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.elevated_border)
                .text_color(colours.text)
                .cursor_pointer()
                .debug_selector(move || format!("{id}-{key}"))
        };
        let step_button = |id: &'static str, glyph: &'static str, direction: i64| {
            button(id, glyph.to_owned()).on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.settings_select(index, window, cx);
                    this.settings_nudge_def(def, direction, cx);
                }),
            )
        };
        let shown = value
            .as_ref()
            .map(|value| def.show(value))
            .unwrap_or_default();
        let value_button = |text: String| {
            div()
                .id(("settings-value", index))
                .debug_selector(move || format!("settings-value-{key}"))
                .max_w(metrics::SETTINGS_CONTROL() - px(76.))
                .truncate()
                .px(px(8.))
                .py(px(3.))
                .rounded(metrics::RADIUS())
                .border_1()
                .border_color(colours.border)
                .cursor_pointer()
                .text_size(metrics::TEXT_BODY())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.settings_select(index, window, cx);
                    this.settings_activate_def(def, window, cx);
                }))
                .child(text)
        };
        let control: gpui_kit::AnyElement = if let Some(edit) = editing {
            div()
                .debug_selector(move || format!("settings-edit-{key}"))
                .flex()
                .flex_col()
                .items_end()
                .gap(px(2.))
                .child(
                    div()
                        .w(metrics::SETTINGS_CONTROL())
                        .px(px(8.))
                        .border_1()
                        .border_color(colours.signal)
                        .rounded(metrics::RADIUS())
                        .child(Input::new(&self.settings_ui.input).appearance(false)),
                )
                .children(edit.error.clone().map(|why| {
                    div()
                        .debug_selector(move || format!("settings-error-{key}"))
                        .text_size(metrics::TEXT_SMALL())
                        .text_color(colours.error)
                        .child(why)
                }))
                .into_any_element()
        } else {
            match def.kind {
                Kind::Toggle => {
                    let on = value.as_ref().and_then(Value::as_bool).unwrap_or(false);
                    div()
                        .id(("settings-toggle", index))
                        .debug_selector(move || format!("settings-toggle-{key}"))
                        .flex_none()
                        .w(px(34.))
                        .h(px(18.))
                        .p(px(2.))
                        .border_1()
                        .border_color(if on {
                            colours.signal
                        } else {
                            colours.elevated_border
                        })
                        .flex()
                        .when(on, |this| this.justify_end())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.settings_select(index, window, cx);
                            this.settings_nudge_def(def, 1, cx);
                        }))
                        .child(div().size(px(12.)).bg(if on {
                            colours.signal
                        } else {
                            colours.text_faint
                        }))
                        .into_any_element()
                }
                Kind::Choice(_) => div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(step_button("settings-prev", "‹", -1))
                    .child(value_button(shown))
                    .child(step_button("settings-next", "›", 1))
                    .into_any_element(),
                Kind::Number { .. } => div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(step_button("settings-prev", "−", -1))
                    .child(value_button(shown))
                    .child(step_button("settings-next", "+", 1))
                    .into_any_element(),
                Kind::Text { .. } | Kind::Path { .. } | Kind::List { .. } => {
                    value_button(shown).into_any_element()
                }
                Kind::Action => {
                    let confirming = key == "reset_all" && self.settings_ui.confirm_reset;
                    let label = match key {
                        "reset_all" if confirming => "CONFIRM RESET",
                        "reset_all" => "RESET…",
                        "open_data_folder" | "open_themes_folder" => "REVEAL",
                        "add_machine" => "ADD…",
                        "share_machine" => "SHARE…",
                        "probe_machine" => "PROBE",
                        _ => "RUN",
                    };
                    button("settings-action", label.to_owned())
                        .text_color(if confirming {
                            colours.error
                        } else {
                            colours.text
                        })
                        .on_mouse_down(
                            gpui_kit::MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.settings_select(index, window, cx);
                                this.settings_activate_def(def, window, cx);
                            }),
                        )
                        .into_any_element()
                }
            }
        };
        let stacked = self.settings_stacked();
        base.debug_selector(move || format!("settings-row-{key}"))
            .when(stacked, |this| this.flex_col().items_stretch().gap_2())
            .child(
                div()
                    .debug_selector(move || format!("settings-text-{key}"))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .debug_selector(move || format!("settings-label-{key}"))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x_2()
                            .child(div().child(def.label))
                            .when(searching, |this| {
                                this.child(
                                    mono(def.section.title().to_uppercase())
                                        .text_color(colours.text_faint),
                                )
                            })
                            .when(modified, |this| {
                                this.child(
                                    div()
                                        .debug_selector(move || format!("settings-modified-{key}"))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .child(led(colours.signal))
                                        .child(mono("MODIFIED").text_color(colours.text_muted)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("settings-desc-{key}"))
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(def.description),
                    ),
            )
            .child(
                div()
                    .debug_selector(move || format!("settings-control-{key}"))
                    .flex_none()
                    .w(metrics::SETTINGS_CONTROL())
                    .when(stacked, |this| this.self_end())
                    .flex()
                    .flex_col()
                    .items_end()
                    .justify_center()
                    .gap_2()
                    .when(modified, |this| {
                        this.child(
                            button("settings-reset", "RESET TO DEFAULT".to_owned())
                                .text_color(colours.text_muted)
                                .on_mouse_down(
                                    gpui_kit::MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.settings_select(index, window, cx);
                                        this.settings_reset(def, cx);
                                    }),
                                ),
                        )
                    })
                    .child(control),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_lists_its_settings_in_the_schemas_order() {
        let list = entries(Section::Window, "", false);
        let keys: Vec<&str> = list
            .iter()
            .filter_map(|entry| match entry {
                Entry::Setting(def) => Some(def.key),
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            ["sidebar_visible", "sidebar_width", "quit_confirmation"]
        );
    }

    #[test]
    fn every_setting_of_the_schema_is_in_a_section_of_the_screen() {
        for def in schema::SETTINGS.iter().filter(|def| def.platform.here()) {
            assert!(
                entries(def.section, "", false).contains(&Entry::Setting(def)),
                "{}",
                def.key
            );
        }
    }

    #[test]
    fn a_search_finds_options_of_every_section_by_label_and_by_keyword() {
        let found = entries(Section::Appearance, "scrollback", false);
        assert!(found.contains(&Entry::Setting(
            schema::find("terminal_scrollback").unwrap()
        )));
        let found = entries(Section::Appearance, "github", false);
        assert!(found.contains(&Entry::Setting(schema::find("fetch_avatars").unwrap())));
        assert!(entries(Section::Appearance, "zzzqqq", false).is_empty());
    }

    #[test]
    fn the_keyboard_section_lists_every_command_with_its_chords() {
        let list = entries(Section::Keyboard, "", false);
        let rows: Vec<&SheetRow> = list
            .iter()
            .filter_map(|entry| match entry {
                Entry::Key(row) => Some(row),
                _ => None,
            })
            .collect();
        assert!(rows.iter().any(|row| row.label == "Command palette"));
        assert!(rows.iter().any(|row| row.label == "Settings"));
        let narrowed = entries(Section::Keyboard, "palette", false);
        assert!(narrowed.len() < list.len());
    }
}
