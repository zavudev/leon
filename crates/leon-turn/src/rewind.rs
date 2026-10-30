//! Taking an agent's conversation back to before one of its turns.
//!
//! Each agent keeps its conversation its own way, so each way back is its own:
//!
//! * Claude Code keeps it as a transcript file. Its SDK's `forkSession` copies
//!   a transcript under a new id; [`fork_transcript`] does that, cut where the
//!   turn began, and the next turn resumes the copy. The original stays as it
//!   was.
//! * Codex's application server cuts the thread itself: `thread/revert` drops a
//!   turn and every later one ([`CodexRevert`] speaks it).
//! * The agents of the Agent Client Protocol have no way back: the next turn
//!   starts a new conversation.
//!
//! Which one an agent has is [`Rewind`]. Pure: text in, text out, as the rest
//! of the crate; whoever runs the process or reads the file does that.

use serde_json::{json, Value};

/// How an agent's conversation can be taken back to before a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rewind {
    /// Its transcript is a file that can be copied up to that turn, under a
    /// new id ([`fork_transcript`]).
    Fork,
    /// Its server drops the turns itself ([`CodexRevert`]).
    Revert,
    /// It cannot: what is left is a conversation that starts over.
    Reset,
}

/// `transcript` (Claude Code's `.jsonl`) cut after its first `keep` bytes and
/// written as the session `new_id`: every `sessionId` it names becomes the
/// new one. The rest of each line is kept as it was, including the ids of the
/// messages, so what the lines point at in each other still holds.
///
/// `keep` is where the turn began: the size of the file just before the
/// prompt was sent. `None` when that is not a place where a line ends (the
/// file is not the one that was measured).
pub fn fork_transcript(transcript: &str, keep: usize, new_id: &str) -> Option<String> {
    let kept = transcript.get(..keep)?;
    if !kept.is_empty() && !kept.ends_with('\n') {
        return None;
    }
    let mut forked = String::with_capacity(kept.len());
    for line in kept.lines() {
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(mut entry))
                if entry.get("sessionId").is_some_and(Value::is_string) =>
            {
                entry.insert("sessionId".to_owned(), Value::from(new_id));
                forked.push_str(&Value::Object(entry).to_string());
            }
            // Not an entry of the session (or not JSON at all): as it was.
            _ => forked.push_str(line),
        }
        forked.push('\n');
    }
    Some(forked)
}

/// The id of the opening request.
const ID_HELLO: &str = "leon-init";
/// The id of the request that loads the thread.
const ID_RESUME: &str = "leon-resume";
/// The id of the request that drops the turns.
const ID_REVERT: &str = "leon-revert";
/// What of the id of a request for turns is the same for all of them.
const ID_TURNS: &str = "leon-turns-";
/// How many turns are asked for at once.
const PAGE: usize = 100;

/// What reading a line of Codex's server led to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevertStep {
    /// Nothing to do yet: it was a notification, or an answer to nothing here.
    Wait,
    /// Write these lines.
    Send(Vec<String>),
    /// The thread was cut (or had nothing to cut): the server may be left.
    Done,
    /// It did not work, and this is why.
    Failed(String),
}

/// A conversation with Codex's application server that drops the last
/// `turns` turns of a thread, as `thread/revert` does: that request takes the
/// id of the first turn to drop, which is found by asking for the thread's
/// turns, newest first.
///
/// It only changes what Codex remembers; the files stay as they are.
#[derive(Debug, Clone)]
pub struct CodexRevert {
    thread: String,
    turns: usize,
    /// How many turns are still to be counted back.
    remaining: usize,
    /// The oldest turn counted so far.
    first: Option<String>,
    pages: usize,
}

impl CodexRevert {
    /// A revert of the last `turns` turns of `thread`.
    pub fn new(thread: &str, turns: usize) -> Self {
        Self {
            thread: thread.to_owned(),
            turns,
            remaining: turns,
            first: None,
            pages: 0,
        }
    }

    fn request(id: &str, method: &str, params: Value) -> String {
        json!({ "id": id, "method": method, "params": params }).to_string()
    }

    /// What is written first: the handshake, and the request that loads the
    /// thread (without its history, which is not wanted here).
    pub fn first(&self) -> Vec<String> {
        vec![
            crate::codex::hello(),
            json!({ "method": "initialized" }).to_string(),
            Self::request(
                ID_RESUME,
                "thread/resume",
                json!({ "threadId": self.thread, "excludeTurns": true }),
            ),
        ]
    }

    /// One JSON value the server printed.
    pub fn line(&mut self, value: &Value) -> RevertStep {
        let Some(id) = value["id"].as_str() else {
            return RevertStep::Wait;
        };
        if value["method"].is_string() {
            // A request of the server's own: nobody here serves it.
            return RevertStep::Wait;
        }
        if !value["error"].is_null() {
            let said = value["error"]["message"].as_str().unwrap_or("it failed");
            return RevertStep::Failed(format!("Codex could not cut the conversation: {said}"));
        }
        match id {
            ID_HELLO => RevertStep::Wait,
            ID_RESUME if self.turns == 0 => RevertStep::Done,
            ID_RESUME => RevertStep::Send(vec![self.page(None)]),
            ID_REVERT => RevertStep::Done,
            _ if id.starts_with(ID_TURNS) => self.listed(&value["result"]),
            _ => RevertStep::Wait,
        }
    }

    fn page(&mut self, cursor: Option<&str>) -> String {
        self.pages += 1;
        Self::request(
            &format!("{ID_TURNS}{}", self.pages),
            "thread/turns/list",
            json!({
                "threadId": self.thread,
                "cursor": cursor,
                "limit": self.remaining.min(PAGE),
                "sortDirection": "desc",
                "itemsView": "summary",
            }),
        )
    }

    fn listed(&mut self, page: &Value) -> RevertStep {
        for turn in page["data"].as_array().into_iter().flatten() {
            if let Some(id) = turn["id"].as_str() {
                self.first = Some(id.to_owned());
                self.remaining = self.remaining.saturating_sub(1);
                if self.remaining == 0 {
                    break;
                }
            }
        }
        let more = page["nextCursor"].as_str().map(str::to_owned);
        match (self.remaining, more) {
            (remaining, Some(cursor)) if remaining > 0 && self.pages < 1_000 => {
                RevertStep::Send(vec![self.page(Some(&cursor))])
            }
            // Fewer turns than were asked for: all of them go.
            _ => match &self.first {
                Some(before) => RevertStep::Send(vec![Self::request(
                    ID_REVERT,
                    "thread/revert",
                    json!({ "threadId": self.thread, "beforeTurnId": before }),
                )]),
                None => RevertStep::Done,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(step: RevertStep) -> Vec<Value> {
        match step {
            RevertStep::Send(lines) => lines
                .iter()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            other => panic!("{other:?}"),
        }
    }

    fn answer(id: &str, result: Value) -> Value {
        json!({ "id": id, "result": result })
    }

    #[test]
    fn a_transcript_is_cut_where_the_turn_began_and_becomes_another_session() {
        let first = r#"{"type":"user","uuid":"u1","sessionId":"old","message":{"content":"one"}}"#;
        let reply = r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"old","message":{"content":"hi"}}"#;
        let mode = r#"{"type":"mode","mode":"normal","sessionId":"old"}"#;
        let second = r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"old","message":{"content":"two"}}"#;
        let snapshot = r#"{"type":"file-history-snapshot","messageId":"u1","snapshot":{}}"#;
        let transcript = format!("{first}\n{snapshot}\n{reply}\n{mode}\n{second}\n");
        let keep = transcript.find(second).unwrap();
        let forked = fork_transcript(&transcript, keep, "new").unwrap();
        let entries: Vec<Value> = forked
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(entries.len(), 4, "the second prompt is not in it: {forked}");
        assert!(!forked.contains("two"));
        for entry in &entries {
            if entry.get("sessionId").is_some() {
                assert_eq!(entry["sessionId"], "new", "{entry}");
            }
        }
        // What points at what is as it was; an entry with no session stays so.
        assert_eq!(entries[2]["parentUuid"], "u1");
        assert_eq!(entries[1].get("sessionId"), None);
        assert!(!forked.contains("old"));
        // The same file, cut at its start, is nothing at all.
        assert_eq!(fork_transcript(&transcript, 0, "new").as_deref(), Some(""));
    }

    #[test]
    fn a_cut_that_is_not_where_a_line_ends_is_refused() {
        let transcript = "{\"sessionId\":\"old\"}\n{\"sessionId\":\"old\"}\n";
        assert_eq!(fork_transcript(transcript, 5, "new"), None);
        assert_eq!(fork_transcript(transcript, 1_000, "new"), None);
        assert!(fork_transcript(transcript, transcript.len(), "new").is_some());
        // What is not JSON is kept as it is.
        let odd = "not json\n{\"sessionId\":\"old\"}\n";
        assert_eq!(
            fork_transcript(odd, odd.len(), "n").unwrap(),
            "not json\n{\"sessionId\":\"n\"}\n"
        );
    }

    #[test]
    fn the_first_lines_load_the_thread_without_its_history() {
        let revert = CodexRevert::new("th1", 2);
        let first: Vec<Value> = revert
            .first()
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(first[0]["method"], "initialize");
        assert_eq!(first[1]["method"], "initialized");
        assert_eq!(first[2]["method"], "thread/resume");
        assert_eq!(first[2]["params"]["threadId"], "th1");
        assert_eq!(first[2]["params"]["excludeTurns"], true);
    }

    #[test]
    fn the_turns_are_counted_back_from_the_newest_and_the_oldest_one_is_the_cut() {
        let mut revert = CodexRevert::new("th1", 2);
        assert_eq!(revert.line(&answer(ID_HELLO, json!({}))), RevertStep::Wait);
        let asked = lines(revert.line(&answer(ID_RESUME, json!({}))));
        assert_eq!(asked[0]["method"], "thread/turns/list");
        assert_eq!(asked[0]["params"]["threadId"], "th1");
        assert_eq!(asked[0]["params"]["sortDirection"], "desc");
        assert_eq!(asked[0]["params"]["limit"], 2);
        let id = asked[0]["id"].as_str().unwrap().to_owned();
        // The newest first: t3, t2, t1. Two to drop: t3 and t2.
        let reverted = lines(revert.line(&answer(
            &id,
            json!({ "data": [{ "id": "t3" }, { "id": "t2" }], "nextCursor": "more" }),
        )));
        assert_eq!(reverted[0]["method"], "thread/revert");
        assert_eq!(
            reverted[0]["params"],
            json!({ "threadId": "th1", "beforeTurnId": "t2" })
        );
        assert_eq!(revert.line(&answer(ID_REVERT, json!({}))), RevertStep::Done);
    }

    #[test]
    fn a_thread_with_fewer_turns_than_asked_loses_all_of_them() {
        let mut revert = CodexRevert::new("th1", 5);
        let asked = lines(revert.line(&answer(ID_RESUME, json!({}))));
        let id = asked[0]["id"].as_str().unwrap().to_owned();
        let more = lines(revert.line(&answer(
            &id,
            json!({ "data": [{ "id": "t2" }, { "id": "t1" }], "nextCursor": "c1" }),
        )));
        // Not enough yet, and there is more: the next page, asked for 3 more.
        assert_eq!(more[0]["method"], "thread/turns/list");
        assert_eq!(more[0]["params"]["cursor"], "c1");
        assert_eq!(more[0]["params"]["limit"], 3);
        let id = more[0]["id"].as_str().unwrap().to_owned();
        let reverted = lines(revert.line(&answer(&id, json!({ "data": [], "nextCursor": null }))));
        assert_eq!(reverted[0]["params"]["beforeTurnId"], "t1");
    }

    #[test]
    fn a_thread_without_turns_has_nothing_to_cut_and_an_error_is_a_failure() {
        let mut revert = CodexRevert::new("th1", 1);
        let asked = lines(revert.line(&answer(ID_RESUME, json!({}))));
        let id = asked[0]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            revert.line(&answer(&id, json!({ "data": [], "nextCursor": null }))),
            RevertStep::Done
        );
        let mut revert = CodexRevert::new("th1", 1);
        let failed =
            revert.line(&json!({ "id": ID_RESUME, "error": { "message": "no such thread" } }));
        assert_eq!(
            failed,
            RevertStep::Failed("Codex could not cut the conversation: no such thread".into())
        );
        // No turns to drop: the thread is loaded and that is all.
        let mut nothing = CodexRevert::new("th1", 0);
        assert_eq!(
            nothing.line(&answer(ID_RESUME, json!({}))),
            RevertStep::Done
        );
        // The server's own requests and notifications are not answers.
        assert_eq!(
            revert.line(&json!({ "method": "thread/started", "params": {} })),
            RevertStep::Wait
        );
    }
}
