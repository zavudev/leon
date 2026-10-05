//! Drives history sources into the store.
//!
//! An import run lists every item of every source, skips the ones whose
//! fingerprint matches the cursor recorded by a previous run, and loads,
//! parses and stores the rest. Each session is written together with its
//! cursor in one transaction, so a run can be interrupted at any point and
//! simply repeated: whatever was committed is skipped next time, and whatever
//! was not is imported again.
//!
//! Parsing is the expensive part of a first import and is independent per
//! item, so items are processed by a small pool of threads. Writes still go
//! through the store one at a time. The run is synchronous and blocks until
//! it is finished; callers on an async runtime use a blocking thread.

use std::ops::AddAssign;
use std::sync::atomic::{AtomicUsize, Ordering};

use leon_core::{set_import_cursor_in, upsert_session_in, MachineId, Store, StoreChange};

use crate::roots::HistoryRoots;
use crate::source::{HistorySource, SourceItem};

/// Upper bound on the number of threads used by one import run.
const MAX_WORKERS: usize = 8;

/// What an import run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Items found across all sources.
    pub scanned: usize,
    /// Items stored as new or updated sessions.
    pub imported: usize,
    /// Items skipped because they had not changed since the previous run.
    pub skipped_unchanged: usize,
    /// Items that were readable but held no messages. They are remembered
    /// and not read again until they change.
    pub empty: usize,
    /// Items, or whole sources, that could not be read or stored. They are
    /// retried on the next run.
    pub failed: usize,
    /// Lines or rows inside imported items that were skipped as unreadable.
    pub malformed: usize,
}

impl AddAssign for ImportReport {
    fn add_assign(&mut self, other: Self) {
        self.scanned += other.scanned;
        self.imported += other.imported;
        self.skipped_unchanged += other.skipped_unchanged;
        self.empty += other.empty;
        self.failed += other.failed;
        self.malformed += other.malformed;
    }
}

/// Imports agent sessions into the unified history.
#[derive(Debug, Clone, Copy, Default)]
pub struct Importer;

impl Importer {
    /// Imports everything found under `roots` on this machine's file system
    /// and attributes it to `machine_id`. Safe to call repeatedly.
    pub fn run(store: &Store, machine_id: &MachineId, roots: &HistoryRoots) -> ImportReport {
        let sources = roots.sources();
        let sources: Vec<&dyn HistorySource> = sources.iter().map(Box::as_ref).collect();
        Self::run_sources(store, machine_id, &sources)
    }

    /// Imports everything the given sources provide and attributes it to
    /// `machine_id`. This is the entry point for sources other than the
    /// local file system.
    pub fn run_sources(
        store: &Store,
        machine_id: &MachineId,
        sources: &[&dyn HistorySource],
    ) -> ImportReport {
        let mut report = ImportReport::default();
        let cursors = match store.import_cursors(machine_id) {
            Ok(cursors) => cursors,
            Err(error) => {
                tracing::warn!(%error, "cannot read import cursors");
                report.failed += 1;
                return report;
            }
        };

        let mut work: Vec<(&dyn HistorySource, SourceItem)> = Vec::new();
        for source in sources {
            match source.list() {
                Ok(items) => {
                    for item in items {
                        report.scanned += 1;
                        if cursors.get(&item.key) == Some(&item.fingerprint) {
                            report.skipped_unchanged += 1;
                        } else {
                            work.push((*source, item));
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(agent = source.agent().as_str(), %error, "cannot list history source");
                    report.failed += 1;
                }
            }
        }

        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(MAX_WORKERS)
            .min(work.len());
        if workers <= 1 {
            for (source, item) in &work {
                report += import_item(store, machine_id, *source, item);
            }
            return report;
        }

        let next = AtomicUsize::new(0);
        let work = &work;
        let next = &next;
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(move || {
                        let mut local = ImportReport::default();
                        while let Some((source, item)) =
                            work.get(next.fetch_add(1, Ordering::Relaxed))
                        {
                            local += import_item(store, machine_id, *source, item);
                        }
                        local
                    })
                })
                .collect();
            for handle in handles {
                match handle.join() {
                    Ok(local) => report += local,
                    // A panicking worker loses its tally, not the data it
                    // committed; its remaining items are retried next run.
                    Err(_) => report.failed += 1,
                }
            }
        });
        report
    }
}

/// Loads one item and stores it together with its cursor.
fn import_item(
    store: &Store,
    machine_id: &MachineId,
    source: &dyn HistorySource,
    item: &SourceItem,
) -> ImportReport {
    let parsed = match source.load(item) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(key = %item.key, %error, "cannot load history item");
            return ImportReport {
                failed: 1,
                ..Default::default()
            };
        }
    };
    let (stored, outcome) = match &parsed {
        Some(parsed) => (
            store.write(StoreChange::Sessions, |tx| {
                upsert_session_in(tx, &parsed.new_session(machine_id), &parsed.messages)?;
                set_import_cursor_in(tx, machine_id, &item.key, &item.fingerprint)
            }),
            ImportReport {
                imported: 1,
                malformed: parsed.malformed,
                ..Default::default()
            },
        ),
        None => (
            store.set_import_cursor(machine_id, &item.key, &item.fingerprint),
            ImportReport {
                empty: 1,
                ..Default::default()
            },
        ),
    };
    match stored {
        Ok(()) => outcome,
        Err(error) => {
            tracing::warn!(key = %item.key, %error, "cannot store history item");
            ImportReport {
                failed: 1,
                ..Default::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencode;
    use crate::session::ParsedSession;
    use crate::source::HistoryError;
    use leon_core::{AgentKind, Role, SearchQuery, SessionFilter};
    use std::fs;
    use std::path::Path;

    fn claude_line(role: &str, text: &str, second: u32) -> String {
        serde_json::json!({
            "type": role, "isSidechain": false, "cwd": "/srv/api",
            "timestamp": format!("2026-03-01T10:00:{second:02}Z"),
            "message": {"role": role, "content": text}
        })
        .to_string()
    }

    fn codex_line(role: &str, text: &str, second: u32) -> String {
        serde_json::json!({
            "timestamp": format!("2026-03-01T11:00:{second:02}Z"), "type": "response_item",
            "payload": {"type": "message", "role": role,
                        "content": [{"type": "input_text", "text": text}]}
        })
        .to_string()
    }

    fn write(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    /// A synthetic home with one session per agent.
    fn fixture(home: &Path) -> HistoryRoots {
        let roots = HistoryRoots {
            claude_projects: Some(home.join("claude")),
            codex_sessions: Some(home.join("codex")),
            opencode_db: Some(home.join("opencode.db")),
        };
        write(
            &home.join("claude/project/claude-session.jsonl"),
            &[
                claude_line("user", "explain the retry policy", 0),
                claude_line("assistant", "It backs off exponentially.", 1),
            ],
        );
        write(
            &home.join("codex/2026/03/01/rollout-2026-03-01T11-00-00-codex.jsonl"),
            &[codex_line("user", "profile the allocator", 0)],
        );
        opencode::fixture::populate(&rusqlite::Connection::open(home.join("opencode.db")).unwrap());
        roots
    }

    #[test]
    fn sessions_of_every_agent_are_imported_and_searchable() {
        let home = tempfile::tempdir().unwrap();
        let roots = fixture(home.path());
        let store = Store::open_in_memory().unwrap();

        let report = Importer::run(&store, &MachineId::local(), &roots);

        assert_eq!(
            report,
            ImportReport {
                scanned: 3,
                imported: 3,
                ..Default::default()
            }
        );
        let sessions = store
            .recent_sessions(&SessionFilter::default(), 10)
            .unwrap();
        let mut agents: Vec<_> = sessions.iter().map(|session| session.agent).collect();
        agents.sort_by_key(|agent| agent.as_str());
        assert_eq!(
            agents,
            [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode]
        );

        let hits = store.search(&SearchQuery::new("exponentially")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.external_id, "claude-session");
        assert_eq!(hits[0].role, Some(Role::Assistant));
        // The Codex prompt matches twice: as the message and as the title
        // derived from it.
        assert_eq!(
            store.search(&SearchQuery::new("allocator")).unwrap().len(),
            2
        );
        assert_eq!(store.search(&SearchQuery::new("loader")).unwrap().len(), 3);
    }

    #[test]
    fn importing_the_same_sources_twice_adds_nothing() {
        let home = tempfile::tempdir().unwrap();
        let roots = fixture(home.path());
        let store = Store::open_in_memory().unwrap();
        Importer::run(&store, &MachineId::local(), &roots);
        let before = store
            .recent_sessions(&SessionFilter::default(), 10)
            .unwrap();

        let report = Importer::run(&store, &MachineId::local(), &roots);

        assert_eq!(
            report,
            ImportReport {
                scanned: 3,
                skipped_unchanged: 3,
                ..Default::default()
            }
        );
        assert_eq!(
            store
                .recent_sessions(&SessionFilter::default(), 10)
                .unwrap(),
            before
        );
    }

    #[test]
    fn only_a_changed_file_is_imported_again_and_its_session_is_updated() {
        let home = tempfile::tempdir().unwrap();
        let roots = fixture(home.path());
        let store = Store::open_in_memory().unwrap();
        Importer::run(&store, &MachineId::local(), &roots);

        write(
            &home.path().join("claude/project/claude-session.jsonl"),
            &[
                claude_line("user", "explain the retry policy", 0),
                claude_line("assistant", "It backs off exponentially.", 1),
                claude_line("user", "and the jitter?", 2),
            ],
        );
        let report = Importer::run(&store, &MachineId::local(), &roots);

        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped_unchanged, 2);
        let filter = SessionFilter {
            agent: Some(AgentKind::Claude),
            ..Default::default()
        };
        let sessions = store.recent_sessions(&filter, 10).unwrap();
        assert_eq!(sessions.len(), 1, "the session was updated, not duplicated");
        assert_eq!(sessions[0].message_count, 3);
    }

    #[test]
    fn a_file_without_messages_is_remembered_and_not_read_again() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join("claude/project/empty.jsonl"),
            &[r#"{"type":"mode","mode":"normal"}"#.to_owned()],
        );
        let roots = HistoryRoots {
            claude_projects: Some(home.path().join("claude")),
            ..Default::default()
        };
        let store = Store::open_in_memory().unwrap();

        let first = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!((first.scanned, first.empty, first.imported), (1, 1, 0));
        let second = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!((second.scanned, second.skipped_unchanged), (1, 1));
    }

    #[test]
    fn missing_roots_import_nothing_and_fail_nothing() {
        let home = tempfile::tempdir().unwrap();
        let roots = HistoryRoots {
            claude_projects: Some(home.path().join("nope")),
            codex_sessions: Some(home.path().join("nope")),
            opencode_db: Some(home.path().join("nope.db")),
        };
        let store = Store::open_in_memory().unwrap();
        assert_eq!(
            Importer::run(&store, &MachineId::local(), &roots),
            ImportReport::default()
        );
    }

    #[test]
    fn malformed_lines_are_reported_without_failing_the_item() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join("claude/project/damaged.jsonl"),
            &[
                claude_line("user", "first", 0),
                "not json".to_owned(),
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"ty".to_owned(),
            ],
        );
        let roots = HistoryRoots {
            claude_projects: Some(home.path().join("claude")),
            ..Default::default()
        };
        let store = Store::open_in_memory().unwrap();
        let report = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(
            (report.imported, report.failed, report.malformed),
            (1, 0, 2)
        );
    }

    #[test]
    fn many_files_are_imported_completely_by_the_worker_pool() {
        let home = tempfile::tempdir().unwrap();
        for index in 0..60 {
            write(
                &home
                    .path()
                    .join(format!("claude/project-{}/s{index}.jsonl", index % 5)),
                &[claude_line("user", &format!("task number {index}"), 0)],
            );
        }
        let roots = HistoryRoots {
            claude_projects: Some(home.path().join("claude")),
            ..Default::default()
        };
        let store = Store::open_in_memory().unwrap();
        let report = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(
            (report.scanned, report.imported, report.failed),
            (60, 60, 0)
        );
        assert_eq!(
            store
                .recent_sessions(&SessionFilter::default(), 100)
                .unwrap()
                .len(),
            60
        );
    }

    /// A source standing in for a remote machine: items come from memory and
    /// one of them cannot be fetched.
    struct InMemory;

    impl HistorySource for InMemory {
        fn agent(&self) -> AgentKind {
            AgentKind::Claude
        }

        fn list(&self) -> Result<Vec<SourceItem>, HistoryError> {
            Ok(["good", "broken"]
                .into_iter()
                .map(|name| SourceItem {
                    key: format!("claude:remote/{name}.jsonl"),
                    fingerprint: "1:1".into(),
                    locator: name.into(),
                })
                .collect())
        }

        fn load(&self, item: &SourceItem) -> Result<Option<ParsedSession>, HistoryError> {
            if item.locator == "broken" {
                return Err(HistoryError::Io {
                    path: item.locator.clone(),
                    source: std::io::Error::other("connection lost"),
                });
            }
            let bytes = claude_line("user", "fetched over the wire", 0);
            Ok(crate::claude::parse_session(
                &item.locator,
                bytes.as_bytes(),
            ))
        }
    }

    #[test]
    fn a_custom_source_feeds_the_same_parsers_and_failures_are_retried() {
        let store = Store::open_in_memory().unwrap();
        let sources: [&dyn HistorySource; 1] = [&InMemory];

        let first = Importer::run_sources(&store, &MachineId::local(), &sources);
        assert_eq!((first.scanned, first.imported, first.failed), (2, 1, 1));

        let second = Importer::run_sources(&store, &MachineId::local(), &sources);
        assert_eq!(
            (second.skipped_unchanged, second.imported, second.failed),
            (1, 0, 1),
            "the failed item has no cursor, so it is attempted again"
        );
    }

    #[test]
    fn sessions_for_an_unknown_machine_are_counted_as_failed() {
        let store = Store::open_in_memory().unwrap();
        let sources: [&dyn HistorySource; 1] = [&InMemory];
        let report = Importer::run_sources(&store, &MachineId::generate(), &sources);
        assert_eq!((report.imported, report.failed), (0, 2));
    }

    #[test]
    fn importing_does_not_modify_the_opencode_database() {
        let home = tempfile::tempdir().unwrap();
        let roots = fixture(home.path());
        let path = home.path().join("opencode.db");
        let before = fs::read(&path).unwrap();
        let store = Store::open_in_memory().unwrap();
        Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
