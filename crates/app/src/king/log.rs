//! The files of King's event log.
//!
//! What a log is, and how much of it is kept, is `leon_king::log`'s. This
//! is where it lives: `<king folder>/log/<conversation>/<target>.jsonl`
//! (`brain.jsonl`, `worker-<n>.jsonl`), one JSON line per record, appended
//! as the conversation goes. The service is the only writer. A conversation
//! put away keeps its folder; a worker's log is in the folder of the
//! conversation that started it.
//!
//! A line that did not get written whole (the service was killed) is cut
//! off when the log is opened, so that what comes after it is a line.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use leon_king::coordinator::{Batch, Target};
use leon_king::log::{self, Events, Logged, Out, Record};

/// What is known of a log without reading it again.
#[derive(Debug, Clone, Copy, Default)]
struct Tail {
    /// The number the next record gets.
    next: u64,
    /// How many records, and how many bytes, the file holds.
    records: usize,
    bytes: usize,
}

/// King's logs, by conversation and target.
pub struct Logs {
    root: PathBuf,
    tails: HashMap<(u64, Target), Tail>,
}

impl Logs {
    /// The logs under `root` (`<king folder>/log`).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tails: HashMap::new(),
        }
    }

    fn folder(&self, conversation: u64) -> PathBuf {
        self.root.join(conversation.to_string())
    }

    fn path(&self, conversation: u64, target: Target) -> PathBuf {
        let name = match target {
            Target::Brain => "brain.jsonl".to_owned(),
            Target::Worker(id) => format!("worker-{id}.jsonl"),
        };
        self.folder(conversation).join(name)
    }

    /// What the file holds, read once and cut off after its last whole line.
    fn open(&mut self, conversation: u64, target: Target) -> Tail {
        if let Some(tail) = self.tails.get(&(conversation, target)) {
            return *tail;
        }
        let path = self.path(conversation, target);
        let mut tail = Tail::default();
        if let Ok(bytes) = std::fs::read(&path) {
            let text = String::from_utf8_lossy(&bytes);
            let mut whole = 0;
            for line in text.split_inclusive('\n') {
                let Some(seq) = line.strip_suffix('\n').and_then(log::peek) else {
                    break;
                };
                whole += line.len();
                tail.next = tail.next.max(seq + 1);
                tail.records += 1;
                tail.bytes += line.len();
            }
            if whole < bytes.len() {
                let cut = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .and_then(|file| file.set_len(whole as u64));
                if let Err(error) = cut {
                    tracing::warn!(%error, "a torn line of Orchestrator's log could not be cut off");
                }
            }
        }
        self.tails.insert((conversation, target), tail);
        tail
    }

    /// The number of the newest record, when the log has one.
    pub fn last_seq(&mut self, conversation: u64, target: Target) -> Option<u64> {
        self.open(conversation, target).next.checked_sub(1)
    }

    /// Writes what a conversation gave: records numbered after the log's
    /// last, and, first, the entries it had before the log when the log
    /// has nothing yet.
    pub fn append(&mut self, batch: Batch) {
        let Batch {
            target,
            conversation,
            items,
        } = batch;
        let mut tail = self.open(conversation, target);
        let mut text = String::new();
        let mut added = 0;
        for out in items {
            let records: Vec<(u64, Record)> = match out {
                Out::Seed(entries) if tail.records == 0 && added == 0 => entries
                    .into_iter()
                    .map(|entry| (entry.at_ms, Record::Legacy { entry }))
                    .collect(),
                Out::Seed(_) => Vec::new(),
                Out::Record { at_ms, record } => vec![(at_ms, record)],
            };
            for (at_ms, record) in records {
                let line = log::line(&Logged {
                    seq: tail.next,
                    at_ms,
                    record,
                });
                text.push_str(&line);
                text.push('\n');
                tail.next += 1;
                added += 1;
            }
        }
        if text.is_empty() {
            return;
        }
        let path = self.path(conversation, target);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
            })
            .and_then(|mut file| file.write_all(text.as_bytes()));
        match written {
            Ok(()) => {
                tail.records += added;
                tail.bytes += text.len();
                self.tails.insert((conversation, target), tail);
                if tail.records > log::MAX_RECORDS || tail.bytes > log::MAX_BYTES {
                    self.compact(conversation, target);
                }
            }
            Err(error) => {
                tracing::warn!(%error, ?target, "Orchestrator's log could not be written");
                // What the file holds is not known any more.
                self.tails.remove(&(conversation, target));
            }
        }
    }

    /// Drops the oldest records of a log that holds more than it keeps.
    fn compact(&mut self, conversation: u64, target: Target) {
        let path = self.path(conversation, target);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let lines: Vec<&str> = text.lines().collect();
        let Some(from) = log::cut_at(&lines) else {
            return;
        };
        let mut kept = lines[from..].join("\n");
        kept.push('\n');
        let temporary = path.with_extension("jsonl.new");
        let written =
            std::fs::write(&temporary, &kept).and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(error) = written {
            tracing::warn!(%error, "Orchestrator's log could not be compacted");
            return;
        }
        let next = self
            .tails
            .get(&(conversation, target))
            .map_or(0, |tail| tail.next);
        self.tails.insert(
            (conversation, target),
            Tail {
                next,
                records: lines.len() - from,
                bytes: kept.len(),
            },
        );
    }

    /// What came after `since` in a log: a page of it.
    pub fn read(
        &mut self,
        conversation: u64,
        target: Target,
        since: Option<u64>,
        limit: Option<usize>,
    ) -> Events {
        // Cut off what was torn before anything is read of it.
        self.open(conversation, target);
        let text = std::fs::read_to_string(self.path(conversation, target)).unwrap_or_default();
        let limit = limit.unwrap_or(log::PAGE_RECORDS).min(log::PAGE_RECORDS);
        let page = log::page(text.lines(), since, limit, log::PAGE_BYTES);
        Events {
            conversation,
            events: page.events,
            last_seq: page.last_seq,
            more: page.more,
        }
    }

    /// Takes away the log of King's conversation `conversation`, which fell
    /// off the list of past ones. A worker's log in its folder stays: the
    /// worker may still be there.
    pub fn forget(&mut self, conversation: u64) {
        self.tails.remove(&(conversation, Target::Brain));
        let _ = std::fs::remove_file(self.path(conversation, Target::Brain));
        // Only when nothing else is in it.
        let _ = std::fs::remove_dir(self.folder(conversation));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_king::transcript::{Entry, Kind};
    use leon_turn::TurnEvent;

    fn record(text: &str) -> Out {
        Out::Record {
            at_ms: 7,
            record: Record::Event {
                event: TurnEvent::Text { text: text.into() },
            },
        }
    }

    fn batch(target: Target, conversation: u64, items: Vec<Out>) -> Batch {
        Batch {
            target,
            conversation,
            items,
        }
    }

    fn entry(seq: u64, text: &str) -> Entry {
        Entry {
            seq,
            at_ms: 3,
            kind: Kind::Prompt,
            text: text.into(),
            report: None,
        }
    }

    fn texts(events: &Events) -> Vec<String> {
        events
            .events
            .iter()
            .map(|logged| match &logged.record {
                Record::Event {
                    event: TurnEvent::Text { text },
                } => text.clone(),
                Record::Legacy { entry } => format!("legacy {}", entry.text),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn records_are_numbered_and_kept_in_a_file_per_conversation_and_target() {
        let dir = tempfile::tempdir().unwrap();
        let mut logs = Logs::new(dir.path().join("log"));
        assert_eq!(logs.last_seq(4, Target::Brain), None);
        logs.append(batch(Target::Brain, 4, vec![record("a"), record("b")]));
        logs.append(batch(Target::Brain, 4, vec![record("c")]));
        logs.append(batch(Target::Worker(2), 4, vec![record("w")]));
        logs.append(batch(Target::Brain, 5, vec![record("next conversation")]));
        assert!(dir.path().join("log/4/brain.jsonl").is_file());
        assert!(dir.path().join("log/4/worker-2.jsonl").is_file());
        assert!(dir.path().join("log/5/brain.jsonl").is_file());
        let read = logs.read(4, Target::Brain, None, None);
        assert_eq!(texts(&read), ["a", "b", "c"]);
        assert_eq!(
            (read.conversation, read.last_seq, read.more),
            (4, Some(2), false)
        );
        assert_eq!(logs.last_seq(4, Target::Worker(2)), Some(0));
        assert_eq!(texts(&logs.read(4, Target::Brain, Some(1), None)), ["c"]);
        let none = logs.read(9, Target::Brain, None, None);
        assert_eq!((none.events.len(), none.last_seq), (0, None));
    }

    #[test]
    fn numbers_go_on_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("log");
        {
            let mut logs = Logs::new(root.clone());
            logs.append(batch(Target::Brain, 0, vec![record("a"), record("b")]));
        }
        let mut logs = Logs::new(root);
        assert_eq!(logs.last_seq(0, Target::Brain), Some(1));
        logs.append(batch(Target::Brain, 0, vec![record("c")]));
        let read = logs.read(0, Target::Brain, None, None);
        let seqs: Vec<u64> = read.events.iter().map(|logged| logged.seq).collect();
        assert_eq!(seqs, [0, 1, 2]);
    }

    #[test]
    fn a_line_torn_by_a_kill_is_cut_off_before_the_next_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("log");
        let mut logs = Logs::new(root.clone());
        logs.append(batch(Target::Brain, 0, vec![record("a")]));
        let path = root.join("0/brain.jsonl");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"seq":1,"at_ms":7,"record":"ev"#)
            .unwrap();
        drop(file);

        let mut logs = Logs::new(root);
        logs.append(batch(Target::Brain, 0, vec![record("b")]));
        let read = logs.read(0, Target::Brain, None, None);
        assert_eq!(texts(&read), ["a", "b"]);
        let seqs: Vec<u64> = read.events.iter().map(|logged| logged.seq).collect();
        assert_eq!(seqs, [0, 1], "the torn record's number is the next one's");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().all(|line| log::peek(line).is_some()), "{text}");
    }

    #[test]
    fn what_was_kept_before_the_log_is_written_first_and_only_when_the_log_is_new() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("log");
        let mut logs = Logs::new(root.clone());
        let seed = || Out::Seed(vec![entry(0, "old one"), entry(1, "old two")]);
        logs.append(batch(Target::Brain, 0, vec![seed(), record("new")]));
        assert_eq!(
            texts(&logs.read(0, Target::Brain, None, None)),
            ["legacy old one", "legacy old two", "new"]
        );
        // After a restart the transcript offers its entries again: the log
        // has records, so they are not written twice.
        let mut logs = Logs::new(root);
        logs.append(batch(Target::Brain, 0, vec![seed(), record("newer")]));
        assert_eq!(
            texts(&logs.read(0, Target::Brain, None, None)),
            ["legacy old one", "legacy old two", "new", "newer"]
        );
        // And a seed of nothing new writes nothing.
        logs.append(batch(Target::Brain, 3, vec![Out::Seed(Vec::new())]));
        assert!(!dir.path().join("log/3").exists());
    }

    #[test]
    fn a_log_over_its_limit_loses_its_oldest_turns_and_keeps_its_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let mut logs = Logs::new(dir.path().join("log"));
        let turn = |n: usize| {
            vec![
                Out::Record {
                    at_ms: 1,
                    record: Record::Prompt {
                        text: format!("q{n}"),
                        attachments: Vec::new(),
                        turn: true,
                    },
                },
                record(&format!("a{n}")),
            ]
        };
        let turns = log::MAX_RECORDS / 2 + 10;
        for n in 0..turns {
            logs.append(batch(Target::Brain, 0, turn(n)));
        }
        let all = logs.read(0, Target::Brain, None, Some(usize::MAX));
        let newest = turns as u64 * 2 - 1;
        assert_eq!(all.last_seq, Some(newest));
        let first = all.events[0].seq;
        assert!(first > 0, "the oldest went");
        assert!(
            matches!(&all.events[0].record, Record::Prompt { .. }),
            "it starts at a turn"
        );
        let text = std::fs::read_to_string(dir.path().join("log/0/brain.jsonl")).unwrap();
        assert!(text.lines().count() <= log::MAX_RECORDS);
        // A client that had up to before the cut is told by the gap.
        let after = logs.read(0, Target::Brain, Some(first.saturating_sub(5)), Some(10));
        assert_eq!(after.events[0].seq, first);
        // And it goes on from where the numbers were.
        logs.append(batch(Target::Brain, 0, vec![record("last")]));
        assert_eq!(logs.last_seq(0, Target::Brain), Some(newest + 1));
    }

    #[test]
    fn a_big_log_is_read_in_pages_that_hold_together() {
        let dir = tempfile::tempdir().unwrap();
        let mut logs = Logs::new(dir.path().join("log"));
        let big = "x".repeat(100_000);
        for n in 0..12 {
            logs.append(batch(Target::Brain, 0, vec![record(&format!("{n}{big}"))]));
        }
        let mut since = None;
        let mut seqs = Vec::new();
        let mut pages = 0;
        loop {
            let page = logs.read(0, Target::Brain, since, None);
            assert!(!page.events.is_empty());
            let line = serde_json::to_string(&page).unwrap();
            assert!(
                line.len() < leon_king::protocol::MAX_LINE / 2,
                "{} bytes",
                line.len()
            );
            seqs.extend(page.events.iter().map(|logged| logged.seq));
            since = page.events.last().map(|logged| logged.seq);
            pages += 1;
            if !page.more {
                break;
            }
        }
        assert_eq!(seqs, (0..12).collect::<Vec<u64>>());
        assert!(pages > 1, "{pages} pages");
    }

    #[test]
    fn a_conversation_that_fell_off_the_list_loses_its_log_and_not_a_workers() {
        let dir = tempfile::tempdir().unwrap();
        let mut logs = Logs::new(dir.path().join("log"));
        logs.append(batch(Target::Brain, 2, vec![record("a")]));
        logs.append(batch(Target::Worker(1), 2, vec![record("w")]));
        logs.append(batch(Target::Brain, 3, vec![record("b")]));
        logs.forget(2);
        logs.forget(3);
        assert!(!dir.path().join("log/2/brain.jsonl").exists());
        assert!(dir.path().join("log/2/worker-1.jsonl").exists());
        assert!(!dir.path().join("log/3").exists());
        assert_eq!(logs.last_seq(2, Target::Brain), None);
    }
}
