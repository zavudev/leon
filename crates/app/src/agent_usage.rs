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
use leon_core::{AgentId, MachineId, Store, UsagePoint};
use leon_usage::{
    series_key_of, AgentUsage, MachineUsage, PercentDisplay, Reason, State, Thresholds,
};
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
            State::Unknown { reason },
        ) if reason.keeps_numbers() => old,
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
pub type Statuses = HashMap<AgentId, SourceStatus>;

/// The statuses of every agent now.
pub fn statuses(engine: &crate::engine::Engine) -> Statuses {
    leon_usage::network::switchable_agents()
        .into_iter()
        .map(|agent| (agent, engine.usage_status(agent)))
        .collect()
}

/// Whether "not signed in" is worth saying for an agent: it has a network
/// source that reads the agent's own sign-in (the opencode Go key is absent
/// for most people, who never subscribed).
pub fn says_signed_out(agent: AgentId) -> bool {
    agent == AgentId::CLAUDE
        || (leon_usage::network::has_network_source(agent) && agent != AgentId::OPENCODE)
}

/// What a row says when nothing is known of an agent: why, and what happens
/// next. A read under way is "Reading…" (with the keychain's heads-up for
/// Claude Code on macOS), not what the last read said.
pub fn unknown_text(
    agent: AgentId,
    reason: Reason,
    status: &SourceStatus,
    now: i64,
    next_read: Option<i64>,
    mac: bool,
) -> String {
    let name = agent.name();
    // The agents whose sign-in lives in the macOS keychain.
    let keychain = matches!(agent, AgentId::CLAUDE | AgentId::CURSOR);
    if status.reading {
        return if keychain && mac {
            format!("Reading… macOS may ask for permission to read {name}'s sign-in.")
        } else {
            "Reading…".to_owned()
        };
    }
    if reason == Reason::KeychainDenied || status.refused {
        return if mac {
            format!("macOS did not let Leon read {name}'s sign-in: the permission was denied or dismissed. Leon will not ask again this session; choose Try again to be asked.")
        } else {
            "The sign-in could not be read. Leon will not try again by itself; choose Try again."
                .to_owned()
        };
    }
    if reason == Reason::NotSignedIn && says_signed_out(agent) {
        return format!("Not signed in to {name}. Sign in there, then choose Try again.");
    }
    let base = match reason {
        Reason::RateLimited(_) => format!(
            "Rate limited by {}: it asked for fewer calls.",
            leon_usage::model::vendor(agent)
        ),
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
    agent: AgentId,
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

/// The sources that were called, one entry per agent: when several accounts of
/// one agent were read, a failure of any of them is the agent's, so the source
/// backs off, and a later success of another account does not clear it.
pub fn fold_called<'a>(
    called: impl IntoIterator<Item = &'a leon_usage::collect::Called>,
) -> Vec<leon_usage::collect::Called> {
    let mut out: Vec<leon_usage::collect::Called> = Vec::new();
    for (agent, reason) in called {
        match out.iter_mut().find(|(known, _)| known == agent) {
            Some((_, kept)) => {
                if !kept.is_some_and(Reason::is_failure) {
                    *kept = *reason;
                }
            }
            None => out.push((*agent, *reason)),
        }
    }
    out
}

/// The folders of the accounts whose limits can be read from them: those of
/// Claude Code and Codex, the one level above the `projects` and `sessions`
/// folders the history is read from.
pub fn account_folders(roots: &leon_history::HistoryRoots) -> Vec<leon_usage::AccountFolder> {
    roots
        .accounts
        .iter()
        .flat_map(|account| {
            let claude = account
                .claude_projects
                .as_deref()
                .map(|projects| (AgentId::CLAUDE, projects));
            let codex = account
                .codex_sessions
                .as_deref()
                .map(|sessions| (AgentId::CODEX, sessions));
            [claude, codex]
                .into_iter()
                .flatten()
                .filter_map(move |(agent, below)| {
                    Some(leon_usage::AccountFolder {
                        account: account.account.clone(),
                        agent,
                        folder: below.parent()?.to_path_buf(),
                    })
                })
        })
        .collect()
}

/// The observations of one agent's account: the window key and the point.
pub type Series = (AgentId, String, Vec<(String, UsagePoint)>);

/// The history points of a collection: per agent, the account key and the
/// observations of every window, the current reading included.
pub fn history_points(collected: &MachineUsage, machine: &str) -> Vec<Series> {
    let mut out = Vec::new();
    for reading in &collected.readings {
        let State::Known { windows } = &reading.state else {
            continue;
        };
        let account = series_key_of(
            machine,
            reading.agent,
            reading.plan.as_deref(),
            reading.account.as_deref(),
        );
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
    pub fn new_for(readings: Vec<AgentUsage>) -> Self {
        Self {
            readings,
            collected_at: None,
        }
    }

    /// A board over these readings, in tests.
    #[cfg(test)]
    pub fn new(readings: Vec<AgentUsage>) -> Self {
        Self::new_for(readings)
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

    /// The reading of `agent` on `machine` for the account with this id, or
    /// for the agent's own setup with `None`.
    pub fn get_for(
        &self,
        machine: &MachineId,
        agent: AgentId,
        account: Option<&str>,
    ) -> Option<&AgentUsage> {
        self.readings.iter().find(|r| {
            r.agent == agent && r.machine == machine.as_str() && r.account.as_deref() == account
        })
    }

    /// The readings the scope asks for, `shown` agents only, in the order of
    /// agents and then machines. With `Context`, `context` is the machine.
    pub fn select(
        &self,
        scope: &Scope,
        context: &MachineId,
        shown: &[AgentId],
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
        let order = leon_usage::network::switchable_agents();
        // An agent's own line first, then its accounts by name.
        out.sort_by_key(|r| {
            (
                order.iter().position(|a| *a == r.agent),
                r.machine.clone(),
                r.account.as_deref().map(leon_core::account::name_of),
            )
        });
        out
    }
}

/// The one-line notice before a session of `agent` starts, when its limit is
/// nearly used up: which window, how full, when it resets. `None` when there
/// is nothing to say (below the critical threshold, unknown, or not read).
#[cfg(test)]
pub fn start_notice(
    board: &Board,
    machine: &MachineId,
    agent: AgentId,
    now: i64,
    thresholds: Thresholds,
    display: PercentDisplay,
) -> Option<String> {
    start_notice_for(board, machine, agent, None, now, thresholds, display)
}

/// [`start_notice`] for a session of an account of the agent (an id), whose
/// limit is its own; `None` is the agent's own setup.
#[allow(clippy::too_many_arguments)] // `start_notice` plus the account
pub fn start_notice_for(
    board: &Board,
    machine: &MachineId,
    agent: AgentId,
    account: Option<&str>,
    now: i64,
    thresholds: Thresholds,
    display: PercentDisplay,
) -> Option<String> {
    let reading = board.get_for(machine, agent, account)?;
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
    let name = leon_usage::heading(agent, account);
    // The window is judged on what is used; only the wording follows the
    // setting.
    let state = if leon_usage::percent_round(worst.percent) >= 100 {
        "is at its limit".to_owned()
    } else if display == PercentDisplay::Remaining {
        format!("has {}", display.label(worst.percent))
    } else {
        format!("is {}", display.label(worst.percent))
    };
    Some(format!("{name}: {} {state}{when}.", worst.kind.long()))
}

/// What a collection of one machine stores: the readings (merged with what was
/// there) as JSON documents, ready to write.
pub fn payloads(
    collected: &MachineUsage,
    previous: &HashMap<AgentId, AgentUsage>,
) -> Vec<(AgentId, AgentUsage)> {
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

    fn known(agent: AgentId, machine: &str, used: f64, reset_in: i64) -> AgentUsage {
        AgentUsage {
            agent,
            machine: machine.into(),
            account_label: None,
            account: None,
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
        let old = known(AgentId::CODEX, "box", 40.0, 3600);
        let new = AgentUsage::unknown(AgentId::CODEX, "box", Reason::Unreachable);
        assert_eq!(merge(Some(old.clone()), new), old);
    }

    #[test]
    fn a_failed_network_read_keeps_the_numbers_and_a_switch_or_sign_out_replaces_them() {
        let old = known(AgentId::CLAUDE, "box", 40.0, 3600);
        for reason in [
            Reason::Offline,
            Reason::VendorError(500),
            Reason::RateLimited(60),
            Reason::KeychainDenied,
            Reason::SessionExpired,
        ] {
            let new = AgentUsage::unknown(AgentId::CLAUDE, "box", reason);
            assert_eq!(merge(Some(old.clone()), new), old, "{reason:?}");
        }
        // A sign-out, a missing permission, an API-key account or a rejected
        // key say something about the account: they replace the numbers.
        for reason in [
            Reason::NotSignedIn,
            Reason::MissingScope,
            Reason::ApiKeyBilling,
            Reason::KeyRejected,
            Reason::NoSubscription,
            Reason::SourceDisabled,
        ] {
            let said = AgentUsage::unknown(AgentId::CLAUDE, "box", reason);
            assert_eq!(merge(Some(old.clone()), said.clone()), said, "{reason:?}");
        }
    }

    #[test]
    fn the_time_of_the_last_read_ignores_rows_that_only_say_why_nothing_was_read() {
        let store = Store::open_in_memory().unwrap();
        let local = MachineId::local();
        let off = AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::SourceDisabled);
        store
            .put_usage_reading(
                &local,
                AgentId::CLAUDE,
                &serde_json::to_string(&off).unwrap(),
                NOW,
            )
            .unwrap();
        assert_eq!(Board::load(&store).collected_at(), None);
        let codex = known(AgentId::CODEX, "local", 12.0, 100);
        store
            .put_usage_reading(
                &local,
                AgentId::CODEX,
                &serde_json::to_string(&codex).unwrap(),
                NOW - 90,
            )
            .unwrap();
        assert_eq!(Board::load(&store).collected_at(), Some(NOW - 90));
    }

    #[test]
    fn the_schedule_is_one_read_per_interval_in_a_focused_window_and_none_in_the_background() {
        // The default interval: ten minutes.
        let interval = 600;
        assert!(due(None, NOW, interval, 0, true), "the first read");
        assert!(!due(None, NOW, interval, 0, false), "not while away");
        // A clock ticking every TICK_SECONDS for an hour, focused: one read
        // per ten minutes.
        let hour = 3600 / TICK_SECONDS as i64;
        let mut last = None;
        let mut reads = 0;
        for tick in 0..hour {
            let now = NOW + tick * TICK_SECONDS as i64;
            if due(last, now, interval, 0, true) {
                reads += 1;
                last = Some(now);
            }
        }
        assert_eq!(reads, 6);
        // An hour in the background, then back: one read at once, not six.
        let mut reads = 0;
        let mut last = Some(NOW);
        for tick in 0..hour {
            let now = NOW + tick * TICK_SECONDS as i64;
            if due(last, now, interval, 0, false) {
                reads += 1;
                last = Some(now);
            }
        }
        assert_eq!(reads, 0, "paused in the background");
        assert!(due(last, NOW + 3600, interval, 0, true), "once on return");
        assert!(
            !due(Some(NOW + 3580), NOW + 3600, interval, 0, true),
            "a fresh reading is not repeated"
        );
        assert!(
            !due(Some(NOW), NOW + 599, interval, 0, true),
            "nothing before the interval"
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
            AgentId::CLAUDE,
            Reason::SourceDisabled,
            &reading,
            NOW,
            None,
            true,
        );
        assert!(on_mac.starts_with("Reading…"), "{on_mac}");
        assert!(on_mac.contains("macOS may ask for permission to read Claude Code's sign-in"));
        let elsewhere = unknown_text(
            AgentId::CLAUDE,
            Reason::SourceDisabled,
            &reading,
            NOW,
            None,
            false,
        );
        assert_eq!(elsewhere, "Reading…");
        let opencode = unknown_text(
            AgentId::OPENCODE,
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
            unknown_text(AgentId::CLAUDE, reason, status, NOW, next, true)
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
        let note = failure_note(AgentId::CLAUDE, &failed, NOW, None, true).unwrap();
        assert!(note.starts_with("The last read failed."), "{note}");
        assert!(failure_note(AgentId::CLAUDE, &status(), NOW, None, true).is_none());
    }

    #[test]
    fn a_reading_that_says_something_replaces_the_earlier_one() {
        let old = known(AgentId::CODEX, "box", 40.0, 3600);
        let newer = known(AgentId::CODEX, "box", 50.0, 3600);
        assert_eq!(merge(Some(old.clone()), newer.clone()), newer);
        let disabled = AgentUsage::unknown(AgentId::CODEX, "box", Reason::SourceDisabled);
        assert_eq!(merge(Some(old), disabled.clone()), disabled);
        assert_eq!(merge(None, disabled.clone()), disabled);
    }

    #[test]
    fn history_points_hold_the_samples_and_the_current_windows() {
        let reading = known(AgentId::CODEX, "local", 40.0, 3600);
        let collected = MachineUsage {
            called: Vec::new(),
            readings: vec![
                reading.clone(),
                AgentUsage::unknown(AgentId::CLAUDE, "local", Reason::SourceDisabled),
            ],
            samples: vec![(
                AgentId::CODEX,
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
        assert_eq!(*agent, AgentId::CODEX);
        assert_eq!(account.len(), 12);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].0, "five_hour");
        assert_eq!(points[1].1.used_percent, 40.0);
    }

    #[test]
    fn the_bar_speaks_for_the_machine_in_context_or_for_all() {
        let board = Board::new(vec![
            known(AgentId::CODEX, "local", 10.0, 100),
            known(AgentId::CODEX, "box", 20.0, 100),
            known(AgentId::CLAUDE, "local", 30.0, 100),
        ]);
        let local = MachineId::local();
        let all = [AgentId::CLAUDE, AgentId::CODEX, AgentId::OPENCODE];
        let ctx = board.select(&Scope::Context, &local, &all);
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx[0].agent, AgentId::CLAUDE);
        let everything = board.select(&Scope::All, &local, &all);
        assert_eq!(everything.len(), 3);
        let one = board.select(&Scope::Machine(MachineId::from_string("box")), &local, &all);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].machine, "box");
        let only_codex = board.select(&Scope::All, &local, &[AgentId::CODEX]);
        assert_eq!(only_codex.len(), 2);
    }

    #[test]
    fn the_notice_appears_from_the_critical_threshold_and_names_the_reset() {
        let board = Board::new(vec![known(
            AgentId::CODEX,
            "local",
            97.0,
            2 * 3600 + 29 * 60,
        )]);
        let local = MachineId::local();
        let t = Thresholds::default();
        assert_eq!(
            start_notice(&board, &local, AgentId::CODEX, NOW, t, PercentDisplay::Used).as_deref(),
            Some("Codex: 5-hour window is 97% used, resets in 2h 29m.")
        );
        let low = Board::new(vec![known(AgentId::CODEX, "local", 79.0, 100)]);
        assert_eq!(
            start_notice(&low, &local, AgentId::CODEX, NOW, t, PercentDisplay::Used),
            None
        );
        assert_eq!(
            start_notice(
                &board,
                &local,
                AgentId::CLAUDE,
                NOW,
                t,
                PercentDisplay::Used
            ),
            None
        );
    }

    #[test]
    fn an_exhausted_window_says_it_is_at_its_limit() {
        let board = Board::new(vec![known(AgentId::CODEX, "local", 100.0, 600)]);
        let notice = start_notice(
            &board,
            &MachineId::local(),
            AgentId::CODEX,
            NOW,
            Thresholds::default(),
            PercentDisplay::Used,
        )
        .unwrap();
        assert!(notice.contains("is at its limit"), "{notice}");
        assert!(notice.contains("resets in 10m"), "{notice}");
    }

    #[test]
    fn a_window_that_has_reset_since_stops_the_notice() {
        let board = Board::new(vec![known(AgentId::CODEX, "local", 99.0, -60)]);
        assert_eq!(
            start_notice(
                &board,
                &MachineId::local(),
                AgentId::CODEX,
                NOW,
                Thresholds::default(),
                PercentDisplay::Used
            ),
            None
        );
    }

    #[test]
    fn a_board_reads_back_what_the_store_holds() {
        let store = Store::open_in_memory().unwrap();
        let reading = known(AgentId::CODEX, "local", 12.0, 100);
        store
            .put_usage_reading(
                &MachineId::local(),
                AgentId::CODEX,
                &serde_json::to_string(&reading).unwrap(),
                NOW,
            )
            .unwrap();
        store
            .put_usage_reading(&MachineId::local(), AgentId::CLAUDE, "not json", NOW)
            .unwrap();
        let board = Board::load(&store);
        assert_eq!(board.all().len(), 1);
        assert_eq!(
            board.get_for(&MachineId::local(), AgentId::CODEX, None),
            Some(&reading)
        );
    }

    #[test]
    fn the_notice_follows_the_display_setting_but_is_judged_on_what_is_used() {
        let board = Board::new(vec![known(AgentId::CODEX, "local", 85.0, 600)]);
        let local = MachineId::local();
        let t = Thresholds::default();
        assert_eq!(
            start_notice(
                &board,
                &local,
                AgentId::CODEX,
                NOW,
                t,
                PercentDisplay::Remaining
            )
            .as_deref(),
            Some("Codex: 5-hour window has 15% left, resets in 10m.")
        );
        // 79 % used is below the critical level whatever the wording.
        let low = Board::new(vec![known(AgentId::CODEX, "local", 79.0, 600)]);
        for display in [PercentDisplay::Used, PercentDisplay::Remaining] {
            assert_eq!(
                start_notice(&low, &local, AgentId::CODEX, NOW, t, display),
                None
            );
        }
        // 12.5 rounds up, as everywhere else.
        let half = Board::new(vec![known(AgentId::CODEX, "local", 99.5, 600)]);
        assert!(
            start_notice(&half, &local, AgentId::CODEX, NOW, t, PercentDisplay::Used)
                .unwrap()
                .contains("is at its limit")
        );
    }

    #[test]
    fn a_rate_limit_names_the_vendor_and_the_next_read_and_an_expired_sign_in_says_what_to_do() {
        let status = SourceStatus {
            retry_at: Some(NOW + 300),
            failed: Some(Reason::RateLimited(0)),
            ..Default::default()
        };
        let text = unknown_text(
            AgentId::CLAUDE,
            Reason::RateLimited(0),
            &status,
            NOW,
            None,
            true,
        );
        assert_eq!(
            text,
            "Rate limited by Anthropic: it asked for fewer calls. Next read in 5m; or choose Try again."
        );
        let expired = unknown_text(
            AgentId::CODEX,
            Reason::SessionExpired,
            &SourceStatus::default(),
            NOW,
            None,
            true,
        );
        assert!(expired.contains("Run the agent once"), "{expired}");
        assert!(expired.contains("Leon never does"), "{expired}");
    }

    // ----- accounts -----------------------------------------------------------

    fn of_account(mut usage: AgentUsage, account: &str) -> AgentUsage {
        usage.account = Some(account.to_owned());
        usage
    }

    #[test]
    fn an_agents_own_reading_and_its_accounts_are_separate_lines_in_a_stable_order() {
        let local = MachineId::local();
        let board = Board::new(vec![
            of_account(known(AgentId::CLAUDE, "local", 80.0, 3600), "claude-zed"),
            known(AgentId::CODEX, "local", 10.0, 3600),
            of_account(known(AgentId::CLAUDE, "local", 20.0, 3600), "claude-ada"),
            known(AgentId::CLAUDE, "local", 50.0, 3600),
        ]);
        // The agent's own line is the one the notice and the lookups mean.
        assert_eq!(
            board
                .get_for(&local, AgentId::CLAUDE, None)
                .unwrap()
                .account,
            None,
            "an account's line is never the agent's own"
        );
        assert_eq!(
            board
                .get_for(&local, AgentId::CLAUDE, Some("claude-ada"))
                .unwrap()
                .account
                .as_deref(),
            Some("claude-ada")
        );
        assert!(board
            .get_for(&local, AgentId::CLAUDE, Some("claude-gone"))
            .is_none());
        // Claude Code's own line first, then its accounts by name (an id that is
        // not registered sorts by the id), then the next agent.
        let order: Vec<(AgentId, Option<&str>)> = board
            .select(&Scope::Context, &local, &[AgentId::CLAUDE, AgentId::CODEX])
            .into_iter()
            .map(|r| (r.agent, r.account.as_deref()))
            .collect();
        assert_eq!(
            order,
            [
                (AgentId::CLAUDE, None),
                (AgentId::CLAUDE, Some("claude-ada")),
                (AgentId::CLAUDE, Some("claude-zed")),
                (AgentId::CODEX, None),
            ]
        );
    }

    #[test]
    fn the_notice_before_a_session_judges_the_account_it_starts_as() {
        let local = MachineId::local();
        let t = Thresholds {
            warning: 60.0,
            critical: 80.0,
        };
        let board = Board::new(vec![
            known(AgentId::CLAUDE, "local", 10.0, 3600),
            of_account(known(AgentId::CLAUDE, "local", 95.0, 3600), "claude-work"),
        ]);
        assert_eq!(
            start_notice(
                &board,
                &local,
                AgentId::CLAUDE,
                NOW,
                t,
                PercentDisplay::Used
            ),
            None,
            "the agent's own limit is not nearly used up"
        );
        let notice = start_notice_for(
            &board,
            &local,
            AgentId::CLAUDE,
            Some("claude-work"),
            NOW,
            t,
            PercentDisplay::Used,
        )
        .unwrap();
        assert!(notice.contains("Claude Code (claude-work)"), "{notice}");
        assert!(notice.contains("95%"), "{notice}");
    }

    #[test]
    fn a_failure_of_any_account_is_the_agents_and_a_later_success_does_not_clear_it() {
        let ok = (AgentId::CLAUDE, None);
        let limited = (AgentId::CLAUDE, Some(Reason::RateLimited(30)));
        let expired = (AgentId::CLAUDE, Some(Reason::SessionExpired));
        let other = (AgentId::CODEX, None);
        assert_eq!(fold_called(&[ok, limited, ok]), [limited]);
        assert_eq!(fold_called(&[limited, ok]), [limited]);
        // The first failure is the one kept.
        assert_eq!(fold_called(&[limited, expired]), [limited]);
        assert_eq!(fold_called(&[ok, other, ok]), [ok, other]);
        assert!(fold_called(&[]).is_empty());
        // A reason that is not a failure (signed out) does not back a source off.
        let signed_out = (AgentId::CLAUDE, Some(Reason::NotSignedIn));
        assert_eq!(fold_called(&[limited, signed_out]), [limited]);
        assert_eq!(fold_called(&[signed_out, ok]), [ok]);
    }

    #[test]
    fn only_the_accounts_with_a_folder_of_claude_code_or_codex_are_read_for_limits() {
        let roots = leon_history::HistoryRoots {
            accounts: vec![
                leon_history::AccountRoot {
                    account: "claude-work".into(),
                    claude_projects: Some("/h/.claude-work/projects".into()),
                    codex_sessions: None,
                },
                leon_history::AccountRoot {
                    account: "codex-lab".into(),
                    claude_projects: None,
                    codex_sessions: Some("/data/lab/sessions".into()),
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            account_folders(&roots),
            [
                leon_usage::AccountFolder {
                    account: "claude-work".into(),
                    agent: AgentId::CLAUDE,
                    folder: "/h/.claude-work".into()
                },
                leon_usage::AccountFolder {
                    account: "codex-lab".into(),
                    agent: AgentId::CODEX,
                    folder: "/data/lab".into()
                },
            ]
        );
        assert!(account_folders(&leon_history::HistoryRoots::default()).is_empty());
    }

    #[test]
    fn two_accounts_on_one_plan_leave_separate_series_in_the_history() {
        let mut collected = MachineUsage::default();
        collected.readings.push(of_account(
            known(AgentId::CODEX, "local", 40.0, 3600),
            "codex-lab",
        ));
        let own = {
            let mut c = MachineUsage::default();
            c.readings.push(known(AgentId::CODEX, "local", 40.0, 3600));
            c
        };
        let key_of = |c: &MachineUsage| history_points(c, "local")[0].1.clone();
        assert_ne!(key_of(&collected), key_of(&own));
    }
}
