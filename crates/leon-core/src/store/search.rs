//! Full-text search over the unified session history.
//!
//! Two FTS5 indexes back the search: one over message text and one over
//! session titles. A query runs against both and the hits are merged, best
//! first. The indexes use the `unicode61` tokenizer with diacritics removed,
//! so matching is case-insensitive and `resume` finds `résumé`.
//!
//! What the user types is never handed to FTS5 as-is. FTS5 has its own query
//! language, and stray quotes, parentheses or words such as `AND` would
//! otherwise raise syntax errors while the user is still typing.
//! [`fts_query`] turns free text into a query that is always valid: each
//! whitespace-separated term becomes a quoted phrase, every term must match,
//! and the last term matches as a prefix so results appear as the user types.
//!
//! Snippets mark the matched words with two control characters,
//! [`SNIPPET_START`] and [`SNIPPET_END`], instead of markup. They cannot be
//! confused with anything in ordinary text and the caller decides how to
//! render them; [`snippet_segments`] splits a snippet accordingly.

use rusqlite::params;

use super::history::{session_from_row, sql_limit, SESSION_COLUMNS, SESSION_COLUMN_COUNT};
use super::{bad_tag, Store};
use crate::error::Result;
use crate::ids::{MachineId, ProjectId};
use crate::model::{AgentId, Role, Session};

/// Placed immediately before each matched word in a snippet (U+0002).
pub const SNIPPET_START: char = '\u{2}';

/// Placed immediately after each matched word in a snippet (U+0003).
pub const SNIPPET_END: char = '\u{3}';

/// Placed at either end of a snippet that was cut out of a longer text.
pub const SNIPPET_ELLIPSIS: &str = "…";

/// Roughly how many words a snippet contains.
const SNIPPET_WORDS: u32 = 16;

/// Upper bound on the number of terms taken from the user's input. Anything
/// beyond it is ignored, which keeps pasted paragraphs from producing
/// enormous queries.
const MAX_TERMS: usize = 16;

/// What to search for and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Free text typed by the user.
    pub text: String,
    /// Only sessions of this agent.
    pub agent: Option<AgentId>,
    /// Only sessions that ran on this machine.
    pub machine_id: Option<MachineId>,
    /// Only sessions linked to this project.
    pub project_id: Option<ProjectId>,
    /// Maximum number of hits to return.
    pub limit: usize,
}

impl SearchQuery {
    /// A query for `text` across the whole history, returning up to 50 hits.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            agent: None,
            machine_id: None,
            project_id: None,
            limit: 50,
        }
    }
}

/// One match of a [`SearchQuery`].
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// The session the match belongs to.
    pub session: Session,
    /// Position of the matching message within the session, or `None` when
    /// the session title matched.
    pub seq: Option<u32>,
    /// Role of the matching message, or `None` when the session title
    /// matched.
    pub role: Option<Role>,
    /// Excerpt around the match, with matched words wrapped in
    /// [`SNIPPET_START`] and [`SNIPPET_END`].
    pub snippet: String,
    /// Relevance score: lower is better. Only meaningful relative to other
    /// hits of the same search.
    pub rank: f64,
}

impl Store {
    /// Searches message text and session titles, best matches first.
    ///
    /// Input that contains nothing searchable yields no hits rather than an
    /// error.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        let Some(expression) = fts_query(&query.text) else {
            return Ok(Vec::new());
        };
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let agent = query.agent.map(AgentId::as_str);
        let machine_id = query.machine_id.as_ref().map(MachineId::as_str);
        let project_id = query.project_id.as_ref().map(ProjectId::as_str);
        let (start, end) = (SNIPPET_START.to_string(), SNIPPET_END.to_string());
        let limit = sql_limit(query.limit);

        let mut hits = self.read(|connection| {
            let mut hits: Vec<SearchHit> = Vec::new();

            let mut in_titles = connection.prepare_cached(&format!(
                "SELECT {SESSION_COLUMNS},
                        snippet(session_fts, 0, ?5, ?6, ?7, {SNIPPET_WORDS}),
                        session_fts.rank
                 FROM session_fts
                 JOIN session s ON s.pk = session_fts.rowid
                 WHERE session_fts MATCH ?1
                   AND (?2 IS NULL OR s.agent = ?2)
                   AND (?3 IS NULL OR s.machine_id = ?3)
                   AND (?4 IS NULL OR s.project_id = ?4)
                 ORDER BY session_fts.rank
                 LIMIT ?8"
            ))?;
            let rows = in_titles.query_map(
                params![
                    expression,
                    agent,
                    machine_id,
                    project_id,
                    start,
                    end,
                    SNIPPET_ELLIPSIS,
                    limit
                ],
                |row| {
                    Ok(SearchHit {
                        session: session_from_row(row, 0)?,
                        seq: None,
                        role: None,
                        snippet: row.get(SESSION_COLUMN_COUNT)?,
                        rank: row.get(SESSION_COLUMN_COUNT + 1)?,
                    })
                },
            )?;
            for hit in rows {
                hits.push(hit?);
            }

            let mut in_messages = connection.prepare_cached(&format!(
                "SELECT {SESSION_COLUMNS}, m.seq, m.role,
                        snippet(message_fts, 0, ?5, ?6, ?7, {SNIPPET_WORDS}),
                        message_fts.rank
                 FROM message_fts
                 JOIN message m ON m.id = message_fts.rowid
                 JOIN session s ON s.pk = m.session_pk
                 WHERE message_fts MATCH ?1
                   AND (?2 IS NULL OR s.agent = ?2)
                   AND (?3 IS NULL OR s.machine_id = ?3)
                   AND (?4 IS NULL OR s.project_id = ?4)
                 ORDER BY message_fts.rank
                 LIMIT ?8"
            ))?;
            let rows = in_messages.query_map(
                params![
                    expression,
                    agent,
                    machine_id,
                    project_id,
                    start,
                    end,
                    SNIPPET_ELLIPSIS,
                    limit
                ],
                |row| {
                    let role: String = row.get(SESSION_COLUMN_COUNT + 1)?;
                    Ok(SearchHit {
                        session: session_from_row(row, 0)?,
                        seq: Some(row.get(SESSION_COLUMN_COUNT)?),
                        role: Some(
                            Role::parse(&role)
                                .ok_or_else(|| bad_tag(SESSION_COLUMN_COUNT + 1, &role))?,
                        ),
                        snippet: row.get(SESSION_COLUMN_COUNT + 2)?,
                        rank: row.get(SESSION_COLUMN_COUNT + 3)?,
                    })
                },
            )?;
            for hit in rows {
                hits.push(hit?);
            }
            Ok(hits)
        })?;

        hits.sort_by(|a, b| {
            a.rank
                .total_cmp(&b.rank)
                .then_with(|| b.session.updated_at.cmp(&a.session.updated_at))
                .then_with(|| a.seq.cmp(&b.seq))
        });
        hits.truncate(query.limit);
        Ok(hits)
    }
}

/// Turns free text into an FTS5 query that cannot fail to parse.
///
/// Returns `None` when the input holds nothing searchable. Otherwise each
/// whitespace-separated term becomes a quoted phrase (so punctuation inside a
/// term, as in `foo.bar`, requires the parts to be adjacent), all terms must
/// match, and the final term matches as a prefix.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .map(|term| {
            term.chars()
                .filter(|c| *c != '"' && !c.is_control())
                .collect::<String>()
        })
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .take(MAX_TERMS)
        .collect();
    if terms.is_empty() {
        return None;
    }
    let mut query = String::new();
    for term in &terms {
        if !query.is_empty() {
            query.push(' ');
        }
        query.push('"');
        query.push_str(term);
        query.push('"');
    }
    query.push('*');
    Some(query)
}

/// Splits a snippet into pieces, each flagged with whether it is a matched
/// word. Concatenating the pieces gives the snippet without its markers.
pub fn snippet_segments(snippet: &str) -> Vec<(bool, &str)> {
    let mut segments = Vec::new();
    let mut rest = snippet;
    while let Some(start) = rest.find(SNIPPET_START) {
        if start > 0 {
            segments.push((false, &rest[..start]));
        }
        let after = &rest[start + SNIPPET_START.len_utf8()..];
        let end = after.find(SNIPPET_END).unwrap_or(after.len());
        if end > 0 {
            segments.push((true, &after[..end]));
        }
        rest = after.get(end + SNIPPET_END.len_utf8()..).unwrap_or("");
    }
    if !rest.is_empty() {
        segments.push((false, rest));
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MachineKind, NewMessage, NewSession};
    use chrono::{DateTime, Utc};

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn session(external_id: &str, title: &str) -> NewSession {
        NewSession {
            agent: AgentId::CLAUDE,
            external_id: external_id.into(),
            machine_id: MachineId::local(),
            cwd: "/srv/api".into(),
            title: title.into(),
            model: None,
            started_at: at(0),
            updated_at: at(0),
        }
    }

    fn message(role: Role, text: &str) -> NewMessage {
        NewMessage {
            role,
            text: text.into(),
            at: at(0),
        }
    }

    fn store_with(sessions: &[(&str, &str, &[&str])]) -> std::sync::Arc<Store> {
        let store = Store::open_in_memory().unwrap();
        for (external_id, title, texts) in sessions {
            let messages: Vec<_> = texts.iter().map(|t| message(Role::User, t)).collect();
            store
                .upsert_session(&session(external_id, title), &messages)
                .unwrap();
        }
        store
    }

    fn plain(snippet: &str) -> String {
        snippet_segments(snippet)
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    }

    #[test]
    fn a_word_in_a_message_is_found_with_a_highlighted_snippet() {
        let store = store_with(&[(
            "s1",
            "Untitled",
            &["first line", "the migration failed on the staging database"],
        )]);
        let hits = store.search(&SearchQuery::new("migration")).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.external_id, "s1");
        assert_eq!(hits[0].seq, Some(1));
        assert_eq!(hits[0].role, Some(Role::User));
        assert_eq!(
            hits[0].snippet,
            format!("the {SNIPPET_START}migration{SNIPPET_END} failed on the staging database")
        );
    }

    #[test]
    fn a_word_in_a_title_is_found_without_a_message_position() {
        let store = store_with(&[("s1", "Refactor the billing module", &["hello"])]);
        let hits = store.search(&SearchQuery::new("billing")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].seq, None);
        assert_eq!(hits[0].role, None);
        assert_eq!(plain(&hits[0].snippet), "Refactor the billing module");
    }

    #[test]
    fn every_term_must_match() {
        let store = store_with(&[
            ("both", "a", &["deploy the worker to staging"]),
            ("one", "b", &["deploy the frontend"]),
        ]);
        let hits = store.search(&SearchQuery::new("deploy staging")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.external_id, "both");
    }

    #[test]
    fn the_last_term_matches_as_a_prefix() {
        let store = store_with(&[("s1", "a", &["investigate the websocket reconnect loop"])]);
        for typed in ["w", "web", "websock", "the webso", "WEBSOCKET"] {
            assert_eq!(
                store.search(&SearchQuery::new(typed)).unwrap().len(),
                1,
                "typed {typed:?}"
            );
        }
        assert!(store
            .search(&SearchQuery::new("webso the"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn matching_ignores_case_and_diacritics() {
        let store = store_with(&[("s1", "a", &["Update the Résumé parser"])]);
        assert_eq!(store.search(&SearchQuery::new("resume")).unwrap().len(), 1);
        assert_eq!(store.search(&SearchQuery::new("UPDATE")).unwrap().len(), 1);
    }

    #[test]
    fn better_matches_come_first() {
        let store = store_with(&[
            (
                "weak",
                "a",
                &[
                    "a long message that mentions cache once among many other unrelated words \
                   about servers, queues, workers, retries, deadlines and dashboards",
                ],
            ),
            ("strong", "b", &["cache cache cache"]),
        ]);
        let hits = store.search(&SearchQuery::new("cache")).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].session.external_id, "strong");
        assert!(hits[0].rank <= hits[1].rank);
    }

    #[test]
    fn hostile_input_never_raises_a_syntax_error() {
        let store = store_with(&[("s1", "a", &["plain text with foo.bar and it's fine"])]);
        let inputs = [
            "",
            "   ",
            "\"",
            "\"\"\"",
            "'",
            "it's",
            "(",
            ")",
            "((()",
            "*",
            "foo*",
            "*foo",
            "^foo",
            "-foo",
            "+foo",
            "foo:",
            "text:foo",
            "col : foo",
            "AND",
            "OR",
            "NOT",
            "foo AND",
            "foo OR bar",
            "NOT foo",
            "NEAR(foo bar, 3)",
            "NEAR(",
            "{foo bar}",
            "foo.bar",
            "foo-bar_baz",
            "a\"b",
            "\"unterminated",
            "terminated\"",
            "semi;colon -- drop",
            "\\",
            "\0",
            "a\0b",
            "\u{2}\u{3}",
            "tab\tseparated",
            "new\nline",
            "émoji 🎉",
            "🎉",
            "日本語",
            "%",
            "_",
            "?",
            "!!!",
            "<tag>",
            "path/to/file.rs",
            "C:\\code\\api",
            "x".repeat(10_000).as_str(),
            "word ".repeat(500).as_str(),
        ]
        .map(str::to_owned);
        for input in inputs {
            let outcome = store.search(&SearchQuery::new(input.clone()));
            assert!(outcome.is_ok(), "input {input:?} failed: {outcome:?}");
        }
    }

    #[test]
    fn punctuation_inside_a_term_requires_adjacent_words() {
        let store = store_with(&[
            ("adjacent", "a", &["call foo.bar here"]),
            ("apart", "b", &["bar comes long before foo"]),
        ]);
        let hits = store.search(&SearchQuery::new("foo.bar")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.external_id, "adjacent");
    }

    #[test]
    fn free_text_becomes_quoted_terms_with_a_prefix_on_the_last() {
        assert_eq!(fts_query("fix login"), Some("\"fix\" \"login\"*".into()));
        assert_eq!(fts_query("  say \"hi\"  "), Some("\"say\" \"hi\"*".into()));
        assert_eq!(fts_query("( ) * -"), None);
        assert_eq!(fts_query(""), None);
    }

    #[test]
    fn hits_can_be_restricted_by_agent_machine_and_project() {
        let store = Store::open_in_memory().unwrap();
        let remote = store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        let project = store
            .add_project(&MachineId::local(), "api", "/srv/api")
            .unwrap();

        let text = [message(Role::User, "tune the scheduler")];
        store
            .upsert_session(&session("local-claude", "a"), &text)
            .unwrap();
        let mut codex = session("local-codex", "b");
        codex.agent = AgentId::CODEX;
        codex.cwd = "/tmp/elsewhere".into();
        store.upsert_session(&codex, &text).unwrap();
        let mut far = session("remote-claude", "c");
        far.machine_id = remote.id.clone();
        store.upsert_session(&far, &text).unwrap();

        let found = |query: SearchQuery| -> Vec<String> {
            let mut ids: Vec<_> = store
                .search(&query)
                .unwrap()
                .into_iter()
                .map(|hit| hit.session.external_id)
                .collect();
            ids.sort();
            ids
        };
        let base = SearchQuery::new("scheduler");
        assert_eq!(found(base.clone()).len(), 3);
        assert_eq!(
            found(SearchQuery {
                agent: Some(AgentId::CODEX),
                ..base.clone()
            }),
            ["local-codex"]
        );
        assert_eq!(
            found(SearchQuery {
                machine_id: Some(remote.id),
                ..base.clone()
            }),
            ["remote-claude"]
        );
        assert_eq!(
            found(SearchQuery {
                project_id: Some(project.id),
                ..base
            }),
            ["local-claude"]
        );
    }

    #[test]
    fn the_limit_caps_the_number_of_hits() {
        let texts: Vec<String> = (0..20).map(|i| format!("needle number {i}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let store = store_with(&[("s1", "a", &refs)]);
        let query = SearchQuery {
            limit: 5,
            ..SearchQuery::new("needle")
        };
        assert_eq!(store.search(&query).unwrap().len(), 5);
        let none = SearchQuery {
            limit: 0,
            ..SearchQuery::new("needle")
        };
        assert!(store.search(&none).unwrap().is_empty());
    }

    #[test]
    fn replaced_and_removed_text_is_no_longer_found() {
        let store = Store::open_in_memory().unwrap();
        let entry = session("s1", "Original heading");
        let id = store
            .upsert_session(&entry, &[message(Role::User, "obsolete wording")])
            .unwrap();

        let mut retitled = entry.clone();
        retitled.title = "Fresh heading".into();
        store
            .upsert_session(&retitled, &[message(Role::User, "current wording")])
            .unwrap();
        assert!(store
            .search(&SearchQuery::new("obsolete"))
            .unwrap()
            .is_empty());
        assert!(store
            .search(&SearchQuery::new("original"))
            .unwrap()
            .is_empty());
        assert_eq!(store.search(&SearchQuery::new("current")).unwrap().len(), 1);
        assert_eq!(store.search(&SearchQuery::new("fresh")).unwrap().len(), 1);

        store.remove_session(&id).unwrap();
        assert!(store
            .search(&SearchQuery::new("current"))
            .unwrap()
            .is_empty());
        assert!(store.search(&SearchQuery::new("fresh")).unwrap().is_empty());
    }

    #[test]
    fn removing_a_machine_leaves_the_search_index_consistent() {
        let store = Store::open_in_memory().unwrap();
        let remote = store
            .add_machine(
                "box",
                MachineKind::Ssh {
                    host: "box.example".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        let mut far = session("remote", "Remote heading");
        far.machine_id = remote.id.clone();
        store
            .upsert_session(&far, &[message(Role::User, "distant wording")])
            .unwrap();

        store.remove_machine(&remote.id).unwrap();

        assert!(store
            .search(&SearchQuery::new("distant"))
            .unwrap()
            .is_empty());
        assert!(store
            .search(&SearchQuery::new("remote"))
            .unwrap()
            .is_empty());
        store
            .write(crate::StoreChange::Everything, |tx| {
                tx.execute_batch(
                    "INSERT INTO message_fts (message_fts) VALUES ('integrity-check');
                     INSERT INTO session_fts (session_fts) VALUES ('integrity-check');",
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_few_thousand_messages_are_searched_correctly() {
        let store = Store::open_in_memory().unwrap();
        let words = [
            "server",
            "queue",
            "worker",
            "retry",
            "deadline",
            "dashboard",
            "schema",
            "index",
            "cursor",
            "branch",
            "commit",
            "review",
            "deploy",
            "config",
            "socket",
            "render",
        ];
        let mut total = 0;
        for s in 0..120 {
            let messages: Vec<NewMessage> = (0..40)
                .map(|m| {
                    let a = words[(s + m) % words.len()];
                    let b = words[(s * 7 + m * 3) % words.len()];
                    let mut text = format!("step {m}: adjust the {a} so the {b} keeps working");
                    if s % 40 == 17 && m == 23 {
                        text.push_str(" and mind the zygomorphic edge case");
                    }
                    message(
                        if m % 2 == 0 {
                            Role::User
                        } else {
                            Role::Assistant
                        },
                        &text,
                    )
                })
                .collect();
            total += messages.len();
            let mut entry = session(&format!("s{s}"), &format!("Session {s}"));
            entry.updated_at = at(s as i64);
            store.upsert_session(&entry, &messages).unwrap();
        }
        assert_eq!(total, 4_800);

        let rare = store.search(&SearchQuery::new("zygomorph")).unwrap();
        let mut found: Vec<_> = rare
            .iter()
            .map(|hit| (hit.session.external_id.clone(), hit.seq))
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                ("s17".to_owned(), Some(23)),
                ("s57".to_owned(), Some(23)),
                ("s97".to_owned(), Some(23))
            ]
        );
        assert!(rare
            .iter()
            .all(|hit| hit.snippet.contains(SNIPPET_START) && hit.snippet.contains(SNIPPET_END)));

        let common = SearchQuery {
            limit: 25,
            ..SearchQuery::new("adjust deadline")
        };
        let hits = store.search(&common).unwrap();
        assert_eq!(hits.len(), 25);
        assert!(hits.windows(2).all(|pair| pair[0].rank <= pair[1].rank));
        assert!(hits
            .iter()
            .all(|hit| plain(&hit.snippet).contains("deadline")));

        let titled = store.search(&SearchQuery::new("session 57")).unwrap();
        assert_eq!(titled[0].session.external_id, "s57");
        assert_eq!(titled[0].seq, None);
    }

    #[test]
    fn a_snippet_splits_into_plain_and_matched_pieces() {
        let snippet = format!("fix {SNIPPET_START}login{SNIPPET_END} flow");
        assert_eq!(
            snippet_segments(&snippet),
            [(false, "fix "), (true, "login"), (false, " flow")]
        );
        assert_eq!(snippet_segments("plain"), [(false, "plain")]);
        assert!(snippet_segments("").is_empty());
    }
}
