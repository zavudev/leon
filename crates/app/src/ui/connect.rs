//! The "Connect a machine" screen: what it remembers, its keys and what its
//! buttons do. What it draws is in `connect_view.rs`; what it says and checks
//! without a window is in `crate::connect`.
//!
//! **One screen for four jobs.** It adds a machine, edits one (the same form,
//! pre-filled), explains why a machine is offline (the checklist of its last
//! failure, with "Test again") and, once a machine is saved, says what comes
//! next. Like Settings it is an overlay: the terminals behind keep running and
//! Escape returns the keyboard to where it was.
//!
//! **Keyboard.** `Tab` and `Shift+Tab` go through the form, the buttons and
//! the explanation; `Enter` on a field goes to the next one (or takes the
//! highlighted suggestion, chosen with the arrows), and on a button presses
//! it. `Ctrl/Cmd+Enter` tests, `Ctrl/Cmd+S` saves, `Ctrl/Cmd+Shift+C` copies
//! the `ssh-copy-id` line. While a field has the keyboard only these, `Enter`,
//! `Escape`, `Tab` and the arrows are Leon's; every other key is the text's.
//!
//! **Testing.** The test runs the checklist of `leon_remote::connect` through
//! the engine's runner, off the UI thread; this screen only reads its
//! progress. Escape (or CANCEL) while it runs stops it, and the `ssh` it
//! started is killed. Nothing is written to `~/.ssh` except when the person
//! confirms "Trust this computer".

use super::shell::{Overlay, Picked, Shell};
use super::tree::NodeId;
use crate::connect::{self as model, Field, Form, Platform};
use crate::engine::StatusKind;
use crate::keys::Command;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Entity, Focusable as _, Keystroke, ScrollHandle, Subscription, Task, Window,
};
use leon_core::{MachineId, MachineKind};
use leon_remote::classify;
use leon_remote::connect::{Checklist, KeyFile, Target};
use std::collections::HashMap;

/// What the screen is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The form, to add a machine or edit one.
    Form,
    /// A machine was saved: what to do next.
    Next,
    /// A machine's last failure, explained.
    Why,
}

/// A place the keyboard can be, besides the fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// A field of the form.
    Field(Field),
    /// "Trust this computer".
    Trust,
    /// The raw output under "Details".
    Details,
    /// Test the connection (cancel while it runs).
    Test,
    /// Save the machine.
    Save,
    /// The platform choice of the explanation.
    Platform,
    /// One of the three "How do I…" rows.
    How(usize),
    /// Confirm adding the host key to `known_hosts`.
    TrustYes,
    /// Do not add it.
    TrustNo,
    /// "Edit" in the explanation of an offline machine.
    Edit,
    /// "Add a project on this machine".
    AddProject,
    /// "Open a shell".
    Shell,
    /// "Done" and "Close".
    Done,
}

/// What the screen remembers while it is open.
pub struct ConnectUi {
    /// The six fields, in [`Field::ALL`] order.
    pub inputs: Vec<Entity<InputState>>,
    /// What they hold, as of the last change.
    pub form: Form,
    /// Where the keyboard is when no field has it.
    pub stop: Stop,
    /// What is shown.
    pub phase: Phase,
    /// The other computer's platform, for the explanation.
    pub platform: Platform,
    /// The "How do I…" row that is open.
    pub open_howto: Option<usize>,
    /// The machine being edited or explained.
    pub editing: Option<MachineId>,
    /// The machine that was just saved.
    pub saved: Option<MachineId>,
    /// The checklist as far as it has got, for `tested`.
    pub checklist: Option<Checklist>,
    /// What the checklist is about: a different target makes it stale.
    pub tested: Option<Target>,
    /// The test under way; dropping it cancels the test.
    pub testing: Option<Task<()>>,
    /// Whether the test is followed by saving.
    pub save_after: bool,
    /// Whether the question "add it to known_hosts?" is open.
    pub trust: bool,
    /// Whether the raw output is shown.
    pub details: bool,
    /// Whether the instructions stay on screen beside a checklist (they give
    /// way to it otherwise, so the checklist is in view without scrolling).
    pub show_help: bool,
    /// Whether the problems of the fields are shown.
    pub shown_problems: bool,
    /// Host names from `~/.ssh/config` and `known_hosts`.
    pub hosts: Vec<String>,
    /// The public keys of `~/.ssh`, by name.
    pub keys: Vec<KeyFile>,
    /// The highlighted suggestion of the field that has the keyboard.
    pub suggest_at: Option<usize>,
    /// What this computer signs in as without a user.
    pub login: String,
    /// One line about what just happened.
    pub notice: Option<String>,
    /// The repositories found on each machine, for "Add a project".
    pub repos: HashMap<MachineId, Vec<String>>,
    /// The search for them, while it runs.
    pub searching: Option<Task<()>>,
    /// The file dialog, while it is open.
    pub picking: Option<Task<()>>,
    /// Whether the sidebar's filter had the keyboard when the screen opened.
    pub restore_filter: bool,
    /// The scroll of the card.
    pub scroll: ScrollHandle,
    /// Frames left in which the card follows its content to the bottom: set
    /// when the checklist changes, so the newest line is in view.
    pub follow: std::cell::Cell<u8>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        let login = model::login_name();
        let inputs: Vec<Entity<InputState>> = Field::ALL
            .iter()
            .map(|field| {
                let placeholder = match field {
                    Field::User => login.clone(),
                    other => other.placeholder().to_owned(),
                };
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
            })
            .collect();
        let subscriptions = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.connect_changed(window, cx);
                    }
                })
            })
            .collect();
        Self {
            inputs,
            form: Form::default(),
            stop: Stop::Field(Field::Name),
            phase: Phase::Form,
            platform: Platform::Linux,
            open_howto: None,
            editing: None,
            saved: None,
            checklist: None,
            tested: None,
            testing: None,
            save_after: false,
            trust: false,
            details: false,
            show_help: false,
            shown_problems: false,
            hosts: Vec::new(),
            keys: Vec::new(),
            suggest_at: None,
            login,
            notice: None,
            repos: HashMap::new(),
            searching: None,
            picking: None,
            restore_filter: false,
            scroll: ScrollHandle::new(),
            follow: std::cell::Cell::new(0),
            _subscriptions: subscriptions,
        }
    }

    /// The field's text entry.
    pub fn input(&self, field: Field) -> &Entity<InputState> {
        let at = Field::ALL.iter().position(|f| *f == field).unwrap_or(0);
        &self.inputs[at]
    }

    /// The `ssh` line as shown: the host is a placeholder until one is typed.
    pub fn shown_ssh_line(&self) -> String {
        let mut target = self.form.target();
        if target.host.is_empty() {
            target.host = "host".to_owned();
        }
        target.ssh_line()
    }

    /// Whether a test is running.
    pub fn is_testing(&self) -> bool {
        self.testing.is_some()
    }

    /// Whether the checklist on screen is about what the form says now.
    pub fn checklist_is_current(&self) -> bool {
        self.checklist.is_some() && self.tested.as_ref() == Some(&self.form.target())
    }

    /// Whether the last test of what the form says failed or was cut short.
    pub fn last_test_failed(&self) -> bool {
        self.checklist_is_current()
            && self
                .checklist
                .as_ref()
                .is_some_and(|list| list.finished && !list.connected())
    }

    /// The places the keyboard goes through with `Tab`, in order.
    pub fn stops(&self) -> Vec<Stop> {
        match self.phase {
            Phase::Next => vec![Stop::AddProject, Stop::Shell, Stop::Done],
            Phase::Why => {
                let mut stops = vec![Stop::Test, Stop::Edit];
                if self.offers_trust() {
                    stops.insert(0, Stop::Trust);
                }
                stops.push(Stop::Done);
                stops
            }
            Phase::Form => {
                let mut stops: Vec<Stop> = Field::ALL.iter().map(|f| Stop::Field(*f)).collect();
                if self.trust {
                    stops.extend([Stop::TrustYes, Stop::TrustNo]);
                } else if self.offers_trust() {
                    stops.push(Stop::Trust);
                }
                if self
                    .checklist
                    .as_ref()
                    .is_some_and(|list| !list.raw.is_empty())
                {
                    stops.push(Stop::Details);
                }
                stops.extend([Stop::Test, Stop::Save, Stop::Platform]);
                stops.extend((0..3).map(Stop::How));
                stops
            }
        }
    }

    /// Whether the checklist stopped at an unknown host key that can be
    /// trusted (a changed one never can).
    pub fn offers_trust(&self) -> bool {
        self.checklist
            .as_ref()
            .is_some_and(|list| list.host_keys.is_some() && !list.connected())
    }

    /// The suggestions for the field, as text to put in it.
    pub fn suggestions(&self, field: Field) -> Vec<String> {
        match field {
            Field::Host => model::matching_hosts(&self.hosts, &self.form.host, 5)
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Field::Identity => self
                .keys
                .iter()
                .map(|key| key.private.clone())
                .filter(|path| {
                    let typed = self.form.identity.trim();
                    typed.is_empty() || (path.starts_with(typed) && path != typed)
                })
                .take(5)
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn secondary(stroke: &Keystroke) -> bool {
    if cfg!(target_os = "macos") {
        stroke.modifiers.platform
    } else {
        stroke.modifiers.control
    }
}

/// The system's file dialog, for a private key.
pub fn system_key_picker(cx: &mut gpui_kit::App) -> Task<Picked> {
    let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Choose key".into()),
    });
    cx.spawn(async move |_| match paths.await {
        Ok(Ok(Some(mut paths))) => paths.pop().map_or(Picked::Cancelled, Picked::Folder),
        Ok(Ok(None)) => Picked::Cancelled,
        _ => Picked::Unavailable,
    })
}

impl Shell {
    // ----- opening and closing ---------------------------------------------------------------

    /// Opens the screen. With `editing` it holds that machine's values; with
    /// `why` it explains that machine's failure instead of asking for values.
    pub(super) fn open_connect(
        &mut self,
        editing: Option<MachineId>,
        why: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay == Overlay::Connect {
            return;
        }
        let from_filter = self.overlay == Overlay::None && self.filter_focused(window, cx);
        self.close_overlay(window, cx);
        let machine = editing
            .as_ref()
            .and_then(|id| self.snapshot.machine(id).cloned());
        let target = machine.as_ref().and_then(Target::of);
        let editing = target.is_some().then_some(editing).flatten();
        let form = match (&machine, &target) {
            (Some(machine), Some(target)) => Form::of(&machine.name, target),
            _ => Form::default(),
        };
        let ssh_dir = self.options.ssh_dir.clone();
        let ui = &mut self.connect_ui;
        ui.hosts = leon_remote::connect::suggested_hosts(ssh_dir.as_ref());
        ui.keys = ssh_dir.public_keys();
        ui.editing = editing.clone();
        ui.saved = None;
        ui.checklist = None;
        ui.tested = None;
        ui.testing = None;
        ui.searching = None;
        ui.save_after = false;
        ui.trust = false;
        ui.details = false;
        ui.show_help = false;
        ui.shown_problems = false;
        ui.open_howto = None;
        ui.suggest_at = None;
        ui.notice = None;
        ui.restore_filter = from_filter;
        ui.phase = if why { Phase::Why } else { Phase::Form };
        ui.stop = if why {
            Stop::Test
        } else {
            Stop::Field(Field::Name)
        };
        ui.scroll
            .set_offset(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)));
        ui.form = form.clone();
        for field in Field::ALL {
            let text = field_text(&form, field);
            ui.input(field)
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        self.overlay = Overlay::Connect;
        if why {
            self.connect_explain(editing, window, cx);
        }
        self.connect_focus(window, cx);
        cx.notify();
    }

    /// The "why offline" state: the last failure as a checklist, or a fresh
    /// test when there is none to explain.
    fn connect_explain(
        &mut self,
        machine: Option<MachineId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reason = machine
            .as_ref()
            .and_then(|id| self.engine.offline_reason(id));
        match reason {
            Some(reason) => {
                let diagnosis = classify(Some(255), &reason);
                self.connect_ui.tested = Some(self.connect_ui.form.target());
                self.connect_ui.checklist = Some(Checklist::from_failure(diagnosis, reason));
            }
            None => self.connect_test(false, window, cx),
        }
    }

    /// Closes the screen and gives the keyboard back to where it was.
    pub(super) fn close_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.connect_ui;
        ui.testing = None;
        ui.searching = None;
        ui.picking = None;
        ui.trust = false;
        self.overlay = Overlay::None;
        if self.connect_ui.restore_filter {
            self.connect_ui.restore_filter = false;
            self.focus_filter(window, cx);
        } else {
            self.focus.focus(window, cx);
            self.sync_focus(window, cx);
        }
        cx.notify();
    }

    /// Puts the keyboard where the stop says: a field has it while one is
    /// being typed into, the window otherwise.
    pub(super) fn connect_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.connect_ui.stop {
            Stop::Field(field) => {
                // The fields are at the top of the card: bring them into view.
                let mut offset = self.connect_ui.scroll.offset();
                offset.y = gpui_kit::px(0.);
                self.connect_ui.scroll.set_offset(offset);
                self.connect_ui.follow.set(0);
                self.connect_ui
                    .input(field)
                    .clone()
                    .update(cx, |input, cx| input.focus(window, cx))
            }
            _ => self.focus.focus(window, cx),
        }
    }

    // ----- what is typed ------------------------------------------------------------------------

    /// A field changed: the host field is split into user, host and port, and
    /// a test of something else is forgotten.
    pub(super) fn connect_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay != Overlay::Connect {
            return;
        }
        let read = |ui: &ConnectUi, field: Field| ui.input(field).read(cx).value().to_string();
        let ui = &self.connect_ui;
        let mut form = Form {
            name: read(ui, Field::Name),
            host: read(ui, Field::Host),
            user: read(ui, Field::User),
            port: read(ui, Field::Port),
            identity: read(ui, Field::Identity),
            folder: read(ui, Field::Folder),
        };
        if let Some(parts) = model::split_host(&form.host) {
            form.host = parts.host;
            if let Some(user) = parts.user {
                form.user = user;
            }
            if let Some(port) = parts.port {
                form.port = port.to_string();
            }
            for field in [Field::Host, Field::User, Field::Port] {
                let text = field_text(&form, field);
                self.connect_ui
                    .input(field)
                    .clone()
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
        if form == self.connect_ui.form {
            return;
        }
        let ui = &mut self.connect_ui;
        let moved = form.target() != ui.form.target();
        ui.form = form;
        ui.suggest_at = None;
        if moved && ui.phase == Phase::Form {
            // What was tested is not what is typed now.
            ui.testing = None;
            ui.save_after = false;
            ui.trust = false;
            ui.checklist = None;
            ui.tested = None;
        }
        cx.notify();
    }

    pub(super) fn connect_set(
        &mut self,
        field: Field,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connect_ui
            .input(field)
            .clone()
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.connect_changed(window, cx);
    }

    // ----- keys ------------------------------------------------------------------------------------

    /// The keys of the screen. `true` when taken.
    pub(super) fn connect_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // A click may have put the keyboard in another field.
        for field in Field::ALL {
            if self
                .connect_ui
                .input(field)
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
            {
                self.connect_ui.stop = Stop::Field(field);
            }
        }
        let m = &stroke.modifiers;
        let key = stroke.key.as_str();
        if secondary(stroke) {
            let taken = match (key, m.shift, m.alt) {
                ("enter", false, false) => {
                    self.connect_test(false, window, cx);
                    true
                }
                ("s", false, false) => {
                    self.connect_save(window, cx);
                    true
                }
                ("c", true, false) => {
                    self.connect_copy_id(cx);
                    true
                }
                ("c", false, true) => {
                    self.connect_copy_ssh(cx);
                    true
                }
                _ => false,
            };
            if taken {
                cx.notify();
            }
            return taken;
        }
        if m.alt || m.function || m.platform || m.control {
            return false;
        }
        let in_field = matches!(self.connect_ui.stop, Stop::Field(_));
        match key {
            "escape" => {
                if self.connect_ui.trust {
                    self.connect_ui.trust = false;
                    self.connect_ui.stop = Stop::Trust;
                    self.connect_focus(window, cx);
                } else if self.connect_ui.is_testing() {
                    self.connect_cancel(window, cx);
                } else {
                    self.close_connect(window, cx);
                    return true;
                }
            }
            "tab" => self.connect_step(if m.shift { -1 } else { 1 }, window, cx),
            "enter" if in_field => self.connect_enter_in_field(window, cx),
            "down" | "up" if in_field => self.connect_arrow(key == "down", window, cx),
            _ if in_field => return false,
            "enter" | "space" => self.connect_press(window, cx),
            "left" | "h" if self.connect_ui.stop == Stop::Platform => self.connect_platform(false),
            "right" | "l" if self.connect_ui.stop == Stop::Platform => self.connect_platform(true),
            "down" | "j" => self.connect_step(1, window, cx),
            "up" | "k" => self.connect_step(-1, window, cx),
            _ => {}
        }
        cx.notify();
        true
    }

    /// Moves to the next or previous stop, wrapping round.
    pub(super) fn connect_step(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.connect_ui.stops();
        let at = stops
            .iter()
            .position(|stop| *stop == self.connect_ui.stop)
            .unwrap_or(0) as i32;
        let next = (at + delta).rem_euclid(stops.len() as i32) as usize;
        self.connect_ui.stop = stops[next];
        self.connect_ui.suggest_at = None;
        self.connect_focus(window, cx);
    }

    fn connect_enter_in_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Stop::Field(field) = self.connect_ui.stop {
            let suggestions = self.connect_ui.suggestions(field);
            if let Some(text) = self
                .connect_ui
                .suggest_at
                .and_then(|at| suggestions.get(at).cloned())
            {
                self.connect_ui.suggest_at = None;
                self.connect_set(field, text, window, cx);
                return;
            }
        }
        self.connect_step(1, window, cx);
    }

    fn connect_arrow(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Stop::Field(field) = self.connect_ui.stop else {
            return;
        };
        let count = self.connect_ui.suggestions(field).len();
        if count == 0 {
            self.connect_step(if down { 1 } else { -1 }, window, cx);
            return;
        }
        let at = self.connect_ui.suggest_at;
        self.connect_ui.suggest_at = match (at, down) {
            (None, true) => Some(0),
            (None, false) => Some(count - 1),
            (Some(at), true) => Some((at + 1).min(count - 1)),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
    }

    fn connect_platform(&mut self, forward: bool) {
        self.connect_ui.platform = self.connect_ui.platform.step(forward);
    }

    /// Enter or Space on the stop the keyboard is on.
    pub(super) fn connect_press(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.connect_ui.stop {
            Stop::Test => {
                if self.connect_ui.is_testing() {
                    self.connect_cancel(window, cx);
                } else {
                    self.connect_test(false, window, cx);
                }
            }
            Stop::Save => self.connect_save(window, cx),
            Stop::Trust => self.connect_ask_trust(window, cx),
            Stop::TrustYes => self.connect_trust_yes(window, cx),
            Stop::TrustNo => {
                self.connect_ui.trust = false;
                self.connect_ui.stop = Stop::Trust;
                self.connect_focus(window, cx);
            }
            Stop::Details => self.connect_ui.details = !self.connect_ui.details,
            Stop::Platform => self.connect_platform(true),
            Stop::How(row) => {
                self.connect_ui.open_howto =
                    (self.connect_ui.open_howto != Some(row)).then_some(row);
            }
            Stop::Edit => self.connect_edit(window, cx),
            Stop::AddProject => self.connect_add_project(window, cx),
            Stop::Shell => self.connect_open_shell(window, cx),
            Stop::Done => self.close_connect(window, cx),
            Stop::Field(_) => {}
        }
    }

    // ----- the test ----------------------------------------------------------------------------------

    /// Tests the connection to what the form says; with `then_save`, saves
    /// when it works.
    pub(super) fn connect_test(
        &mut self,
        then_save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connect_ui.phase == Phase::Next {
            return;
        }
        let problems = self.connect_ui.form.problems();
        let host_missing = problems.iter().any(|p| p.field == Field::Host);
        let blocking: Vec<Field> = problems
            .iter()
            .map(|p| p.field)
            .filter(|f| *f != Field::Name || then_save)
            .collect();
        if self.connect_ui.phase == Phase::Form && (host_missing || !blocking.is_empty()) {
            self.connect_ui.shown_problems = true;
            if let Some(first) = blocking
                .first()
                .copied()
                .or(host_missing.then_some(Field::Host))
            {
                self.connect_ui.stop = Stop::Field(first);
                self.connect_focus(window, cx);
            }
            cx.notify();
            return;
        }
        let target = self.connect_ui.form.target();
        let run = self.engine.check_connection(target.clone());
        let ui = &mut self.connect_ui;
        ui.shown_problems = false;
        ui.save_after = then_save;
        ui.trust = false;
        ui.tested = Some(target);
        ui.show_help = false;
        ui.checklist = Some(running_checklist());
        ui.stop = Stop::Test;
        self.connect_ui.testing = Some(cx.spawn(async move |this, cx| {
            let mut run = run;
            while run.updates.changed().await.is_ok() {
                let list = run.updates.borrow().clone();
                let live = this
                    .update(cx, |this, cx| {
                        this.connect_ui.checklist = Some(list);
                        this.connect_ui.follow.set(2);
                        cx.notify();
                    })
                    .is_ok();
                if !live {
                    return;
                }
            }
            let list = run.updates.borrow().clone();
            this.update(cx, |this, cx| {
                this.connect_ui.checklist = Some(list);
                this.connect_ui.follow.set(2);
                this.connect_ui.testing = None;
                this.connect_tested(cx);
            })
            .ok();
        }));
        self.connect_focus(window, cx);
        cx.notify();
    }

    /// A test ended: a machine that works is marked online; one that was
    /// being saved is saved.
    fn connect_tested(&mut self, cx: &mut Context<Self>) {
        let (connected, report) = match &self.connect_ui.checklist {
            Some(list) => (list.connected(), list.report.clone()),
            None => return,
        };
        if connected && self.connect_ui.phase == Phase::Why {
            if let (Some(id), Some(report)) = (self.connect_ui.editing.clone(), report) {
                self.engine.mark_online(&id, report);
            }
        }
        if self.connect_ui.save_after {
            self.connect_ui.save_after = false;
            if connected {
                self.connect_finish_save(cx);
            }
        }
        cx.notify();
    }

    /// Stops the test that is running.
    pub(super) fn connect_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.connect_ui;
        ui.testing = None;
        ui.save_after = false;
        ui.checklist = None;
        ui.tested = None;
        ui.notice = Some("Test cancelled.".to_owned());
        self.connect_focus(window, cx);
        cx.notify();
    }

    // ----- the host key ---------------------------------------------------------------------------

    /// Asks before adding the host keys to `known_hosts`.
    pub(super) fn connect_ask_trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connect_ui.offers_trust() {
            return;
        }
        self.connect_ui.trust = true;
        self.connect_ui.stop = Stop::TrustYes;
        self.connect_focus(window, cx);
        cx.notify();
    }

    /// The person confirmed: the keys the computer showed are appended to
    /// `~/.ssh/known_hosts`, and the test runs again. This is the only write
    /// to `~/.ssh` Leon makes.
    pub(super) fn connect_trust_yes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connect_ui.trust {
            return;
        }
        let lines = self
            .connect_ui
            .checklist
            .as_ref()
            .and_then(|list| list.host_keys.as_ref())
            .map(|keys| keys.lines.clone())
            .unwrap_or_default();
        self.connect_ui.trust = false;
        if lines.is_empty() {
            return;
        }
        match self.options.ssh_dir.append_known_hosts(&lines) {
            Ok(()) => {
                self.connect_ui.notice =
                    Some("Added to ~/.ssh/known_hosts. Testing again.".to_owned());
                self.connect_test(self.connect_ui.save_after, window, cx);
            }
            Err(error) => {
                self.connect_ui.notice = Some(format!("Could not write known_hosts: {error}"));
                self.connect_ui.stop = Stop::Trust;
            }
        }
        cx.notify();
    }

    // ----- saving -----------------------------------------------------------------------------------

    /// Saves the machine: after a test that worked, anyway after one that did
    /// not, and with a test first when there is none.
    pub(super) fn connect_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connect_ui.phase != Phase::Form {
            return;
        }
        if !self.connect_ui.form.problems().is_empty() {
            self.connect_ui.shown_problems = true;
            if let Some(first) = self.connect_ui.form.problems().first() {
                self.connect_ui.stop = Stop::Field(first.field);
                self.connect_focus(window, cx);
            }
            cx.notify();
            return;
        }
        if self.connect_ui.is_testing() {
            self.connect_ui.save_after = true;
            return;
        }
        if self.connect_ui.checklist_is_current() {
            // Tested already: a machine that works is saved, one that did not
            // is saved anyway on the second press.
            self.connect_finish_save(cx);
        } else {
            self.connect_test(true, window, cx);
        }
    }

    fn connect_finish_save(&mut self, cx: &mut Context<Self>) {
        let form = self.connect_ui.form.clone();
        let target = form.target();
        let editing = self.connect_ui.editing.clone();
        let machine = match self
            .engine
            .save_machine(editing.as_ref(), &form.name, &target)
        {
            Ok(machine) => machine,
            Err(error) => {
                self.connect_ui.notice = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        match self
            .connect_ui
            .checklist
            .as_ref()
            .filter(|_| self.connect_ui.checklist_is_current())
            .and_then(|list| list.report.clone())
        {
            Some(report) => self.engine.mark_online(&machine.id, report),
            None => self
                .engine
                .submit(crate::engine::Op::Probe(machine.id.clone())),
        }
        self.engine
            .report(StatusKind::Info, format!("Saved {}.", machine.name));
        self.reload(cx);
        self.show(&NodeId::Machine(machine.id.clone()));
        self.pane = super::shell::Pane::Sidebar;
        self.connect_ui.saved = Some(machine.id.clone());
        self.connect_ui.editing = Some(machine.id.clone());
        self.connect_ui.phase = Phase::Next;
        self.connect_ui.stop = Stop::AddProject;
        self.connect_ui.notice = None;
        self.connect_search(machine.id, form.start_folder(), cx);
        cx.notify();
    }

    /// Looks for repositories on the machine, in the background.
    fn connect_search(&mut self, machine: MachineId, base: Option<String>, cx: &mut Context<Self>) {
        let search = self.engine.find_repositories(machine.clone(), base);
        self.connect_ui.searching = Some(cx.spawn(async move |this, cx| {
            let found = search.finish().await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.connect_ui.repos.insert(machine, found);
                this.connect_ui.searching = None;
                cx.notify();
            })
            .ok();
        }));
    }

    // ----- after saving ------------------------------------------------------------------------------

    /// "Add a project on this machine": the folders found there are offered.
    pub(super) fn connect_add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.connect_ui.saved.clone() else {
            return;
        };
        self.close_connect(window, cx);
        self.begin_flow_with(
            Command::AddProject,
            vec![id.as_str().to_owned()],
            window,
            cx,
        );
    }

    /// "Open a shell" on the machine, in its home folder.
    pub(super) fn connect_open_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.connect_ui.saved.clone() else {
            return;
        };
        let home = match self.engine.machine_state(&id) {
            crate::engine::MachineState::Online(Some(report)) => report.home,
            _ => None,
        }
        .unwrap_or_else(|| "/".to_owned());
        self.close_connect(window, cx);
        self.start_live(
            crate::launch::Launch::Shell,
            &id,
            &home,
            super::terminals::Place::Tab,
            None,
            window,
            cx,
        );
    }

    /// From the explanation of an offline machine to the form.
    fn connect_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.connect_ui.phase = Phase::Form;
        self.connect_ui.stop = Stop::Field(Field::Host);
        self.connect_ui.checklist = None;
        self.connect_ui.tested = None;
        self.connect_focus(window, cx);
    }

    // ----- small things ------------------------------------------------------------------------------

    /// Copies the `ssh-copy-id` line as it is shown.
    pub(super) fn connect_copy_id(&mut self, cx: &mut Context<Self>) {
        let line = self
            .connect_ui
            .form
            .shown_target(&self.connect_ui.login)
            .copy_id_line();
        self.copy_text("ssh-copy-id command", line, cx);
    }

    /// Copies the `ssh` line as it is shown.
    pub(super) fn connect_copy_ssh(&mut self, cx: &mut Context<Self>) {
        let line = self.connect_ui.shown_ssh_line();
        self.copy_text("ssh command", line, cx);
    }

    /// Asks for a key file with the system's dialog.
    pub(super) fn connect_pick_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = (self.options.pick_key)(cx);
        self.connect_ui.picking = Some(cx.spawn_in(window, async move |this, cx| {
            let picked = task.await;
            this.update_in(cx, |this, window, cx| {
                this.connect_ui.picking = None;
                if let Picked::Folder(path) = picked {
                    let text = path.to_string_lossy().into_owned();
                    this.connect_set(Field::Identity, text, window, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The SSH machine the keyboard is on, for the commands that explain or
    /// edit one.
    pub(super) fn ssh_machine_here(&self) -> Option<MachineId> {
        let id = self.current_machine();
        let machine = self.snapshot.machine(&id)?;
        matches!(machine.kind, MachineKind::Ssh { .. }).then_some(id)
    }
}

/// A checklist whose first line is running.
fn running_checklist() -> Checklist {
    let mut list = Checklist::new();
    if let Some(first) = list.checks.first_mut() {
        first.state = leon_remote::connect::CheckState::Running;
    }
    list
}

/// What a field of `form` holds, as text.
fn field_text(form: &Form, field: Field) -> String {
    match field {
        Field::Name => form.name.clone(),
        Field::Host => form.host.clone(),
        Field::User => form.user.clone(),
        Field::Port => form.port.clone(),
        Field::Identity => form.identity.clone(),
        Field::Folder => form.folder.clone(),
    }
}
