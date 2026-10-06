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

use std::time::Instant;

use chrono::{DateTime, Utc};
use leon_core::{
    set_import_cursor_in, upsert_session_in, ImportRun, MachineId, Store, StoreChange,
};

use crate::roots::HistoryRoots;
use crate::source::{HistoryError, HistorySource, SourceItem};

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
    /// Sources whose layout Leon does not know (a database without the
    /// tables or columns it reads). They are also counted in `failed`.
    pub unsupported: usize,
}

impl AddAssign for ImportReport {
    fn add_assign(&mut self, other: Self) {
        self.scanned += other.scanned;
        self.imported += other.imported;
        self.skipped_unchanged += other.skipped_unchanged;
        self.empty += other.empty;
        self.failed += other.failed;
        self.malformed += other.malformed;
        self.unsupported += other.unsupported;
    }
}

/// Imports agent sessions into the unified history.
#[derive(Debug, Clone, Copy, Default)]
pub struct Importer;

impl Importer {
    /// Imports everything found under `roots` on this machine's file system
    /// and attributes it to `machine_id`. Safe to call repeatedly.
    ///
    /// Each source is imported on its own and what it did is recorded in the
    /// store (when, how long, how it ended) for the history diagnosis.
    pub fn run(store: &Store, machine_id: &MachineId, roots: &HistoryRoots) -> ImportReport {
        Self::run_with_clock(store, machine_id, roots, Utc::now)
    }

    /// [`Importer::run`] with the clock that stamps the recorded runs, for
    /// tests.
    pub fn run_with_clock(
        store: &Store,
        machine_id: &MachineId,
        roots: &HistoryRoots,
        now: impl Fn() -> DateTime<Utc>,
    ) -> ImportReport {
        let mut total = ImportReport::default();
        for source in roots.sources() {
            let started = Instant::now();
            let report = Self::run_sources(store, machine_id, &[source.as_ref()]);
            let run = ImportRun {
                agent: source.agent().as_str().to_owned(),
                at: now(),
                duration_ms: started.elapsed().as_millis() as u64,
                scanned: report.scanned as u64,
                imported: report.imported as u64,
                unchanged: report.skipped_unchanged as u64,
                empty: report.empty as u64,
                failed: report.failed as u64,
                malformed: report.malformed as u64,
                unsupported: report.unsupported as u64,
            };
            if let Err(error) = store.record_import_run(machine_id, &run) {
                tracing::debug!(%error, "cannot record an import run");
            }
            total += report;
        }
        total
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
                    // A part of the source that could not be read is still a
                    // problem to report, though the rest was listed.
                    let problems = source.problems();
                    if !problems.is_empty() {
                        report.failed += 1;
                        report.unsupported += 1;
                    }
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
                    if matches!(error, HistoryError::Unsupported { .. }) {
                        report.unsupported += 1;
                    }
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
    use leon_core::{AgentId, Role, SearchQuery, SessionFilter};
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
        assert_eq!(agents, [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE]);

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
            agent: Some(AgentId::CLAUDE),
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
        fn agent(&self) -> AgentId {
            AgentId::CLAUDE
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

    fn one_claude_file(dir: &Path) -> (HistoryRoots, std::path::PathBuf) {
        let roots = HistoryRoots {
            claude_projects: Some(dir.join("claude")),
            codex_sessions: None,
            opencode_db: None,
        };
        (roots, dir.join("claude/project/cut-session.jsonl"))
    }

    fn count_messages(store: &Store) -> usize {
        let sessions = store
            .recent_sessions(&SessionFilter::default(), 10)
            .unwrap();
        sessions.first().map_or(0, |s| s.message_count as usize)
    }

    #[test]
    fn a_file_cut_mid_line_by_a_power_loss_is_imported_up_to_its_last_complete_line_and_again_when_it_grows(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (roots, file) = one_claude_file(dir.path());
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let first = claude_line("user", "first question", 0);
        let second = claude_line("assistant", "first answer", 1);
        let third = claude_line("user", "second question", 2);
        // The third line is cut in the middle of its JSON, and the cut may
        // even end in zero bytes, as a file system leaves after a power cut.
        let cut = &third[..third.len() / 2];
        fs::write(&file, format!("{first}\n{second}\n{cut}\0\0\0")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let report = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(report.imported, 1, "{report:?}");
        assert_eq!(count_messages(&store), 2);
        // Nothing changed: not read again.
        let again = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(again.skipped_unchanged, 1);
        // The file later grows (the agent resumed and wrote on): re-imported.
        fs::write(&file, format!("{first}\n{second}\n{third}\n")).unwrap();
        let grown = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(grown.imported, 1, "{grown:?}");
        assert_eq!(count_messages(&store), 3);
    }

    #[test]
    fn a_zero_length_file_is_empty_not_a_failure_and_is_imported_when_it_gets_content() {
        let dir = tempfile::tempdir().unwrap();
        let (roots, file) = one_claude_file(dir.path());
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"").unwrap();
        let store = Store::open_in_memory().unwrap();
        let report = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(
            (report.empty, report.failed, report.imported),
            (1, 0, 0),
            "{report:?}"
        );
        fs::write(&file, claude_line("user", "hello", 0) + "\n").unwrap();
        let later = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(later.imported, 1, "{later:?}");
    }

    #[test]
    fn each_run_is_recorded_per_agent_and_an_unknown_layout_is_counted() {
        let home = tempfile::tempdir().unwrap();
        let mut roots = fixture(home.path());
        let store = Store::open_in_memory().unwrap();
        let stamp = DateTime::from_timestamp_millis(7_000).unwrap();
        Importer::run_with_clock(&store, &MachineId::local(), &roots, || stamp);
        let overview = store.history_overview(&MachineId::local()).unwrap();
        assert_eq!(overview.len(), 3);
        let claude = overview.iter().find(|row| row.agent == "claude").unwrap();
        assert_eq!(claude.sessions, 1);
        assert_eq!(claude.last_run.as_ref().unwrap().at, stamp);
        assert_eq!(claude.last_run.as_ref().unwrap().imported, 1);

        // A database with another layout is reported, not silently empty.
        let odd = home.path().join("odd.db");
        rusqlite::Connection::open(&odd)
            .unwrap()
            .execute_batch("CREATE TABLE other (id TEXT);")
            .unwrap();
        roots.opencode_db = Some(odd);
        let report = Importer::run(&store, &MachineId::local(), &roots);
        assert_eq!(report.unsupported, 1);
        assert!(report.failed >= 1);
    }
}
