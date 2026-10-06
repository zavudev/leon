//! Where each agent keeps its sessions.
//!
//! Importers never guess locations: they are handed a [`HistoryRoots`]. This
//! module provides the conventional locations as a convenience, computed from
//! a home directory and the environment variables the agents themselves
//! honour, so the logic is testable without touching the real home directory.

use std::path::{Path, PathBuf};

use crate::source::{ClaudeFiles, CodexFiles, HistorySource, OpencodeData};

/// The locations to import from. An absent entry means "do not import that
/// agent".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryRoots {
    /// Claude Code's `projects` directory.
    pub claude_projects: Option<PathBuf>,
    /// Codex's `sessions` directory.
    pub codex_sessions: Option<PathBuf>,
    /// The opencode database file.
    pub opencode_db: Option<PathBuf>,
}

impl HistoryRoots {
    /// One local source per configured location.
    pub fn sources(&self) -> Vec<Box<dyn HistorySource>> {
        let mut sources: Vec<Box<dyn HistorySource>> = Vec::new();
        if let Some(root) = &self.claude_projects {
            sources.push(Box::new(ClaudeFiles::new(root)));
        }
        if let Some(root) = &self.codex_sessions {
            sources.push(Box::new(CodexFiles::new(root)));
        }
        if let Some(path) = &self.opencode_db {
            sources.push(Box::new(OpencodeData::new(path)));
        }
        sources
    }
}

/// The conventional locations for the current user, read from the process
/// environment. Every entry is absent when no home directory can be
/// determined.
pub fn default_roots() -> HistoryRoots {
    match home_dir() {
        Some(home) => default_roots_in(&home, env_variable),
        None => HistoryRoots::default(),
    }
}

/// A variable of the process environment; unset and empty are the same.
pub fn env_variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The current user's home directory as the agents see it.
pub fn home_dir() -> Option<PathBuf> {
    // On Windows the profile folder is `USERPROFILE`; `HOME` may be set by a
    // POSIX shell (Git Bash) to a path the agents do not use.
    let home = if cfg!(windows) {
        env_variable("USERPROFILE").or_else(|| env_variable("HOME"))
    } else {
        env_variable("HOME").or_else(|| env_variable("USERPROFILE"))
    };
    home.map(PathBuf::from)
}

/// The conventional locations for a given home directory.
///
/// `variable` looks up an environment variable. The overrides the agents
/// document are honoured: `CLAUDE_CONFIG_DIR` (default `<home>/.claude`),
/// `CODEX_HOME` (default `<home>/.codex`) and `XDG_DATA_HOME` (default
/// `<home>/.local/share`, which opencode uses on every operating system,
/// Windows included; see below for its fallbacks).
pub fn default_roots_in(home: &Path, variable: impl Fn(&str) -> Option<String>) -> HistoryRoots {
    let directory = |name: &str, fallback: PathBuf| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map_or(fallback, PathBuf::from)
    };
    let claude = directory("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let codex = directory("CODEX_HOME", home.join(".codex"));
    // opencode takes its data folder from `xdg-basedir`, which on Windows is
    // still `~/.local/share`; `%APPDATA%` and `%LOCALAPPDATA%` are looked at
    // only when the database is already there and the XDG one is not.
    let xdg = directory("XDG_DATA_HOME", home.join(".local").join("share"));
    let db = |base: &Path| base.join("opencode").join("opencode.db");
    let data = if db(&xdg).exists() {
        xdg
    } else {
        ["APPDATA", "LOCALAPPDATA"]
            .iter()
            .filter_map(|name| variable(name))
            .map(PathBuf::from)
            .find(|base| db(base).exists())
            .unwrap_or(xdg)
    };
    // `OPENCODE_DB` moves the database itself: an absolute path is used as
    // given, a relative one is taken inside opencode's data folder.
    let opencode_db = match variable("OPENCODE_DB").filter(|value| !value.is_empty()) {
        Some(value) if value != ":memory:" => {
            let path = PathBuf::from(&value);
            if path.is_absolute() {
                path
            } else {
                data.join("opencode").join(value)
            }
        }
        _ => data.join("opencode").join("opencode.db"),
    };
    HistoryRoots {
        claude_projects: Some(claude.join("projects")),
        codex_sessions: Some(codex.join("sessions")),
        opencode_db: Some(opencode_db),
    }
}

/// Every place the importers consider for each agent, with where the
/// location comes from, for the history diagnosis. The locations an
/// environment variable moves are listed only when it is set.
pub fn considered(
    home: &Path,
    variable: impl Fn(&str) -> Option<String>,
) -> Vec<(leon_core::AgentId, crate::survey::Place)> {
    use crate::survey::Place;
    use leon_core::AgentId;
    let set = |name: &str| variable(name).filter(|value| !value.is_empty());
    let mut places = Vec::new();
    let mut add = |agent: AgentId, origin: String, path: PathBuf| {
        let exists = path.exists();
        places.push((agent, Place::new(&origin, path, exists)));
    };
    if let Some(dir) = set("CLAUDE_CONFIG_DIR") {
        add(
            AgentId::CLAUDE,
            "environment CLAUDE_CONFIG_DIR".into(),
            Path::new(&dir).join("projects"),
        );
    }
    add(
        AgentId::CLAUDE,
        "default".into(),
        home.join(".claude").join("projects"),
    );
    add(
        AgentId::CLAUDE,
        "default (XDG config; not imported)".into(),
        home.join(".config").join("claude").join("projects"),
    );
    if let Some(dir) = set("CODEX_HOME") {
        add(
            AgentId::CODEX,
            "environment CODEX_HOME".into(),
            Path::new(&dir).join("sessions"),
        );
    }
    add(
        AgentId::CODEX,
        "default".into(),
        home.join(".codex").join("sessions"),
    );
    let db = |base: &Path| base.join("opencode").join("opencode.db");
    if let Some(value) = set("OPENCODE_DB") {
        add(
            AgentId::OPENCODE,
            "environment OPENCODE_DB".into(),
            PathBuf::from(value),
        );
    }
    if let Some(dir) = set("XDG_DATA_HOME") {
        add(
            AgentId::OPENCODE,
            "environment XDG_DATA_HOME".into(),
            db(Path::new(&dir)),
        );
    }
    add(
        AgentId::OPENCODE,
        "default".into(),
        db(&home.join(".local").join("share")),
    );
    for name in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(dir) = set(name) {
            add(
                AgentId::OPENCODE,
                format!("environment {name} (Windows fallback)"),
                db(Path::new(&dir)),
            );
        }
    }
    places
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::AgentId;

    #[test]
    fn default_locations_hang_off_the_home_directory() {
        let home = Path::new("home").join("dev");
        let roots = default_roots_in(&home, |_| None);
        assert_eq!(
            roots.claude_projects,
            Some(home.join(".claude").join("projects"))
        );
        assert_eq!(
            roots.codex_sessions,
            Some(home.join(".codex").join("sessions"))
        );
        assert_eq!(
            roots.opencode_db,
            Some(
                home.join(".local")
                    .join("share")
                    .join("opencode")
                    .join("opencode.db")
            )
        );
    }

    #[test]
    fn the_importers_are_exactly_the_agents_the_catalogue_says_have_history() {
        let home = Path::new("home").join("dev");
        let roots = default_roots_in(&home, |_| None);
        let mut imported: Vec<&str> = roots
            .sources()
            .iter()
            .map(|source| source.agent().as_str())
            .collect();
        imported.sort_unstable();
        let mut catalogued: Vec<&str> = leon_core::agent::builtin()
            .iter()
            .filter(|spec| spec.history.is_some())
            .map(|spec| spec.id.as_str())
            .collect();
        catalogued.sort_unstable();
        assert_eq!(imported, catalogued);
    }

    #[test]
    fn on_windows_the_profile_folder_is_the_home_and_opencode_is_found_where_its_database_is() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Users").join("me");
        let appdata = dir.path().join("AppData").join("Roaming");
        std::fs::create_dir_all(appdata.join("opencode")).unwrap();
        std::fs::write(appdata.join("opencode").join("opencode.db"), b"").unwrap();
        let appdata_text = appdata.to_string_lossy().into_owned();
        let found = default_roots_in(&profile, |name| {
            (name == "APPDATA").then(|| appdata_text.clone())
        });
        assert_eq!(
            found.opencode_db,
            Some(appdata.join("opencode").join("opencode.db"))
        );
        // The Claude and Codex folders hang off the profile.
        assert_eq!(
            found.claude_projects,
            Some(profile.join(".claude").join("projects"))
        );
        // With the XDG database present, it wins.
        let xdg = profile.join(".local").join("share").join("opencode");
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::write(xdg.join("opencode.db"), b"").unwrap();
        let both = default_roots_in(&profile, |name| {
            (name == "APPDATA").then(|| appdata_text.clone())
        });
        assert_eq!(both.opencode_db, Some(xdg.join("opencode.db")));
    }

    #[test]
    fn environment_overrides_are_honoured() {
        let home = Path::new("home").join("dev");
        let roots = default_roots_in(&home, |name| match name {
            "CLAUDE_CONFIG_DIR" => Some("custom-claude".into()),
            "CODEX_HOME" => Some("custom-codex".into()),
            "XDG_DATA_HOME" => Some("custom-data".into()),
            _ => None,
        });
        assert_eq!(
            roots.claude_projects,
            Some(Path::new("custom-claude").join("projects"))
        );
        assert_eq!(
            roots.codex_sessions,
            Some(Path::new("custom-codex").join("sessions"))
        );
        assert_eq!(
            roots.opencode_db,
            Some(
                Path::new("custom-data")
                    .join("opencode")
                    .join("opencode.db")
            )
        );
    }

    #[test]
    fn opencode_db_moves_the_database() {
        let home = Path::new("home").join("dev");
        let absolute = std::env::temp_dir().join("elsewhere.db");
        let text = absolute.to_string_lossy().into_owned();
        let roots = default_roots_in(&home, |name| (name == "OPENCODE_DB").then(|| text.clone()));
        assert_eq!(roots.opencode_db, Some(absolute));
        let roots = default_roots_in(&home, |name| {
            (name == "OPENCODE_DB").then(|| "mine.db".to_owned())
        });
        assert_eq!(
            roots.opencode_db,
            Some(home.join(".local/share/opencode/mine.db"))
        );
        let roots = default_roots_in(&home, |name| {
            (name == "OPENCODE_DB").then(|| ":memory:".to_owned())
        });
        assert_eq!(roots, default_roots_in(&home, |_| None));
    }

    #[test]
    fn the_considered_places_name_their_origin_and_whether_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codex/sessions")).unwrap();
        let places = considered(dir.path(), |name| {
            (name == "XDG_DATA_HOME").then(|| "/nowhere".to_owned())
        });
        let codex = places
            .iter()
            .find(|(agent, place)| *agent == leon_core::AgentId::CODEX && place.origin == "default")
            .unwrap();
        assert!(codex.1.exists);
        let xdg = places
            .iter()
            .find(|(_, place)| place.origin == "environment XDG_DATA_HOME")
            .unwrap();
        assert!(!xdg.1.exists);
        // Unset variables add no entries.
        assert!(!places.iter().any(|(_, p)| p.origin.contains("CODEX_HOME")));
    }

    #[test]
    fn an_empty_override_is_ignored() {
        let home = Path::new("home").join("dev");
        let roots = default_roots_in(&home, |_| Some(String::new()));
        assert_eq!(roots, default_roots_in(&home, |_| None));
    }

    #[test]
    fn only_configured_locations_become_sources() {
        assert!(HistoryRoots::default().sources().is_empty());
        let roots = HistoryRoots {
            codex_sessions: Some("sessions".into()),
            opencode_db: Some("opencode.db".into()),
            ..Default::default()
        };
        let agents: Vec<_> = roots
            .sources()
            .iter()
            .map(|source| source.agent())
            .collect();
        assert_eq!(agents, [AgentId::CODEX, AgentId::OPENCODE]);
    }
}
