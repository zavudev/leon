//! "Why is a session missing?": the report of `leon --diagnose history` and of
//! the palette command of the same name.
//!
//! For each agent with an importer it says every place Leon looked (the
//! defaults of this operating system, the variables the agent honours, the
//! Settings override), whether each exists, what format was found, how many
//! sessions the source holds against how many Leon imported, and why the rest
//! is not imported. It holds counts, paths relative to the home folder,
//! versions and times only: no title, no transcript, nothing about an
//! account.
//!
//! [`collect`] reads the sources (it may take a moment on a large history, so
//! callers run it off the UI thread) and [`render`] is pure.

use chrono::{DateTime, Utc};
use leon_core::{AgentId, AgentOverview, MachineId};
use leon_history::{HistoryRoots, HistorySource, Place, Survey};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the report knows about one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    /// The agent.
    pub agent: AgentId,
    /// Places considered by default or through the environment, with their
    /// origin.
    pub considered: Vec<Place>,
    /// What the configured source holds.
    pub survey: Survey,
    /// Items the source lists that the store has not imported yet (no cursor,
    /// or a cursor that no longer matches), when the store was readable.
    pub pending: Option<usize>,
    /// Items the store has already imported and that have not changed.
    pub unchanged: Option<usize>,
    /// What the store holds of this agent, when it could be read.
    pub stored: Option<AgentOverview>,
}

/// Everything the report is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The home folder, shown as `~`.
    pub home: Option<PathBuf>,
    /// One entry per agent with an importer.
    pub agents: Vec<AgentReport>,
    /// Why the store could not be read, when it could not.
    pub store_problem: Option<String>,
}

/// The stored side of the report: what the store holds and its cursors.
pub struct Stored {
    /// Counts per agent.
    pub overview: Vec<AgentOverview>,
    /// The import cursors, source key to fingerprint.
    pub cursors: HashMap<String, String>,
}

/// Looks at every source and, when `stored` is given, compares it with what
/// the store holds. `variable` reads the environment.
pub fn collect(
    roots: &HistoryRoots,
    home: Option<&Path>,
    variable: &dyn Fn(&str) -> Option<String>,
    stored: Result<Stored, String>,
    only: Option<AgentId>,
) -> Report {
    let considered = home
        .map(|home| leon_history::considered(home, variable))
        .unwrap_or_default();
    let (stored, store_problem) = match stored {
        Ok(stored) => (Some(stored), None),
        Err(problem) => (None, Some(problem)),
    };
    let mut agents = Vec::new();
    for source in roots.sources() {
        let agent = source.agent();
        if only.is_some_and(|only| only != agent) {
            continue;
        }
        agents.push(agent_report(source.as_ref(), &considered, stored.as_ref()));
    }
    Report {
        home: home.map(Path::to_path_buf),
        agents,
        store_problem,
    }
}

fn agent_report(
    source: &dyn HistorySource,
    considered: &[(AgentId, Place)],
    stored: Option<&Stored>,
) -> AgentReport {
    let agent = source.agent();
    let survey = source.survey();
    let mut places: Vec<Place> = considered
        .iter()
        .filter(|(who, _)| *who == agent)
        .map(|(_, place)| place.clone())
        .collect();
    // The location in force may come from the Settings or the environment and
    // be none of the defaults: it is always listed.
    for place in &survey.places {
        if let Some(known) = places.iter_mut().find(|known| known.path == place.path) {
            known.details.clone_from(&place.details);
        } else {
            places.push(Place {
                origin: if place.origin == "configured" {
                    "in force (setting, environment or default)".to_owned()
                } else {
                    place.origin.clone()
                },
                ..place.clone()
            });
        }
    }
    let (pending, unchanged) = match (stored, source.list()) {
        (Some(stored), Ok(items)) => {
            let unchanged = items
                .iter()
                .filter(|item| stored.cursors.get(&item.key) == Some(&item.fingerprint))
                .count();
            (Some(items.len() - unchanged), Some(unchanged))
        }
        _ => (None, None),
    };
    AgentReport {
        agent,
        considered: places,
        survey,
        pending,
        unchanged,
        stored: stored.and_then(|stored| {
            stored
                .overview
                .iter()
                .find(|row| row.agent == agent.as_str())
                .cloned()
        }),
    }
}

/// A path with the home folder written as `~`.
pub fn home_relative(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home {
        if let Ok(rest) = path.strip_prefix(home) {
            let rest = rest.display().to_string();
            return if rest.is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.replace('\\', "/"))
            };
        }
    }
    path.display().to_string()
}

/// A sentence that names paths: the home folder written as `~` and every path
/// separator as `/`, whatever the platform spells them (a Windows path is
/// `C:\Users\me\...`; the report reads the same on every system).
fn home_relative_text(text: &str, home: Option<&Path>) -> String {
    let mut text = text.to_owned();
    if let Some(home) = home {
        let home = home.display().to_string();
        if !home.is_empty() {
            text = text.replace(&home, "~");
        }
    }
    text.replace('\\', "/")
}

fn time(at: Option<DateTime<Utc>>) -> String {
    at.map_or_else(
        || "none".to_owned(),
        |at| at.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

/// The report as text, one line each.
pub fn render(report: &Report, platform: &str) -> Vec<String> {
    let home = report.home.as_deref();
    let mut lines = vec![
        "Leon history report".to_owned(),
        format!(
            "Leon {} on {platform}. No titles, messages or account data are included.",
            env!("CARGO_PKG_VERSION")
        ),
    ];
    if let Some(problem) = &report.store_problem {
        lines.push(format!("Leon's own database could not be read: {problem}"));
    }
    for agent in &report.agents {
        lines.push(String::new());
        lines.push(format!("== {} ==", crate::format::agent_name(agent.agent)));
        lines.push("Looked at:".to_owned());
        for place in &agent.considered {
            lines.push(format!(
                "  [{}] {}  ({})",
                if place.exists { "exists " } else { "missing" },
                home_relative(&place.path, home),
                place.origin
            ));
            for detail in &place.details {
                lines.push(format!("      {detail}"));
            }
        }
        let survey = &agent.survey;
        lines.push(format!(
            "Sessions in the source: {} (newest {})",
            survey.found,
            time(survey.newest)
        ));
        match &agent.stored {
            Some(stored) => lines.push(format!(
                "Sessions Leon imported: {} (newest {}); in no project: {}",
                stored.sessions,
                time(stored.newest),
                stored.unplaced
            )),
            None if report.store_problem.is_none() => {
                lines.push("Sessions Leon imported: 0".to_owned());
            }
            None => {}
        }
        if let (Some(pending), Some(unchanged)) = (agent.pending, agent.unchanged) {
            lines.push(format!(
                "Waiting to be imported: {pending}; already imported and unchanged: {unchanged}"
            ));
        }
        if survey.skipped.is_empty() {
            lines.push("Not imported: nothing is skipped.".to_owned());
        } else {
            lines.push("Not imported, by reason:".to_owned());
            for (reason, count) in &survey.skipped {
                lines.push(format!("  {count} {}", reason.label()));
            }
        }
        lines.push(format!(
            "Rows or lines that could not be read: {}",
            survey.malformed
        ));
        lines.push(format!(
            "Imported without an absolute folder (they belong to no project): {}",
            survey.without_folder
        ));
        match agent.stored.as_ref().and_then(|stored| stored.last_run.as_ref()) {
            Some(run) => lines.push(format!(
                "Last import: {}, took {} ms; scanned {}, stored {}, unchanged {}, empty {}, failed {}, unreadable rows {}, unsupported {}",
                time(Some(run.at)),
                run.duration_ms,
                run.scanned,
                run.imported,
                run.unchanged,
                run.empty,
                run.failed,
                run.malformed,
                run.unsupported
            )),
            None => lines.push("Last import: none recorded.".to_owned()),
        }
        for problem in &survey.problems {
            lines.push(format!("PROBLEM: {}", home_relative_text(problem, home)));
        }
    }
    lines
}

/// `leon --diagnose history`: collects and prints the report. Exit code 0.
pub fn run(only: Option<AgentId>, data_dir: &Path) -> i32 {
    let home = leon_history::home_dir();
    let mut roots = leon_history::default_roots();
    apply_settings(&mut roots, &data_dir.join(crate::settings::FILE_NAME));
    let db = data_dir.join("leon.db");
    let machine = MachineId::local();
    let stored = if db.is_file() {
        leon_core::history_overview_at(&db, &machine)
            .and_then(|overview| {
                Ok(Stored {
                    overview,
                    cursors: leon_core::import_cursors_at(&db, &machine)?,
                })
            })
            .map_err(|error| error.to_string())
    } else {
        Err(format!(
            "there is no database at {}",
            home_relative(&db, home.as_deref())
        ))
    };
    let report = collect(
        &roots,
        home.as_deref(),
        &leon_history::env_variable,
        stored,
        only,
    );
    for line in render(&report, &crate::platform::describe()) {
        println!("{line}");
    }
    0
}

/// Applies the folders of the Settings file over the defaults.
pub fn apply_settings(roots: &mut HistoryRoots, settings_file: &Path) {
    let store = crate::settings::stored(settings_file);
    let text = |key: &str| match store.value(key) {
        crate::schema::Value::Text(text) if !text.is_empty() => Some(text),
        _ => None,
    };
    if let Some(folder) = text("history_dir_claude") {
        roots.claude_projects = Some(folder.into());
    }
    if let Some(folder) = text("history_dir_codex") {
        roots.codex_sessions = Some(folder.into());
    }
    if let Some(file) = text("history_dir_opencode") {
        roots.opencode_db = Some(file.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_history::SkipReason;

    fn text(report: &Report) -> String {
        render(report, "test").join("\n")
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn claude_line(text: &str) -> String {
        serde_json::json!({"type": "user", "cwd": "/srv/api",
            "timestamp": "2026-03-01T10:00:00Z",
            "message": {"role": "user", "content": text}})
        .to_string()
    }

    #[test]
    fn a_missing_source_is_listed_as_missing_with_its_origin() {
        let home = tempfile::tempdir().unwrap();
        let roots = leon_history::default_roots_in(home.path(), no_env);
        let report = collect(
            &roots,
            Some(home.path()),
            &no_env,
            Err("no database".into()),
            Some(AgentId::OPENCODE),
        );
        let out = text(&report);
        assert!(
            out.contains("[missing] ~/.local/share/opencode/opencode.db  (default)"),
            "{out}"
        );
        assert!(out.contains("Leon's own database could not be read: no database"));
        assert!(!out.contains("== Claude"), "only the asked agent: {out}");
    }

    #[test]
    fn the_report_counts_sources_skips_and_what_leon_imported() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude/projects");
        write(&claude.join("p/one.jsonl"), &claude_line("hello"));
        write(&claude.join("p/blank.jsonl"), "\n");
        write(
            &claude.join("p/one/subagents/agent.jsonl"),
            &claude_line("child"),
        );
        let roots = leon_history::default_roots_in(home.path(), no_env);

        let store = leon_core::Store::open_in_memory().unwrap();
        leon_history::Importer::run(&store, &MachineId::local(), &roots);
        let stored = Stored {
            overview: store.history_overview(&MachineId::local()).unwrap(),
            cursors: store.import_cursors(&MachineId::local()).unwrap(),
        };
        let report = collect(
            &roots,
            Some(home.path()),
            &no_env,
            Ok(stored),
            Some(AgentId::CLAUDE),
        );
        let agent = &report.agents[0];
        assert_eq!(agent.survey.found, 2);
        assert_eq!(agent.survey.skipped[&SkipReason::Empty], 1);
        assert_eq!(agent.survey.skipped[&SkipReason::ChildSession], 1);
        assert_eq!(agent.stored.as_ref().unwrap().sessions, 1);
        assert_eq!(agent.pending, Some(0));
        let out = text(&report);
        assert!(out.contains("Sessions in the source: 2"), "{out}");
        assert!(out.contains("Sessions Leon imported: 1"), "{out}");
        assert!(out.contains("1 child (sub-agent) sessions"), "{out}");
        assert!(out.contains("1 empty sessions (no messages)"), "{out}");
        assert!(out.contains("Last import:"), "{out}");
        // No transcript text and no absolute home path leak into the report.
        assert!(!out.contains("hello"), "{out}");
        assert!(!out.contains(home.path().to_str().unwrap()), "{out}");
    }

    #[test]
    fn an_unknown_database_layout_is_a_problem_line_with_the_tables_found() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join(".local/share/opencode/opencode.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        leon_core::rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE sessions_v2 (id TEXT);")
            .unwrap();
        let roots = leon_history::default_roots_in(home.path(), no_env);
        let report = collect(
            &roots,
            Some(home.path()),
            &no_env,
            Err(String::new()),
            Some(AgentId::OPENCODE),
        );
        let out = text(&report);
        assert!(out.contains("tables: sessions_v2"), "{out}");
        assert!(out.contains("PROBLEM: ~/.local/share/opencode/opencode.db: unsupported layout: no `session` table"), "{out}");
        assert!(out.contains("1 unsupported database layout"), "{out}");
    }

    #[test]
    fn settings_folders_override_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(crate::settings::FILE_NAME);
        write(&file, r#"{"history_dir_opencode": "/custom/opencode.db"}"#);
        let mut roots = HistoryRoots::default();
        apply_settings(&mut roots, &file);
        assert_eq!(roots.opencode_db, Some("/custom/opencode.db".into()));
        assert_eq!(roots.claude_projects, None);
    }

    #[test]
    fn paths_are_written_relative_to_the_home_folder() {
        let home = Path::new("/home/dev");
        assert_eq!(
            home_relative(Path::new("/home/dev/.codex"), Some(home)),
            "~/.codex"
        );
        assert_eq!(home_relative(Path::new("/home/dev"), Some(home)), "~");
        assert_eq!(home_relative(Path::new("/srv/x"), Some(home)), "/srv/x");
    }

    #[test]
    fn a_sentence_with_paths_reads_the_same_with_windows_separators() {
        let home = Path::new(r"C:\Users\me");
        assert_eq!(
            home_relative_text(
                r"C:\Users\me\AppData\opencode.db: no `session` table",
                Some(home)
            ),
            "~/AppData/opencode.db: no `session` table"
        );
        assert_eq!(
            home_relative_text("/srv/x: odd", Some(Path::new("/home/me"))),
            "/srv/x: odd"
        );
        assert_eq!(home_relative_text("plain", None), "plain");
    }
}
