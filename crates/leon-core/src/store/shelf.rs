//! Where a session stands on the sidebar's shelves.
//!
//! A session can be settled (put away on the Settled shelf), snoozed (hidden
//! until a time) or returned (taken back from a shelf by hand, which also tells
//! the automatic settling to leave it alone). A session with no row here has
//! never been touched. The rows live in a table of their own, keyed by the
//! session's id, so an import that rewrites a session keeps its place on the
//! shelf and removing a session takes its row with it.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, OptionalExtension, Transaction};

use super::Store;
use crate::change::StoreChange;
use crate::error::Result;
use crate::ids::SessionId;

/// What a session's row on the shelves says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shelf {
    /// Put away: the Settled shelf holds it.
    Settled,
    /// Hidden until this time: the Snoozed shelf holds it until then.
    Snoozed(DateTime<Utc>),
    /// Taken back from a shelf by hand. It is in its usual place, and the
    /// automatic settling does not take it again.
    Returned,
}

impl Shelf {
    /// The tag and the time the database keeps.
    fn parts(self) -> (&'static str, Option<i64>) {
        match self {
            Shelf::Settled => ("settled", None),
            Shelf::Snoozed(until) => ("snoozed", Some(until.timestamp_millis())),
            Shelf::Returned => ("returned", None),
        }
    }

    /// The state a stored row means; `None` for a tag this version does not
    /// know, so a row written by a newer one reads as untouched.
    fn from_parts(tag: &str, until: Option<i64>) -> Option<Self> {
        match tag {
            "settled" => Some(Shelf::Settled),
            "snoozed" => Utc
                .timestamp_millis_opt(until?)
                .single()
                .map(Shelf::Snoozed),
            "returned" => Some(Shelf::Returned),
            _ => None,
        }
    }
}

impl Store {
    /// Every session that has a row on the shelves.
    pub fn shelves(&self) -> Result<HashMap<SessionId, Shelf>> {
        self.read(|connection| {
            let mut statement =
                connection.prepare_cached("SELECT session_id, kind, until FROM session_shelf")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?;
            let mut out = HashMap::new();
            for row in rows {
                let (id, tag, until) = row?;
                if let Some(shelf) = Shelf::from_parts(&tag, until) {
                    out.insert(SessionId::from_string(id), shelf);
                }
            }
            Ok(out)
        })
    }

    /// Writes where sessions stand, all or none: `None` clears a session's
    /// row. A session the store does not know is skipped (it was removed
    /// meanwhile), and a row that already says this is left alone. Listeners
    /// are told only when something changed, so a caller that decides again
    /// after every change cannot start a loop. Returns whether anything did.
    pub fn set_shelves(&self, changes: &[(SessionId, Option<Shelf>)]) -> Result<bool> {
        let changed = self.transact(|tx| {
            let mut changed = false;
            for (id, wanted) in changes {
                changed |= set_one(tx, id, *wanted)?;
            }
            Ok(changed)
        })?;
        if changed {
            self.notify(StoreChange::Sessions);
        }
        Ok(changed)
    }
}

fn set_one(tx: &Transaction<'_>, id: &SessionId, wanted: Option<Shelf>) -> Result<bool> {
    let known: bool = tx
        .prepare_cached("SELECT 1 FROM session WHERE id = ?1")?
        .exists([id.as_str()])?;
    if !known {
        return Ok(false);
    }
    let current = tx
        .prepare_cached("SELECT kind, until FROM session_shelf WHERE session_id = ?1")?
        .query_row([id.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .optional()?
        .and_then(|(tag, until)| Shelf::from_parts(&tag, until));
    if current == wanted {
        return Ok(false);
    }
    match wanted {
        Some(shelf) => {
            let (tag, until) = shelf.parts();
            tx.prepare_cached(
                "INSERT INTO session_shelf (session_id, kind, until) VALUES (?1, ?2, ?3)
                 ON CONFLICT (session_id) DO UPDATE SET kind = excluded.kind, until = excluded.until",
            )?
            .execute(params![id.as_str(), tag, until])?;
        }
        None => {
            tx.prepare_cached("DELETE FROM session_shelf WHERE session_id = ?1")?
                .execute([id.as_str()])?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentId, MachineId, NewMessage, NewSession, Role};

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 10, minute, 0).unwrap()
    }

    fn add(store: &Store, external: &str, title: &str) -> SessionId {
        store
            .upsert_session(
                &NewSession {
                    agent: AgentId::CLAUDE,
                    external_id: external.to_owned(),
                    machine_id: MachineId::local(),
                    cwd: "/srv/api".to_owned(),
                    title: title.to_owned(),
                    model: None,
                    started_at: at(1),
                    updated_at: at(2),
                },
                &[NewMessage {
                    role: Role::User,
                    text: title.to_owned(),
                    at: at(2),
                }],
            )
            .unwrap()
    }

    #[test]
    fn a_shelf_round_trips_and_clearing_it_leaves_no_row() {
        let store = Store::open_in_memory().unwrap();
        let a = add(&store, "a", "one");
        let b = add(&store, "b", "two");
        let c = add(&store, "c", "three");
        let until = at(30);
        assert!(store
            .set_shelves(&[
                (a.clone(), Some(Shelf::Settled)),
                (b.clone(), Some(Shelf::Snoozed(until))),
                (c.clone(), Some(Shelf::Returned)),
            ])
            .unwrap());
        let all = store.shelves().unwrap();
        assert_eq!(all[&a], Shelf::Settled);
        assert_eq!(all[&b], Shelf::Snoozed(until));
        assert_eq!(all[&c], Shelf::Returned);
        assert!(store.set_shelves(&[(a.clone(), None)]).unwrap());
        assert!(!store.shelves().unwrap().contains_key(&a));
    }

    #[test]
    fn writing_what_is_already_there_changes_nothing_and_announces_nothing() {
        let store = Store::open_in_memory().unwrap();
        let a = add(&store, "a", "one");
        assert!(store
            .set_shelves(&[(a.clone(), Some(Shelf::Settled))])
            .unwrap());
        let mut listener = store.subscribe();
        assert!(!store
            .set_shelves(&[(a.clone(), Some(Shelf::Settled))])
            .unwrap());
        assert!(!store.set_shelves(&[(a, Some(Shelf::Settled))]).unwrap());
        assert_eq!(listener.try_next(), None, "no write, no announcement");
    }

    #[test]
    fn a_session_that_is_gone_is_skipped_and_the_rest_is_written() {
        let store = Store::open_in_memory().unwrap();
        let a = add(&store, "a", "one");
        let gone = SessionId::from_string("not-a-session");
        assert!(store
            .set_shelves(&[
                (gone.clone(), Some(Shelf::Settled)),
                (a.clone(), Some(Shelf::Settled)),
            ])
            .unwrap());
        let all = store.shelves().unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all.contains_key(&gone));
    }

    #[test]
    fn removing_a_session_takes_its_row_but_an_import_keeps_it() {
        let store = Store::open_in_memory().unwrap();
        let a = add(&store, "a", "one");
        store
            .set_shelves(&[(a.clone(), Some(Shelf::Settled))])
            .unwrap();
        // The agent wrote the session again: same id, new title.
        let again = add(&store, "a", "one, renamed");
        assert_eq!(again, a);
        assert_eq!(store.shelves().unwrap()[&a], Shelf::Settled);
        store.remove_session(&a).unwrap();
        assert!(store.shelves().unwrap().is_empty());
    }

    #[test]
    fn a_row_of_a_kind_this_version_does_not_know_reads_as_untouched() {
        let store = Store::open_in_memory().unwrap();
        let a = add(&store, "a", "one");
        store
            .write(StoreChange::Sessions, |tx| {
                tx.execute(
                    "INSERT INTO session_shelf (session_id, kind) VALUES (?1, 'someday')",
                    [a.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(store.shelves().unwrap().is_empty());
    }
}
