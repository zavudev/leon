//! The user's preferences, kept across restarts.
//!
//! One small JSON file, `settings.json`, in the application's data directory
//! (`--data-dir`, or the platform's, see
//! [`product::data_dir`](crate::product::data_dir)). It holds nothing secret
//! and nothing the store owns, so it sits next to the database rather than
//! inside it.
//!
//! A missing or unreadable file means the defaults. A write that fails is
//! logged and the choice still applies for the session.
//!
//! Two choices make the look: the *theme* (`theme_id`, which brand: Leon, Zavu
//! and the rest of [`ThemeId::ALL`]) and the *appearance* (`theme`: light,
//! dark or the desktop's). They are independent. The file's `theme` key kept
//! its name from before themes existed, so an old file still reads; a file
//! without `theme_id`, or with one this build does not know, wears the default
//! theme.
//!
//! A choice can also be *previewed*: shown for as long as the palette's
//! question is open and never saved. The saved choice is not touched, so
//! closing the question restores exactly what was there.

use crate::schema::{self, Problem, Store, Value};
use crate::theme::{self, Appearance, ThemeId};
use gpui_kit::{App, Global, WindowAppearance};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name inside the data directory.
pub const FILE_NAME: &str = "settings.json";

/// Light, dark or the desktop's: how the theme is worn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceChoice {
    /// Always light.
    Light,
    /// Always dark: the brand's default.
    #[default]
    Dark,
    /// Whatever the desktop is set to.
    System,
}

impl AppearanceChoice {
    /// Every choice, in the order they are listed.
    pub const ALL: [AppearanceChoice; 3] = [Self::System, Self::Light, Self::Dark];

    /// Reads `light`, `dark` or `system`, in any case.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::System => "System",
        }
    }
}

/// The settings the look of the window is made of, read from the file's
/// store; every other setting is read by key (see [`value`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Light, dark or the desktop's.
    pub theme: AppearanceChoice,
    /// Which theme: Leon, Zavu...
    pub theme_id: ThemeId,
    /// The size of the whole interface, in percent of the design: one of
    /// [`theme::SCALE_STEPS`].
    pub interface_scale: u16,
    /// Whether the sidebar is showing.
    pub sidebar_visible: bool,
    /// The sidebar's width, in pixels at the design size: between
    /// [`theme::SIDEBAR_MIN`] and [`theme::SIDEBAR_MAX`].
    pub sidebar_width: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_store(&Store::new(), false)
    }
}

impl Settings {
    /// The window's settings as the store holds them. `note` records a theme
    /// id nothing answers to, so that the window can say so once.
    fn from_store(store: &Store, note: bool) -> Self {
        let text = |key: &str| store.value(key).as_text().unwrap_or_default().to_owned();
        let int = |key: &str| store.value(key).as_int().unwrap_or_default();
        let id = text("theme_id");
        Self {
            theme: AppearanceChoice::parse(&text("theme")).unwrap_or_default(),
            theme_id: ThemeId::parse(&id).unwrap_or_else(|| {
                if note {
                    theme::registry::note_missing(&id);
                }
                ThemeId::DEFAULT
            }),
            interface_scale: theme::nearest_step(int("interface_scale") as u16),
            sidebar_visible: store.value("sidebar_visible").as_bool().unwrap_or(true),
            sidebar_width: theme::clamp_sidebar(int("sidebar_width") as u16),
        }
    }

    /// Writes these five keys into a store.
    fn write_into(&self, store: &mut Store) {
        let set = |store: &mut Store, key: &str, value: Value| {
            if let Some(def) = schema::find(key) {
                store.set(def, value);
            }
        };
        let appearance = match self.theme {
            AppearanceChoice::Light => "light",
            AppearanceChoice::Dark => "dark",
            AppearanceChoice::System => "system",
        };
        set(store, "theme", Value::Text(appearance.to_owned()));
        set(
            store,
            "theme_id",
            Value::Text(self.theme_id.slug().to_owned()),
        );
        set(
            store,
            "interface_scale",
            Value::Int(i64::from(self.interface_scale)),
        );
        set(store, "sidebar_visible", Value::Bool(self.sidebar_visible));
        set(
            store,
            "sidebar_width",
            Value::Int(i64::from(self.sidebar_width)),
        );
    }

    /// Reads the file, falling back to the defaults for whatever is missing,
    /// invalid or unreadable.
    #[cfg(test)]
    pub fn load(path: &Path) -> Self {
        Self::from_store(&read_store(path).0, true)
    }

    /// Writes the file whole, through a temporary one.
    #[cfg(test)]
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut store = Store::new();
        self.write_into(&mut store);
        write_atomic(path, &store.to_bytes())
    }
}

/// Reads the settings file: the store, the problems of its values, and its
/// bytes. A missing or unreadable file is an empty store.
fn read_store(path: &Path) -> (Store, Vec<Problem>, Option<Vec<u8>>) {
    match std::fs::read(path) {
        Ok(bytes) => match Store::parse(&bytes) {
            Ok(store) => {
                let problems = store.problems();
                (store, problems, Some(bytes))
            }
            Err(error) => {
                tracing::warn!(%error, "the settings file is unreadable; using the defaults");
                let problem = Problem {
                    key: String::new(),
                    message: format!("is unreadable ({error}); using the defaults."),
                };
                (Store::new(), vec![problem], Some(bytes))
            }
        },
        Err(_) => (Store::new(), Vec::new(), None),
    }
}

/// Writes a file whole, through a temporary one, so a crash midway never
/// leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let draft = path.with_extension("json.tmp");
    std::fs::write(&draft, bytes)?;
    std::fs::rename(&draft, path)
}

/// The settings in use, and where they are kept.
struct Active {
    /// Everything the file holds.
    store: Store,
    /// The window's settings, read from `store`.
    values: Settings,
    /// `None` keeps them in memory only (tests).
    path: Option<PathBuf>,
    /// `--theme`: wins over the saved appearance until the user picks one.
    theme_override: Option<AppearanceChoice>,
    /// `--theme-name`: wins over the saved theme until the user picks one.
    name_override: Option<ThemeId>,
    /// What the palette is showing for now, over everything else.
    preview: Preview,
    /// Values of the file that could not be used, not yet reported.
    problems: Vec<Problem>,
    /// Counts every change of any setting: what reacts to settings looks at
    /// it.
    generation: u64,
    /// The bytes of the file as last read or written by this process.
    seen: Option<Vec<u8>>,
    /// Bytes read from the file that differ from `seen`, until two reads
    /// agree on them.
    pending: Option<Vec<u8>>,
    /// The custom agents this store put in the catalogue, so that a change
    /// takes out only what it put there.
    registered: std::sync::Mutex<Vec<leon_core::AgentId>>,
}

/// A choice shown without being kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// A theme to wear.
    pub theme: Option<ThemeId>,
    /// An appearance to wear.
    pub appearance: Option<AppearanceChoice>,
}

impl Active {
    fn new(store: Store, path: Option<PathBuf>) -> Self {
        let values = Settings::from_store(&store, false);
        Self {
            store,
            values,
            path,
            theme_override: None,
            name_override: None,
            preview: Preview::default(),
            problems: Vec::new(),
            generation: 0,
            seen: None,
            pending: None,
            registered: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Global for Active {}

/// What the command line asked for this run, over the saved choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// `--theme`: the appearance.
    pub appearance: Option<AppearanceChoice>,
    /// `--theme-name`: the theme.
    pub theme: Option<ThemeId>,
}

/// Loads the settings and applies them. Call once, before the first window
/// opens, so the first paint wears the right theme. `path` is the settings
/// file; `None` keeps them in memory.
pub fn init(path: Option<PathBuf>, overrides: Overrides, cx: &mut App) {
    let (store, problems, seen) = match path.as_deref() {
        Some(path) => read_store(path),
        None => (Store::new(), Vec::new(), None),
    };
    let mut active = Active::new(store, path);
    active.values = Settings::from_store(&active.store, true);
    active.problems = problems;
    active.seen = seen;
    active.theme_override = overrides.appearance;
    active.name_override = overrides.theme;
    apply_look(&active);
    cx.set_global(active);
    apply_theme(cx);
}

/// Tells the catalogue which custom agents the settings hold.
fn sync_custom_agents(active: &Active) {
    let entries = active
        .store
        .value("custom_agents")
        .as_list()
        .map(<[String]>::to_vec)
        .unwrap_or_default();
    let specs = leon_core::agent::custom_from_entries(&entries);
    let Ok(mut registered) = active.registered.lock() else {
        return;
    };
    for id in registered
        .iter()
        .filter(|id| !specs.iter().any(|s| s.id == **id))
    {
        leon_core::agent::unregister_custom(*id);
    }
    *registered = specs.iter().map(|spec| spec.id).collect();
    for spec in specs {
        leon_core::agent::register_custom(spec);
    }
}

/// Adds an agent of the user's: any command line tool. The name and the
/// command are checked here, and the agent is offered everywhere the built-in
/// ones are. Returns the new agent's id.
pub fn add_custom_agent(
    cx: &mut App,
    name: &str,
    command: &str,
    args: &str,
    resume_args: &str,
) -> Result<leon_core::AgentId, leon_core::agent::CustomError> {
    let taken: Vec<leon_core::AgentId> = leon_core::agent::all().iter().map(|s| s.id).collect();
    let agent = leon_core::CustomAgent::new(name, command, args, resume_args, &taken)?;
    let names: Vec<String> = leon_core::agent::all()
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    let others: Vec<&str> = names.iter().map(String::as_str).collect();
    let spec = agent.to_spec(&others)?;
    let mut entries = list(cx, "custom_agents");
    entries.push(serde_json::to_string(&agent).unwrap_or_default());
    if let Some(def) = schema::find("custom_agents") {
        set_value(cx, def, Value::List(entries));
    }
    Ok(spec.id)
}

/// Removes a custom agent of the user's.
pub fn remove_custom_agent(cx: &mut App, id: leon_core::AgentId) {
    let entries: Vec<String> = list(cx, "custom_agents")
        .into_iter()
        .filter(|entry| {
            serde_json::from_str::<leon_core::CustomAgent>(entry)
                .map_or(true, |agent| agent.id != id.as_str())
        })
        .collect();
    if let Some(def) = schema::find("custom_agents") {
        set_value(cx, def, Value::List(entries));
    }
}

thread_local! {
    /// How many sessions a worktree lists before "show more". Per thread, like
    /// the interface size: the tree is built on the interface thread.
    static SESSIONS_SHOWN: std::cell::Cell<usize> =
        const { std::cell::Cell::new(crate::ui::DEFAULT_SESSIONS_SHOWN) };
}

/// How many sessions a worktree lists before "show more".
pub fn sessions_shown() -> usize {
    SESSIONS_SHOWN.with(std::cell::Cell::get)
}

/// What the settings change that is global to the interface thread: the
/// interface size, the sidebar, the blueprint lines and the length of the
/// tree's lists.
fn apply_look(active: &Active) {
    sync_custom_agents(active);
    let shown = active
        .store
        .value("sessions_per_worktree")
        .as_int()
        .unwrap_or(8);
    SESSIONS_SHOWN.with(|cell| cell.set(shown.max(1) as usize));
    theme::set_scale(active.values.interface_scale);
    theme::set_sidebar(active.values.sidebar_width, active.values.sidebar_visible);
    let lines = match active.store.value("blueprint_lines").as_text() {
        Some("on") => theme::LinesMode::On,
        Some("off") => theme::LinesMode::Off,
        _ => theme::LinesMode::Theme,
    };
    theme::set_lines_mode(lines);
    crate::logging::set_level(active.store.value("log_level").as_text().unwrap_or("info"));
}

/// A file kept next to the settings file, by name. `None` when the settings
/// live in memory only.
pub fn sibling(name: &str, cx: &App) -> Option<PathBuf> {
    cx.try_global::<Active>()
        .and_then(|active| active.path.as_ref())
        .map(|path| path.with_file_name(name))
}

/// The settings file itself. `None` when the settings live in memory only.
pub fn file(cx: &App) -> Option<PathBuf> {
    cx.try_global::<Active>()
        .and_then(|active| active.path.clone())
}

/// The window's settings in use.
pub fn get(cx: &App) -> Settings {
    cx.try_global::<Active>()
        .map(|active| active.values)
        .unwrap_or_default()
}

/// The value of the setting with this key: what was chosen, else its default.
pub fn value(cx: &App, key: &str) -> Value {
    match cx.try_global::<Active>() {
        Some(active) => active.store.value(key),
        None => Store::new().value(key),
    }
}

/// The value of the setting with this key; `None` for a button.
pub fn value_opt(cx: &App, key: &str) -> Option<Value> {
    let def = schema::find(key)?;
    match cx.try_global::<Active>() {
        Some(active) => active.store.get(def),
        None => Store::new().get(def),
    }
}

/// A toggle's value.
pub fn flag(cx: &App, key: &str) -> bool {
    value(cx, key).as_bool().unwrap_or_default()
}

/// A number's value.
pub fn int(cx: &App, key: &str) -> i64 {
    value(cx, key).as_int().unwrap_or_default()
}

/// A text, path or choice's value.
pub fn text(cx: &App, key: &str) -> String {
    value(cx, key).as_text().unwrap_or_default().to_owned()
}

/// A list's value.
pub fn list(cx: &App, key: &str) -> Vec<String> {
    value(cx, key)
        .as_list()
        .map(<[String]>::to_vec)
        .unwrap_or_default()
}

/// Whether a setting differs from its default.
pub fn is_modified(cx: &App, def: &schema::Def) -> bool {
    match cx.try_global::<Active>() {
        Some(active) => active.store.is_modified(def),
        None => false,
    }
}

/// Whether the interface should stay still: the "Reduce motion" setting, which
/// by default follows the desktop's preference (the toolkit reads it into
/// [`App::reduce_motion`]).
pub fn reduce_motion(cx: &App) -> bool {
    match text(cx, "reduce_motion").as_str() {
        "on" => true,
        "off" => false,
        _ => cx.reduce_motion(),
    }
}

/// How many times any setting has changed. What reacts to settings
/// compares it with the count it last saw.
pub fn generation(cx: &App) -> u64 {
    cx.try_global::<Active>()
        .map_or(0, |active| active.generation)
}

/// The values of the file that could not be used, once: they are forgotten
/// by this call.
pub fn take_problems(cx: &mut App) -> Vec<Problem> {
    if !cx.has_global::<Active>() {
        return Vec::new();
    }
    std::mem::take(&mut cx.global_mut::<Active>().problems)
}

/// Applies a change to the store: saves it when it changed, applies what is
/// global (theme, size, sidebar, lines) and counts the change.
fn commit(cx: &mut App, change: impl FnOnce(&mut Active)) {
    commit_with(cx, true, change);
}

/// [`commit`], writing the file only when `persist`: a change that came from
/// the file itself is not written back.
fn commit_with(cx: &mut App, persist: bool, change: impl FnOnce(&mut Active)) {
    if !cx.has_global::<Active>() {
        cx.set_global(Active::new(Store::new(), None));
    }
    let active = cx.global_mut::<Active>();
    let before = active.store.clone();
    change(active);
    active.values = Settings::from_store(&active.store, false);
    if active.store != before {
        active.generation += 1;
        let bytes = active.store.to_bytes();
        let target = if persist { active.path.clone() } else { None };
        if let Some(path) = &target {
            match write_atomic(path, &bytes) {
                Ok(()) => active.seen = Some(bytes),
                Err(error) => tracing::warn!(%error, "the settings could not be saved"),
            }
        }
    }
    apply_look(active);
    apply_theme(cx);
}

/// Chooses a value for a setting: kept, saved and applied at once.
pub fn set_value(cx: &mut App, def: &schema::Def, value: Value) {
    commit(cx, |active| active.store.set(def, value));
}

/// Puts a setting back to its default.
pub fn reset_value(cx: &mut App, def: &schema::Def) {
    commit(cx, |active| active.store.reset(def));
}

/// Puts every setting back to its default, unknown keys included.
pub fn reset_all(cx: &mut App) {
    commit(cx, |active| {
        active.store.reset_all();
        active.theme_override = None;
        active.name_override = None;
    });
}

/// What reading the settings file again found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Polled {
    /// Nothing new.
    Unchanged,
    /// The file changed; it is applied once two reads agree on it.
    Pending,
    /// The file was applied, with the values that could not be used.
    Applied(Vec<Problem>),
    /// The file is not usable as a whole: what was in force stays.
    Invalid(String),
}

/// Looks at the settings file: an edit made outside Leon is applied when two
/// consecutive reads agree on it (an editor may write in steps), and a file
/// that is not valid JSON leaves what is in force alone.
pub fn poll_file(cx: &mut App) -> Polled {
    let Some(active) = cx.try_global::<Active>() else {
        return Polled::Unchanged;
    };
    let Some(path) = active.path.clone() else {
        return Polled::Unchanged;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Polled::Unchanged;
    };
    if active.seen.as_deref() == Some(bytes.as_slice()) {
        cx.global_mut::<Active>().pending = None;
        return Polled::Unchanged;
    }
    if active.pending.as_deref() != Some(bytes.as_slice()) {
        cx.global_mut::<Active>().pending = Some(bytes);
        return Polled::Pending;
    }
    cx.global_mut::<Active>().pending = None;
    match Store::parse(&bytes) {
        Ok(store) => {
            let problems = store.problems();
            commit_with(cx, false, |active| active.store = store);
            // Whatever the file says is now what is in force and on disk.
            cx.global_mut::<Active>().seen = Some(bytes);
            Polled::Applied(problems)
        }
        Err(error) => {
            cx.global_mut::<Active>().seen = Some(bytes);
            Polled::Invalid(error)
        }
    }
}

/// What the settings ask of `engine`: how it calls SSH, where each agent's
/// history is (the setting's folder, else the engine's default) and which
/// background jobs run.
pub fn engine_prefs(cx: &App, engine: &crate::engine::Engine) -> crate::engine::Prefs {
    let mut roots = engine.default_roots();
    let folder = |key: &str| Some(text(cx, key)).filter(|folder| !folder.is_empty());
    if let Some(folder) = folder("history_dir_claude") {
        roots.claude_projects = Some(folder.into());
    }
    if let Some(folder) = folder("history_dir_codex") {
        roots.codex_sessions = Some(folder.into());
    }
    if let Some(file) = folder("history_dir_opencode") {
        roots.opencode_db = Some(file.into());
    }
    let seconds = int(cx, "ssh_connect_timeout");
    crate::engine::Prefs {
        ssh_multiplex: flag(cx, "ssh_multiplex"),
        ssh_persist_minutes: int(cx, "ssh_persist_minutes").max(1) as u32,
        ssh_connect_timeout: (seconds > 0).then_some(seconds as u32),
        roots,
        discover_projects: flag(cx, "discover_projects"),
        detect_logos: flag(cx, "detect_logos"),
        fetch_avatars: flag(cx, "fetch_avatars"),
    }
}

/// Which sources of the usage limits the settings switched on.
pub fn usage_policy(cx: &App) -> leon_usage::network::NetworkPolicy {
    let mut policy = leon_usage::network::NetworkPolicy::none();
    for agent in leon_usage::network::switchable_agents() {
        policy.set(agent, flag(cx, &crate::schema::usage_setting(agent, true)));
    }
    policy
}

/// Where a limit turns to a warning and to critical, from the settings. The
/// critical level is never below the warning one.
pub fn usage_thresholds(cx: &App) -> leon_usage::Thresholds {
    let warning = int(cx, "usage_warn") as f64;
    leon_usage::Thresholds {
        warning,
        critical: (int(cx, "usage_critical") as f64).max(warning),
    }
}

/// The agents whose limits the settings show.
pub fn usage_agents(cx: &App) -> Vec<leon_core::AgentId> {
    leon_usage::network::switchable_agents()
        .into_iter()
        .filter(|agent| flag(cx, &crate::schema::usage_setting(*agent, false)))
        .collect()
}

/// Tells the engine what the settings ask and, unless they say not to, brings
/// the store up to date: what happens when the application starts.
pub fn start_engine(cx: &App, engine: &crate::engine::Engine) {
    engine.set_prefs(engine_prefs(cx, engine));
    engine.set_usage_policy(usage_policy(cx));
    // Usage limits are not read here: the window starts them once it is up
    // (`Engine::start_usage`), so that the first keychain prompt, if any,
    // never comes before there is a window to read it against.
    if flag(cx, "import_on_start") {
        engine.submit(crate::engine::Op::Refresh);
    }
}

/// How many seconds between two scheduled readings of the usage limits.
pub fn usage_interval_seconds(cx: &App) -> i64 {
    int(cx, "usage_refresh_seconds").max(30)
}

/// Changes the window's settings, saves them and applies what changed.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    commit(cx, |active| {
        let mut values = active.values;
        change(&mut values);
        values.write_into(&mut active.store);
    });
}

/// Chooses the appearance. Unlike [`update`], it always ends the command line's say,
/// even when the choice equals the saved one.
pub fn set_appearance(cx: &mut App, choice: AppearanceChoice) {
    if cx.has_global::<Active>() {
        let active = cx.global_mut::<Active>();
        active.theme_override = None;
        active.preview.appearance = None;
    }
    update(cx, |settings| settings.theme = choice);
}

/// Chooses the theme (Leon, Zavu...). Like [`set_appearance`], it ends the command
/// line's say and any preview, and is saved.
pub fn set_theme_id(cx: &mut App, id: ThemeId) {
    if cx.has_global::<Active>() {
        let active = cx.global_mut::<Active>();
        active.name_override = None;
        active.preview.theme = None;
    }
    update(cx, |settings| settings.theme_id = id);
}

/// Shows `preview` over the saved choices, without saving it. An empty preview
/// shows the saved choices again.
pub fn preview(cx: &mut App, preview: Preview) {
    if !cx.has_global::<Active>() {
        cx.set_global(Active::new(Store::new(), None));
    }
    let active = cx.global_mut::<Active>();
    if active.preview == preview {
        return;
    }
    active.preview = preview;
    apply_theme(cx);
}

/// Ends any preview, showing the saved choices again.
pub fn clear_preview(cx: &mut App) {
    if cx
        .try_global::<Active>()
        .is_some_and(|active| active.preview != Preview::default())
    {
        preview(cx, Preview::default());
    }
}

/// The theme the settings ask for right now, a preview included.
pub fn theme_id(cx: &App) -> ThemeId {
    cx.try_global::<Active>()
        .map(|active| {
            active
                .preview
                .theme
                .or(active.name_override)
                .unwrap_or(active.values.theme_id)
        })
        .unwrap_or_default()
}

/// The theme that is kept: the command line's or the saved one, never a
/// preview. What a cycle chord steps from, and what a cancelled question
/// restores.
pub fn kept_theme_id(cx: &App) -> ThemeId {
    cx.try_global::<Active>()
        .map(|active| active.name_override.unwrap_or(active.values.theme_id))
        .unwrap_or_default()
}

/// The appearance the settings ask for right now, a preview included.
pub fn appearance(cx: &App) -> Appearance {
    let choice = cx
        .try_global::<Active>()
        .map(|active| {
            active
                .preview
                .appearance
                .or(active.theme_override)
                .unwrap_or(active.values.theme)
        })
        .unwrap_or_default();
    match choice {
        AppearanceChoice::Light => Appearance::Light,
        AppearanceChoice::Dark => Appearance::Dark,
        AppearanceChoice::System => match cx.window_appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
        },
    }
}

/// Puts what the settings ask for on screen again: after a theme file changed.
pub fn reapply(cx: &mut App) {
    apply_theme(cx);
}

/// Puts the chosen appearance on screen.
fn apply_theme(cx: &mut App) {
    theme::apply(theme_id(cx), appearance(cx), cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[test]
    fn the_default_theme_is_dark_as_the_brand_asks() {
        assert_eq!(Settings::default().theme, AppearanceChoice::Dark);
        assert_eq!(Settings::default().interface_scale, 100);
    }

    #[test]
    fn the_theme_flag_values_are_read_in_any_case() {
        assert_eq!(
            AppearanceChoice::parse("Light"),
            Some(AppearanceChoice::Light)
        );
        assert_eq!(
            AppearanceChoice::parse(" DARK "),
            Some(AppearanceChoice::Dark)
        );
        assert_eq!(
            AppearanceChoice::parse("system"),
            Some(AppearanceChoice::System)
        );
        assert_eq!(AppearanceChoice::parse("solarized"), None);
    }

    #[test]
    fn settings_survive_a_round_trip_through_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("deep").join(FILE_NAME);
        let settings = Settings {
            theme: AppearanceChoice::Light,
            theme_id: ThemeId::Zavu,
            interface_scale: 110,
            sidebar_visible: false,
            sidebar_width: 400,
        };
        settings.save(&file).unwrap();
        assert_eq!(Settings::load(&file), settings);
    }

    #[test]
    fn an_old_file_without_the_sidebar_opens_it_at_the_default_width() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(
            &file,
            r#"{"theme":"dark","theme_id":"leon","interface_scale":100}"#,
        )
        .unwrap();
        let loaded = Settings::load(&file);
        assert!(loaded.sidebar_visible);
        assert_eq!(loaded.sidebar_width, theme::SIDEBAR_DEFAULT);
    }

    #[test]
    fn the_sidebar_width_is_kept_between_its_limits_when_read() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, r#"{"sidebar_width":5}"#).unwrap();
        assert_eq!(Settings::load(&file).sidebar_width, theme::SIDEBAR_MIN);
        std::fs::write(&file, r#"{"sidebar_width":9000}"#).unwrap();
        assert_eq!(Settings::load(&file).sidebar_width, theme::SIDEBAR_MAX);
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            Settings::load(&directory.path().join(FILE_NAME)),
            Settings::default()
        );
    }

    #[test]
    fn an_unreadable_file_gives_the_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, b"{ nope").unwrap();
        assert_eq!(Settings::load(&file), Settings::default());
    }

    #[test]
    fn a_scale_that_is_not_a_step_snaps_to_the_nearest_one() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"interface_scale": 104}"#).unwrap();
        assert_eq!(Settings::load(&file).interface_scale, 100);
    }

    #[test]
    fn fields_missing_from_an_older_file_take_their_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"theme": "light"}"#).unwrap();
        let settings = Settings::load(&file);
        assert_eq!(settings.theme, AppearanceChoice::Light);
        assert_eq!(settings.interface_scale, 100);
    }

    #[test]
    fn a_file_from_before_themes_existed_wears_the_default_theme_and_keeps_its_appearance() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"theme": "light", "interface_scale": 110}"#).unwrap();
        let settings = Settings::load(&file);
        assert_eq!(settings.theme_id, ThemeId::DEFAULT);
        assert_eq!(settings.theme, AppearanceChoice::Light);
        assert_eq!(settings.interface_scale, 110);
    }

    #[test]
    fn an_unknown_theme_id_in_the_file_falls_back_to_the_default_without_losing_the_rest() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(
            &file,
            br#"{"theme": "light", "theme_id": "from-the-future", "interface_scale": 90}"#,
        )
        .unwrap();
        let settings = Settings::load(&file);
        assert_eq!(settings.theme_id, ThemeId::DEFAULT);
        assert_eq!(settings.theme, AppearanceChoice::Light);
        assert_eq!(settings.interface_scale, 90);
    }

    #[test]
    fn a_fresh_install_is_leon_in_the_dark() {
        assert_eq!(Settings::default().theme_id, ThemeId::Leon);
        assert_eq!(Settings::default().theme, AppearanceChoice::Dark);
    }

    #[gpui_kit::test]
    fn the_theme_flag_wins_until_a_pick_and_a_pick_is_saved(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(
                Some(file.clone()),
                Overrides {
                    appearance: None,
                    theme: Some(ThemeId::Zavu),
                },
                cx,
            );
            assert_eq!(theme_id(cx), ThemeId::Zavu, "the flag has the say");
            assert_eq!(theme::current(cx), ThemeId::Zavu);
            assert_eq!(get(cx).theme_id, ThemeId::DEFAULT, "the flag is not saved");
            set_theme_id(cx, ThemeId::Leon);
            assert_eq!(theme_id(cx), ThemeId::Leon, "a pick ends the flag");
            assert_eq!(theme::current(cx), ThemeId::Leon);
        });
        assert_eq!(Settings::load(&file).theme_id, ThemeId::Leon);
    }

    #[gpui_kit::test]
    fn a_preview_changes_what_is_worn_but_not_what_is_kept_and_clearing_restores_it(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(Some(file.clone()), Overrides::default(), cx);
            let before = theme::palette(cx).signal;
            preview(
                cx,
                Preview {
                    theme: Some(ThemeId::Zavu),
                    appearance: Some(AppearanceChoice::Light),
                },
            );
            assert_eq!(theme::current(cx), ThemeId::Zavu);
            assert_eq!(theme::palette(cx).appearance, Appearance::Light);
            assert_eq!(theme::fonts::sans(), "Space Grotesk");
            assert_eq!(kept_theme_id(cx), ThemeId::Leon);
            clear_preview(cx);
            assert_eq!(theme::current(cx), ThemeId::Leon);
            assert_eq!(theme::palette(cx).appearance, Appearance::Dark);
            assert_eq!(theme::palette(cx).signal, before);
            assert_eq!(theme::fonts::sans(), "Inter");
        });
        assert!(!file.exists(), "a preview is never written");
    }

    #[gpui_kit::test]
    fn picking_a_theme_overrides_the_flag_and_is_saved(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(
                Some(file.clone()),
                Overrides {
                    appearance: Some(AppearanceChoice::Light),
                    theme: None,
                },
                cx,
            );
            assert_eq!(appearance(cx), Appearance::Light, "the flag has the say");
            set_appearance(cx, AppearanceChoice::Dark);
            assert_eq!(appearance(cx), Appearance::Dark, "a pick ends the flag");
        });
        assert_eq!(Settings::load(&file).theme, AppearanceChoice::Dark);
    }

    fn open(cx: &mut TestAppContext, file: &std::path::Path) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(Some(file.to_path_buf()), Overrides::default(), cx);
        });
    }

    #[gpui_kit::test]
    fn a_chosen_value_is_saved_and_read_back_and_unknown_keys_are_kept(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"from_the_future": true, "theme": "light"}"#).unwrap();
        open(cx, &file);
        cx.update(|cx| {
            let size = schema::find("terminal_font_size").unwrap();
            assert_eq!(int(cx, "terminal_font_size"), 13);
            set_value(cx, size, Value::Int(17));
            assert_eq!(int(cx, "terminal_font_size"), 17);
            assert!(is_modified(cx, size));
            reset_value(cx, size);
            assert!(!is_modified(cx, size));
            set_value(cx, size, Value::Int(15));
        });
        let saved = Store::parse(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved.value("terminal_font_size"), Value::Int(15));
        assert_eq!(saved.unknown_keys(), ["from_the_future"]);
        assert_eq!(saved.value("theme"), Value::Text("light".into()));
    }

    #[gpui_kit::test]
    fn an_external_edit_is_applied_once_two_reads_agree_and_a_broken_file_changes_nothing(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        open(cx, &file);
        cx.update(|cx| {
            assert_eq!(poll_file(cx), Polled::Unchanged);
            let before = generation(cx);
            std::fs::write(
                &file,
                br#"{"terminal_font_size": 18, "terminal_cursor": "star"}"#,
            )
            .unwrap();
            assert_eq!(
                poll_file(cx),
                Polled::Pending,
                "the first read only notes it"
            );
            assert_eq!(int(cx, "terminal_font_size"), 13);
            match poll_file(cx) {
                Polled::Applied(problems) => {
                    assert_eq!(problems.len(), 1);
                    assert_eq!(problems[0].key, "terminal_cursor");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(int(cx, "terminal_font_size"), 18);
            assert_eq!(text(cx, "terminal_cursor"), "block");
            assert!(generation(cx) > before);
            assert_eq!(poll_file(cx), Polled::Unchanged);
            // A half-written file is reported and leaves the values alone.
            std::fs::write(&file, b"{ \"terminal_font").unwrap();
            poll_file(cx);
            assert!(matches!(poll_file(cx), Polled::Invalid(_)));
            assert_eq!(int(cx, "terminal_font_size"), 18);
            assert_eq!(poll_file(cx), Polled::Unchanged, "it is reported once");
        });
    }

    #[gpui_kit::test]
    fn an_edit_of_the_theme_in_the_file_is_worn_live(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        open(cx, &file);
        cx.update(|cx| {
            std::fs::write(&file, br#"{"theme": "light", "theme_id": "zavu"}"#).unwrap();
            poll_file(cx);
            poll_file(cx);
            assert_eq!(theme::current(cx), ThemeId::Zavu);
            assert_eq!(appearance(cx), Appearance::Light);
        });
    }

    #[gpui_kit::test]
    fn invalid_values_of_the_file_are_reported_once(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"sidebar_visible": "yes"}"#).unwrap();
        open(cx, &file);
        cx.update(|cx| {
            assert!(get(cx).sidebar_visible, "that key alone is the default");
            let problems = take_problems(cx);
            assert_eq!(problems.len(), 1);
            assert!(take_problems(cx).is_empty());
        });
    }

    #[gpui_kit::test]
    fn resetting_everything_gives_the_defaults_back_and_drops_the_file_keys(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"terminal_font_size": 20, "theme": "light"}"#).unwrap();
        open(cx, &file);
        cx.update(|cx| {
            reset_all(cx);
            assert_eq!(int(cx, "terminal_font_size"), 13);
            assert_eq!(appearance(cx), Appearance::Dark);
        });
        assert_eq!(std::fs::read(&file).unwrap(), b"{}\n");
    }

    fn engine(runtime: &tokio::runtime::Runtime) -> crate::engine::Engine {
        crate::engine::Engine::new(
            leon_core::Store::open_in_memory().unwrap(),
            std::sync::Arc::new(leon_remote::ScriptedRunner::new()),
            leon_remote::SshOptions::without_multiplexing(),
            leon_history::HistoryRoots {
                claude_projects: Some("/default/claude".into()),
                codex_sessions: Some("/default/codex".into()),
                opencode_db: None,
            },
            runtime.handle().clone(),
        )
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn settle(runtime: &tokio::runtime::Runtime) {
        runtime.block_on(async {
            for _ in 0..64 {
                tokio::task::yield_now().await;
            }
        });
    }

    #[gpui_kit::test]
    fn import_on_start_off_does_not_refresh_and_on_does(cx: &mut TestAppContext) {
        let runtime = runtime();
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        std::fs::write(&file, br#"{"import_on_start": false}"#).unwrap();
        open(cx, &file);
        let quiet = engine(&runtime);
        cx.update(|cx| start_engine(cx, &quiet));
        settle(&runtime);
        assert!(quiet.status().is_none(), "nothing was imported");
        std::fs::write(&file, b"{}").unwrap();
        open(cx, &file);
        let busy = engine(&runtime);
        cx.update(|cx| start_engine(cx, &busy));
        settle(&runtime);
        assert!(busy.status().is_some(), "the history was imported");
    }

    #[gpui_kit::test]
    fn the_engine_is_given_the_settings_folders_else_its_own_defaults(cx: &mut TestAppContext) {
        let runtime = runtime();
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        open(cx, &file);
        let engine = engine(&runtime);
        let roots = |cx: &mut TestAppContext| cx.update(|cx| engine_prefs(cx, &engine).roots);
        assert_eq!(roots(cx).claude_projects, Some("/default/claude".into()));
        cx.update(|cx| {
            set_value(
                cx,
                schema::find("history_dir_claude").unwrap(),
                Value::Text("/elsewhere/claude".into()),
            );
            set_value(
                cx,
                schema::find("history_dir_opencode").unwrap(),
                Value::Text("/elsewhere/opencode.db".into()),
            );
        });
        let chosen = roots(cx);
        assert_eq!(chosen.claude_projects, Some("/elsewhere/claude".into()));
        assert_eq!(chosen.codex_sessions, Some("/default/codex".into()));
        assert_eq!(chosen.opencode_db, Some("/elsewhere/opencode.db".into()));
        cx.update(|cx| reset_value(cx, schema::find("history_dir_claude").unwrap()));
        assert_eq!(roots(cx).claude_projects, Some("/default/claude".into()));
    }

    #[gpui_kit::test]
    fn the_ssh_and_project_settings_reach_the_engine_prefs(cx: &mut TestAppContext) {
        let runtime = runtime();
        let directory = tempfile::tempdir().unwrap();
        open(cx, &directory.path().join(FILE_NAME));
        let engine = engine(&runtime);
        cx.update(|cx| {
            for (key, value) in [
                ("ssh_persist_minutes", Value::Int(60)),
                ("ssh_connect_timeout", Value::Int(12)),
                ("fetch_avatars", Value::Bool(false)),
                ("detect_logos", Value::Bool(false)),
                ("discover_projects", Value::Bool(false)),
                ("ssh_multiplex", Value::Bool(false)),
            ] {
                set_value(cx, schema::find(key).unwrap(), value);
            }
            let prefs = engine_prefs(cx, &engine);
            assert_eq!(prefs.ssh_persist_minutes, 60);
            assert_eq!(prefs.ssh_connect_timeout, Some(12));
            assert!(!prefs.fetch_avatars && !prefs.detect_logos && !prefs.discover_projects);
            assert!(!prefs.ssh_multiplex);
        });
    }
}
