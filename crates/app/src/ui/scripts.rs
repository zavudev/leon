//! The project's scripts and its worktree setup, in the window.
//!
//! `leon.toml` (see [`crate::project`]) is read by the engine, through the
//! machine's runner, and the window only reads what the engine kept
//! ([`Engine::project_state`]). It is asked to read when the project in view
//! changes (so a script's shortcut works at once), when **Run a script…** is
//! used and when a worktree is made. A script run by its key uses the file as
//! it was when the project came into view; the palette and a new worktree read
//! it again first.
//!
//! * **Run a script…** reads the file and then asks which script, in the
//!   palette ([`super::steps`]); the command is typed into a new tab of the
//!   worktree in view, as written, into that terminal's own shell. A script
//!   with a `key` also answers its chord, checked after the registry has had
//!   its say (`handle_key`), so a script can never shadow a command:
//!   [`ProjectFile::script_for`] does not answer a key that collides.
//! * **Worktree setup.** When a worktree has been made and listed, the session
//!   that starts in it is started *behind* the project's `setup` command
//!   ([`Shell::start_live_first`]): on a POSIX shell the shell types
//!   `sh -c '<setup>'`, and the agent, when there is one, follows only if the
//!   command succeeded. A file that is wrong or cannot be read means no setup,
//!   and [`setup_notice`] says so.
//! * **Trust.** What the file says is written by whoever can push to the
//!   repository, so a command that is not trusted yet ([`crate::trust`]) is
//!   shown, in full, in a card of its own and nothing runs until the person
//!   answers: run it and remember it, run it once, or not run it. The card can
//!   come from a background task while the person is typing, so no plain key
//!   answers it ([`key_answer`]): the answers are its buttons and two chords
//!   with `Cmd` or `Ctrl`; a plain Enter does nothing. Cards wait their turn
//!   (one at a time, and not over a palette or a menu), and one that asks about
//!   a command remembered meanwhile is not shown. A script that is not run
//!   says so on the status line; a worktree whose setup is not run still gets
//!   its session.

use super::shell::{Overlay, Shell};
use super::steps::{ScriptEntry, ScriptsFile, ScriptsView};
use super::terminals::Place;
use super::tree;
use super::updates_view::pill;
use super::widgets::{mono, section_label};
use crate::engine::StatusKind;
use crate::keys::{self, Command};
use crate::launch::{First, Flavor, Launch};
use crate::project::{ProjectFile, ProjectState};
use crate::theme::{metrics, px, Palette};
use crate::trust::{self, Trusted, Verdict};
use gpui_kit::prelude::*;
use gpui_kit::{div, Context, Div, Keystroke, Stateful, Task, Window};
use leon_core::{MachineId, Project, ProjectId};
use std::collections::VecDeque;
use std::path::PathBuf;

/// What the window keeps about the project files and the commands it was
/// asked to trust.
pub struct ScriptsUi {
    /// The commands the user allowed.
    pub trusted: Trusted,
    /// Where they are kept; `None` when the settings live in memory.
    trusted_file: Option<PathBuf>,
    /// The question on screen, while there is one.
    pub ask: Option<TrustAsk>,
    /// The questions that are waiting for it: one card at a time.
    pub queued: VecDeque<TrustAsk>,
    /// The project whose file was last asked for.
    watched: Option<ProjectId>,
    /// The read that precedes the palette's list of scripts.
    reading: Option<Task<()>>,
}

impl ScriptsUi {
    /// The window's state at the start: the answers of earlier runs.
    pub fn new(file: Option<PathBuf>) -> Self {
        Self {
            trusted: file.as_deref().map(Trusted::load).unwrap_or_default(),
            trusted_file: file,
            ask: None,
            queued: VecDeque::new(),
            watched: None,
            reading: None,
        }
    }
}

/// A command waiting for the person's answer.
pub struct TrustAsk {
    /// The project whose file the command is from.
    pub project: ProjectId,
    /// Its name.
    pub project_name: String,
    /// What the command is: the script's name, or the worktree setup.
    pub what: String,
    /// The command, as the file has it.
    pub command: String,
    /// How it is typed into the terminal (see [`typing_note`]).
    pub note: &'static str,
    /// The folder it would run in.
    pub folder: String,
    /// The machine it would run on.
    pub machine_name: String,
    /// Whether it is a script or a setup.
    first: First,
    /// What happens with the answer.
    next: Next,
    /// The overlay that was open under the question.
    from: Overlay,
}

/// What follows the answer.
enum Next {
    /// Run a script in a new tab.
    Script {
        machine: MachineId,
        cwd: String,
        name: String,
        place: Place,
    },
    /// Start the sessions of new worktrees, each behind the setup: the folder
    /// of each and what starts in it.
    Worktrees {
        machine: MachineId,
        starts: Vec<(String, Launch)>,
    },
}

/// The person's answer to a [`TrustAsk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Run it, and do not ask again for this text.
    Trust,
    /// Run it this time.
    Once,
    /// Do not run it.
    Cancel,
}

/// Where the scripts of the project in view run.
struct ScriptsPlace {
    project: Project,
    machine: MachineId,
    /// The worktree (or the project's folder) in view.
    cwd: String,
}

/// The scripts of `file` as the palette lists them, with what each one's key
/// does on this platform.
pub fn entries(file: &ProjectFile, mac: bool) -> Vec<ScriptEntry> {
    file.scripts
        .iter()
        .map(|script| ScriptEntry {
            name: script.name.clone(),
            command: script.command.clone(),
            icon: script.icon.clone(),
            key: script.bound().map(|key| key.label(mac)),
            key_note: script.key.zip(script.collision()).map(|(key, command)| {
                format!("{} is Leon's {}", key.label(mac), keys::label(command))
            }),
        })
        .collect()
}

/// The person's answer a keystroke gives to the card, if any. The card can
/// appear on its own, from a background task, while the person is typing, so
/// only a chord with `Cmd` (macOS) or `Ctrl` (elsewhere) answers: with Enter it
/// runs the command once, with Shift as well it runs it and remembers it. No
/// plain key answers, so a stray Enter or letter can neither run nor trust a
/// command nobody read.
pub fn key_answer(stroke: &Keystroke, mac: bool) -> Option<Answer> {
    let m = &stroke.modifiers;
    let secondary = if mac {
        m.platform && !m.control
    } else {
        m.control && !m.platform
    };
    if stroke.key != "enter" || !secondary || m.alt {
        return None;
    }
    Some(if m.shift { Answer::Trust } else { Answer::Once })
}

/// The chords of [`key_answer`] as the card tells them.
pub fn chord_hint(mac: bool) -> String {
    let modifier = if mac { "Cmd" } else { "Ctrl" };
    format!(
        "{modifier}+Enter runs it once, {modifier}+Shift+Enter runs it and remembers it, Esc does not run it. A plain Enter or letter does nothing here."
    )
}

/// How a command is typed into a terminal whose shell is `flavor`, for the card.
pub fn typing_note(first: First, flavor: Flavor) -> &'static str {
    match (first, flavor) {
        (First::Setup, Flavor::Posix) => {
            "It runs under sh -c, as one quoted word, so it means the same in every shell."
        }
        (First::Script, Flavor::Posix) => {
            "It is typed as written into the shell of the terminal, so that shell's aliases, functions and history expansion apply."
        }
        _ => "It is typed as written into the shell of the terminal.",
    }
}

/// What to tell when the project's file gave no setup because it is wrong or
/// could not be read; nothing when it has none or was read fine. `state` is
/// what the read just before the sessions started answered.
pub fn setup_notice(state: &Option<ProjectState>) -> Option<String> {
    match state {
        None => Some(format!(
            "Leon could not read {}, so no setup was run.",
            crate::project::FILE_NAME
        )),
        Some(ProjectState::Invalid(error)) => Some(format!("No setup was run: {error}")),
        Some(ProjectState::Absent | ProjectState::Loaded(_)) => None,
    }
}

impl Shell {
    /// Where the scripts of the project in view run, when a project is in
    /// view: the worktree the keyboard is on or the terminal on screen is in.
    fn scripts_place(&self) -> Option<ScriptsPlace> {
        let here = self.here()?;
        let root = tree::workspace_root(&self.snapshot, &here.machine, &here.cwd);
        let (id, _) = tree::detail_of_root(&self.snapshot, &here.machine, &root)?;
        let project = self.snapshot.project(&id)?.project.clone();
        Some(ScriptsPlace {
            machine: project.machine_id.clone(),
            project,
            cwd: root,
        })
    }

    /// Asks the engine to read the file of the project that came into view.
    /// Called on every paint; it asks once per project change. A project on
    /// another machine is read on its own only while that machine is known to
    /// be reachable: a look nobody asked for must not wait on a dead host (the
    /// file is read when the person asks for a script or makes a worktree).
    pub(super) fn watch_project_file(&mut self) {
        let id = self
            .scripts_place()
            .filter(|place| {
                place.machine.is_local()
                    || matches!(
                        self.engine.machine_state(&place.machine),
                        crate::engine::MachineState::Online(_)
                    )
            })
            .map(|place| place.project.id);
        if id == self.scripts.watched {
            return;
        }
        self.scripts.watched = id.clone();
        if let Some(id) = id {
            // The answer is kept by the engine; nobody waits for it here.
            drop(self.engine.read_project_file(id));
        }
    }

    /// The scripts of the project in view, for the palette's question.
    pub(super) fn scripts_view(&self) -> Option<ScriptsView> {
        let place = self.scripts_place()?;
        let file = match self.engine.project_state(&place.project.id) {
            None => ScriptsFile::Unknown,
            Some(ProjectState::Absent) => ScriptsFile::Absent,
            Some(ProjectState::Invalid(error)) => ScriptsFile::Invalid(error.to_string()),
            Some(ProjectState::Loaded(file)) => {
                ScriptsFile::Scripts(entries(&file, crate::platform::is_mac()))
            }
        };
        Some(ScriptsView {
            project: place.project.id,
            project_name: place.project.name,
            machine: place.machine,
            cwd: place.cwd,
            file,
        })
    }

    /// "Run a script…": reads the file again and then asks which script.
    pub(super) fn begin_scripts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self.scripts_place() else {
            self.engine
                .report(StatusKind::Info, "Select a project or a worktree first.");
            return;
        };
        let read = self.engine.read_project_file(place.project.id);
        self.scripts.reading = Some(cx.spawn_in(window, async move |this, cx| {
            // A failed read leaves what was known; the question says so.
            let _ = read.await;
            this.update_in(cx, |this, window, cx| {
                this.begin_flow(Command::RunScript, window, cx)
            })
            .ok();
        }));
    }

    /// The script a key stands for in the project in view, run. `true` when
    /// there was one.
    pub(super) fn run_script_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.overlay != Overlay::None {
            return false;
        }
        let Some(place) = self.scripts_place() else {
            return false;
        };
        let Some(ProjectState::Loaded(file)) = self.engine.project_state(&place.project.id) else {
            return false;
        };
        let Some(script) = file.script_for(stroke, crate::platform::is_mac()) else {
            return false;
        };
        let name = script.name.clone();
        self.run_script(
            &place.project.id,
            &place.machine,
            &place.cwd,
            &name,
            window,
            cx,
        );
        true
    }

    /// Runs the script `name` of the project's file in a new tab of the
    /// worktree `cwd`, after the person trusted its command.
    pub(super) fn run_script(
        &mut self,
        project: &ProjectId,
        machine: &MachineId,
        cwd: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let script = match self.engine.project_state(project) {
            Some(ProjectState::Loaded(file)) => file
                .scripts
                .iter()
                .find(|script| script.name == name)
                .cloned(),
            _ => None,
        };
        let Some(script) = script else {
            self.engine.report(
                StatusKind::Error,
                format!(
                    "There is no script {name:?} in {} any more.",
                    crate::project::FILE_NAME
                ),
            );
            return;
        };
        // A tab of the terminal in view when that is in this worktree, else
        // a session of its own.
        let place = match self.session_here() {
            Some(of)
                if self.live.get(of).is_some_and(|live| {
                    tree::workspace_root(&self.snapshot, machine, &live.cwd)
                        == tree::workspace_root(&self.snapshot, machine, cwd)
                }) =>
            {
                Place::Tab(of)
            }
            _ => Place::Session,
        };
        match trust::verdict(&self.scripts.trusted, project, &script.command) {
            Verdict::Run => self.start_live_first(
                &script.command,
                First::Script,
                Some(script.name),
                Launch::Shell,
                machine,
                cwd,
                place,
                window,
                cx,
            ),
            Verdict::Ask => {
                let ask = self.trust_ask(
                    project,
                    format!("Script \"{}\"", script.name),
                    script.command,
                    First::Script,
                    machine,
                    cwd,
                    cx,
                    Next::Script {
                        machine: machine.clone(),
                        cwd: cwd.to_owned(),
                        name: script.name,
                        place,
                    },
                );
                self.show_trust(ask, window, cx);
            }
        }
    }

    /// Starts the session of a worktree that was just made, behind the
    /// project's setup command when it has one.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_worktree_session(
        &mut self,
        project: &ProjectId,
        machine: &MachineId,
        path: &str,
        launch: Launch,
        state: Option<ProjectState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let starts = vec![(path.to_owned(), launch)];
        if let Some(notice) = setup_notice(&state) {
            self.engine.report(StatusKind::Error, notice);
        }
        self.start_worktree_sessions(project, machine, starts, state, window, cx);
    }

    /// Starts the sessions of several worktrees that were just made, each
    /// behind the project's setup command when it has one. The command is the
    /// same for all of them, so the person is asked about it once.
    pub(super) fn start_worktree_sessions(
        &mut self,
        project: &ProjectId,
        machine: &MachineId,
        starts: Vec<(String, Launch)>,
        state: Option<ProjectState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Nobody is asked here: a new agent session of a new worktree starts
        // as the account the settings name for the agent, else as its own setup.
        let starts: Vec<(String, Launch)> = starts
            .into_iter()
            .map(|(path, mut launch)| {
                if let Launch::Agent {
                    kind,
                    resume: None,
                    account: account @ None,
                } = &mut launch
                {
                    *account = super::terminals::default_account(*kind, cx);
                }
                (path, launch)
            })
            .collect();
        let setup = state
            .as_ref()
            .and_then(ProjectState::file)
            .and_then(|file| file.setup.clone());
        let Some(setup) = setup else {
            for (path, launch) in starts {
                self.start_live(launch, machine, &path, Place::Session, None, window, cx);
            }
            return;
        };
        match trust::verdict(&self.scripts.trusted, project, &setup.command) {
            Verdict::Run => {
                for (path, launch) in starts {
                    self.start_live_first(
                        &setup.command,
                        First::Setup,
                        None,
                        launch,
                        machine,
                        &path,
                        Place::Session,
                        window,
                        cx,
                    );
                }
            }
            Verdict::Ask => {
                let paths: Vec<&str> = starts.iter().map(|(path, _)| path.as_str()).collect();
                let what = match paths.len() {
                    1 => "Setup of a new worktree".to_owned(),
                    count => format!("Setup of {count} new worktrees"),
                };
                let ask = self.trust_ask(
                    project,
                    what,
                    setup.command,
                    First::Setup,
                    machine,
                    &paths.join(", "),
                    cx,
                    Next::Worktrees {
                        machine: machine.clone(),
                        starts,
                    },
                );
                self.show_trust(ask, window, cx);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn trust_ask(
        &self,
        project: &ProjectId,
        what: String,
        command: String,
        first: First,
        machine: &MachineId,
        folder: &str,
        cx: &gpui_kit::App,
        next: Next,
    ) -> TrustAsk {
        TrustAsk {
            project: project.clone(),
            project_name: self
                .snapshot
                .project(project)
                .map_or_else(String::new, |entry| entry.project.name.clone()),
            what,
            command,
            note: typing_note(first, self.shell_flavor_of(machine, cx)),
            folder: folder.to_owned(),
            machine_name: self
                .snapshot
                .machine(machine)
                .map_or_else(String::new, |found| found.name.clone()),
            first,
            next,
            from: Overlay::None,
        }
    }

    /// Puts the question in line and shows it as soon as the screen is free.
    fn show_trust(&mut self, ask: TrustAsk, window: &mut Window, cx: &mut Context<Self>) {
        self.scripts.queued.push_back(ask);
        self.show_queued_trust(window, cx);
    }

    /// Shows the next waiting question when none is on screen and nothing but
    /// the dialog of a new worktree is open: a card never replaces a palette
    /// or a menu, and never another card. A question about a command that was
    /// remembered while it waited is not asked: that command just runs. Called
    /// on every paint, so a card waiting behind a palette shows when it closes.
    pub(super) fn show_queued_trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while self.scripts.ask.is_none() && !self.scripts.queued.is_empty() {
            let free = self.overlay == Overlay::None
                || (self.overlay == Overlay::NewWorktree && self.new_worktree_ui.is_some());
            if !free {
                return;
            }
            let Some(mut ask) = self.scripts.queued.pop_front() else {
                return;
            };
            if trust::verdict(&self.scripts.trusted, &ask.project, &ask.command) == Verdict::Run {
                self.carry_out(ask, true, window, cx);
                continue;
            }
            ask.from = self.overlay;
            self.scripts.ask = Some(ask);
            self.overlay = Overlay::Trust;
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Does what the person answered to the question on screen.
    pub(super) fn answer_trust(
        &mut self,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ask) = self.scripts.ask.take() else {
            return;
        };
        // What was open under the question is open again.
        self.overlay = match ask.from {
            Overlay::NewWorktree if self.new_worktree_ui.is_some() => Overlay::NewWorktree,
            _ => Overlay::None,
        };
        self.focus.focus(window, cx);
        if answer == Answer::Trust && self.scripts.trusted.allow(&ask.project, &ask.command) {
            if let Some(file) = &self.scripts.trusted_file {
                if let Err(error) = self.scripts.trusted.save(file) {
                    tracing::warn!(%error, "could not remember the trusted command");
                }
            }
        }
        self.carry_out(ask, answer != Answer::Cancel, window, cx);
        self.sync_focus(window, cx);
        // The next question, when one was waiting.
        self.show_queued_trust(window, cx);
        cx.notify();
    }

    /// Runs what the question was about when `run`, and says so when not.
    fn carry_out(&mut self, ask: TrustAsk, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let TrustAsk {
            command,
            project_name,
            first,
            next,
            ..
        } = ask;
        match next {
            Next::Script {
                machine,
                cwd,
                name,
                place,
            } => {
                if run {
                    self.start_live_first(
                        &command,
                        first,
                        Some(name),
                        Launch::Shell,
                        &machine,
                        &cwd,
                        place,
                        window,
                        cx,
                    );
                } else {
                    self.engine.report(
                        StatusKind::Info,
                        format!("Did not run \"{name}\": its command was not trusted."),
                    );
                }
            }
            Next::Worktrees { machine, starts } => {
                if !run {
                    // The worktrees are never left without a session.
                    self.engine.report(
                        StatusKind::Info,
                        format!(
                            "Did not run the setup of {project_name}: its command was not trusted."
                        ),
                    );
                }
                for (cwd, launch) in starts {
                    if run {
                        self.start_live_first(
                            &command,
                            first,
                            None,
                            launch,
                            &machine,
                            &cwd,
                            Place::Session,
                            window,
                            cx,
                        );
                    } else {
                        self.start_live(launch, &machine, &cwd, Place::Session, None, window, cx);
                    }
                }
            }
        }
    }

    pub(super) fn trust_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(answer) = key_answer(stroke, crate::platform::is_mac()) {
            self.answer_trust(answer, window, cx);
            return true;
        }
        // A plain Enter is swallowed: it must not open the row under the card
        // either. Escape and the other chords go on their way.
        stroke.key == "enter"
    }

    /// The question: the command in full, where it comes from and where it
    /// would run.
    pub(super) fn render_trust(&self, colours: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(ask) = &self.scripts.ask else {
            return div().id("trust-empty");
        };
        self.card("trust", colours)
            .w(px(620.))
            .max_w(self.viewport.width - px(32.))
            .max_h(self.viewport.height - px(112.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py(px(10.))
                    .border_b_1()
                    .border_color(colours.border)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(section_label("Run a command", colours))
                    .child(
                        div()
                            .debug_selector(|| "trust-what".into())
                            .child(format!("{} of {}", ask.what, ask.project_name)),
                    )
                    .child(
                        div()
                            .text_size(metrics::TEXT_SMALL())
                            .text_color(colours.text_muted)
                            .child(format!(
                                "It comes from {} in the repository and would run in {} on {}.",
                                crate::project::FILE_NAME,
                                ask.folder,
                                ask.machine_name
                            )),
                    ),
            )
            .child(
                div()
                    .id("trust-command-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_4()
                    .py_3()
                    .child(
                        mono(ask.command.clone())
                            .id("trust-command")
                            .debug_selector(|| "trust-command".into())
                            .w_full()
                            .px(px(8.))
                            .py(px(6.))
                            .border_1()
                            .border_color(colours.elevated_border)
                            .bg(colours.surface_2)
                            .font_weight(gpui_kit::FontWeight::NORMAL),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .pb(px(8.))
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.text_muted)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(ask.note)
                    .child("Anyone who can push to the repository can change this file. Leon asks again whenever the command changes.")
                    .child(
                        div()
                            .debug_selector(|| "trust-keys".into())
                            .child(chord_hint(crate::platform::is_mac())),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py(px(8.))
                    .border_t_1()
                    .border_color(colours.border)
                    .flex()
                    .gap(px(8.))
                    .child(pill("trust-always", "RUN AND TRUST", colours).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.answer_trust(Answer::Trust, window, cx)
                        }),
                    ))
                    .child(pill("trust-once", "RUN ONCE", colours).on_click(cx.listener(
                        |this, _, window, cx| this.answer_trust(Answer::Once, window, cx),
                    )))
                    .child(pill("trust-cancel", "CANCEL", colours).on_click(cx.listener(
                        |this, _, window, cx| this.answer_trust(Answer::Cancel, window, cx),
                    ))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::parse;

    fn stroke(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    #[test]
    fn only_a_chord_with_the_main_modifier_answers_the_card() {
        for mac in [false, true] {
            let main = if mac { "cmd" } else { "ctrl" };
            let other = if mac { "ctrl" } else { "cmd" };
            assert_eq!(
                key_answer(&stroke(&format!("{main}-enter")), mac),
                Some(Answer::Once)
            );
            assert_eq!(
                key_answer(&stroke(&format!("{main}-shift-enter")), mac),
                Some(Answer::Trust)
            );
            // Not the other platform's modifier, not with Alt, and no plain key.
            for no in [
                format!("{other}-enter"),
                format!("{main}-alt-enter"),
                format!("{main}-o"),
                "enter".to_owned(),
                "shift-enter".to_owned(),
                "o".to_owned(),
                "y".to_owned(),
                "space".to_owned(),
                "escape".to_owned(),
            ] {
                assert_eq!(key_answer(&stroke(&no), mac), None, "{no} on mac={mac}");
            }
        }
    }

    #[test]
    fn the_card_tells_its_chords_and_how_the_command_is_typed() {
        assert!(chord_hint(false).contains("Ctrl+Shift+Enter runs it and remembers it"));
        assert!(chord_hint(true).contains("Cmd+Enter runs it once"));
        assert!(typing_note(First::Setup, Flavor::Posix).contains("sh -c"));
        assert!(typing_note(First::Script, Flavor::Posix).contains("aliases"));
        assert!(typing_note(First::Script, Flavor::PowerShell).contains("as written"));
    }

    #[test]
    fn a_wrong_or_unreadable_file_says_why_no_setup_ran() {
        let wrong = parse("[worktree]\nrun = \"x\"\n").unwrap_err();
        let notice = setup_notice(&Some(ProjectState::Invalid(wrong))).unwrap();
        assert!(
            notice.starts_with("No setup was run: leon.toml line 2"),
            "{notice}"
        );
        assert!(setup_notice(&None).unwrap().contains("could not read"));
        assert_eq!(setup_notice(&Some(ProjectState::Absent)), None);
        let fine = parse("[worktree]\nsetup = \"make\"\n").unwrap();
        assert_eq!(
            setup_notice(&Some(ProjectState::Loaded(std::sync::Arc::new(fine)))),
            None
        );
    }

    #[test]
    fn the_palette_lists_each_script_with_the_key_that_works() {
        let file = parse(
            "[[script]]\nname = \"Test\"\ncommand = \"cargo test\"\nicon = \"terminal\"\nkey = \"mod+shift+alt+f9\"\n\n[[script]]\nname = \"Shell\"\ncommand = \"bash\"\nkey = \"mod+shift+t\"\n\n[[script]]\nname = \"Plain\"\ncommand = \"make\"\n",
        )
        .unwrap();
        let listed = entries(&file, false);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].key.as_deref(), Some("Ctrl+Shift+Alt+F9"));
        assert_eq!(listed[0].key_note, None);
        assert_eq!(listed[0].icon.as_deref(), Some("terminal"));
        assert_eq!(listed[1].key, None, "a colliding key is not offered");
        assert!(
            listed[1]
                .key_note
                .as_deref()
                .is_some_and(|note| note.contains("Ctrl+Shift+T")),
            "{:?}",
            listed[1].key_note
        );
        assert_eq!(
            (listed[2].key.as_deref(), listed[2].key_note.as_deref()),
            (None, None)
        );
        let mac = entries(&file, true);
        assert!(mac[0].key.as_deref().is_some_and(|key| key.contains('⌘')));
    }
}
