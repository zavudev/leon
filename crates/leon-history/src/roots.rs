//! Where each agent keeps its sessions.
//!
//! Importers never guess locations: they are handed a [`HistoryRoots`]. This
//! module provides the conventional locations as a convenience, computed from
//! a home directory and the environment variables the agents themselves
//! honour, so the logic is testable without touching the real home directory.

use std::path::{Path, PathBuf};

use crate::source::{ClaudeFiles, CodexFiles, HistorySource, OpencodeDb};

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
            sources.push(Box::new(OpencodeDb::new(path)));
        }
        sources
    }
}

/// The conventional locations for the current user, read from the process
/// environment. Every entry is absent when no home directory can be
/// determined.
pub fn default_roots() -> HistoryRoots {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    match variable("HOME").or_else(|| variable("USERPROFILE")) {
        Some(home) => default_roots_in(Path::new(&home), variable),
        None => HistoryRoots::default(),
    }
}

/// The conventional locations for a given home directory.
///
/// `variable` looks up an environment variable. The overrides the agents
/// document are honoured: `CLAUDE_CONFIG_DIR` (default `<home>/.claude`),
/// `CODEX_HOME` (default `<home>/.codex`) and `XDG_DATA_HOME` (default
/// `<home>/.local/share`, which opencode uses on every operating system).
pub fn default_roots_in(home: &Path, variable: impl Fn(&str) -> Option<String>) -> HistoryRoots {
    let directory = |name: &str, fallback: PathBuf| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map_or(fallback, PathBuf::from)
    };
    let claude = directory("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let codex = directory("CODEX_HOME", home.join(".codex"));
    let data = directory("XDG_DATA_HOME", home.join(".local").join("share"));
    HistoryRoots {
        claude_projects: Some(claude.join("projects")),
        codex_sessions: Some(codex.join("sessions")),
        opencode_db: Some(data.join("opencode").join("opencode.db")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_core::AgentKind;

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
        assert_eq!(agents, [AgentKind::Codex, AgentKind::Opencode]);
    }
}
