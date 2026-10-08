//! The settings in the window: following `settings.json`, the commands that
//! open it, the buttons of the Settings screen and the palette, and what a
//! change of a setting does to the rest of the window.
//!
//! The values themselves live in [`crate::settings`] and the list of what can
//! be set in [`crate::schema`]; this is the part that touches the user and
//! the shell. Like the themes folder, the file is polled (see
//! `theme/watch.rs` for why) and an edit made outside Leon is applied once
//! two reads agree on it.

use super::shell::Shell;
use crate::engine::StatusKind;
use crate::schema::{self, Problem, Value};
use crate::settings::{self, Polled};
use gpui_kit::{Context, Window};

impl Shell {
    /// Lists the settings file; an edit made outside Leon is applied and its
    /// unusable values reported.
    pub(super) fn poll_settings(&mut self, cx: &mut Context<Self>) {
        match settings::poll_file(cx) {
            Polled::Applied(problems) => {
                if problems.is_empty() {
                    self.engine
                        .report(StatusKind::Info, "settings.json changed: applied.");
                } else {
                    self.report_problems(&problems);
                }
                cx.notify();
            }
            Polled::Invalid(error) => {
                self.engine.report(
                    StatusKind::Error,
                    format!("settings.json is not valid ({error}): the settings in use are kept."),
                );
                cx.notify();
            }
            Polled::Unchanged | Polled::Pending => {}
        }
    }

    /// Says what in the file could not be used: the first, and how many more.
    pub(super) fn report_problems(&self, problems: &[Problem]) {
        let Some(first) = problems.first() else {
            return;
        };
        let more = match problems.len() {
            1 => String::new(),
            n => format!(" (and {} more)", n - 1),
        };
        self.engine
            .report(StatusKind::Error, format!("{}{more}", first.line()));
    }

    /// Opens `settings.json` in the system's editor, creating it when there is
    /// none yet.
    pub(super) fn open_settings_file(&mut self, cx: &mut Context<Self>) {
        let Some(file) = settings::file(cx) else {
            self.engine.report(
                StatusKind::Info,
                "There is no settings file: the settings are kept in memory.",
            );
            return;
        };
        if !file.exists() {
            if let Some(folder) = file.parent() {
                let _ = std::fs::create_dir_all(folder);
            }
            if let Err(error) = std::fs::write(&file, b"{}\n") {
                self.engine.report(
                    StatusKind::Error,
                    format!("Could not create {}: {error}.", file.display()),
                );
                return;
            }
        }
        let open = self.options.open_file.clone();
        open(cx, &file);
        self.engine
            .report(StatusKind::Info, format!("Opened {}", file.display()));
    }

    /// Shows the folder that holds the settings in the file manager.
    pub(super) fn reveal_settings_folder(&mut self, cx: &mut Context<Self>) {
        let Some(folder) =
            settings::file(cx).and_then(|file| file.parent().map(std::path::Path::to_path_buf))
        else {
            self.engine.report(
                StatusKind::Info,
                "There is no settings folder: the settings are kept in memory.",
            );
            return;
        };
        let reveal = self.options.reveal.clone();
        reveal(cx, &folder);
        self.engine.report(
            StatusKind::Info,
            format!("Settings folder: {}", folder.display()),
        );
    }

    /// Does what a button of the settings does.
    pub(super) fn run_setting_action(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::keys::Command as C;
        match key {
            "reimport" => self.engine.submit(crate::engine::Op::Refresh),
            "usage_forget_history" => self.engine.submit(crate::engine::Op::ForgetUsageHistory),
            "add_machine" => {
                self.run_command(C::AddMachine, window, cx);
            }
            "share_machine" => {
                self.run_command(C::ShareMachine, window, cx);
            }
            "probe_machine" => {
                self.run_command(C::ProbeMachine, window, cx);
            }
            "about" => {
                self.run_command(C::About, window, cx);
            }
            "check_for_updates" => {
                self.run_command(C::CheckForUpdates, window, cx);
            }
            "open_data_folder" => self.reveal_settings_folder(cx),
            "open_themes_folder" => self.open_themes_folder(cx),
            "reset_all" => {
                settings::reset_all(cx);
                self.engine
                    .report(StatusKind::Info, "Every setting is back to its default.");
            }
            other => tracing::warn!(key = other, "a setting button with no action"),
        }
        cx.notify();
    }

    /// Chooses a value for a setting after checking it against the system:
    /// a font must exist, a program or folder must be there.
    pub(super) fn choose_setting(
        &mut self,
        def: &'static schema::Def,
        value: Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Err(why) = check_value(def, &value, self.options.system.as_ref()) {
            self.engine.report(StatusKind::Error, why);
            cx.notify();
            return false;
        }
        settings::set_value(cx, def, value);
        cx.notify();
        true
    }
}

/// Checks a value against the computer: a font family must be bundled or
/// installed, an executable must be a runnable file or a program the login
/// shell finds, a folder must exist. An empty text is always the default.
pub fn check_value(
    def: &schema::Def,
    value: &Value,
    system: &dyn crate::launch::System,
) -> Result<(), String> {
    let Value::Text(text) = value else {
        return check_list(def, value);
    };
    if text.is_empty() {
        return Ok(());
    }
    match def.key {
        "terminal_font_family" if !crate::theme::user::has_font(text) => Err(format!(
            "There is no font family called {text:?}: use a bundled or installed one."
        )),
        "terminal_shell" if !is_program(text, system) => {
            Err(format!("{text:?} is not a program that can run."))
        }
        key if key.starts_with("agent_") && key.ends_with("_executable") => {
            if is_program(text, system) {
                Ok(())
            } else {
                Err(format!("{text:?} is not a program that can run."))
            }
        }
        key if key.starts_with("history_dir_") && !system.dir_exists(text) => {
            // The opencode setting is a file, not a folder.
            if key == "history_dir_opencode" && std::path::Path::new(text).is_file() {
                Ok(())
            } else {
                Err(format!("{text:?} does not exist."))
            }
        }
        _ => Ok(()),
    }
}

fn check_list(def: &schema::Def, value: &Value) -> Result<(), String> {
    if def.key != "terminal_env" {
        return Ok(());
    }
    let Value::List(items) = value else {
        return Ok(());
    };
    for item in items {
        match item.split_once('=') {
            Some((name, _))
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !name.starts_with(|c: char| c.is_ascii_digit()) => {}
            _ => return Err(format!("{item:?} is not NAME=value.")),
        }
    }
    Ok(())
}

/// Whether `text` names a program: a path to a runnable file, or a name the
/// login shell's search finds.
fn is_program(text: &str, system: &dyn crate::launch::System) -> bool {
    let path = std::path::Path::new(text);
    if path.components().count() > 1 {
        return path.is_file();
    }
    system.find_program(text).is_some()
}

// ----- what a change of a setting does to the window -----------------------------------------------

impl Shell {
    /// Puts what the settings ask on what is already alive: the engine, the
    /// terminals, the tree. Runs when any setting has changed since it last
    /// did (a change made on the screen, in the palette or in the file).
    pub(super) fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let generation = settings::generation(cx);
        if generation == self.applied_settings {
            return;
        }
        self.applied_settings = generation;
        self.sync_share(cx);
        let prefs = settings::engine_prefs(cx, &self.engine);
        if prefs != self.engine.prefs() {
            self.engine.set_prefs(prefs);
        }
        let before = self.engine.usage_policy();
        self.engine.set_usage_policy(settings::usage_policy(cx));
        self.usage_settings_changed(before, cx);
        for session in self.live.all() {
            apply_terminal_prefs(&session.view, cx);
        }
        // The length of the lists of sessions, and whether only the active
        // ones are listed.
        self.active_only = settings::flag(cx, "sidebar_active_only");
        self.rebuild_rows();
        // The Den may have somebody to follow now: sessions elsewhere.
        self.den_watch(cx);
        cx.notify();
    }

    /// The font of terminals: the setting's family when it can be drawn, else
    /// the theme's mono font; the size and the line height of the settings at
    /// the interface size in use.
    pub(super) fn terminal_font(cx: &gpui_kit::App) -> leon_term::FontSettings {
        let family = settings::text(cx, "terminal_font_family");
        let family = if !family.is_empty() && crate::theme::user::has_font(&family) {
            family
        } else {
            crate::theme::fonts::mono().to_owned()
        };
        leon_term::FontSettings {
            family: family.into(),
            size: crate::theme::px(settings::int(cx, "terminal_font_size") as f32),
            line_height: settings::int(cx, "terminal_line_height") as f32 / 100.0,
        }
    }

    /// What starting a terminal changes: the shell, the environment and how
    /// each agent is started.
    pub(super) fn launch_prefs(cx: &gpui_kit::App) -> crate::launch::LaunchPrefs {
        use crate::launch::{split_words, AgentPrefs};
        let shell = settings::text(cx, "terminal_shell");
        let mut prefs = crate::launch::LaunchPrefs {
            shell: (!shell.is_empty()).then(|| {
                (
                    shell,
                    split_words(&settings::text(cx, "terminal_shell_args")),
                )
            }),
            env: settings::list(cx, "terminal_env")
                .iter()
                .filter_map(|entry| entry.split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            ..crate::launch::LaunchPrefs::default()
        };
        // The catalogue's agents; a custom agent's command and arguments are
        // its own spec, so only a program of the user's choice overrides it.
        for spec in leon_core::agent::builtin() {
            let key = |what: &str| crate::schema::agent_setting(spec.id, what);
            let program = settings::text(cx, &key("executable"));
            prefs.agents.insert(
                spec.id,
                AgentPrefs {
                    executable: (!program.is_empty()).then_some(program),
                    args: split_words(&settings::text(cx, &key("args"))),
                    resume_args: if spec.can_resume() {
                        split_words(&settings::text(cx, &key("resume_args")))
                    } else {
                        Vec::new()
                    },
                },
            );
        }
        prefs
    }

    /// What the settings change in the palette's questions.
    pub(super) fn step_prefs(cx: &gpui_kit::App) -> super::steps::Prefs {
        let enabled: Vec<leon_core::AgentId> = leon_core::agent::all()
            .iter()
            .filter(|spec| {
                spec.custom || settings::flag(cx, &crate::schema::agent_setting(spec.id, "enabled"))
            })
            .map(|spec| spec.id)
            .collect();
        super::steps::Prefs {
            agents_enabled: enabled,
            default_agent: leon_core::AgentId::parse(&settings::text(cx, "default_agent")),
            confirm_close: settings::flag(cx, "terminal_confirm_close"),
            quit: super::steps::QuitConfirm::parse(&settings::text(cx, "quit_confirmation")),
        }
    }

    /// What a plain paste does with a clipboard that holds text and an image.
    pub(super) fn paste_mode(cx: &gpui_kit::App) -> super::paste::How {
        match settings::text(cx, "terminal_paste").as_str() {
            "text" => super::paste::How::Text,
            "image" => super::paste::How::ImageFirst,
            _ => super::paste::How::Auto,
        }
    }

    /// Opens a history session the way the settings say: resumed in a
    /// terminal, or its transcript.
    pub(super) fn open_history(
        &mut self,
        session: leon_core::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if settings::text(cx, "history_open") == "transcript" {
            self.open_session(session, None, cx);
        } else {
            self.resume_session(session, window, cx);
        }
    }

    /// How often this computer is looked at for sessions running elsewhere
    /// while the window is focused.
    pub(super) fn elsewhere_interval(cx: &gpui_kit::App) -> std::time::Duration {
        std::time::Duration::from_secs(settings::int(cx, "elsewhere_interval").max(1) as u64)
    }

    /// Whether sessions running in another terminal are looked for: the
    /// engine can scan and the settings do not forbid it.
    pub(super) fn detecting_elsewhere(&self, cx: &gpui_kit::App) -> bool {
        self.engine.scanning() && settings::flag(cx, "detect_elsewhere")
    }

    /// Whether quitting asks a question now.
    pub(super) fn quit_asks(&self, cx: &gpui_kit::App) -> bool {
        super::steps::QuitConfirm::parse(&settings::text(cx, "quit_confirmation"))
            .asks(self.busy_sessions(cx))
    }
}

/// Gives a terminal what the settings ask of it: its scrollback, the shape of
/// its cursor, what Option does and whether a selection is copied.
pub(super) fn apply_terminal_prefs(
    view: &gpui_kit::Entity<leon_term::TerminalView>,
    cx: &mut gpui_kit::App,
) {
    let scrollback = settings::int(cx, "terminal_scrollback").max(0) as usize;
    let shape = match settings::text(cx, "terminal_cursor").as_str() {
        "beam" => leon_term::CursorShape::Beam,
        "underline" => leon_term::CursorShape::Underline,
        _ => leon_term::CursorShape::Block,
    };
    let meta = settings::flag(cx, "terminal_option_as_meta");
    let copy = settings::flag(cx, "terminal_copy_on_select");
    view.update(cx, |view, _| {
        view.terminal().set_scrollback(scrollback);
        view.terminal().set_cursor_shape(shape);
        view.set_option_as_meta(meta);
        view.set_copy_on_select(copy);
    });
}
