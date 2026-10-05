//! What the application does with the usage readings besides drawing them.
//!
//! The engine collects per machine (`Engine::collect_usage`), keeps the latest
//! reading of each agent and a bounded history in the store, and the window
//! reads them back as a [`Board`]. The pure decisions are here: which reading
//! to keep when a collection comes back empty-handed, which observations feed
//! the history, which machine the bar speaks for, and the one-line notice
//! before a session starts.

use std::collections::HashMap;

use leon_core::{AgentKind, MachineId, Store, UsagePoint};
use leon_usage::{series_key, AgentUsage, MachineUsage, Reason, State, Thresholds};
use leon_usage::{view, Body, Level};

/// How long the stored observations are kept, in seconds.
pub const HISTORY_HORIZON: i64 = 14 * leon_usage::model::DAY;

/// The reading to keep after a collection.
///
/// A machine that cannot be reached right now has not stopped having limits:
/// the earlier reading stays (its age is shown and judged), instead of being
/// replaced by "unreachable". A reading that actually says something replaces
/// the earlier one.
pub fn merge(previous: Option<AgentUsage>, new: AgentUsage) -> AgentUsage {
    match (previous, &new.state) {
        (
            Some(
                old @ AgentUsage {
                    state: State::Known { .. },
                    ..
                },
            ),
            State::Unknown {
                reason: Reason::Unreachable | Reason::NoData,
            },
        ) => old,
        _ => new,
    }
}

/// The observations of one agent's account: the window key and the point.
pub type Series = (AgentKind, String, Vec<(String, UsagePoint)>);

/// The history points of a collection: per agent, the account key and the
/// observations of every window, the current reading included.
pub fn history_points(collected: &MachineUsage, machine: &str) -> Vec<Series> {
    let mut out = Vec::new();
    for reading in &collected.readings {
        let State::Known { windows } = &reading.state else {
            continue;
        };
        let account = series_key(machine, reading.agent, reading.plan.as_deref());
        let mut points: Vec<(String, UsagePoint)> = collected
            .samples
            .iter()
            .filter(|(agent, ..)| *agent == reading.agent)
            .map(|(_, kind, sample)| {
                (
                    kind.key(),
                    UsagePoint {
                        at: sample.at,
                        used_percent: sample.used_percent,
                    },
                )
            })
            .collect();
        if let Some(at) = reading.observed_at {
            for window in windows {
                points.push((
                    window.kind.key(),
                    UsagePoint {
                        at,
                        used_percent: window.used_percent,
                    },
                ));
            }
        }
        out.push((reading.agent, account, points));
    }
    out
}

/// Which machine's limits are shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The machine in context (the one of what is selected).
    Context,
    /// One machine, chosen in the usage view.
    Machine(MachineId),
    /// Every machine.
    All,
}

/// The latest readings of every machine, as the window reads them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    readings: Vec<AgentUsage>,
    collected_at: Option<i64>,
}

impl Board {
    /// A board over these readings.
    #[cfg(test)]
    pub fn new(readings: Vec<AgentUsage>) -> Self {
        Self {
            readings,
            collected_at: None,
        }
    }

    /// The same board, collected at this time (Unix seconds).
    #[cfg(test)]
    pub fn collected(mut self, at: i64) -> Self {
        self.collected_at = Some(at);
        self
    }

    /// When the newest collection was made (Unix seconds).
    pub fn collected_at(&self) -> Option<i64> {
        self.collected_at
    }

    /// Reads the latest readings from the store. A reading that does not
    /// parse is left out: nothing is claimed that is not understood.
    pub fn load(store: &Store) -> Self {
        let rows = store.usage_readings().unwrap_or_default();
        Self {
            collected_at: rows.iter().map(|row| row.collected_at).max(),
            readings: rows
                .into_iter()
                .filter_map(|row| serde_json::from_str::<AgentUsage>(&row.payload).ok())
                .collect(),
        }
    }

    /// Every reading.
    pub fn all(&self) -> &[AgentUsage] {
        &self.readings
    }

    /// Whether nothing has been collected yet.
    pub fn is_empty(&self) -> bool {
        self.readings.is_empty()
    }

    /// The reading of `agent` on `machine`.
    pub fn get(&self, machine: &MachineId, agent: AgentKind) -> Option<&AgentUsage> {
        self.readings
            .iter()
            .find(|r| r.agent == agent && r.machine == machine.as_str())
    }

    /// The readings the scope asks for, `shown` agents only, in the order of
    /// agents and then machines. With `Context`, `context` is the machine.
    pub fn select(
        &self,
        scope: &Scope,
        context: &MachineId,
        shown: &[AgentKind],
    ) -> Vec<&AgentUsage> {
        let mut out: Vec<&AgentUsage> = self
            .readings
            .iter()
            .filter(|r| shown.contains(&r.agent))
            .filter(|r| match scope {
                Scope::Context => r.machine == context.as_str(),
                Scope::Machine(id) => r.machine == id.as_str(),
                Scope::All => true,
            })
            .collect();
        out.sort_by_key(|r| {
            (
                AgentKind::ALL.iter().position(|a| *a == r.agent),
                r.machine.clone(),
            )
        });
        out
    }
}

/// The one-line notice before a session of `agent` starts, when its limit is
/// nearly used up: which window, how full, when it resets. `None` when there
/// is nothing to say (below the critical threshold, unknown, or not read).
pub fn start_notice(
    board: &Board,
    machine: &MachineId,
    agent: AgentKind,
    now: i64,
    thresholds: Thresholds,
) -> Option<String> {
    let reading = board.get(machine, agent)?;
    let v = view(reading, now, thresholds);
    let Body::Ready { meters, .. } = &v.body else {
        return None;
    };
    let worst = meters
        .iter()
        .filter(|m| m.level == Level::Critical)
        .max_by(|a, b| a.percent.total_cmp(&b.percent))?;
    let when = worst
        .resets_in
        .map(|s| format!(", resets in {}", leon_usage::compact_duration(s)))
        .unwrap_or_default();
    let name = match agent {
        AgentKind::Claude => "Claude Code",
        AgentKind::Codex => "Codex",
        AgentKind::Opencode => "opencode",
    };
    let state = if worst.percent >= 100.0 {
        "is at its limit".to_owned()
    } else {
        format!("is {:.0}% used", worst.percent)
    };
    Some(format!("{name}: {} {state}{when}.", worst.kind.long()))
}

/// What a collection of one machine stores: the readings (merged with what was
/// there) as JSON documents, ready to write.
pub fn payloads(
    collected: &MachineUsage,
    previous: &HashMap<AgentKind, AgentUsage>,
) -> Vec<(AgentKind, AgentUsage)> {
    collected
        .readings
        .iter()
        .map(|reading| {
            (
                reading.agent,
                merge(previous.get(&reading.agent).cloned(), reading.clone()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use leon_usage::{Sample, Source, UsageWindow, WindowKind};

    const NOW: i64 = 1_790_000_000;

    fn known(agent: AgentKind, machine: &str, used: f64, reset_in: i64) -> AgentUsage {
        AgentUsage {
            agent,
            machine: machine.into(),
            account_label: None,
            plan: Some("plus".into()),
            source: Some(Source::Local),
            observed_at: Some(NOW - 60),
            state: State::Known {
                windows: vec![UsageWindow {
                    kind: WindowKind::FiveHour,
                    used_percent: used,
                    resets_at: Some(NOW + reset_in),
                    window_length: None,
                }],
            },
        }
    }

    #[test]
    fn an_unreachable_machine_keeps_its_earlier_reading() {
        let old = known(AgentKind::Codex, "box", 40.0, 3600);
        let new = AgentUsage::unknown(AgentKind::Codex, "box", Reason::Unreachable);
        assert_eq!(merge(Some(old.clone()), new), old);
    }

    #[test]
    fn a_reading_that_says_something_replaces_the_earlier_one() {
        let old = known(AgentKind::Codex, "box", 40.0, 3600);
        let newer = known(AgentKind::Codex, "box", 50.0, 3600);
        assert_eq!(merge(Some(old.clone()), newer.clone()), newer);
        let disabled = AgentUsage::unknown(AgentKind::Codex, "box", Reason::SourceDisabled);
        assert_eq!(merge(Some(old), disabled.clone()), disabled);
        assert_eq!(merge(None, disabled.clone()), disabled);
    }

    #[test]
    fn history_points_hold_the_samples_and_the_current_windows() {
        let reading = known(AgentKind::Codex, "local", 40.0, 3600);
        let collected = MachineUsage {
            readings: vec![
                reading.clone(),
                AgentUsage::unknown(AgentKind::Claude, "local", Reason::SourceDisabled),
            ],
            samples: vec![(
                AgentKind::Codex,
                WindowKind::FiveHour,
                Sample {
                    at: NOW - 600,
                    used_percent: 30.0,
                },
            )],
        };
        let got = history_points(&collected, "local");
        assert_eq!(got.len(), 1);
        let (agent, account, points) = &got[0];
        assert_eq!(*agent, AgentKind::Codex);
        assert_eq!(account.len(), 12);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].0, "five_hour");
        assert_eq!(points[1].1.used_percent, 40.0);
    }

    #[test]
    fn the_bar_speaks_for_the_machine_in_context_or_for_all() {
        let board = Board::new(vec![
            known(AgentKind::Codex, "local", 10.0, 100),
            known(AgentKind::Codex, "box", 20.0, 100),
            known(AgentKind::Claude, "local", 30.0, 100),
        ]);
        let local = MachineId::local();
        let all = AgentKind::ALL;
        let ctx = board.select(&Scope::Context, &local, &all);
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx[0].agent, AgentKind::Claude);
        let everything = board.select(&Scope::All, &local, &all);
        assert_eq!(everything.len(), 3);
        let one = board.select(&Scope::Machine(MachineId::from_string("box")), &local, &all);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].machine, "box");
        let only_codex = board.select(&Scope::All, &local, &[AgentKind::Codex]);
        assert_eq!(only_codex.len(), 2);
    }

    #[test]
    fn the_notice_appears_from_the_critical_threshold_and_names_the_reset() {
        let board = Board::new(vec![known(
            AgentKind::Codex,
            "local",
            97.0,
            2 * 3600 + 29 * 60,
        )]);
        let local = MachineId::local();
        let t = Thresholds::default();
        assert_eq!(
            start_notice(&board, &local, AgentKind::Codex, NOW, t).as_deref(),
            Some("Codex: 5-hour window is 97% used, resets in 2h 29m.")
        );
        let low = Board::new(vec![known(AgentKind::Codex, "local", 89.0, 100)]);
        assert_eq!(start_notice(&low, &local, AgentKind::Codex, NOW, t), None);
        assert_eq!(
            start_notice(&board, &local, AgentKind::Claude, NOW, t),
            None
        );
    }

    #[test]
    fn an_exhausted_window_says_it_is_at_its_limit() {
        let board = Board::new(vec![known(AgentKind::Codex, "local", 100.0, 600)]);
        let notice = start_notice(
            &board,
            &MachineId::local(),
            AgentKind::Codex,
            NOW,
            Thresholds::default(),
        )
        .unwrap();
        assert!(notice.contains("is at its limit"), "{notice}");
        assert!(notice.contains("resets in 10m"), "{notice}");
    }

    #[test]
    fn a_window_that_has_reset_since_stops_the_notice() {
        let board = Board::new(vec![known(AgentKind::Codex, "local", 99.0, -60)]);
        assert_eq!(
            start_notice(
                &board,
                &MachineId::local(),
                AgentKind::Codex,
                NOW,
                Thresholds::default()
            ),
            None
        );
    }

    #[test]
    fn a_board_reads_back_what_the_store_holds() {
        let store = Store::open_in_memory().unwrap();
        let reading = known(AgentKind::Codex, "local", 12.0, 100);
        store
            .put_usage_reading(
                &MachineId::local(),
                AgentKind::Codex,
                &serde_json::to_string(&reading).unwrap(),
                NOW,
            )
            .unwrap();
        store
            .put_usage_reading(&MachineId::local(), AgentKind::Claude, "not json", NOW)
            .unwrap();
        let board = Board::load(&store);
        assert_eq!(board.all().len(), 1);
        assert_eq!(
            board.get(&MachineId::local(), AgentKind::Codex),
            Some(&reading)
        );
    }
}
