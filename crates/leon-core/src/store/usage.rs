//! Usage limits: the latest reading and a bounded history.
//!
//! The store keeps what it is told and does not interpret it: a reading is a
//! JSON document per agent and machine, a history point is a window key, a
//! time and a percentage under a local hash of the account. Nothing that
//! identifies an account is stored. The history is bounded twice: points older
//! than the horizon go, and no series keeps more than [`MAX_POINTS`].

use rusqlite::params;

use super::Store;
use crate::change::StoreChange;
use crate::error::Result;
use crate::ids::MachineId;
use crate::model::AgentKind;

/// The most points one series (machine, agent, account, window) keeps.
pub const MAX_POINTS: usize = 300;

/// The latest reading of one agent on one machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRow {
    /// The machine.
    pub machine: MachineId,
    /// The agent.
    pub agent: AgentKind,
    /// The reading, as the JSON the collector wrote.
    pub payload: String,
    /// When it was collected (Unix seconds).
    pub collected_at: i64,
}

/// One stored observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsagePoint {
    /// When (Unix seconds).
    pub at: i64,
    /// How much was used then, 0 to 100.
    pub used_percent: f64,
}

impl Store {
    /// Replaces the latest reading of `agent` on `machine`.
    pub fn put_usage_reading(
        &self,
        machine: &MachineId,
        agent: AgentKind,
        payload: &str,
        collected_at: i64,
    ) -> Result<()> {
        self.write(StoreChange::Usage, |tx| {
            tx.execute(
                "INSERT INTO usage_reading (machine_id, agent, payload, collected_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (machine_id, agent) DO UPDATE
                 SET payload = excluded.payload, collected_at = excluded.collected_at",
                params![machine.as_str(), agent.as_str(), payload, collected_at],
            )?;
            Ok(())
        })
    }

    /// Every latest reading.
    pub fn usage_readings(&self) -> Result<Vec<UsageRow>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT machine_id, agent, payload, collected_at FROM usage_reading
                 ORDER BY machine_id, agent",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (machine, agent, payload, collected_at) = row?;
                if let Some(agent) = AgentKind::parse(&agent) {
                    out.push(UsageRow {
                        machine: MachineId::from_string(machine),
                        agent,
                        payload,
                        collected_at,
                    });
                }
            }
            Ok(out)
        })
    }

    /// Records observations of the windows of one account (`window` is the
    /// window's stable key). A point already stored is left alone. Points
    /// older than `keep_since` are dropped, and each series is cut to its
    /// newest [`MAX_POINTS`].
    pub fn record_usage_points(
        &self,
        machine: &MachineId,
        agent: AgentKind,
        account: &str,
        points: &[(String, UsagePoint)],
        keep_since: i64,
    ) -> Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        self.write(StoreChange::Usage, |tx| {
            for (window, point) in points.iter().filter(|(_, p)| p.at >= keep_since) {
                tx.execute(
                    "INSERT OR IGNORE INTO usage_history
                     (machine_id, agent, account, window, observed_at, used_percent)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        machine.as_str(),
                        agent.as_str(),
                        account,
                        window,
                        point.at,
                        point.used_percent
                    ],
                )?;
            }
            tx.execute(
                "DELETE FROM usage_history WHERE observed_at < ?1",
                params![keep_since],
            )?;
            let mut windows: Vec<String> = points.iter().map(|(w, _)| w.clone()).collect();
            windows.sort();
            windows.dedup();
            for window in windows {
                tx.execute(
                    "DELETE FROM usage_history
                     WHERE machine_id = ?1 AND agent = ?2 AND account = ?3 AND window = ?4
                       AND observed_at NOT IN (
                         SELECT observed_at FROM usage_history
                         WHERE machine_id = ?1 AND agent = ?2 AND account = ?3 AND window = ?4
                         ORDER BY observed_at DESC LIMIT ?5)",
                    params![
                        machine.as_str(),
                        agent.as_str(),
                        account,
                        window,
                        MAX_POINTS as i64
                    ],
                )?;
            }
            Ok(())
        })
    }

    /// The stored observations of one window since `since`, oldest first.
    pub fn usage_history(
        &self,
        machine: &MachineId,
        agent: AgentKind,
        account: &str,
        window: &str,
        since: i64,
    ) -> Result<Vec<UsagePoint>> {
        self.read(|connection| {
            let mut statement = connection.prepare_cached(
                "SELECT observed_at, used_percent FROM usage_history
                 WHERE machine_id = ?1 AND agent = ?2 AND account = ?3 AND window = ?4
                   AND observed_at >= ?5
                 ORDER BY observed_at",
            )?;
            let rows = statement.query_map(
                params![machine.as_str(), agent.as_str(), account, window, since],
                |row| {
                    Ok(UsagePoint {
                        at: row.get(0)?,
                        used_percent: row.get(1)?,
                    })
                },
            )?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Forgets every stored observation. The latest readings stay. Returns how
    /// many points were removed.
    pub fn forget_usage_history(&self) -> Result<usize> {
        self.write(StoreChange::Usage, |tx| {
            Ok(tx.execute("DELETE FROM usage_history", [])?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(at: i64, used: f64) -> (String, UsagePoint) {
        (
            "five_hour".to_owned(),
            UsagePoint {
                at,
                used_percent: used,
            },
        )
    }

    #[test]
    fn a_reading_is_replaced_by_the_next_one() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        store
            .put_usage_reading(&local, AgentKind::Codex, "{\"a\":1}", 10)
            .unwrap();
        store
            .put_usage_reading(&local, AgentKind::Codex, "{\"a\":2}", 20)
            .unwrap();
        let rows = store.usage_readings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].payload, "{\"a\":2}");
        assert_eq!(rows[0].collected_at, 20);
    }

    #[test]
    fn history_comes_back_oldest_first_and_ignores_repeats() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        let points = [
            point(300, 30.0),
            point(100, 10.0),
            point(200, 20.0),
            point(100, 10.0),
        ];
        store
            .record_usage_points(&local, AgentKind::Codex, "acct", &points, 0)
            .unwrap();
        let got = store
            .usage_history(&local, AgentKind::Codex, "acct", "five_hour", 0)
            .unwrap();
        let times: Vec<i64> = got.iter().map(|p| p.at).collect();
        assert_eq!(times, [100, 200, 300]);
    }

    #[test]
    fn points_older_than_the_horizon_are_dropped() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        store
            .record_usage_points(
                &local,
                AgentKind::Codex,
                "a",
                &[point(10, 1.0), point(500, 2.0)],
                0,
            )
            .unwrap();
        store
            .record_usage_points(&local, AgentKind::Codex, "a", &[point(900, 3.0)], 400)
            .unwrap();
        let got = store
            .usage_history(&local, AgentKind::Codex, "a", "five_hour", 0)
            .unwrap();
        assert_eq!(got.iter().map(|p| p.at).collect::<Vec<_>>(), [500, 900]);
    }

    #[test]
    fn a_series_keeps_only_its_newest_points() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        let points: Vec<_> = (0..(MAX_POINTS as i64 + 25))
            .map(|i| point(1000 + i, 1.0))
            .collect();
        store
            .record_usage_points(&local, AgentKind::Claude, "a", &points, 0)
            .unwrap();
        let got = store
            .usage_history(&local, AgentKind::Claude, "a", "five_hour", 0)
            .unwrap();
        assert_eq!(got.len(), MAX_POINTS);
        assert_eq!(got.last().unwrap().at, 1000 + MAX_POINTS as i64 + 24);
        assert_eq!(got.first().unwrap().at, 1025);
    }

    #[test]
    fn series_do_not_cut_each_other() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        store
            .record_usage_points(&local, AgentKind::Codex, "a", &[point(5, 1.0)], 0)
            .unwrap();
        store
            .record_usage_points(&local, AgentKind::Claude, "a", &[point(6, 1.0)], 0)
            .unwrap();
        assert_eq!(
            store
                .usage_history(&local, AgentKind::Codex, "a", "five_hour", 0)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .usage_history(&local, AgentKind::Claude, "a", "five_hour", 0)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn forgetting_history_keeps_the_latest_readings() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        store
            .put_usage_reading(&local, AgentKind::Codex, "{}", 1)
            .unwrap();
        store
            .record_usage_points(
                &local,
                AgentKind::Codex,
                "a",
                &[point(5, 1.0), point(6, 2.0)],
                0,
            )
            .unwrap();
        assert_eq!(store.forget_usage_history().unwrap(), 2);
        assert!(store
            .usage_history(&local, AgentKind::Codex, "a", "five_hour", 0)
            .unwrap()
            .is_empty());
        assert_eq!(store.usage_readings().unwrap().len(), 1);
    }

    #[test]
    fn writes_announce_a_usage_change() {
        let store = Store::open_in_memory().unwrap();
        let mut listener = store.subscribe();
        store
            .put_usage_reading(&MachineId::local(), AgentKind::Codex, "{}", 1)
            .unwrap();
        assert_eq!(listener.try_next(), Some(StoreChange::Usage));
    }

    #[test]
    fn removing_a_machine_removes_its_usage() {
        let store = Store::open_in_memory().unwrap();
        let machine = store
            .add_machine(
                "box",
                crate::model::MachineKind::Ssh {
                    host: "h".into(),
                    user: None,
                    port: None,
                    identity_file: None,
                },
            )
            .unwrap();
        store
            .put_usage_reading(&machine.id, AgentKind::Codex, "{}", 1)
            .unwrap();
        store
            .record_usage_points(&machine.id, AgentKind::Codex, "a", &[point(5, 1.0)], 0)
            .unwrap();
        store.remove_machine(&machine.id).unwrap();
        assert!(store.usage_readings().unwrap().is_empty());
    }
}
