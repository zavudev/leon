//! What the application does with the usage readings besides drawing them.
//!
//! The engine collects per machine (`Engine::collect_usage`), keeps the latest
//! reading of each agent and a bounded history in the store, and the window
//! reads them back as a [`Board`]. The pure decisions are here: which reading
//! to keep when a collection comes back empty-handed, which observations feed
//! the history, which machine the bar speaks for, and the one-line notice
//! before a session starts.

use std::collections::HashMap;

use crate::engine::SourceStatus;
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
                reason:
                    Reason::Unreachable
                    | Reason::NoData
                    | Reason::Offline
                    | Reason::VendorError(_)
                    | Reason::RateLimited(_)
                    | Reason::KeychainDenied,
            },
        ) => old,
        _ => new,
    }
}

/// How often the schedule looks at the clock, in seconds. A read happens only
/// when [`due`] says so.
pub const TICK_SECONDS: u64 = 5;

/// Whether a scheduled read is due: only in a focused window, and when the last
/// one is at least `interval` seconds (plus this install's `jitter`) old, or
/// there was none. Coming back to the window asks the same question, so a
/// reading older than the interval is made once on return and none was made
/// while away.
pub fn due(last: Option<i64>, now: i64, interval: i64, jitter: i64, focused: bool) -> bool {
    focused && last.is_none_or(|last| now - last >= interval + jitter)
}

/// A jitter for the next wait, in seconds: from nothing to a tenth of the
/// interval, from `entropy`, so that many installs do not read at the same
/// moment.
pub fn jitter_seconds(interval: i64, entropy: u32) -> i64 {
    interval / 10 * i64::from(entropy % 11) / 10
}

/// What the engine knows of each agent's last read.
pub type Statuses = HashMap<AgentKind, SourceStatus>;

/// The statuses of every agent now.
pub fn statuses(engine: &crate::engine::Engine) -> Statuses {
    AgentKind::ALL
        .into_iter()
        .map(|agent| (agent, engine.usage_status(agent)))
        .collect()
}

/// What a row says when nothing is known of an agent: why, and what happens
/// next. A read under way is "Reading…" (with the keychain's heads-up for
/// Claude Code on macOS), not what the last read said.
pub fn unknown_text(
    agent: AgentKind,
    reason: Reason,
    status: &SourceStatus,
    now: i64,
    next_read: Option<i64>,
    mac: bool,
) -> String {
    let claude = agent == AgentKind::Claude;
    if status.reading {
        return if claude && mac {
            "Reading… macOS may ask for permission to read Claude Code's sign-in.".to_owned()
        } else {
            "Reading…".to_owned()
        };
    }
    if reason == Reason::KeychainDenied || status.refused {
        return if mac {
            "macOS did not let Leon read Claude Code's sign-in: the permission was denied or dismissed. Leon will not ask again this session; choose Try again to be asked."
                .to_owned()
        } else {
            "The sign-in could not be read. Leon will not try again by itself; choose Try again."
                .to_owned()
        };
    }
    if reason == Reason::NotSignedIn && claude {
        return "Not signed in to Claude Code. Sign in there, then choose Try again.".to_owned();
    }
    let base = match reason {
        Reason::RateLimited(_) => "Rate limited: the service asked for fewer calls.".to_owned(),
        Reason::ParseError => "Unexpected response from the service.".to_owned(),
        other => other.text(),
    };
    if !reason.is_failure() {
        return base;
    }
    format!("{base} {}", retry_clause(status, now, next_read))
}

/// "Next read in 2m." or, when nothing is scheduled, how to try again.
pub fn retry_clause(status: &SourceStatus, now: i64, next_read: Option<i64>) -> String {
    let at = status
        .retry_at
        .filter(|at| *at > now)
        .or(next_read.filter(|at| *at > now));
    match at {
        Some(at) => format!(
            "Next read in {}; or choose Try again.",
            leon_usage::compact_duration(at - now)
        ),
        None => "Choose Try again to read now.".to_owned(),
    }
}

/// What a row with numbers says when the last read failed.
pub fn failure_note(
    agent: AgentKind,
    status: &SourceStatus,
    now: i64,
    next_read: Option<i64>,
    mac: bool,
) -> Option<String> {
    let reason = status.failed?;
    Some(format!(
        "The last read failed. {}",
        unknown_text(
            agent,
            reason,
            &SourceStatus {
                reading: false,
                ..*status
            },
            now,
            next_read,
            mac
        )
    ))
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
            readings: rows
                .iter()
                .filter_map(|row| serde_json::from_str::<AgentUsage>(&row.payload).ok())
                .collect(),
            // The last time numbers were read: a row that only says "source
            // off", "not installed" or a failure is not a reading.
            collected_at: rows
                .iter()
                .filter(|row| {
                    serde_json::from_str::<AgentUsage>(&row.payload)
                        .is_ok_and(|usage| matches!(usage.state, State::Known { .. }))
                })
                .map(|row| row.collected_at)
                .max(),
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
    fn a_failed_network_read_keeps_the_numbers_and_a_switch_or_sign_out_replaces_them() {
        let old = known(AgentKind::Claude, "box", 40.0, 3600);
        for reason in [
            Reason::Offline,
            Reason::VendorError(500),
            Reason::RateLimited(60),
            Reason::KeychainDenied,
        ] {
            let new = AgentUsage::unknown(AgentKind::Claude, "box", reason);
            assert_eq!(merge(Some(old.clone()), new), old, "{reason:?}");
        }
        let signed_out = AgentUsage::unknown(AgentKind::Claude, "box", Reason::NotSignedIn);
        assert_eq!(merge(Some(old), signed_out.clone()), signed_out);
    }

    #[test]
    fn the_time_of_the_last_read_ignores_rows_that_only_say_why_nothing_was_read() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        let off = AgentUsage::unknown(AgentKind::Claude, "local", Reason::SourceDisabled);
        store
            .put_usage_reading(
                &local,
                AgentKind::Claude,
                &serde_json::to_string(&off).unwrap(),
                NOW,
            )
            .unwrap();
        assert_eq!(Board::load(&store).collected_at(), None);
        let codex = known(AgentKind::Codex, "local", 12.0, 100);
        store
            .put_usage_reading(
                &local,
                AgentKind::Codex,
                &serde_json::to_string(&codex).unwrap(),
                NOW - 90,
            )
            .unwrap();
        assert_eq!(Board::load(&store).collected_at(), Some(NOW - 90));
    }

    #[test]
    fn the_schedule_is_one_read_a_minute_in_a_focused_window_and_none_in_the_background() {
        let interval = 60;
        assert!(due(None, NOW, interval, 0, true), "the first read");
        assert!(!due(None, NOW, interval, 0, false), "not while away");
        // A clock ticking every TICK_SECONDS for ten minutes, focused: one
        // read a minute.
        let mut last = None;
        let mut reads = 0;
        for tick in 0..(600 / TICK_SECONDS as i64) {
            let now = NOW + tick * TICK_SECONDS as i64;
            if due(last, now, interval, 0, true) {
                reads += 1;
                last = Some(now);
            }
        }
        assert_eq!(reads, 10);
        // Ten minutes in the background, then back: one read at once, not ten.
        let mut reads = 0;
        let mut last = Some(NOW);
        for tick in 0..(600 / TICK_SECONDS as i64) {
            let now = NOW + tick * TICK_SECONDS as i64;
            if due(last, now, interval, 0, false) {
                reads += 1;
                last = Some(now);
            }
        }
        assert_eq!(reads, 0, "paused in the background");
        assert!(due(last, NOW + 600, interval, 0, true), "once on return");
        assert!(
            !due(Some(NOW + 580), NOW + 600, interval, 0, true),
            "a fresh reading is not repeated"
        );
    }

    #[test]
    fn the_jitter_is_a_tenth_of_the_interval_at_most() {
        assert_eq!(jitter_seconds(60, 0), 0);
        assert_eq!(jitter_seconds(60, 10), 6);
        assert_eq!(jitter_seconds(60, 1234), jitter_seconds(60, 1234 % 11));
        assert!((0..=6).contains(&jitter_seconds(60, 7)));
        assert!(
            !due(Some(NOW), NOW + 62, 60, 6, true),
            "the jitter lengthens the wait"
        );
    }

    fn status() -> SourceStatus {
        SourceStatus::default()
    }

    #[test]
    fn a_read_under_way_says_reading_and_warns_of_the_keychain_prompt_on_a_mac() {
        let reading = SourceStatus {
            reading: true,
            ..status()
        };
        let on_mac = unknown_text(
            AgentKind::Claude,
            Reason::SourceDisabled,
            &reading,
            NOW,
            None,
            true,
        );
        assert!(on_mac.starts_with("Reading…"), "{on_mac}");
        assert!(on_mac.contains("macOS may ask for permission to read Claude Code's sign-in"));
        let elsewhere = unknown_text(
            AgentKind::Claude,
            Reason::SourceDisabled,
            &reading,
            NOW,
            None,
            false,
        );
        assert_eq!(elsewhere, "Reading…");
        let opencode = unknown_text(
            AgentKind::Opencode,
            Reason::SourceDisabled,
            &reading,
            NOW,
            None,
            true,
        );
        assert_eq!(opencode, "Reading…");
    }

    #[test]
    fn each_failure_says_its_reason_and_when_the_next_read_is() {
        let retry = SourceStatus {
            retry_at: Some(NOW + 120),
            ..status()
        };
        let text = |reason, status: &SourceStatus, next| {
            unknown_text(AgentKind::Claude, reason, status, NOW, next, true)
        };
        assert!(text(Reason::Offline, &retry, None).contains("Offline"));
        assert!(text(Reason::Offline, &retry, None).contains("Next read in 2m"));
        assert!(text(Reason::VendorError(503), &retry, None).contains("HTTP 503"));
        assert!(text(Reason::ParseError, &retry, None).contains("Unexpected response"));
        let limited = text(Reason::RateLimited(120), &retry, None);
        assert!(limited.starts_with("Rate limited"), "{limited}");
        assert!(limited.contains("Next read in 2m"), "{limited}");
        assert!(text(Reason::Offline, &status(), Some(NOW + 150)).contains("Next read in 2m"));
        assert!(text(Reason::Offline, &status(), None).contains("Try again"));
        assert!(text(Reason::NotSignedIn, &status(), None).contains("Not signed in to Claude Code"));
        let denied = text(Reason::KeychainDenied, &status(), None);
        assert!(
            denied.contains("denied or dismissed") && denied.contains("Try again"),
            "{denied}"
        );
        assert_eq!(
            text(Reason::SourceDisabled, &status(), None),
            Reason::SourceDisabled.sentence()
        );
    }

    #[test]
    fn numbers_with_a_failed_read_behind_them_say_so() {
        let failed = SourceStatus {
            failed: Some(Reason::Offline),
            retry_at: Some(NOW + 60),
            ..status()
        };
        let note = failure_note(AgentKind::Claude, &failed, NOW, None, true).unwrap();
        assert!(note.starts_with("The last read failed."), "{note}");
        assert!(failure_note(AgentKind::Claude, &status(), NOW, None, true).is_none());
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
