//! Token counts: what each session used, per model and UTC day.
//!
//! The history import reads the counts out of the agents' own transcripts and
//! hands them over as plain numbers; the store keeps them and sums them for
//! the usage view. It knows no price and no model: a price is applied where
//! the figures are shown, so a corrected price table corrects the past too.
//!
//! A session's rows are replaced as a whole whenever its transcript is
//! imported ([`set_session_tokens_in`]), which is what makes importing a file
//! twice, or a file that grew, count each message once.

use rusqlite::{params, Transaction};

use super::Store;
use crate::error::Result;
use crate::ids::{MachineId, SessionId};
use crate::model::AgentId;

/// The five counts a model call is billed by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCounts {
    /// Input that was not served from the cache.
    pub input: u64,
    /// Output, reasoning included.
    pub output: u64,
    /// Input served from the cache.
    pub cache_read: u64,
    /// Input written to the cache (the five-minute kind, or when the agent
    /// does not say which).
    pub cache_write: u64,
    /// Input written to the one-hour cache.
    pub cache_write_1h: u64,
}

impl TokenCounts {
    /// Whether nothing was counted.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Every token counted, whatever it was billed as.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write + self.cache_write_1h
    }

    /// All the input side: fresh, cached and cache writes.
    pub fn all_input(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.cache_write_1h
    }

    /// Adds `other`, saturating.
    pub fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.cache_write_1h = self.cache_write_1h.saturating_add(other.cache_write_1h);
    }
}

/// What one session used of one model on one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTokens {
    /// The model's name as the transcript gives it; empty when it does not.
    pub model: String,
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// The counts.
    pub counts: TokenCounts,
}

/// The counts of one agent on one machine, for one model and day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    /// The agent.
    pub agent: AgentId,
    /// The machine the sessions ran on.
    pub machine: MachineId,
    /// The model; empty when the transcripts did not say.
    pub model: String,
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// The sum over the sessions.
    pub counts: TokenCounts,
}

/// Replaces the counts of `session` with `tokens`, inside the transaction that
/// stores the session.
pub fn set_session_tokens_in(
    tx: &Transaction<'_>,
    session: &SessionId,
    tokens: &[SessionTokens],
) -> Result<()> {
    let pk: i64 = tx
        .prepare_cached("SELECT pk FROM session WHERE id = ?1")?
        .query_row([session.as_str()], |row| row.get(0))?;
    tx.prepare_cached("DELETE FROM token_usage WHERE session_pk = ?1")?
        .execute([pk])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO token_usage
             (session_pk, model, day, input, output, cache_read, cache_write, cache_write_1h)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (session_pk, model, day) DO UPDATE
         SET input = input + excluded.input, output = output + excluded.output,
             cache_read = cache_read + excluded.cache_read,
             cache_write = cache_write + excluded.cache_write,
             cache_write_1h = cache_write_1h + excluded.cache_write_1h",
    )?;
    for entry in tokens {
        let counts = &entry.counts;
        insert.execute(params![
            pk,
            entry.model,
            entry.day,
            clamp(counts.input),
            clamp(counts.output),
            clamp(counts.cache_read),
            clamp(counts.cache_write),
            clamp(counts.cache_write_1h),
        ])?;
    }
    Ok(())
}

/// SQLite integers are signed 64-bit; a count never gets near that.
fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl Store {
    /// The counts of every agent, machine, model and day since `since_day`
    /// (`YYYY-MM-DD`, inclusive; all of them when `None`), oldest day first.
    pub fn token_usage(&self, since_day: Option<&str>) -> Result<Vec<TokenRow>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT s.agent, s.machine_id, t.model, t.day,
                        SUM(t.input), SUM(t.output), SUM(t.cache_read),
                        SUM(t.cache_write), SUM(t.cache_write_1h)
                 FROM token_usage t JOIN session s ON s.pk = t.session_pk
                 WHERE t.day >= ?1
                 GROUP BY s.agent, s.machine_id, t.model, t.day
                 ORDER BY t.day, s.agent, s.machine_id, t.model",
            )?;
            let count = |row: &rusqlite::Row<'_>, at: usize| -> rusqlite::Result<u64> {
                Ok(row.get::<_, i64>(at)?.max(0) as u64)
            };
            let rows = statement.query_map([since_day.unwrap_or("")], |row| {
                let tag: String = row.get(0)?;
                let machine: String = row.get(1)?;
                let model: String = row.get(2)?;
                let day: String = row.get(3)?;
                let counts = TokenCounts {
                    input: count(row, 4)?,
                    output: count(row, 5)?,
                    cache_read: count(row, 6)?,
                    cache_write: count(row, 7)?,
                    cache_write_1h: count(row, 8)?,
                };
                Ok(AgentId::parse(&tag).map(|agent| TokenRow {
                    agent,
                    machine: MachineId::from_string(machine),
                    model,
                    day,
                    counts,
                }))
            })?;
            // A row of an agent this build does not know is left out.
            let mut found = Vec::new();
            for row in rows {
                found.extend(row?);
            }
            Ok(found)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NewMessage, NewSession, Role};
    use chrono::{DateTime, Utc};

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn session(agent: AgentId, id: &str) -> NewSession {
        NewSession {
            agent,
            external_id: id.to_owned(),
            machine_id: MachineId::local(),
            cwd: "/srv/api".to_owned(),
            title: "t".to_owned(),
            model: None,
            started_at: at(1),
            updated_at: at(2),
        }
    }

    fn message() -> Vec<NewMessage> {
        vec![NewMessage {
            role: Role::User,
            text: "hi".to_owned(),
            at: at(1),
        }]
    }

    fn tokens(model: &str, day: &str, input: u64, output: u64) -> SessionTokens {
        SessionTokens {
            model: model.to_owned(),
            day: day.to_owned(),
            counts: TokenCounts {
                input,
                output,
                cache_read: 10,
                cache_write: 2,
                cache_write_1h: 1,
            },
        }
    }

    fn store_with(agent: AgentId, id: &str, tokens: &[SessionTokens]) -> std::sync::Arc<Store> {
        let store = Store::open_in_memory().unwrap();
        put(&store, agent, id, tokens);
        store
    }

    fn put(store: &Store, agent: AgentId, id: &str, tokens: &[SessionTokens]) {
        store
            .write(crate::StoreChange::Sessions, |tx| {
                let stored = crate::upsert_session_in(tx, &session(agent, id), &message())?;
                set_session_tokens_in(tx, &stored, tokens)
            })
            .unwrap();
    }

    #[test]
    fn counts_are_summed_per_agent_machine_model_and_day() {
        let store = store_with(
            AgentId::CLAUDE,
            "a",
            &[
                tokens("m1", "2026-03-01", 100, 50),
                tokens("m1", "2026-03-02", 1, 1),
            ],
        );
        put(
            &store,
            AgentId::CLAUDE,
            "b",
            &[tokens("m1", "2026-03-01", 5, 5)],
        );
        put(
            &store,
            AgentId::CODEX,
            "c",
            &[tokens("m2", "2026-03-01", 7, 7)],
        );

        let rows = store.token_usage(None).unwrap();
        assert_eq!(rows.len(), 3);
        let claude_first = rows
            .iter()
            .find(|row| row.agent == AgentId::CLAUDE && row.day == "2026-03-01")
            .unwrap();
        assert_eq!(
            (claude_first.counts.input, claude_first.counts.output),
            (105, 55)
        );
        assert_eq!(claude_first.counts.cache_read, 20);
        assert_eq!(claude_first.counts.cache_write_1h, 2);
        assert_eq!(rows[0].day, "2026-03-01", "oldest day first");
    }

    #[test]
    fn storing_a_session_again_replaces_its_counts_instead_of_adding_them() {
        let store = store_with(AgentId::CLAUDE, "a", &[tokens("m1", "2026-03-01", 100, 50)]);
        put(
            &store,
            AgentId::CLAUDE,
            "a",
            &[tokens("m1", "2026-03-01", 100, 50)],
        );
        assert_eq!(store.token_usage(None).unwrap()[0].counts.input, 100);

        // A transcript that grew states the whole new total.
        put(
            &store,
            AgentId::CLAUDE,
            "a",
            &[tokens("m1", "2026-03-01", 160, 80)],
        );
        let rows = store.token_usage(None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].counts.input, rows[0].counts.output), (160, 80));

        // And one that no longer has any leaves none.
        put(&store, AgentId::CLAUDE, "a", &[]);
        assert!(store.token_usage(None).unwrap().is_empty());
    }

    #[test]
    fn the_first_day_asked_for_is_included_and_earlier_ones_are_not() {
        let store = store_with(
            AgentId::CLAUDE,
            "a",
            &[
                tokens("m1", "2026-02-28", 1, 1),
                tokens("m1", "2026-03-01", 2, 2),
                tokens("m1", "2026-03-02", 3, 3),
            ],
        );
        let days: Vec<_> = store
            .token_usage(Some("2026-03-01"))
            .unwrap()
            .into_iter()
            .map(|row| row.day)
            .collect();
        assert_eq!(days, ["2026-03-01", "2026-03-02"]);
    }

    #[test]
    fn deleting_a_session_takes_its_counts_with_it() {
        let store = store_with(AgentId::CLAUDE, "a", &[tokens("m1", "2026-03-01", 1, 1)]);
        store
            .write(crate::StoreChange::Sessions, |tx| {
                tx.execute("DELETE FROM session", [])?;
                Ok(())
            })
            .unwrap();
        assert!(store.token_usage(None).unwrap().is_empty());
    }

    #[test]
    fn counts_know_their_totals() {
        let counts = TokenCounts {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cache_write_1h: 5,
        };
        assert_eq!(counts.total(), 15);
        assert_eq!(counts.all_input(), 13);
        assert!(TokenCounts::default().is_empty());
    }
}
